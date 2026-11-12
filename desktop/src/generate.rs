//! Generate: the top bar's AI button and its panel, which sends the canvas and a prompt to a
//! ComfyUI server through one of the workflows kept there for ProCraft, and takes back a layer.
//!
//! The panel hangs from its button as the Adjustments menu does (see [`crate::adjustments::menu`]).

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Align2, Id, Order, Rect, ScrollArea, Sense, Ui, UiBuilder, pos2, vec2};
use procraft_core::{CanvasSession, ComfyJob, ComfyPhase, ComfyRequest, ComfyWorkflow};

use crate::icons::{Icon, Part};
use crate::look::{Look, Themed, Weight, argb, pc, text};
use crate::panel;

/// The bar button's glyph: a four-pointed sparkle and a small one, in the bar's 44 frame.
pub const SPARKLE: Icon = Icon {
    name: "generate-sparkle",
    width: 44.0,
    height: 44.0,
    parts: &[
        Part {
            d: "M20 11C20.9 18.2 23.8 21.1 31 22C23.8 22.9 20.9 25.8 20 33C19.1 25.8 16.2 22.9 9 22C16.2 21.1 19.1 18.2 20 11Z",
            stroke: 0.0,
            fill: true,
            even_odd: false,
        },
        Part {
            d: "M30 9C30.4 11.9 31.6 13.1 34.5 13.5C31.6 13.9 30.4 15.1 30 18C29.6 15.1 28.4 13.9 25.5 13.5C28.4 13.1 29.6 11.9 30 9Z",
            stroke: 0.0,
            fill: true,
            even_odd: false,
        },
    ],
};

/// The Settings tab's glyph: a gear with eight teeth, in a 44 frame.
const GEAR: Icon = Icon {
    name: "generate-gear",
    width: 44.0,
    height: 44.0,
    parts: &[Part {
        d: "M33.26 19.20L36.83 19.77L36.83 24.23L33.26 24.80L31.94 27.98L34.07 30.91L30.91 34.07L27.98 31.94L24.80 33.26L24.23 36.83L19.77 36.83L19.20 33.26L16.02 31.94L13.09 34.07L9.93 30.91L12.06 27.98L10.74 24.80L7.17 24.23L7.17 19.77L10.74 19.20L12.06 16.02L9.93 13.09L13.09 9.93L16.02 12.06L19.20 10.74L19.77 7.17L24.23 7.17L24.80 10.74L27.98 12.06L30.91 9.93L34.07 13.09L31.94 16.02ZM27.2 22A5.2 5.2 0 1 0 16.8 22A5.2 5.2 0 1 0 27.2 22Z",
        stroke: 0.0,
        fill: true,
        even_odd: true,
    }],
};

const WIDTH: f32 = 380.0;
const HEIGHT: f32 = 480.0;
const BOTTOM_MARGIN: f32 = 25.0;
const GROUP_FILL: Themed = Themed(0x420C0C0C, 0xFFE8E8E8);
const LABEL: Themed = Themed(0xFFAAAAAA, 0xFF555555);
const TAB_IDLE: Themed = Themed(0xFFA3A3A3, 0xFF404040);
const CONNECTING: u32 = 0xFFFF9F0A;

/// How the bar's button shows the job: idle, under way (its share done, if known), or how the last
/// one ended, for a moment after.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum BarStatus {
    #[default]
    Idle,
    /// Queued or running: the share done, 0 to 1, or none while it can't be told (a spinning arc).
    Working(Option<f32>),
    Done,
    Failed,
}

/// How long the button shows how the last job ended.
const OUTCOME_SHOWN: Duration = Duration::from_secs(2);

/// What a tap on the panel asks for, done by the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// List the workflows the server keeps for ProCraft.
    Connect,
    /// Send the canvas and the prompt through the chosen workflow.
    Generate,
    /// Call the job under way off.
    Cancel,
}

/// The panel's two pages: what to make, and where and with what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Generate,
    Settings,
}

/// How the panel stands with the server.
#[derive(Clone, Debug, PartialEq)]
enum Connection {
    /// Not asked yet this run.
    Unknown,
    Connecting,
    /// It answered, listing these many workflows (and these many usable).
    Connected { listed: usize, usable: usize },
    Failed(String),
}

