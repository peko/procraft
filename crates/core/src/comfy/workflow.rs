//! A ProCraft workflow, as kept on a ComfyUI server in its API format ("Export (API)"): which of its
//! nodes take the canvas, the prompt, the strength, the seed and the size, and those filled in.
//!
//! A node is bound by its title (`_meta.title`), which may name several roles:
//! `procraft:image`, `procraft:prompt`, `procraft:negative`, `procraft:strength`, `procraft:seed`,
//! `procraft:size`, `procraft:empty`, `procraft:encode`, `procraft:steps`. Without titles, the first
//! `LoadImage` takes the canvas, the first text encoder the prompt, and every sampler's seed is set.
//!
//! Strength works as krita-ai-diffusion's: at 100 % a picture is made new (Generate), below it the
//! canvas is refined (Refine). A workflow with both an empty latent (`procraft:empty`) and the
//! canvas encoded (`procraft:encode`) starts from the one the strength asks for, what takes the
//! other wired to it instead, and the other left out (at 100 % the canvas isn't even sent). The
//! strength itself goes where `procraft:strength` is:
//! - on a `SplitSigmas` (krita's way), or a `KSamplerAdvanced`: the last steps of the model's own
//!   schedule are run, as many as the strength asks for and at least [`MIN_STEPS`]; the schedule's
//!   steps are the `SplitSigmas`'s scheduler's (or the node titled `procraft:steps`);
//! - else on its `denoise` (or `image_strength`, `strength`), as a plain `KSampler` takes it.
//!
//! A text node whose words hold `{prompt}` keeps them, the prompt in its place (a model's quality
//! tags around it, say); else the prompt takes its words. An empty negative leaves the workflow's own.

use serde_json::{Map, Value};

/// What the panel fills in.
pub(crate) struct Inputs<'a> {
    /// The canvas's name on the server, once uploaded.
    pub image: Option<&'a str>,
    pub prompt: &'a str,
    pub negative: &'a str,
    /// How far from the canvas, 0 to 1.
    pub strength: f32,
    pub seed: u64,
    /// The canvas's size: what a `procraft:size` node makes, kept about a megapixel.
    pub canvas: (u32, u32),
}

/// What a workflow takes, found before it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    /// It starts from the canvas (image to image), else from the prompt alone.
    pub takes_image: bool,
    /// It can start from an empty latent instead, at 100 % (`procraft:empty` and `procraft:encode`).
    pub starts_empty: bool,
}

impl Shape {
    /// Whether a job at `strength` sends the canvas.
    pub(crate) fn sends_canvas(&self, strength: f32) -> bool {
        self.takes_image && !(self.starts_empty && strength >= 1.0)
    }
}

/// The fewest steps a refine runs, as krita-ai-diffusion's sampler presets have it.
pub(crate) const MIN_STEPS: u32 = 4;

/// A refine at `strength` (below 1) over a schedule of `steps`: the steps the schedule is made of,
/// and the one it starts at, as krita-ai-diffusion's `apply_strength` (the schedule lengthened when
/// fewer than [`MIN_STEPS`] would be left).
pub(crate) fn split_steps(strength: f32, steps: u32) -> (u32, u32) {
    let strength = strength.clamp(0.01, 1.0);
    let start = (steps as f32 * (1.0 - strength)).round() as u32;
    if steps.saturating_sub(start) < MIN_STEPS {
        let steps = (MIN_STEPS as f32 / strength).floor() as u32;
        return (steps, steps.saturating_sub(MIN_STEPS));
    }
    (steps, start)
}

/// The roles a node's title gives it.
fn roles(node: &Value) -> Vec<&str> {
    let title = node.get("_meta").and_then(|m| m.get("title")).and_then(Value::as_str).unwrap_or("");
    title.split(|c: char| c.is_whitespace() || c == ',').filter_map(|w| w.strip_prefix("procraft:")).collect()
}

fn class(node: &Value) -> &str {
    node.get("class_type").and_then(Value::as_str).unwrap_or("")
}

/// The nodes' ids in the order they were numbered (the map keeps them as text, "10" before "2").
fn ids(nodes: &Map<String, Value>) -> Vec<String> {
    let mut ids: Vec<String> = nodes.keys().cloned().collect();
    ids.sort_by_key(|id| (id.parse::<u64>().unwrap_or(u64::MAX), id.clone()));
    ids
}

