#![doc = include_str!("../README.md")]

use serde_json::{json, Map, Value};
use std::fmt;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The service this crate talks to.
pub const BASE_URL: &str = "https://freeimagetovideo.ai";
/// Where an API key is created.
pub const KEYS_URL: &str = "https://freeimagetovideo.ai/settings/apikeys";
/// The model the site runs: MiniMax H3.
pub const DEFAULT_MODEL: &str = "minimax-h3";
/// Every model id the API takes.
pub const MODELS: &[&str] = &["minimax-h3"];

/// What went wrong, split by what to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// API key missing or invalid (HTTP 401).
    Auth(String),
    /// Not enough credits for this run (`data.reason` = `insufficient_credits`).
    Quota(String),
    /// API runs need a paid plan (`data.reason` = `plan_required`).
    Plan(String),
    /// Too many runs generating at once on this account (`data.reason` = `concurrency`).
    Busy(String),
    /// The content filter refused the prompt, or a cooldown is active (code -2 / -3). Reword it.
    Rejected(String),
    /// The run was accepted but ended as failed or canceled.
    Generation {
        task_id: String,
        error_message: String,
        failure_kind: Option<String>,
    },
    /// Still generating when the deadline passed; check the run later with [`Client::status`].
    Timeout { task_id: String },
    /// Any other refusal, transport failure or unexpected answer.
    Api { http_status: u16, code: i64, message: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Auth(m) if m.contains("settings/apikeys") => write!(f, "freeimagetovideo: {m}"),
            Error::Auth(m) => write!(f, "freeimagetovideo: {m} (create a key at {KEYS_URL})"),
            Error::Quota(m) | Error::Plan(m) | Error::Busy(m) => write!(f, "freeimagetovideo: {m}"),
            Error::Rejected(m) => write!(f, "freeimagetovideo: {m} (reword the prompt rather than retrying)"),
            Error::Generation { task_id, error_message, .. } => {
                write!(f, "freeimagetovideo: generation failed: run {task_id}: {error_message}")
            }
            Error::Timeout { task_id } => write!(f, "freeimagetovideo: run {task_id} is still generating; check it later"),
            Error::Api { message, .. } => write!(f, "freeimagetovideo: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// The model, an optional photo and the video options. `Options::new()` is a text-to-video run with site defaults.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `None` = `minimax-h3`.
    pub model: Option<String>,
    /// A public https image: image to video, with the photo as the first frame.
    pub image_url: Option<String>,
    /// Sent as `options` as they are: `duration` (4/5/6/8/10/15), `resolution` (480p/768p), `aspect_ratio` (16:9/9:16/1:1).
    pub options: Map<String, Value>,
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }
    /// Animate this photo (image to video).
    pub fn image(mut self, url: impl Into<String>) -> Self {
        self.image_url = Some(url.into());
        self
    }
    /// Set one option, e.g. `.set("duration", 6).set("aspect_ratio", "9:16")`.
    pub fn set(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.options.insert(key.into(), value.into());
        self
    }
    /// The request body for `POST /api/ai/generate`.
    pub fn body(&self, prompt: &str) -> Value {
        let mut opts = self.options.clone();
        opts.insert("sound".into(), Value::Bool(true));
        let mut scene = "text-to-video";
        if let Some(img) = self.image_url.as_deref().filter(|s| !s.is_empty()) {
            opts.insert("image_input".into(), json!([img]));
            scene = "image-to-video";
        }
        let model = self.model.as_deref().filter(|s| !s.is_empty()).unwrap_or(DEFAULT_MODEL);
        // The server routes by model; provider only has to be non-empty.
        json!({"provider": "auto", "mediaType": "video", "model": model, "scene": scene, "prompt": prompt, "options": Value::Object(opts)})
    }
}

/// One run as `/api/ai/query` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    /// `pending`, `processing`, `success`, `failed` or `canceled`.
    pub status: String,
    pub videos: Vec<String>,
    pub error_message: Option<String>,
    pub failure_kind: Option<String>,
    /// Waiting in the relaxed queue.
    pub queued: bool,
    /// Seconds until the queue reaches this run, when known.
    pub queue_eta: Option<f64>,
}

/// A blocking client bound to one API key.
#[derive(Clone, Debug)]
pub struct Client {
    key: String,
    base: String,
    /// Time between status checks. Default 10 seconds.
    pub poll_every: Duration,
    /// Total wait for one run. Default 30 minutes; runs can queue.
    pub timeout: Duration,
}

