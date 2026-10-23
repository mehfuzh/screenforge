//! Claude Messages API over HTTP (there is no Rust SDK): how requests reach Claude, the model
//! list with prices, and one request with retries.
//!
//! Released builds go through ScreenForge's `agent` function on Supabase, which holds the
//! Anthropic key as a secret ([`Route::Hosted`]). A local build with `ANTHROPIC_API_KEY` in the
//! environment or in a `.env` file calls the API directly ([`Route::Direct`], see [`local_key`]).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2026-10-21";
/// Room for a long answer plus several tool calls in one reply (non-streaming stays well
/// under the HTTP timeout at this size).
pub const MAX_TOKENS: u32 = 16_000;
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// A model the agent offers, with list prices in US$ per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    pub id: &'static str,
    pub label: &'static str,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    /// Accepts `fallbacks: "default"` (a declined request is retried on another model).
    pub fallbacks: bool,
}

const SONNET: Model = Model { id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", input: 2.0, output: 10.0, cache_read: 0.20, fallbacks: true };
const HAIKU: Model = Model { id: "claude-haiku-5-5", label: "Claude Haiku 5.5", input: 0.10, output: 0.50, cache_read: 0.01, fallbacks: false };
/// Only through Auto, for complex requests.
const OPUS: Model = Model { id: "claude-opus-5-5", label: "Claude Opus 5.5", input: 4.0, output: 20.0, cache_read: 0.20, fallbacks: true };
/// Never asked for, but a declined request can be answered by it through the server-side
/// fallback: priced so its usage is not counted as free.
const FABLE: Model = Model { id: "claude-fable-5-1", label: "Claude Fable 5.1", input: 10.0, output: 50.0, cache_read: 0.25, fallbacks: true };

/// The models the panel lists after Auto. The backend's `MODELS` (functions/agent/index.ts)
/// also allows Opus, which Auto uses.
pub const MODELS: &[Model] = &[SONNET, HAIKU];
const PRICED: &[Model] = &[SONNET, HAIKU, OPUS, FABLE];

/// The panel's first choice: Sonnet, or Opus for a complex request ([`is_complex`]).
pub const AUTO: &str = "auto";
pub const DEFAULT_MODEL: &str = AUTO;

/// A model the agent may call (offered, or picked by Auto).
pub fn model(id: &str) -> Option<&'static Model> {
    PRICED.iter().filter(|m| m.id != FABLE.id).find(|m| m.id == id)
}

/// Label for a model id ("Claude Opus 5.5"), or the id itself.
pub fn label(id: &str) -> &str {
    PRICED.iter().find(|m| m.id == id).map_or(id, |m| m.label)
}

/// Whether Auto should give `request` to Opus: edits across the whole video (auto edit, every
/// pause, all the zooms), several changes in one request, or a long one. A cheap rule, so
/// choosing costs nothing and adds no wait.
pub fn is_complex(request: &str) -> bool {
    let r = request.to_lowercase();
    const WHOLE: [&str; 13] = ["auto edit", "auto-edit", "whole video", "entire video", "everything", "every ", "all the ", "all of ", "throughout", "tighten", "pauses", "filler", "silences"];
    if WHOLE.iter().any(|w| r.contains(w)) {
        return true;
    }
    // several requests in one: "cut the intro, speed up the middle and add music"
    let parts = r.split([',', ';', '\n']).flat_map(|p| p.split(" and ")).filter(|p| p.split_whitespace().count() >= 2).count();
    parts >= 3 || r.split_whitespace().count() > 45
}

/// How requests reach Claude.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Straight to the API with a developer's own key (local builds).
    Direct { api_key: String },
    /// Through the ScreenForge backend (`url` is the `agent` function), which adds the key.
    Hosted { url: String, anon_key: String, device: String },
}

/// `ANTHROPIC_API_KEY` from the environment, or from the first `.env` found in the current
/// folder, the executable's folder, or one of their parents (a checkout running
/// `target/debug/screenforge` finds the repository's `.env`).
pub fn local_key() -> Option<String> {
    if let Some(k) = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty()) {
        return Some(k.trim().to_string());
    }
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()))];
    starts.into_iter().flatten().flat_map(|d| d.ancestors().map(|a| a.join(".env")).collect::<Vec<_>>()).find_map(|p| dotenv_value(&std::fs::read_to_string(p).ok()?, "ANTHROPIC_API_KEY"))
}

