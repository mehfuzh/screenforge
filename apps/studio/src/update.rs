//! "Update available": the newest GitHub release compared with this build's version.
//!
//! One HTTPS GET of the public releases API (no account, nothing about the user is sent). It runs
//! on launch at most once a day while `Prefs::check_updates` is on, and whenever "Check for
//! updates now" is pressed. The result is kept for the process so the picker and the editor can
//! both show it.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const LATEST_URL: &str = "https://api.github.com/repos/mehfuzh/screenforge/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/mehfuzh/screenforge/releases/latest";
/// Automatic checks run at most this often.
pub const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Checking,
    /// A newer release: its version (no `v`) and page.
    Available { version: String, url: String },
    UpToDate,
    Failed(String),
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::Idle))
}

pub fn current() -> State {
    state().lock().map(|s| s.clone()).unwrap_or(State::Idle)
}

/// The newer release, if one was found.
pub fn available() -> Option<(String, String)> {
    match current() {
        State::Available { version, url } => Some((version, url)),
        _ => None,
    }
}

/// Ask GitHub for the newest release in the background; `ctx` is repainted when it answers.
pub fn check(ctx: &egui::Context) {
    {
        let Ok(mut s) = state().lock() else { return };
        if *s == State::Checking {
            return;
        }
        *s = State::Checking;
    }
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = fetch_latest().map(|(tag, url)| if is_newer(&tag, env!("CARGO_PKG_VERSION")) { State::Available { version: tag.trim_start_matches('v').to_string(), url } } else { State::UpToDate });
        let next = result.unwrap_or_else(|e| {
            tracing::warn!("update check failed: {e}");
            State::Failed(e)
        });
        if let Ok(mut s) = state().lock() {
            *s = next;
        }
        ctx.request_repaint();
    });
}

/// Whether an automatic check is due (`last` = unix seconds of the previous one).
pub fn due(last: Option<u64>, now: u64) -> bool {
    last.is_none_or(|t| now.saturating_sub(t) >= CHECK_EVERY_SECS)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `(tag_name, html_url)` of the newest published release (drafts and pre-releases excluded by
/// the API).
fn fetch_latest() -> Result<(String, String), String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    let body = agent
        .get(LATEST_URL)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", concat!("ScreenForge/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("no tag_name in the release")?.to_string();
    let url = v["html_url"].as_str().unwrap_or(RELEASES_URL).to_string();
    Ok((tag, url))
}

/// `1.2.3` from `v1.2.3` / `1.2.3`; missing parts are 0, anything after `-` or `+` is ignored.
fn parse(v: &str) -> Option<[u64; 3]> {
    let core = v.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut out = [0u64; 3];
    for (i, part) in core.split('.').enumerate() {
        *out.get_mut(i)? = part.parse().ok()?;
    }
    Some(out)
}

/// Whether release `tag` is a newer version than `current`.
pub fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse(tag), parse(current)), (Some(a), Some(b)) if a > b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("v0.1.17", "0.1.16"));
        assert!(is_newer("v0.2.0", "0.1.99"));
        assert!(is_newer("v0.1.10", "0.1.9"), "not a string comparison");
        assert!(!is_newer("v0.1.16", "0.1.16"));
        assert!(!is_newer("v0.1.15", "0.1.16"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("nightly", "0.1.16"));
        assert!(!is_newer("v0.1.17-beta", "0.1.17"));
    }

    #[test]
    fn checks_at_most_daily() {
        assert!(due(None, 1000));
        assert!(!due(Some(1000), 1000 + CHECK_EVERY_SECS - 1));
        assert!(due(Some(1000), 1000 + CHECK_EVERY_SECS));
    }
}

#[cfg(test)]
mod network {
    /// Talks to GitHub: `cargo test -p studio -- --ignored latest_release`.
    #[test]
    #[ignore]
    fn latest_release_has_a_version_tag() {
        let (tag, url) = super::fetch_latest().unwrap();
        assert!(super::parse(&tag).is_some(), "{tag}");
        assert!(url.starts_with("https://github.com/"), "{url}");
    }
}