impl Client {
    /// A client for this key. An empty key reads `FREEIMAGETOVIDEO_API_KEY`; the host is `FREEIMAGETOVIDEO_BASE_URL` when set.
    pub fn new(api_key: impl Into<String>) -> Self {
        let mut key: String = api_key.into().trim().to_string();
        if key.is_empty() {
            key = std::env::var("FREEIMAGETOVIDEO_API_KEY").unwrap_or_default().trim().to_string();
        }
        let base = std::env::var("FREEIMAGETOVIDEO_BASE_URL").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Client {
            key,
            base: base.unwrap_or_else(|| BASE_URL.into()).trim_end_matches('/').into(),
            poll_every: Duration::from_secs(10),
            timeout: Duration::from_secs(30 * 60),
        }
    }

    /// Read the key from `FREEIMAGETOVIDEO_API_KEY`.
    pub fn from_env() -> Self {
        Client::new("")
    }

    /// Point the client at another host (tests, proxies).
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into().trim_end_matches('/').into();
        self
    }

    /// Start a run and return its task id without waiting.
    pub fn submit(&self, prompt: &str, options: &Options) -> Result<String, Error> {
        if prompt.trim().is_empty() {
            return Err(api(0, 0, "prompt is required"));
        }
        let (st, raw) = self.post("/api/ai/generate", Some(options.body(prompt)))?;
        parse_submit(st, &raw)
    }

    /// Look up a run.
    pub fn status(&self, task_id: &str) -> Result<Task, Error> {
        let (st, raw) = self.post("/api/ai/query", Some(json!({ "taskId": task_id })))?;
        parse_status(st, &raw)
    }

    /// Credits left on the account.
    pub fn credits(&self) -> Result<i64, Error> {
        let (st, raw) = self.post("/api/user/get-user-credits", None)?;
        parse_credits(st, &raw)
    }

    /// Submit, wait, and return the URL of the finished clip.
    pub fn generate_video(&self, prompt: &str, options: &Options) -> Result<String, Error> {
        let id = self.submit(prompt, options)?;
        self.wait(&id)
    }

    /// Poll a submitted run until it finishes and return its video URL.
    pub fn wait(&self, task_id: &str) -> Result<String, Error> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let t = self.status(task_id)?;
            if let Some(url) = finished(task_id, &t)? {
                return Ok(url);
            }
            if Instant::now() + self.poll_every > deadline {
                return Err(Error::Timeout { task_id: task_id.into() });
            }
            sleep(self.poll_every);
        }
    }

    fn post(&self, path: &str, body: Option<Value>) -> Result<(u16, String), Error> {
        if self.key.is_empty() {
            return Err(Error::Auth(format!("set FREEIMAGETOVIDEO_API_KEY (create a key at {KEYS_URL})")));
        }
        let req = ureq::post(&format!("{}{path}", self.base))
            .set("Authorization", &format!("Bearer {}", self.key))
            .set("User-Agent", concat!("fitv-rust/", env!("CARGO_PKG_VERSION")));
        let resp = match body {
            Some(b) => req.send_json(b),
            None => req.set("Content-Type", "application/json").call(),
        };
        match resp {
            Ok(r) => {
                let code = r.status();
                Ok((code, r.into_string().unwrap_or_default()))
            }
            Err(ureq::Error::Status(code, r)) => Ok((code, r.into_string().unwrap_or_default())),
            Err(e) => Err(api(0, 0, &format!("could not reach freeimagetovideo.ai: {e}"))),
        }
    }
}

fn api(http_status: u16, code: i64, message: &str) -> Error {
    Error::Api { http_status, code, message: message.into() }
}

/// `Some(url)` when the run succeeded, an error when it failed or was canceled, `None` while it runs.
pub fn finished(task_id: &str, t: &Task) -> Result<Option<String>, Error> {
    match t.status.as_str() {
        "success" => match t.videos.first() {
            Some(u) => Ok(Some(u.clone())),
            None => Err(Error::Generation { task_id: task_id.into(), error_message: "finished with no video".into(), failure_kind: None }),
        },
        "failed" | "canceled" => Err(Error::Generation {
            task_id: task_id.into(),
            error_message: t.error_message.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| t.status.clone()),
            failure_kind: t.failure_kind.clone(),
        }),
        _ => Ok(None),
    }
}

