//! A ComfyUI server, asked over HTTP (as krita-ai-diffusion's `ComfyClient` asks it, without its
//! custom nodes): the workflows kept for ProCraft, the canvas uploaded, a job queued and followed
//! on the websocket, its picture taken from the history, and a job called off.

use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

/// The folder under the server's workflows that ProCraft's are kept in.
pub(crate) const FOLDER: &str = "procraft";

/// The most bytes a picture or a workflow taken back may be.
const MOST_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) struct Server {
    /// Its address, as `http://host:port`, no slash after.
    base: String,
    agent: ureq::Agent,
}

/// `text` with everything but the unreserved characters percent-encoded, for a path or a query.
fn encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Why a request failed, in a few words for the panel.
fn failed(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

impl Server {
    /// The server at `address` (`host:port`, with or without `http://`).
    pub(crate) fn new(address: &str) -> Result<Server, String> {
        let trimmed = address.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err("No server address".to_owned());
        }
        let base =
            if trimmed.contains("://") { trimmed.to_owned() } else { format!("http://{trimmed}") };
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .build()
            .into();
        Ok(Server { base, agent })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    /// The answer's body if it says 200, else what it says went wrong.
    fn read(what: &str, response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Vec<u8>, String> {
        let mut response = response.map_err(|e| failed(what, e))?;
        let status = response.status();
        let body = response.body_mut().with_config().limit(MOST_BYTES).read_to_vec().map_err(|e| failed(what, e))?;
        if status.is_success() {
            Ok(body)
        } else {
            let text = String::from_utf8_lossy(&body);
            Err(format!("{what}: the server said {} {}", status.as_u16(), comfy_error(&text)))
        }
    }

    fn get_json(&self, what: &str, path: &str) -> Result<Value, String> {
        let body = Server::read(what, self.agent.get(self.url(path)).call())?;
        serde_json::from_slice(&body).map_err(|e| failed(what, e))
    }

    fn post_json(&self, what: &str, path: &str, body: &Value) -> Result<Vec<u8>, String> {
        Server::read(what, self.agent.post(self.url(path)).send_json(body))
    }

    /// The names of the workflows kept for ProCraft (paths under its folder), in order.
    pub(crate) fn workflows(&self) -> Result<Vec<String>, String> {
        let path = format!("api/userdata?dir={}&recurse=true", encoded(&format!("workflows/{FOLDER}")));
        let response = self.agent.get(self.url(&path)).call().map_err(|e| failed("Listing the workflows", e))?;
        // no folder yet: none kept
        if response.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        let body = Server::read("Listing the workflows", Ok(response))?;
        let listed: Vec<String> = serde_json::from_slice(&body).map_err(|e| failed("Listing the workflows", e))?;
        let mut names: Vec<String> = listed.into_iter().filter(|n| n.ends_with(".json")).collect();
        names.sort_by_key(|n| n.to_lowercase());
        Ok(names)
    }

    /// The workflow called `name` in ProCraft's folder.
    pub(crate) fn workflow(&self, name: &str) -> Result<Value, String> {
        let path = format!("api/userdata/{}", encoded(&format!("workflows/{FOLDER}/{name}")));
        self.get_json(&format!("Reading {name}"), &path)
    }

    /// Uploads `png` as the input picture `name`, over any of that name; the name the server keeps it as.
    pub(crate) fn upload(&self, png: &[u8], name: &str) -> Result<String, String> {
        let boundary = "procraft-canvas-7d1f0c2a9e";
        let mut body = Vec::with_capacity(png.len() + 512);
        let field = |body: &mut Vec<u8>, name: &str, value: &str| {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
            );
        };
        field(&mut body, "overwrite", "true");
        field(&mut body, "type", "input");
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(png);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let request = self
            .agent
            .post(self.url("upload/image"))
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .send(&body[..]);
        let answer = Server::read("Sending the canvas", request)?;
        let answer: Value = serde_json::from_slice(&answer).map_err(|e| failed("Sending the canvas", e))?;
        let kept = answer.get("name").and_then(Value::as_str).unwrap_or(name);
        Ok(match answer.get("subfolder").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            Some(folder) => format!("{folder}/{kept}"),
            None => kept.to_owned(),
        })
    }

    /// Queues `workflow` for `client`: the job's id.
    pub(crate) fn queue(&self, workflow: &Value, client: &str) -> Result<String, String> {
        let body = serde_json::json!({ "prompt": workflow, "client_id": client });
        let answer = self.post_json("Queueing the job", "prompt", &body)?;
        let answer: Value = serde_json::from_slice(&answer).map_err(|e| failed("Queueing the job", e))?;
        answer
            .get("prompt_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "Queueing the job: the server gave no id".to_owned())
    }

    /// The job's entry in the history, once it has one (it ended).
    pub(crate) fn history(&self, id: &str) -> Result<Option<Value>, String> {
        let all = self.get_json("Asking how the job went", &format!("history/{}", encoded(id)))?;
        Ok(all.get(id).cloned())
    }

    /// The picture an output names (its file name, folder and kind).
    pub(crate) fn view(&self, image: &Value) -> Result<Vec<u8>, String> {
        let part = |k: &str| encoded(image.get(k).and_then(Value::as_str).unwrap_or(""));
        let path = format!("view?filename={}&subfolder={}&type={}", part("filename"), part("subfolder"), part("type"));
        Server::read("Taking the picture back", self.agent.get(self.url(&path)).call())
    }

    /// Calls job `id` off: out of the queue if it waits, stopped if it runs.
    pub(crate) fn cancel(&self, id: &str) {
        let _ = self.post_json("Calling the job off", "queue", &serde_json::json!({ "delete": [id] }));
        let _ = self.post_json("Calling the job off", "interrupt", &serde_json::json!({ "prompt_id": id }));
    }

    /// The server's websocket for `client`, reading with a short timeout so a job can be called off
    /// while it waits; none over HTTPS or if it can't be reached (the history is asked instead).
    pub(crate) fn listen(&self, client: &str) -> Option<tungstenite::WebSocket<TcpStream>> {
        let rest = self.base.strip_prefix("http://")?;
        let host = rest.split('/').next()?;
        let address = host.to_socket_addrs().ok()?.next()?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(5)).ok()?;
        stream.set_read_timeout(Some(Duration::from_millis(250))).ok()?;
        let url = format!("ws://{rest}/ws?clientId={}", encoded(client));
        let (socket, _) = tungstenite::client::client(url, stream).ok()?;
        Some(socket)
    }
}