/// The workflow's nodes, or why it can't be used.
fn nodes(workflow: &Value) -> Result<&Map<String, Value>, String> {
    if workflow.get("nodes").is_some() {
        return Err("saved in the editor's format: save it with Export (API)".to_owned());
    }
    let nodes = workflow.as_object().ok_or("not a workflow")?;
    if nodes.is_empty() || nodes.values().any(|n| n.get("class_type").is_none()) {
        return Err("not a workflow in the API format".to_owned());
    }
    Ok(nodes)
}

/// The node that plays `role`: the one titled so, else the first that `fallback` takes.
fn find(nodes: &Map<String, Value>, role: &str, fallback: impl Fn(&Value) -> bool) -> Option<String> {
    let order = ids(nodes);
    let titled = order.iter().find(|id| nodes.get(*id).is_some_and(|n| roles(n).contains(&role)));
    titled.or_else(|| order.iter().find(|id| nodes.get(*id).is_some_and(&fallback))).cloned()
}

fn is_text_encoder(node: &Value) -> bool {
    let c = class(node);
    (c.contains("TextEncode") || c.contains("TextEncoder")) && text_field(node).is_some()
}

/// The input a text node takes its words in.
fn text_field(node: &Value) -> Option<&'static str> {
    let inputs = node.get("inputs")?;
    ["text", "prompt", "value"].into_iter().find(|f| inputs.get(*f).is_some_and(Value::is_string))
}

/// The canvas's node, the prompt's, and the negative's, as the workflow has them.
fn parts(nodes: &Map<String, Value>) -> (Option<String>, Option<String>, Option<String>) {
    let image = find(nodes, "image", |n| class(n) == "LoadImage");
    let prompt = find(nodes, "prompt", is_text_encoder);
    let negative = find(nodes, "negative", |_| false);
    (image, prompt, negative)
}

/// What `workflow` takes, or why it can't be used.
pub(crate) fn describe(workflow: &Value) -> Result<Shape, String> {
    let nodes = nodes(workflow)?;
    let (image, prompt, _) = parts(nodes);
    if prompt.is_none() {
        return Err("no node for the prompt: title one procraft:prompt".to_owned());
    }
    let starts_empty = find(nodes, "empty", |_| false).is_some() && find(nodes, "encode", |_| false).is_some();
    Ok(Shape { takes_image: image.is_some(), starts_empty })
}

/// Whether `value` is a link to node `id` (`[id, output]`).
fn links_to(value: &Value, id: &str) -> bool {
    value.as_array().and_then(|l| l.first()).and_then(Value::as_str) == Some(id)
}

/// Every link to node `from` made a link to `to` (same output).
fn rewire(nodes: &mut Map<String, Value>, from: &str, to: &str) {
    for node in nodes.values_mut() {
        let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) else { continue };
        for value in inputs.values_mut() {
            if links_to(value, from)
                && let Some(link) = value.as_array_mut()
                && let Some(first) = link.first_mut()
            {
                *first = Value::from(to);
            }
        }
    }
}