/// What the panel keeps between runs of the app (in the app's folder, `generate.json`): what a tap
/// on the bar's button sends again.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Kept {
    server: String,
    workflow: String,
    prompt: String,
    strength: f32,
    /// Each workflow's negative prompt, by its name (none: the workflow's own).
    negatives: std::collections::BTreeMap<String, String>,
    /// The seed last used or typed, and whether each job takes a new one instead.
    seed: u64,
    random_seed: bool,
}

impl Default for Kept {
    fn default() -> Kept {
        Kept {
            server: "http://127.0.0.1:8188".to_owned(),
            workflow: String::new(),
            prompt: String::new(),
            strength: 0.9,
            negatives: std::collections::BTreeMap::new(),
            seed: 0,
            random_seed: true,
        }
    }
}

fn kept_path() -> std::path::PathBuf {
    crate::files::app_dir().join("generate.json")
}

/// The panel's state: its fields, the server's workflows, and the job under way.
pub struct Generate {
    pub server: String,
    /// The workflows the server keeps for ProCraft.
    pub workflows: Vec<ComfyWorkflow>,
    /// The chosen workflow's name (kept even before the server lists it).
    pub workflow: String,
    pub prompt: String,
    /// How much the result may differ from the sketch, 0 to 1.
    pub strength: f32,
    /// Each workflow's negative prompt, by its name.
    negatives: std::collections::BTreeMap<String, String>,
    /// The seed as typed (the last used while it is random), and whether each job takes a new one.
    seed: String,
    random_seed: bool,
    tab: Tab,
    connection: Connection,
    /// How the last job went, or the one under way goes.
    status: String,
    /// The workflows being listed, on a thread of their own.
    listing: Option<Receiver<Result<Vec<ComfyWorkflow>, String>>>,
    job: Option<Arc<ComfyJob>>,
    /// The job's share done, 0 to 1, once the server tells it.
    progress: Option<f32>,
    /// How the last job ended (true: it made a layer), and when.
    outcome: Option<(bool, Instant)>,
    /// What was last written to `generate.json`.
    saved: Kept,
}

impl Default for Generate {
    fn default() -> Generate {
        let kept: Kept = std::fs::read(kept_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Generate {
            server: kept.server.clone(),
            workflows: Vec::new(),
            workflow: kept.workflow.clone(),
            prompt: kept.prompt.clone(),
            strength: kept.strength,
            negatives: kept.negatives.clone(),
            seed: kept.seed.to_string(),
            random_seed: kept.random_seed,
            tab: Tab::Generate,
            connection: Connection::Unknown,
            status: String::new(),
            listing: None,
            job: None,
            progress: None,
            outcome: None,
            saved: kept,
        }
    }
}

impl Generate {
    /// The chosen workflow's name, if one is.
    pub fn workflow(&self) -> Option<&str> {
        Some(self.workflow.as_str()).filter(|w| !w.is_empty())
    }

    /// The chosen workflow as the server listed it.
    fn chosen(&self) -> Option<&ComfyWorkflow> {
        self.workflows.iter().find(|w| w.name == self.workflow)
    }

    /// A job is under way.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Lists the server's workflows, on a thread of its own.
    pub fn connect(&mut self, ctx: &egui::Context) {
        let (send, receive) = std::sync::mpsc::channel();
        let server = self.server.trim().to_owned();
        let repaint = ctx.clone();
        let started = std::thread::Builder::new().name("comfy list".into()).spawn(move || {
            let _ = send.send(procraft_core::comfy_workflows(server).map_err(|e| e.to_string()));
            repaint.request_repaint();
        });
        self.connection = match started {
            Ok(_) => {
                self.listing = Some(receive);
                Connection::Connecting
            }
            Err(e) => Connection::Failed(format!("Couldn't connect: {e}")),
        };
        self.keep();
    }

    /// Sends the canvas and the prompt through the chosen workflow.
    pub fn start(&mut self, session: &CanvasSession) {
        let Some(workflow) = self.workflow().map(str::to_owned) else { return };
        if self.busy() {
            return;
        }
        let request = ComfyRequest {
            server: self.server.trim().to_owned(),
            negative: self.negatives.get(&workflow).map(|n| n.trim().to_owned()).unwrap_or_default(),
            workflow,
            prompt: self.prompt.trim().to_owned(),
            strength: self.strength,
            seed: if self.random_seed { None } else { Some(self.typed_seed()) },
        };
        let job = session.comfy_generate(request);
        // the seed it uses shows at once, to keep if the picture is liked
        self.seed = job.seed().to_string();
        self.job = Some(job);
        self.progress = None;
        self.outcome = None;
        self.status = "Sending the canvas…".to_owned();
        self.keep();
    }