/// Map an HTTP status and envelope to the error that says what to do next.
pub fn classify(http_status: u16, env: &Value) -> Error {
    let msg = env
        .get("message")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| format!("request failed (HTTP {http_status})"));
    let code = env.get("code").and_then(Value::as_i64).unwrap_or(0);
    let reason = env.pointer("/data/reason").and_then(Value::as_str).unwrap_or("");
    if http_status == 401 {
        Error::Auth(msg)
    } else if code == -2 || code == -3 {
        Error::Rejected(msg)
    } else if reason == "insufficient_credits" {
        Error::Quota(msg)
    } else if reason == "plan_required" {
        Error::Plan(msg)
    } else if reason == "concurrency" {
        Error::Busy(msg)
    } else if msg.to_lowercase().contains("no auth") {
        Error::Auth(msg)
    } else {
        Error::Api { http_status, code, message: msg }
    }
}

/// Read one answer: the envelope on code 0, else the error that says what to do next.
///
/// freeimagetovideo.ai answers almost every refusal as HTTP 200 with a non-zero
/// code and a `data.reason`, so the status code alone reads a refusal as success.
pub fn parse_envelope(http_status: u16, raw: &str) -> Result<Value, Error> {
    let env: Value = if raw.trim().is_empty() {
        Value::Null
    } else {
        match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) if http_status == 401 => return Err(classify(401, &Value::Null)),
            Err(_) => {
                return Err(api(http_status, 0, &format!("freeimagetovideo.ai returned a non-JSON response (HTTP {http_status})")))
            }
        }
    };
    if http_status < 400 && env.get("code").and_then(Value::as_i64) == Some(0) {
        return Ok(env);
    }
    Err(classify(http_status, &env))
}

/// Read the answer of `POST /api/ai/generate` and return the task id.
pub fn parse_submit(http_status: u16, raw: &str) -> Result<String, Error> {
    let env = parse_envelope(http_status, raw)?;
    match env.pointer("/data/id") {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => Err(api(http_status, 0, "freeimagetovideo.ai accepted the run but returned no id")),
    }
}

/// Read the answer of `POST /api/ai/query`.
pub fn parse_status(http_status: u16, raw: &str) -> Result<Task, Error> {
    let env = parse_envelope(http_status, raw)?;
    let d = match env.get("data") {
        Some(d @ Value::Object(_)) => d,
        _ => return Err(api(http_status, 0, "unexpected status answer")),
    };
    let s = |k: &str| d.get(k).and_then(Value::as_str).map(String::from);
    let id = match d.get("id") {
        Some(Value::Number(n)) => n.to_string(),
        _ => s("id").unwrap_or_default(),
    };
    Ok(Task {
        id,
        status: s("status").unwrap_or_default(),
        videos: d
            .get("videos")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
            .unwrap_or_default(),
        error_message: s("errorMessage"),
        failure_kind: s("failureKind"),
        queued: d.get("queued").and_then(Value::as_bool).unwrap_or(false),
        queue_eta: d.get("queueEta").and_then(Value::as_f64),
    })
}

/// Read the answer of `POST /api/user/get-user-credits`.
pub fn parse_credits(http_status: u16, raw: &str) -> Result<i64, Error> {
    let env = parse_envelope(http_status, raw)?;
    Ok(env.pointer("/data/remainingCredits").and_then(Value::as_f64).unwrap_or(0.0) as i64)
}

