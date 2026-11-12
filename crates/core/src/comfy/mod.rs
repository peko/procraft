//! Generate: the canvas and a prompt sent to a ComfyUI server through one of the workflows kept
//! there for ProCraft (in its `workflows/procraft/` folder, in the API format), and the picture it
//! makes taken back as a new layer, one step undo takes back.
//!
//! [`comfy_workflows`] lists them; [`CanvasSession::comfy_generate`] starts a [`ComfyJob`] on a
//! thread of its own, which the interface asks how it goes each frame; once done,
//! [`CanvasSession::comfy_put_result`] makes its layer.

mod client;
mod workflow;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use procraft_engine::document::LayerId;
use procraft_engine::geom::IRect;
use serde_json::Value;

use crate::CanvasSession;
use client::Server;

/// The longest the canvas sent may be on its longer side: the workflow scales it to its own size.
const SENT_SIDE: u32 = 2048;
/// The longest a job may take, queued and running.
const MOST_WAIT: Duration = Duration::from_secs(15 * 60);
/// How often the history is asked while the websocket says nothing.
const HISTORY_EVERY: Duration = Duration::from_millis(1500);

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ComfyError {
    #[error("{reason}")]
    Failed { reason: String },
}

impl From<String> for ComfyError {
    fn from(reason: String) -> ComfyError {
        ComfyError::Failed { reason }
    }
}

/// A workflow the server keeps for ProCraft.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ComfyWorkflow {
    /// Its name in ProCraft's folder there, such as `moss-sketch.json`.
    pub name: String,
    /// It starts from the canvas (image to image), else from the prompt alone.
    pub takes_image: bool,
    /// Why it can't be used, if it can't.
    pub problem: Option<String>,
}

/// The workflows the server at `server` keeps for ProCraft. It waits on the server: call it off
/// the interface's thread.
#[uniffi::export]
pub fn comfy_workflows(server: String) -> Result<Vec<ComfyWorkflow>, ComfyError> {
    let server = Server::new(&server)?;
    let names = server.workflows()?;
    Ok(names
        .into_iter()
        .map(|name| match server.workflow(&name).and_then(|w| workflow::describe(&w)) {
            Ok(shape) => ComfyWorkflow { name, takes_image: shape.takes_image, problem: None },
            Err(problem) => ComfyWorkflow { name, takes_image: false, problem: Some(problem) },
        })
        .collect())
}

/// What a job is asked to make.
#[derive(Clone, Debug, uniffi::Record)]
pub struct ComfyRequest {
    pub server: String,
    /// The workflow's name in ProCraft's folder.
    pub workflow: String,
    pub prompt: String,
    pub negative: String,
    /// How far from the canvas, 0 to 1 (image to image only).
    pub strength: f32,
    /// A seed, or none for a new one each time.
    pub seed: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ComfyPhase {
    /// The canvas sent and the workflow filled in.
    Sending,
    /// Waiting in the server's queue.
    Queued,
    Running,
    /// Its picture is here, for [`CanvasSession::comfy_put_result`].
    Done,
    Failed,
    Cancelled,
}

/// How a job goes.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ComfyStatus {
    pub phase: ComfyPhase,
    /// How far it has come, 0 to 1, or below 0 while that can't be told.
    pub progress: f32,
    /// A few words on it: the step it's on, or why it failed.
    pub message: String,
}

/// A picture the server made: straight RGBA8, its rows.
struct Made {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// A job sent to the server, followed on a thread of its own.
#[derive(uniffi::Object)]
pub struct ComfyJob {
    status: Mutex<ComfyStatus>,
    made: Mutex<Option<Made>>,
    cancel: AtomicBool,
    /// The prompt, for the layer's name.
    prompt: String,
    /// The seed it makes its picture with: the one asked for, or one picked as it started.
    seed: u64,
}

#[uniffi::export]
impl ComfyJob {
    pub fn status(&self) -> ComfyStatus {
        self.status.lock().clone()
    }