    /// The seed as typed: its digits, or 0.
    fn typed_seed(&self) -> u64 {
        let digits: String = self.seed.chars().filter(char::is_ascii_digit).collect();
        digits.parse().unwrap_or(0)
    }

    pub fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.cancel();
            self.status = "Calling it off…".to_owned();
        }
    }

    /// The job ended, well (`ok`) or not, saying `status`.
    fn ended(&mut self, ok: bool, status: String) {
        self.job = None;
        self.progress = None;
        self.outcome = Some((ok, Instant::now()));
        self.status = status;
    }

    /// Takes in what the listing and the job have come to; each frame. A finished job's picture
    /// goes on a new layer.
    pub fn tick(&mut self, ctx: &egui::Context, session: &CanvasSession) {
        if let Some(listed) = self.listing.as_ref().and_then(|r| r.try_recv().ok()) {
            self.listing = None;
            match listed {
                Ok(workflows) => {
                    let usable = workflows.iter().filter(|w| w.problem.is_none()).count();
                    self.connection = Connection::Connected { listed: workflows.len(), usable };
                    // the one chosen stays chosen if it's still there, else the first usable
                    if !workflows.iter().any(|w| w.name == self.workflow && w.problem.is_none()) {
                        self.workflow =
                            workflows.iter().find(|w| w.problem.is_none()).map(|w| w.name.clone()).unwrap_or_default();
                    }
                    self.workflows = workflows;
                    self.keep();
                }
                Err(why) => self.connection = Connection::Failed(why),
            }
        }
        let Some(job) = self.job.clone() else { return };
        let status = job.status();
        match status.phase {
            ComfyPhase::Done => {
                if session.comfy_put_result(job) {
                    self.ended(true, format!("{}: a new layer", status.message));
                } else {
                    self.ended(false, "The picture couldn't go on a new layer".to_owned());
                }
            }
            ComfyPhase::Failed => {
                // a server that can't be reached shows as such on the connection too
                if status.message.contains("Connection refused") || status.message.contains("timed out") {
                    self.connection = Connection::Failed("The server didn't answer".to_owned());
                }
                self.ended(false, status.message);
            }
            ComfyPhase::Cancelled => {
                self.ended(false, "Called off".to_owned());
                self.outcome = None;
            }
            ComfyPhase::Sending | ComfyPhase::Queued | ComfyPhase::Running => {
                self.progress = (status.progress >= 0.0).then_some(status.progress);
                self.status = status.message;
                // the job moves on its own thread: look again soon
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    /// Writes what the panel keeps, if it changed.
    pub fn keep(&mut self) {
        let kept = Kept {
            server: self.server.trim().to_owned(),
            workflow: self.workflow.clone(),
            prompt: self.prompt.clone(),
            strength: self.strength,
            negatives: self.negatives.iter().filter(|(_, n)| !n.trim().is_empty()).map(|(k, v)| (k.clone(), v.clone())).collect(),
            seed: self.typed_seed(),
            random_seed: self.random_seed,
        };
        if kept == self.saved {
            return;
        }
        if let Ok(text) = serde_json::to_vec_pretty(&kept)
            && std::fs::write(kept_path(), text).is_ok()
        {
            self.saved = kept;
        }
    }

    /// How the bar's button shows the job now.
    pub fn bar_status(&self) -> BarStatus {
        if self.busy() {
            return BarStatus::Working(self.progress);
        }
        match self.outcome {
            Some((ok, at)) if at.elapsed() < OUTCOME_SHOWN => {
                if ok {
                    BarStatus::Done
                } else {
                    BarStatus::Failed
                }
            }
            _ => BarStatus::Idle,
        }
    }

    /// Shows the panel hanging from `button`: where it is, and what was tapped. Opened before the
    /// server was asked this run, it asks.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        look: &mut Look,
        button: Rect,
        backdrops: &mut Vec<f32>,
    ) -> (Rect, Option<Act>) {
        let screen = ctx.content_rect();
        let (left, arrow_x) = panel::place(ctx, WIDTH, button.center().x);
        let top = screen.top() + pc::BAR_HEIGHT;
        let height = HEIGHT.min(screen.bottom() - BOTTOM_MARGIN - top);
        let rect = Rect::from_min_size(pos2(left, top), vec2(WIDTH, height));
        let mut act = (self.connection == Connection::Unknown && !self.server.trim().is_empty()).then_some(Act::Connect);
        egui::Area::new(Id::unique("generate panel")).fixed_pos(rect.min).order(Order::Foreground).show(ctx, |ui| {
            ui.allocate_rect(rect, Sense::click());
            panel::frame(ui, look, "generate frame", rect, arrow_x, backdrops);
            let body = panel::body(rect);
            let title = pos2(body.left() + 24.0, body.top() + 22.5);
            let heading = if self.tab == Tab::Generate { "Generate" } else { "Settings" };
            text(ui.painter(), title, Align2::LEFT_TOP, heading, 24.0, Weight::Bold, argb(panel::PANEL_TITLE));
            self.tabs(ui, look, body);
            let inner = Rect::from_min_max(pos2(body.left() + 18.0, body.top() + 64.0), body.max - vec2(18.0, 14.0));
            ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
                ScrollArea::vertical().id_salt(("generate", self.tab == Tab::Generate)).show(ui, |ui| {
                    ui.set_width(inner.width());
                    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                    let tapped = match self.tab {
                        Tab::Generate => self.generate_page(ui),
                        Tab::Settings => self.settings_page(ui),
                    };
                    act = tapped.or(act);
                });
            });
        });
        (rect, act)
    }

    /// The two tabs at the header's right, as the Actions panel draws its own: blue when chosen.
    fn tabs(&mut self, ui: &mut Ui, look: &mut Look, body: Rect) {
        let tabs = [(Tab::Generate, &SPARKLE, 30.0), (Tab::Settings, &GEAR, 34.0)];
        for (i, (tab, icon, size)) in tabs.into_iter().enumerate() {
            let centre = pos2(body.right() - 34.0 - 44.0 * (1 - i) as f32, body.top() + 34.0);
            let cell = Rect::from_center_size(centre, vec2(40.0, 40.0));
            let response = ui.interact(cell, ui.scope_id().with(("generate tab", i)), Sense::click());
            if response.clicked() {
                self.tab = tab;
            }
            let lit = self.tab == tab || response.is_pointer_button_down_on();
            look.icon(ui, icon, centre, size, if lit { argb(pc::ACCENT) } else { argb(TAB_IDLE) });
        }
    }

    /// The main page: the model and how the server stands, the prompt, the strength, Generate.
    fn generate_page(&mut self, ui: &mut Ui) -> Option<Act> {
        let mut act = None;
        let width = ui.available_width();

        // the model and the server, a tap away from their settings
        let (dot, words) = self.connection_line();
        let model = self.workflow().map_or("No model chosen".to_owned(), |w| w.trim_end_matches(".json").to_owned());
        let row = egui::Frame::NONE.fill(argb(GROUP_FILL)).corner_radius(10).inner_margin(egui::Margin::symmetric(10, 7));
        let shown = row.show(ui, |ui| {
            ui.set_width(width - 20.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&model).size(15.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(&words).size(12.0).color(argb(LABEL)));
                    let (spot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().circle_filled(spot.center(), 4.0, dot);
                });
            });
        });
        if ui.interact(shown.response.rect, ui.scope_id().with("model row"), Sense::click()).clicked() {
            self.tab = Tab::Settings;
        }
        ui.add_space(8.0);

        label(ui, "Prompt");
        let prompt = egui::TextEdit::multiline(&mut self.prompt).hint_text("What to make of the sketch").desired_rows(6);
        ui.add_sized([width, 132.0], prompt);
        ui.add_space(8.0);

        if self.chosen().is_none_or(|w| w.takes_image) {
            label(ui, "Strength");
            let mut percent = (self.strength * 100.0).round();
            ui.spacing_mut().slider_width = width - 70.0;
            if ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).step_by(1.0).fixed_decimals(0).suffix(" %")).changed() {
                self.strength = percent / 100.0;
            }
            ui.add_space(4.0);
        }

        label(ui, "Seed");
        ui.horizontal(|ui| {
            // random, the field shows the seed last used and can't be typed in; fixed, it's used as typed
            let field = egui::TextEdit::singleline(&mut self.seed).hint_text("0");
            ui.add_enabled_ui(!self.random_seed, |ui| ui.add_sized([width - 112.0, 28.0], field));
            let words = if self.random_seed { "Random" } else { "Fixed" };
            let toggled = ui.add_sized([104.0, 28.0], egui::Button::selectable(self.random_seed, words));
            if toggled.on_hover_text("Random: a new seed each time. Fixed: the seed shown, to keep a picture's composition").clicked() {
                self.random_seed = !self.random_seed;
            }
        });
        ui.add_space(12.0);

        if self.busy() {
            let words = match self.progress {
                Some(done) => format!("Cancel  ·  {}%", (done * 100.0).round()),
                None => "Cancel".to_owned(),
            };
            if ui.add(egui::Button::new(words).min_size(vec2(width, 36.0))).clicked() {
                act = Some(Act::Cancel);
            }
            ui.add_space(4.0);
            let bar = egui::ProgressBar::new(self.progress.unwrap_or(0.0)).desired_width(width).desired_height(6.0);
            ui.add(if self.progress.is_none() { bar.animate(true) } else { bar });
        } else {
            let button = egui::Button::new("Generate").min_size(vec2(width, 36.0));
            if ui.add_enabled(self.workflow().is_some(), button).clicked() {
                act = Some(Act::Generate);
            }
            ui.add_space(4.0);
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        act
    }

    /// The settings page: the server, the model, the model's negative prompt.
    fn settings_page(&mut self, ui: &mut Ui) -> Option<Act> {
        let mut act = None;
        let width = ui.available_width();

        label(ui, "Server");
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut self.server).hint_text("http://host:8188");
            let entered = ui.add_sized([width - 96.0, 28.0], field).lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let connecting = self.connection == Connection::Connecting;
            let button = egui::Button::new(if connecting { "…" } else { "Connect" });
            let tapped = ui.add_enabled_ui(!connecting, |ui| ui.add_sized([88.0, 28.0], button)).inner.clicked();
            if (tapped || entered) && !connecting {
                act = Some(Act::Connect);
            }
        });
        let (dot, words) = self.connection_line();
        ui.horizontal(|ui| {
            let (spot, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
            ui.painter().circle_filled(spot.center(), 4.0, dot);
            ui.label(egui::RichText::new(self.connection_words(&words)).size(13.0).color(argb(LABEL)));
        });
        ui.add_space(10.0);

        label(ui, "Model (the server's workflows/procraft/)");
        egui::Frame::NONE.fill(argb(GROUP_FILL)).corner_radius(10).inner_margin(6).show(ui, |ui| {
            ui.set_width(width - 12.0);
            ScrollArea::vertical().id_salt("generate workflows").max_height(150.0).show(ui, |ui| {
                ui.set_width(width - 12.0);
                if self.workflows.is_empty() {
                    let words = match self.workflow() {
                        Some(name) => format!("{name} (Connect to list the others)"),
                        None => "Connect to list the workflows".to_owned(),
                    };
                    ui.weak(words);
                }
                for w in &self.workflows {
                    let name = w.name.trim_end_matches(".json");
                    match &w.problem {
                        None => {
                            let kind = if w.takes_image { "sketch → image" } else { "text → image" };
                            if ui.selectable_label(self.workflow == w.name, format!("{name}   ·  {kind}")).clicked() {
                                self.workflow = w.name.clone();
                            }
                        }
                        Some(problem) => {
                            ui.weak(format!("{name}: {problem}"));
                        }
                    }
                }
            });
        });
        ui.add_space(10.0);

        if let Some(name) = self.workflow().map(str::to_owned) {
            label(ui, "Negative prompt for this model");
            let negative = self.negatives.entry(name).or_default();
            let field = egui::TextEdit::multiline(negative).hint_text("Empty: the workflow's own").desired_rows(3);
            ui.add_sized([width, 70.0], field);
        }
        act
    }

    /// The connection's colour and a word or two for it.
    fn connection_line(&self) -> (egui::Color32, String) {
        match &self.connection {
            Connection::Unknown => (argb(TAB_IDLE), "Not connected".to_owned()),
            Connection::Connecting => (argb(CONNECTING), "Connecting…".to_owned()),
            Connection::Connected { .. } => (argb(DONE), self.host()),
            Connection::Failed(_) => (argb(FAILED), "Offline".to_owned()),
        }
    }

    /// The settings page's longer line on the connection.
    fn connection_words(&self, short: &str) -> String {
        match &self.connection {
            Connection::Connected { listed: 0, .. } => format!("Connected to {short}: no workflows in workflows/procraft/"),
            Connection::Connected { listed, usable } if listed == usable => format!("Connected to {short}: {listed} workflows"),
            Connection::Connected { listed, usable } => format!("Connected to {short}: {usable} of {listed} workflows usable"),
            Connection::Failed(why) => why.clone(),
            _ => short.to_owned(),
        }
    }

    /// The server's host and port, without its scheme.
    fn host(&self) -> String {
        let s = self.server.trim();
        s.split_once("://").map_or(s, |(_, rest)| rest).trim_end_matches('/').to_owned()
    }
}