/// What a failed answer says, made short: ComfyUI's own message when it gives one.
fn comfy_error(text: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(text).ok();
    let message = parsed.as_ref().and_then(|v| {
        let error = v.get("error")?;
        let mut words = error.get("message").and_then(Value::as_str).unwrap_or("").to_owned();
        if let Some(details) = error.get("details").and_then(Value::as_str).filter(|d| !d.is_empty()) {
            words = format!("{words}: {details}");
        }
        // the first node error, which names what is missing
        if let Some((_, node)) = v.get("node_errors").and_then(Value::as_object).and_then(|m| m.iter().next())
            && let Some(first) = node.get("errors").and_then(Value::as_array).and_then(|e| e.first())
        {
            let what = first.get("details").or_else(|| first.get("message")).and_then(Value::as_str).unwrap_or("");
            let class = node.get("class_type").and_then(Value::as_str).unwrap_or("a node");
            words = format!("{words} ({class}: {what})");
        }
        Some(words)
    });
    let words = message.unwrap_or_else(|| text.trim().to_owned());
    words.chars().take(300).collect()
}

/// Whether a websocket read only timed out (nothing came yet).
pub(crate) fn timed_out(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_encoded_and_addresses_take_http() {
        assert_eq!(encoded("workflows/procraft/a b.json"), "workflows%2Fprocraft%2Fa%20b.json");
        assert_eq!(Server::new("10.0.0.1:8188/").map(|s| s.base), Ok("http://10.0.0.1:8188".to_owned()));
        assert!(Server::new("  ").is_err());
    }

    #[test]
    fn a_refused_prompt_names_the_node() {
        let text = r#"{"error": {"message": "Prompt outputs failed validation", "details": ""},
            "node_errors": {"1": {"class_type": "UNETLoader", "errors": [{"message": "Value not in list", "details": "unet_name: 'x' not in []"}]}}}"#;
        assert_eq!(comfy_error(text), "Prompt outputs failed validation (UNETLoader: unet_name: 'x' not in [])");
    }
}