    /// Calls it off: out of the server's queue, or stopped there.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The seed it makes its picture with, to make the same again.
    pub fn seed(&self) -> u64 {
        self.seed
    }
}

impl ComfyJob {
    fn set(&self, phase: ComfyPhase, progress: f32, message: impl Into<String>) {
        *self.status.lock() = ComfyStatus { phase, progress, message: message.into() };
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// How far a job has come, as krita-ai-diffusion counts it: a fifth by the nodes run (one more
/// than there are, so it can't reach the end before the picture), four fifths by the samplers'
/// steps, each sampler a like share.
struct Progress {
    nodes: usize,
    ran: usize,
    /// The samplers in the workflow (at least one), those finished, and the share of the one running.
    samplers: usize,
    sampled: usize,
    steps: f32,
}

impl Progress {
    fn new(graph: &Value) -> Progress {
        let nodes = graph.as_object().map_or(0, serde_json::Map::len);
        let samplers = graph.as_object().map_or(0, |n| {
            n.values().filter(|node| node.get("inputs").and_then(|i| i.get("steps")).is_some()).count()
        });
        Progress { nodes, ran: 0, samplers: samplers.max(1), sampled: 0, steps: 0.0 }
    }

    /// Another node starts: a sampler that was running has finished.
    fn next_node(&mut self) {
        self.ran += 1;
        if self.steps > 0.0 {
            self.sampled += 1;
            self.steps = 0.0;
        }
    }

    fn value(&self) -> f32 {
        let nodes = self.ran as f32 / (self.nodes + 1) as f32;
        let sampled = ((self.sampled as f32 + self.steps) / self.samplers as f32).min(1.0);
        (0.2 * nodes + 0.8 * sampled).clamp(0.0, 0.99)
    }
}

/// A new seed: the clock's nanoseconds, mixed (splitmix64), kept within what JavaScript's numbers
/// (and so ComfyUI's interface) hold exactly.
fn new_seed() -> u64 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) & ((1 << 53) - 1)
}

/// The canvas as a PNG to send: straight `rgba` laid on white (a workflow reads no alpha), at most
/// `SENT_SIDE` on its longer side.
fn canvas_png(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut picture = image::RgbaImage::from_raw(width, height, rgba).ok_or("the canvas's picture is short")?;
    for px in picture.pixels_mut() {
        let [r, g, b, a] = px.0;
        let a = u32::from(a);
        let on_white = |c: u8| ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
        px.0 = [on_white(r), on_white(g), on_white(b), 255];
    }
    let longer = width.max(height);
    if longer > SENT_SIDE {
        let scale = SENT_SIDE as f32 / longer as f32;
        let (w, h) = (((width as f32 * scale).round() as u32).max(1), ((height as f32 * scale).round() as u32).max(1));
        picture = image::imageops::resize(&picture, w, h, image::imageops::FilterType::Triangle);
    }
    let rgb = image::DynamicImage::ImageRgba8(picture).to_rgb8();
    let mut png = Vec::new();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png)
}

/// Runs `job` to its end: the canvas `png` sent (if the workflow takes it), the job queued and
/// followed, its picture taken back.
fn run(job: &ComfyJob, request: &ComfyRequest, canvas: (u32, u32), png: Vec<u8>) -> Result<(), String> {
    let server = Server::new(&request.server)?;
    let graph = server.workflow(&request.workflow)?;
    run_graph(job, &server, graph, request, canvas, png)
}