/// A field's name, above it.
fn label(ui: &mut Ui, words: &str) {
    ui.label(egui::RichText::new(words).size(14.0).color(argb(LABEL)));
}

const RING: Themed = Themed(0xFFFFFFFF, 0xFF0076FD);
const RING_TRACK: Themed = Themed(0x40FFFFFF, 0x30000000);
const DONE: u32 = 0xFF34C759;
const FAILED: u32 = 0xFFFF453A;

/// Draws the job's state round the bar button's disc at `centre`, `disc` across: a ring filling
/// with the share done, a spinning arc while it can't be told, and for a moment after it ends a
/// green or red ring. Asks for the next frame while anything moves.
pub fn draw_status(ui: &Ui, centre: egui::Pos2, disc: f32, status: BarStatus) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let radius = disc / 2.0 + 3.5;
    let painter = ui.painter();
    let time = ui.input(|i| i.time) as f32;
    let arc = |from: f32, sweep: f32, colour: egui::Color32| {
        let steps = ((sweep.abs() / TAU) * 64.0).ceil().max(2.0) as usize;
        let points: Vec<egui::Pos2> = (0..=steps)
            .map(|k| {
                let a = from + sweep * k as f32 / steps as f32;
                centre + vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(2.5, colour)));
    };
    match status {
        BarStatus::Idle => return,
        BarStatus::Working(None) => {
            painter.circle_stroke(centre, radius, egui::Stroke::new(2.5, argb(RING_TRACK)));
            arc(time * TAU, TAU * 0.28, argb(RING));
        }
        BarStatus::Working(Some(done)) => {
            painter.circle_stroke(centre, radius, egui::Stroke::new(2.5, argb(RING_TRACK)));
            if done > 0.0 {
                arc(-FRAC_PI_2, TAU * done.clamp(0.0, 1.0), argb(RING));
            }
        }
        BarStatus::Done | BarStatus::Failed => {
            let colour = argb(if status == BarStatus::Done { DONE } else { FAILED });
            painter.circle_stroke(centre, radius, egui::Stroke::new(2.5, colour));
        }
    }
    ui.ctx().request_repaint();
}