/// Takes node `id` out if nothing links to it any more, and so on up what it took in.
fn prune(nodes: &mut Map<String, Value>, id: &str) {
    let used = nodes.values().any(|n| n.get("inputs").and_then(Value::as_object).is_some_and(|i| i.values().any(|v| links_to(v, id))));
    if used {
        return;
    }
    let Some(node) = nodes.remove(id) else { return };
    let sources: Vec<String> = node
        .get("inputs")
        .and_then(Value::as_object)
        .map(|i| i.values().filter_map(|v| v.as_array()?.first()?.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    for source in sources {
        prune(nodes, &source);
    }
}

/// The steps of the schedule a `SplitSigmas` (`id`) splits: the node titled `procraft:steps`, else
/// the one its sigmas come from.
fn schedule_of(nodes: &Map<String, Value>, id: &str) -> Option<String> {
    find(nodes, "steps", |_| false).or_else(|| {
        let from = nodes.get(id)?.get("inputs")?.get("sigmas")?.as_array()?.first()?.as_str()?;
        has(nodes, from, "steps").then(|| from.to_owned())
    })
}

/// Sets `strength` on node `id` the way it takes it (see the module's notes).
fn set_strength(nodes: &mut Map<String, Value>, id: &str, strength: f32) {
    let steps_of = |nodes: &Map<String, Value>, at: &str| {
        nodes.get(at).and_then(|n| n.get("inputs")).and_then(|i| i.get("steps")).and_then(Value::as_u64).map(|s| s as u32)
    };
    // (where the schedule is, the input the start goes in)
    let split = if has(nodes, id, "step") {
        schedule_of(nodes, id).map(|at| (at, "step"))
    } else if has(nodes, id, "start_at_step") {
        Some((id.to_owned(), "start_at_step"))
    } else {
        None
    };
    if let Some((at, start_field)) = split {
        let Some(steps) = steps_of(nodes, &at) else { return };
        let (steps, start) = if strength >= 1.0 { (steps, 0) } else { split_steps(strength, steps) };
        set(nodes, &at, "steps", Value::from(steps));
        set(nodes, id, start_field, Value::from(start));
        return;
    }
    if let Some(field) = ["denoise", "image_strength", "strength"].into_iter().find(|f| has(nodes, id, f)) {
        set(nodes, id, field, Value::from(f64::from(strength)));
    }
}

/// A size near a megapixel with the canvas's proportions, each side a multiple of 16.
pub(crate) fn megapixel_size((w, h): (u32, u32)) -> (u32, u32) {
    let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let scale = (1024.0 * 1024.0 / (w * h)).sqrt();
    let side = |v: f64| ((v * scale / 16.0).round().max(1.0) as u32) * 16;
    (side(w), side(h))
}

/// Sets `field` of node `id`'s inputs to `value`.
fn set(nodes: &mut Map<String, Value>, id: &str, field: &str, value: Value) {
    if let Some(inputs) = nodes.get_mut(id).and_then(|n| n.get_mut("inputs")).and_then(Value::as_object_mut) {
        inputs.insert(field.to_owned(), value);
    }
}

/// Whether node `id` has a plain (unconnected) input `field`.
fn has(nodes: &Map<String, Value>, id: &str, field: &str) -> bool {
    nodes.get(id).and_then(|n| n.get("inputs")).and_then(|i| i.get(field)).is_some_and(|v| !v.is_array())
}

/// `workflow` with what the panel says filled in, or why it can't be.
pub(crate) fn apply(workflow: &mut Value, inputs: &Inputs<'_>) -> Result<Shape, String> {
    let shape = describe(workflow)?;
    let nodes = workflow.as_object_mut().ok_or("not a workflow")?;
    let (image, prompt, negative) = parts(nodes);
    if let (Some(id), Some(name)) = (&image, inputs.image) {
        set(nodes, id, "image", Value::from(name));
    }
    for (id, words) in [(&prompt, inputs.prompt), (&negative, inputs.negative)] {
        if words.trim().is_empty() && id == &negative {
            continue;
        }
        let node = id.as_deref().and_then(|id| nodes.get(id));
        let field = node.and_then(text_field);
        if let (Some(id), Some(field)) = (id, field) {
            let had = node.and_then(|n| n.get("inputs")).and_then(|i| i.get(field)).and_then(Value::as_str).unwrap_or("");
            let text = if had.contains("{prompt}") { had.replace("{prompt}", words) } else { words.to_owned() };
            set(nodes, id, field, Value::from(text));
        }
    }
    let strength = inputs.strength.clamp(0.0, 1.0);
    // from an empty latent at 100 %, else from the canvas: what takes the other is wired to it
    if shape.starts_empty
        && let (Some(empty), Some(encode)) = (find(nodes, "empty", |_| false), find(nodes, "encode", |_| false))
    {
        let (unused, used) = if strength >= 1.0 { (encode, empty) } else { (empty, encode) };
        rewire(nodes, &unused, &used);
        prune(nodes, &unused);
    }
    // the strength where the titled node has it, else every sampler's denoise (image to image only)
    let titled = find(nodes, "strength", |_| false);
    let strength_nodes = match titled {
        Some(id) => vec![id],
        None if shape.takes_image => ids(nodes).into_iter().filter(|id| has(nodes, id, "denoise")).collect(),
        None => Vec::new(),
    };
    for id in strength_nodes {
        set_strength(nodes, &id, strength);
    }
    // the seed where titled, else every seed (a fixed one would give the same picture again)
    let seeded: Vec<String> = match find(nodes, "seed", |_| false) {
        Some(id) => vec![id],
        None => ids(nodes).into_iter().filter(|id| has(nodes, id, "seed") || has(nodes, id, "noise_seed")).collect(),
    };
    for id in seeded {
        for field in ["seed", "noise_seed"] {
            if has(nodes, &id, field) {
                set(nodes, &id, field, Value::from(inputs.seed));
            }
        }
    }
    if let Some(id) = find(nodes, "size", |_| false) {
        let (w, h) = megapixel_size(inputs.canvas);
        set(nodes, &id, "width", Value::from(w));
        set(nodes, &id, "height", Value::from(h));
    }
    Ok(shape)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sketch() -> Value {
        json!({
            "5": {"class_type": "LoadImage", "inputs": {"image": "x.png"}, "_meta": {"title": "procraft:image"}},
            "8": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["3", 0], "text": "old"}, "_meta": {"title": "procraft:prompt"}},
            "9": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["3", 0], "text": "neg"}, "_meta": {"title": "procraft:negative"}},
            "10": {"class_type": "KSampler", "inputs": {"seed": 0, "denoise": 0.5, "model": ["2", 0]},
                   "_meta": {"title": "procraft:strength procraft:seed"}}
        })
    }

    fn inputs(image: Option<&str>) -> Inputs<'_> {
        Inputs { image, prompt: "a cabin", negative: "blur", strength: 0.9, seed: 42, canvas: (2048, 1536) }
    }

    #[test]
    fn a_titled_workflow_takes_the_canvas_prompt_strength_and_seed() {
        let mut w = sketch();
        let shape = apply(&mut w, &inputs(Some("canvas.png")));
        assert_eq!(shape, Ok(Shape { takes_image: true, starts_empty: false }));
        assert_eq!(w["5"]["inputs"]["image"], "canvas.png");
        assert_eq!(w["8"]["inputs"]["text"], "a cabin");
        assert_eq!(w["9"]["inputs"]["text"], "blur");
        assert!((w["10"]["inputs"]["denoise"].as_f64().unwrap_or(0.0) - 0.9).abs() < 1e-6);
        assert_eq!(w["10"]["inputs"]["seed"], 42);
        // links stay links
        assert_eq!(w["10"]["inputs"]["model"], json!(["2", 0]));
    }

    #[test]
    fn a_prompt_goes_in_its_place_among_the_tags_and_an_empty_negative_keeps_the_workflows() {
        let mut w = sketch();
        w["8"]["inputs"]["text"] = json!("masterpiece, best quality, {prompt}, highres");
        let mut given = inputs(Some("c.png"));
        given.negative = "  ";
        apply(&mut w, &given).unwrap();
        assert_eq!(w["8"]["inputs"]["text"], "masterpiece, best quality, a cabin, highres");
        assert_eq!(w["9"]["inputs"]["text"], "neg");
    }

    #[test]
    fn untitled_nodes_are_found_by_their_kind() {
        let mut w = json!({
            "3": {"class_type": "CLIPTextEncode", "inputs": {"text": "", "clip": ["1", 1]}},
            "4": {"class_type": "LoadImage", "inputs": {"image": "a.png"}},
            "12": {"class_type": "KSampler", "inputs": {"seed": 1, "denoise": 1.0}}
        });
        assert_eq!(apply(&mut w, &inputs(Some("c.png"))), Ok(Shape { takes_image: true, starts_empty: false }));
        assert_eq!(w["3"]["inputs"]["text"], "a cabin");
        assert_eq!(w["4"]["inputs"]["image"], "c.png");
        assert_eq!(w["12"]["inputs"]["seed"], 42);
    }

    #[test]
    fn text_to_image_keeps_its_denoise_and_takes_the_canvas_size() {
        let mut w = json!({
            "7": {"class_type": "EmptyFlux2LatentImage", "inputs": {"width": 1, "height": 1}, "_meta": {"title": "procraft:size"}},
            "8": {"class_type": "CLIPTextEncode", "inputs": {"text": ""}, "_meta": {"title": "procraft:prompt"}},
            "10": {"class_type": "KSampler", "inputs": {"seed": 0, "denoise": 1.0}}
        });
        assert_eq!(apply(&mut w, &inputs(None)), Ok(Shape { takes_image: false, starts_empty: false }));
        assert_eq!(w["10"]["inputs"]["denoise"], 1.0);
        assert_eq!((w["7"]["inputs"]["width"].clone(), w["7"]["inputs"]["height"].clone()), (json!(1184), json!(880)));
    }

    #[test]
    fn the_editors_format_and_a_workflow_without_a_prompt_are_refused() {
        assert!(describe(&json!({"nodes": [], "links": []})).is_err_and(|e| e.contains("Export (API)")));
        assert!(describe(&json!({"1": {"class_type": "LoadImage", "inputs": {}}})).is_err_and(|e| e.contains("prompt")));
    }

    /// A graph as krita-ai-diffusion builds it, titled for ProCraft.
    fn krita() -> Value {
        json!({
            "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "m"}},
            "4": {"class_type": "CLIPTextEncode", "inputs": {"text": "{prompt}"}, "_meta": {"title": "procraft:prompt"}},
            "6": {"class_type": "LoadImage", "inputs": {"image": "x.png"}, "_meta": {"title": "procraft:image"}},
            "7": {"class_type": "ImageScaleToTotalPixels", "inputs": {"image": ["6", 0]}},
            "8": {"class_type": "VAEEncode", "inputs": {"pixels": ["7", 0]}, "_meta": {"title": "procraft:encode"}},
            "9": {"class_type": "EmptyLatentImage", "inputs": {"width": 1, "height": 1}, "_meta": {"title": "procraft:size procraft:empty"}},
            "11": {"class_type": "BasicScheduler", "inputs": {"model": ["1", 0], "steps": 8}},
            "12": {"class_type": "SplitSigmas", "inputs": {"sigmas": ["11", 0], "step": 1}, "_meta": {"title": "procraft:strength"}},
            "13": {"class_type": "RandomNoise", "inputs": {"noise_seed": 0}, "_meta": {"title": "procraft:seed"}},
            "15": {"class_type": "SamplerCustomAdvanced", "inputs": {"noise": ["13", 0], "sigmas": ["12", 1], "latent_image": ["8", 0]}}
        })
    }

    #[test]
    fn strength_runs_the_last_steps_of_the_schedule_as_krita_does() {
        assert_eq!(split_steps(0.6, 8), (8, 3));
        assert_eq!(split_steps(0.9, 8), (8, 1));
        // fewer than four left: the schedule lengthened
        assert_eq!(split_steps(0.3, 8), (13, 9));
        assert_eq!(split_steps(0.05, 8), (80, 76));
    }

    #[test]
    fn below_100_the_canvas_is_refined_over_the_schedules_last_steps() {
        let mut w = krita();
        let shape = apply(&mut w, &Inputs { strength: 0.6, ..inputs(Some("c.png")) }).unwrap();
        assert!(shape.sends_canvas(0.6) && !shape.sends_canvas(1.0));
        assert_eq!(w["15"]["inputs"]["latent_image"], json!(["8", 0]));
        assert!(w.get("9").is_none(), "the empty latent left out");
        assert_eq!((w["11"]["inputs"]["steps"].clone(), w["12"]["inputs"]["step"].clone()), (json!(8), json!(3)));
        assert_eq!(w["6"]["inputs"]["image"], "c.png");
        assert_eq!(w["13"]["inputs"]["noise_seed"], 42);
    }

    #[test]
    fn at_100_a_picture_is_made_new_without_the_canvas() {
        let mut w = krita();
        apply(&mut w, &Inputs { strength: 1.0, ..inputs(None) }).unwrap();
        assert_eq!(w["15"]["inputs"]["latent_image"], json!(["9", 0]));
        for gone in ["6", "7", "8"] {
            assert!(w.get(gone).is_none(), "{gone} left out");
        }
        assert_eq!((w["11"]["inputs"]["steps"].clone(), w["12"]["inputs"]["step"].clone()), (json!(8), json!(0)));
        assert_eq!((w["9"]["inputs"]["width"].clone(), w["9"]["inputs"]["height"].clone()), (json!(1184), json!(880)));
        assert!(w.get("1").is_some(), "the rest kept");
    }

    #[test]
    fn a_ksampler_advanced_takes_the_start_step() {
        let mut w = json!({
            "4": {"class_type": "CLIPTextEncode", "inputs": {"text": ""}, "_meta": {"title": "procraft:prompt"}},
            "6": {"class_type": "LoadImage", "inputs": {"image": "x.png"}},
            "10": {"class_type": "KSamplerAdvanced", "inputs": {"steps": 8, "start_at_step": 0, "noise_seed": 0},
                   "_meta": {"title": "procraft:strength procraft:seed"}}
        });
        apply(&mut w, &Inputs { strength: 0.75, ..inputs(Some("c.png")) }).unwrap();
        assert_eq!((w["10"]["inputs"]["steps"].clone(), w["10"]["inputs"]["start_at_step"].clone()), (json!(8), json!(2)));
    }

    #[test]
    fn a_megapixel_keeps_the_proportions_in_sixteens() {
        assert_eq!(megapixel_size((1024, 1024)), (1024, 1024));
        let (w, h) = megapixel_size((3000, 1000));
        assert!(w % 16 == 0 && h % 16 == 0 && (w as f32 / h as f32 - 3.0).abs() < 0.1);
    }
}