/// As [`run`], with the workflow `graph` in hand.
fn run_graph(
    job: &ComfyJob,
    server: &Server,
    mut graph: Value,
    request: &ComfyRequest,
    canvas: (u32, u32),
    png: Vec<u8>,
) -> Result<(), String> {
    let shape = workflow::describe(&graph).map_err(|p| format!("{}: {p}", request.workflow))?;
    let uploaded = if shape.takes_image { Some(server.upload(&png, "procraft_canvas.png")?) } else { None };
    if job.cancelled() {
        job.set(ComfyPhase::Cancelled, -1.0, "Called off");
        return Ok(());
    }
    let inputs = workflow::Inputs {
        image: uploaded.as_deref(),
        prompt: &request.prompt,
        negative: &request.negative,
        strength: request.strength,
        seed: job.seed,
        canvas,
    };
    workflow::apply(&mut graph, &inputs)?;

    // listening before it's queued, so nothing it says is missed
    let client = format!("procraft-{:x}", new_seed());
    let mut socket = server.listen(&client);
    log::info!("generate: progress {}", if socket.is_some() { "from the websocket" } else { "from the history alone" });
    let id = server.queue(&graph, &client)?;
    job.set(ComfyPhase::Queued, -1.0, "Queued");

    let started = Instant::now();
    let mut asked = Instant::now();
    let mut progress = Progress::new(&graph);
    let entry = loop {
        if job.cancelled() {
            server.cancel(&id);
            job.set(ComfyPhase::Cancelled, -1.0, "Called off");
            return Ok(());
        }
        if started.elapsed() > MOST_WAIT {
            server.cancel(&id);
            return Err("The job took too long".to_owned());
        }
        let mut ended = false;
        if let Some(ws) = socket.as_mut() {
            match ws.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    let message: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    ended = follow(job, &id, &message, &mut progress)?;
                }
                Ok(_) => {}
                Err(e) if client::timed_out(&e) => {}
                // the socket gone: the history goes on telling
                Err(_) => socket = None,
            }
        } else {
            std::thread::sleep(Duration::from_millis(250));
        }
        if ended || asked.elapsed() >= HISTORY_EVERY {
            asked = Instant::now();
            if let Some(entry) = server.history(&id)? {
                break entry;
            }
        }
    };

    let status = entry.get("status");
    if status.and_then(|s| s.get("status_str")).and_then(Value::as_str) == Some("error") {
        return Err(history_error(status).unwrap_or_else(|| "The job failed on the server".to_owned()));
    }
    let image = entry
        .get("outputs")
        .and_then(Value::as_object)
        .and_then(|outputs| {
            outputs.values().find_map(|o| o.get("images").and_then(Value::as_array).and_then(|i| i.first()).cloned())
        })
        .ok_or("The workflow made no picture (it needs a SaveImage or PreviewImage)")?;
    let bytes = server.view(&image)?;
    let picture = image::load_from_memory(&bytes).map_err(|e| format!("The picture didn't read: {e}"))?.to_rgba8();
    let (width, height) = picture.dimensions();
    *job.made.lock() = Some(Made { width, height, rgba: picture.into_raw() });
    job.set(ComfyPhase::Done, 1.0, format!("Done in {:.0} s", started.elapsed().as_secs_f32()));
    Ok(())
}

/// Takes in one websocket `message` about job `id`: whether it ended, or why it failed.
fn follow(job: &ComfyJob, id: &str, message: &Value, progress: &mut Progress) -> Result<bool, String> {
    let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
    let data = message.get("data").unwrap_or(&Value::Null);
    if data.get("prompt_id").and_then(Value::as_str).is_some_and(|p| p != id) {
        return Ok(false);
    }
    match kind {
        "execution_start" => job.set(ComfyPhase::Running, progress.value(), "Starting"),
        "execution_cached" => {
            progress.ran += data.get("nodes").and_then(Value::as_array).map_or(0, Vec::len);
            job.set(ComfyPhase::Running, progress.value(), "Running");
        }
        "executing" if data.get("node").is_some_and(Value::is_null) => return Ok(true),
        "executing" => {
            progress.next_node();
            job.set(ComfyPhase::Running, progress.value(), "Running");
        }
        "progress" => {
            let value = data.get("value").and_then(Value::as_f64).unwrap_or(0.0);
            let max = data.get("max").and_then(Value::as_f64).unwrap_or(1.0).max(1.0);
            progress.steps = (value / max) as f32;
            job.set(ComfyPhase::Running, progress.value(), format!("Step {value:.0} of {max:.0}"));
        }
        "execution_success" => return Ok(true),
        "execution_error" => {
            let why = data.get("exception_message").and_then(Value::as_str).unwrap_or("it failed on the server");
            let node = data.get("node_type").and_then(Value::as_str).unwrap_or("");
            return Err(if node.is_empty() { why.trim().to_owned() } else { format!("{node}: {}", why.trim()) });
        }
        "execution_interrupted" => return Err("Stopped on the server".to_owned()),
        _ => {}
    }
    Ok(false)
}