/// Generate one clip with sound (key from `FREEIMAGETOVIDEO_API_KEY`) and return its URL.
pub fn generate_video(prompt: &str, options: &Options) -> Result<String, Error> {
    Client::from_env().generate_video(prompt, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(code: i64, message: &str, data: Value) -> String {
        json!({"code": code, "message": message, "data": data}).to_string()
    }

    #[test]
    fn body_shapes() {
        let b = Options::new().set("duration", 6).body("a boat");
        assert_eq!(
            b,
            json!({"provider": "auto", "mediaType": "video", "model": "minimax-h3", "scene": "text-to-video",
                   "prompt": "a boat", "options": {"duration": 6, "sound": true}})
        );
        let b = Options::new().image("https://example.com/s.jpg").set("aspect_ratio", "9:16").body("orbit");
        assert_eq!(b["scene"], "image-to-video");
        assert_eq!(b["options"]["image_input"], json!(["https://example.com/s.jpg"]));
        assert_eq!(b["options"]["sound"], json!(true));
    }

    #[test]
    fn submit_status_credits() {
        assert_eq!(parse_submit(200, &env(0, "ok", json!({"id": "t1", "status": "pending"}))).unwrap(), "t1");
        assert!(matches!(parse_submit(200, &env(0, "ok", json!({}))), Err(Error::Api { .. })));
        let t = parse_status(200, &env(0, "ok", json!({"id": "t1", "status": "pending", "queued": true, "queueEta": 120}))).unwrap();
        assert!(t.queued);
        assert_eq!(t.queue_eta, Some(120.0));
        assert_eq!(finished("t1", &t).unwrap(), None);
        let t = parse_status(200, &env(0, "ok", json!({"id": "t1", "status": "success", "videos": ["https://cdn.example/v.mp4"]}))).unwrap();
        assert_eq!(finished("t1", &t).unwrap().as_deref(), Some("https://cdn.example/v.mp4"));
        let t = parse_status(200, &env(0, "ok", json!({"id": "t1", "status": "failed", "errorMessage": "provider error", "failureKind": "provider"}))).unwrap();
        match finished("t1", &t) {
            Err(Error::Generation { error_message, failure_kind, .. }) => {
                assert_eq!(error_message, "provider error");
                assert_eq!(failure_kind.as_deref(), Some("provider"));
            }
            e => panic!("{e:?}"),
        }
        let t = parse_status(200, &env(0, "ok", json!({"id": "t1", "status": "canceled"}))).unwrap();
        assert!(matches!(finished("t1", &t), Err(Error::Generation { .. })));
        let t = parse_status(200, &env(0, "ok", json!({"id": "t1", "status": "success", "videos": []}))).unwrap();
        assert!(matches!(finished("t1", &t), Err(Error::Generation { .. })));
        assert_eq!(parse_credits(200, &env(0, "ok", json!({"remainingCredits": 42, "hasPurchasedCredits": false}))).unwrap(), 42);
    }

    #[test]
    fn error_mapping() {
        let k = "Invalid API key. Create one at https://freeimagetovideo.ai/settings/apikeys";
        assert!(matches!(parse_submit(401, &env(-1, k, json!({"reason": "invalid_api_key"}))), Err(Error::Auth(_))));
        assert!(matches!(parse_submit(401, "<html>nope</html>"), Err(Error::Auth(_))));
        assert!(matches!(parse_submit(200, &env(-2, "blocked", Value::Null)), Err(Error::Rejected(_))));
        assert!(matches!(parse_submit(200, &env(-3, "cooldown", Value::Null)), Err(Error::Rejected(_))));
        assert!(matches!(parse_submit(200, &env(-1, "x", json!({"reason": "insufficient_credits"}))), Err(Error::Quota(_))));
        assert!(matches!(parse_submit(200, &env(-1, "x", json!({"reason": "plan_required"}))), Err(Error::Plan(_))));
        assert!(matches!(parse_submit(200, &env(-1, "x", json!({"reason": "concurrency"}))), Err(Error::Busy(_))));
        assert!(matches!(parse_credits(200, &env(-1, "no auth, please sign in", Value::Null)), Err(Error::Auth(_))));
        assert!(matches!(parse_submit(500, "<html>oops</html>"), Err(Error::Api { http_status: 500, .. })));
        match parse_submit(200, &env(-1, "duration must be one of 4, 5, 6, 8, 10, 15", Value::Null)) {
            Err(Error::Api { code: -1, message, .. }) => assert!(message.contains("duration")),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn needs_key_and_prompt() {
        std::env::remove_var("FREEIMAGETOVIDEO_API_KEY");
        assert!(matches!(Client::new("").credits(), Err(Error::Auth(m)) if m.contains("settings/apikeys")));
        assert!(matches!(Client::new("sk-x").submit("  ", &Options::new()), Err(Error::Api { .. })));
    }

    /// Live: FITV_LIVE=1 cargo test -- --ignored. A wrong key must land on Error::Auth; no run is started.
    #[test]
    #[ignore]
    fn live_wrong_key_is_auth() {
        let c = Client::new("sk-wrong-key-for-test");
        assert!(matches!(c.submit("a paper lantern drifting over a night river", &Options::new()), Err(Error::Auth(_))));
        assert!(matches!(c.credits(), Err(Error::Auth(_))));
    }
}