/// How lit the glyph is while a job runs: a slow pulse, 0 to 1.
pub fn pulse(ui: &Ui, status: BarStatus) -> f32 {
    match status {
        BarStatus::Working(_) => {
            let time = ui.input(|i| i.time) as f32;
            0.5 + 0.5 * (time * std::f32::consts::TAU / 1.6).sin()
        }
        _ => 0.0,
    }
}

const BADGE_FILL: Themed = Themed(0xE6141414, 0xF0F2F2F2);
const BADGE_TEXT: Themed = Themed(0xFFFFFFFF, 0xFF222222);

/// While the panel is closed, a small badge under the bar button at `button` saying how the job
/// goes: Queued, its share done, or how it ended (fading with the ring's moment after).
pub fn badge(ctx: &egui::Context, button: Rect, status: BarStatus) {
    let words = match status {
        BarStatus::Idle => return,
        BarStatus::Working(None) => "Queued".to_owned(),
        BarStatus::Working(Some(done)) => format!("{}%", (done.clamp(0.0, 1.0) * 100.0).round()),
        BarStatus::Done => "Done".to_owned(),
        BarStatus::Failed => "Failed".to_owned(),
    };
    let accent = match status {
        BarStatus::Done => Some(argb(DONE)),
        BarStatus::Failed => Some(argb(FAILED)),
        _ => None,
    };
    let size = vec2(64.0, 22.0);
    let rect = Rect::from_center_size(pos2(button.center().x, button.bottom() + 6.0 + size.y / 2.0), size);
    egui::Area::new(Id::unique("generate badge")).fixed_pos(rect.min).order(Order::Foreground).interactable(false).show(
        ctx,
        |ui| {
            let painter = ui.painter();
            painter.rect_filled(rect, 11, argb(BADGE_FILL));
            if let Some(colour) = accent {
                painter.rect_stroke(rect, 11, egui::Stroke::new(1.5, colour), egui::StrokeKind::Inside);
            }
            text(painter, rect.center(), Align2::CENTER_CENTER, &words, 13.0, Weight::SemiBold, argb(BADGE_TEXT));
        },
    );
}