/// Why the history says a job failed.
fn history_error(status: Option<&Value>) -> Option<String> {
    let messages = status?.get("messages")?.as_array()?;
    messages.iter().find_map(|m| {
        let (kind, data) = (m.get(0)?.as_str()?, m.get(1)?);
        (kind == "execution_error").then(|| data.get("exception_message")?.as_str().map(|s| s.trim().to_owned()))?
    })
}

/// `made` stretched to `width` × `height` and premultiplied, as a layer takes it.
fn fitted(made: Made, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut picture = image::RgbaImage::from_raw(made.width, made.height, made.rgba)?;
    if (made.width, made.height) != (width, height) {
        picture = image::imageops::resize(&picture, width, height, image::imageops::FilterType::Lanczos3);
    }
    let mut rgba = picture.into_raw();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
    Some(rgba)
}

#[uniffi::export]
impl CanvasSession {
    /// Starts a job making `request` from the canvas as it looks now; ask it how it goes.
    pub fn comfy_generate(&self, request: ComfyRequest) -> Arc<ComfyJob> {
        let job = Arc::new(ComfyJob {
            status: Mutex::new(ComfyStatus { phase: ComfyPhase::Sending, progress: -1.0, message: "Sending".into() }),
            made: Mutex::new(None),
            cancel: AtomicBool::new(false),
            prompt: request.prompt.clone(),
            // past what JavaScript's numbers hold, ComfyUI's interface would round it
            seed: request.seed.map_or_else(new_seed, |s| s & ((1 << 53) - 1)),
        });
        // the canvas read here, on the interface's thread, as an export reads it
        let picture = self.finished_picture().map(|(w, h, rgba, _)| (w, h, rgba));
        let info = self.info();
        let canvas = (info.width, info.height);
        let worker = Arc::clone(&job);
        let started = std::thread::Builder::new().name("comfy".into()).spawn(move || {
            let png = match picture {
                Ok((w, h, rgba)) => canvas_png(w, h, rgba),
                Err(e) => Err(e.to_string()),
            };
            let ran = png.and_then(|png| run(&worker, &request, canvas, png));
            if let Err(why) = ran {
                log::warn!("generate: {why}");
                worker.set(ComfyPhase::Failed, -1.0, why);
            }
        });
        if let Err(e) = started {
            job.set(ComfyPhase::Failed, -1.0, format!("Couldn't start: {e}"));
        }
        job
    }