/// `key`'s value in `.env` text: `KEY=value`, optionally `export`ed or quoted; `#` comments.
fn dotenv_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (k, v) = line.split_once('=')?;
        if k.trim() != key {
            return None;
        }
        let v = v.trim();
        let v = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
            _ => v.split(" #").next().unwrap_or(v).trim(),
        };
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// The agent's preferences, saved as JSON next to the license key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { model: DEFAULT_MODEL.into() }
    }
}

impl Settings {
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    /// The chosen model: Auto, or one from [`MODELS`] (anything else, such as a model no
    /// longer offered, is Auto).
    pub fn choice(&self) -> &str {
        let m = self.model.trim();
        if MODELS.iter().any(|x| x.id == m) {
            m
        } else {
            AUTO
        }
    }

    /// The model for `request`: the chosen one, or Auto's pick.
    pub fn pick(&self, request: &str) -> &'static str {
        match MODELS.iter().find(|x| x.id == self.choice()) {
            Some(m) => m.id,
            None if is_complex(request) => OPUS.id,
            None => SONNET.id,
        }
    }
}

/// Tokens of one or more requests and what they cost at list price.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

impl Usage {
    /// From a response's `usage`, priced for the model that answered.
    pub fn from_response(resp: &Value) -> Self {
        let u = &resp["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let mut usage = Usage { input_tokens: n("input_tokens"), output_tokens: n("output_tokens"), cache_read_tokens: n("cache_read_input_tokens"), cache_write_tokens: n("cache_creation_input_tokens"), cost_usd: 0.0 };
        let id = resp["model"].as_str().unwrap_or("");
        if let Some(m) = PRICED.iter().find(|m| m.id == id) {
            let per = |tokens: u64, price: f64| tokens as f64 * price / 1_000_000.0;
            // cache writes (5 minute) cost 1.25× the input price
            usage.cost_usd = per(usage.input_tokens, m.input) + per(usage.output_tokens, m.output) + per(usage.cache_read_tokens, m.cache_read) + per(usage.cache_write_tokens, m.input * 1.25);
        }
        usage
    }

    pub fn add(&mut self, o: Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_tokens += o.cache_read_tokens;
        self.cache_write_tokens += o.cache_write_tokens;
        self.cost_usd += o.cost_usd;
    }
}

/// POST a Messages API `body` to `model_id` (see [`Settings::pick`]). Rate limits, overload and
/// server errors are retried twice; the error text is for the user.
pub fn send(route: &Route, model_id: &str, mut body: Value) -> Result<Value, String> {
    body["model"] = Value::from(model_id);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let resp = match route {
            Route::Direct { api_key } => {
                let mut req = ureq::post(API_URL).set("x-api-key", api_key).set("anthropic-version", API_VERSION).set("content-type", "application/json");
                let mut b = body.clone();
                if model(model_id).is_some_and(|m| m.fallbacks) {
                    req = req.set("anthropic-beta", FALLBACK_BETA);
                    b["fallbacks"] = Value::from("default");
                }
                req.timeout(std::time::Duration::from_secs(600)).send_json(b)
            }
            // the function adds the key and the fallback option itself
            Route::Hosted { url, anon_key, device } => ureq::post(url)
                .set("apikey", anon_key)
                .set("Authorization", &format!("Bearer {anon_key}"))
                .timeout(std::time::Duration::from_secs(600))
                .send_json(json!({ "device": device, "request": body })),
        };
        match resp {
            Ok(r) => return r.into_json::<Value>().map_err(|e| format!("Claude sent a reply that could not be read: {e}")),
            Err(ureq::Error::Status(code, r)) => {
                let retry_after = r.header("retry-after").and_then(|s| s.parse::<u64>().ok());
                let err: Value = r.into_json().unwrap_or(Value::Null);
                let message = err["error"]["message"].as_str().or(err["error"].as_str()).unwrap_or("").to_string();
                let retryable = code == 429 || code == 529 || code >= 500;
                if retryable && attempt <= 2 {
                    std::thread::sleep(std::time::Duration::from_secs(retry_after.unwrap_or(2 * attempt as u64).min(30)));
                    continue;
                }
                let hosted = matches!(route, Route::Hosted { .. });
                return Err(match code {
                    // the backend's own refusals (allowance used up, unknown device) are worded for the user
                    _ if hosted && (400..500).contains(&code) && code != 429 && !message.is_empty() => message,
                    404 if hosted => "The agent is not available on the ScreenForge server yet.".into(),
                    401 => "Claude did not accept the API key in ANTHROPIC_API_KEY.".into(),
                    429 => "Too many requests right now. Wait a moment and try again.".into(),
                    529 => "Claude is overloaded right now. Try again in a moment.".into(),
                    _ if message.is_empty() => format!("Claude returned an error ({code})."),
                    _ => format!("Claude returned an error ({code}): {message}"),
                });
            }
            Err(ureq::Error::Transport(t)) => {
                if attempt <= 2 {
                    std::thread::sleep(std::time::Duration::from_secs(2 * attempt as u64));
                    continue;
                }
                let to = if matches!(route, Route::Hosted { .. }) { "the ScreenForge server" } else { "the Claude API" };
                return Err(format!("Could not reach {to} ({}). Check your connection.", t.kind()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_is_priced_by_the_answering_model() {
        let resp = json!({ "model": "claude-sonnet-5-5", "usage": { "input_tokens": 1_000_000, "output_tokens": 100_000, "cache_read_input_tokens": 1_000_000, "cache_creation_input_tokens": 0 } });
        let u = Usage::from_response(&resp);
        assert!((u.cost_usd - (2.0 + 1.0 + 0.20)).abs() < 1e-9);
        // a reply from a fallback model is priced at that model's rates
        let fallback = Usage::from_response(&json!({ "model": "claude-opus-5-5", "usage": { "output_tokens": 1_000_000 } }));
        assert!((fallback.cost_usd - 20.0).abs() < 1e-9);
        assert!(model("claude-fable-5-1").is_none(), "Fable is never asked for");
        assert!(MODELS.iter().all(|m| m.id != "claude-opus-5-5"), "Opus is only reached through Auto");
        let unknown = Usage::from_response(&json!({ "model": "x", "usage": { "input_tokens": 5 } }));
        assert_eq!((unknown.input_tokens, unknown.cost_usd), (5, 0.0));
    }

    #[test]
    fn settings_round_trip_and_fall_back_to_a_known_model() {
        let dir = std::env::temp_dir().join(format!("sf-agent-test-{}", std::process::id()));
        let path = dir.join("agent.json");
        let s = Settings { model: "claude-haiku-5-5".into() };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        // files from the first preview builds also had an endpoint and key: ignored; a model
        // no longer offered (Opus, Fable picked by hand) becomes Auto
        std::fs::write(&path, r#"{"endpoint":"x","api_key":"y","model":"claude-fable-5-1"}"#).unwrap();
        assert_eq!(Settings::load(&path).choice(), AUTO);
        assert_eq!(Settings::default().choice(), AUTO);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn auto_uses_sonnet_and_opus_for_complex_requests() {
        let auto = Settings::default();
        assert_eq!(auto.pick("cut the first 5 seconds"), "claude-sonnet-5-5");
        assert_eq!(auto.pick("zoom less here"), "claude-sonnet-5-5");
        assert_eq!(auto.pick("Auto edit: tighten the whole video"), "claude-opus-5-5");
        assert_eq!(auto.pick("Remove the long pauses and filler words"), "claude-opus-5-5");
        assert_eq!(auto.pick("cut the intro, speed up the middle part and add calm music"), "claude-opus-5-5");
        // a model chosen by hand is used as it is
        let haiku = Settings { model: "claude-haiku-5-5".into() };
        assert_eq!(haiku.pick("Auto edit the whole video"), "claude-haiku-5-5");
        assert_eq!(label("claude-opus-5-5"), "Claude Opus 5.5");
    }

    #[test]
    fn dotenv_values() {
        let text = "# keys\nOTHER=1\nexport ANTHROPIC_API_KEY=\"sk-ant-quoted\" # comment\n";
        assert_eq!(dotenv_value(text, "ANTHROPIC_API_KEY").as_deref(), Some("sk-ant-quoted"));
        assert_eq!(dotenv_value("ANTHROPIC_API_KEY = sk-ant-plain # note", "ANTHROPIC_API_KEY").as_deref(), Some("sk-ant-plain"));
        assert_eq!(dotenv_value("ANTHROPIC_API_KEY='single'", "ANTHROPIC_API_KEY").as_deref(), Some("single"));
        assert_eq!(dotenv_value("ANTHROPIC_API_KEY=", "ANTHROPIC_API_KEY"), None);
        assert_eq!(dotenv_value("NOT_ANTHROPIC_API_KEY=x", "ANTHROPIC_API_KEY"), None);
    }
}