    /// The picture `job` made, as a new layer over the current one, named for its prompt, as one
    /// step undo takes back. False if it has none (yet) or the artwork has no room for a layer.
    pub fn comfy_put_result(&self, job: Arc<ComfyJob>) -> bool {
        let Some(made) = job.made.lock().take() else { return false };
        let info = self.info();
        let (width, height) = (info.width, info.height);
        let Some(pixels) = fitted(made, width, height) else { return false };
        let _step = self.plugin_step();
        let id = self.add_layer_quietly();
        if id == 0 {
            return false;
        }
        {
            let mut guard = self.shared.lock();
            let st = &mut *guard;
            st.rewrite_layer(LayerId(id), IRect::from_size(width, height), pixels);
            self.shared.frame(st);
        }
        let words: String = job.prompt.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(40).collect();
        self.rename_layer(id, if words.is_empty() { "Generated".to_owned() } else { format!("AI: {words}") });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_counts_a_fifth_by_nodes_and_never_goes_back_or_ends_before_the_picture() {
        let graph = serde_json::json!({
            "1": {"class_type": "LoadImage", "inputs": {}},
            "2": {"class_type": "KSampler", "inputs": {"steps": 8}},
            "3": {"class_type": "VAEDecode", "inputs": {}}
        });
        let mut p = Progress::new(&graph);
        assert_eq!(p.value(), 0.0);
        p.next_node();
        p.next_node();
        p.steps = 1.0;
        let sampled = p.value();
        // the decoder after the sampler keeps what the sampler made
        p.next_node();
        assert!(p.value() >= sampled);
        assert!(p.value() < 1.0 && p.value() > 0.9);
    }

    #[test]
    fn seeds_stay_within_a_javascript_number() {
        assert!(new_seed() < (1 << 53));
    }

    #[test]
    fn the_canvas_is_sent_on_white_and_the_picture_taken_back_premultiplied() {
        // a clear pixel goes white; an opaque one stays
        let png = canvas_png(2, 1, vec![0, 0, 0, 0, 10, 20, 30, 255]).unwrap();
        let back = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(back.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(back.get_pixel(1, 0).0, [10, 20, 30]);
        let fitted = fitted(Made { width: 1, height: 1, rgba: vec![200, 100, 50, 128] }, 1, 1).unwrap();
        assert_eq!(fitted, vec![100, 50, 25, 128]);
    }

    #[test]
    fn a_failed_job_says_why_from_the_history() {
        let status = serde_json::json!({"status_str": "error", "messages": [
            ["execution_start", {}], ["execution_error", {"exception_message": " out of memory\n"}]]});
        assert_eq!(history_error(Some(&status)), Some("out of memory".to_owned()));
    }

    /// The server's ProCraft workflows, as Connect lists them: `COMFY_URL=http://host:8188 cargo test
    /// -p procraft-core comfy_list_live -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a ComfyUI server"]
    fn comfy_list_live() {
        let listed = comfy_workflows(std::env::var("COMFY_URL").unwrap_or_default()).unwrap();
        for w in &listed {
            println!("{} takes_image={} problem={:?}", w.name, w.takes_image, w.problem);
        }
        assert!(!listed.is_empty());
    }

    /// Against a real server: `COMFY_URL=http://host:8188 COMFY_WORKFLOW=path/to/api.json
    /// COMFY_SKETCH=path/to/sketch.png cargo test -p procraft-core comfy_live -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a ComfyUI server"]
    fn comfy_live() {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let graph: Value = serde_json::from_slice(&std::fs::read(var("COMFY_WORKFLOW")).unwrap()).unwrap();
        let sketch = image::open(var("COMFY_SKETCH")).unwrap().to_rgba8();
        let (w, h) = sketch.dimensions();
        let png = canvas_png(w, h, sketch.into_raw()).unwrap();
        let job = ComfyJob {
            status: Mutex::new(ComfyStatus { phase: ComfyPhase::Sending, progress: -1.0, message: String::new() }),
            made: Mutex::new(None),
            cancel: AtomicBool::new(false),
            seed: 7,
            prompt: "a cosy cabin in a snowy forest at dusk, warm light in the windows".into(),
        };
        let request = ComfyRequest {
            server: var("COMFY_URL"),
            workflow: "local".into(),
            prompt: job.prompt.clone(),
            negative: String::new(),
            strength: 0.9,
            seed: None,
        };
        println!("socket: {}", Server::new(&var("COMFY_URL")).unwrap().listen("procraft-probe").is_some());
        let server = Server::new(&request.server).unwrap();
        std::thread::scope(|scope| {
            let watcher = scope.spawn(|| {
                let mut last = String::new();
                loop {
                    let s = job.status();
                    let line = format!("{:?} {:.2} {}", s.phase, s.progress, s.message);
                    if line != last {
                        println!("{line}");
                        last = line;
                    }
                    if matches!(s.phase, ComfyPhase::Done | ComfyPhase::Failed | ComfyPhase::Cancelled) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let ran = run_graph(&job, &server, graph, &request, (w, h), png);
            if let Err(why) = &ran {
                job.set(ComfyPhase::Failed, -1.0, why.clone());
            }
            let _ = watcher.join();
            ran.unwrap();
        });
        let made = job.made.lock().take().unwrap();
        println!("made {} x {}", made.width, made.height);
        let out = std::env::temp_dir().join("procraft_comfy_live.png");
        image::RgbaImage::from_raw(made.width, made.height, made.rgba).unwrap().save(&out).unwrap();
        println!("saved {}", out.display());
    }
}
