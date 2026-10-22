//! License keys gate video export. Recording and editing are free and open source; a key
//! (US$49 once, lifetime) is checked offline against the embedded Ed25519 public key.
//!
//! Key format: `SF1.<base64url payload>.<base64url signature>`, payload JSON:
//! `{ "email": "...", "plan": "monthly" | "lifetime", "expires": "2026-11-12" | null, "issued": "2026-10-12" }`
//! (only lifetime keys are sold now; monthly keys from earlier subscriptions stay valid until
//! they expire and keep renewing while the subscription is active)
//!
//! Keys are produced by `sf license sign` with the private key (kept in the payment backend);
//! nothing in the app can mint keys.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use chrono::{NaiveDate, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

/// Public key of the ScreenForge license issuer (replace with yours before shipping;
/// `sf license keygen` prints a matching pair).
pub const ISSUER_PUBLIC_KEY_B64: &str = "fq9T9G8fJeZSPJtnjCsgO1V0BzXYds8UxAqAEjir4CM";

pub const PRICE_LABEL: &str = "US$49 once";
pub const PRICE_LIFETIME: &str = "US$49 once";
pub const PURCHASE_URL: &str = "https://screenforge.studio/pricing.html";

/// Supabase project hosting the edge functions in `backend/` (see backend/README.md).
/// Placeholders until the project exists; activation reports a clear error meanwhile.
pub const SUPABASE_URL: &str = "https://kxpppjdcqkxpybdawxum.supabase.co";
pub const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6Imt4cHBwamRjcWt4cHliZGF3eHVtIiwicm9sZSI6ImFub24iLCJpYXQiOjE3OTE4MTc3NjAsImV4cCI6MjEwNzM5Mzc2MH0.FT7EFB8DG8WW49NllafP4v6GfrGQ4ELYB-8NuvY5Jmw";

fn backend_configured() -> bool {
    !SUPABASE_URL.contains("SUPABASE_PROJECT_REF")
}

/// POST `body` to edge function `name`: the JSON reply, or the HTTP status (0 for a network
/// failure) and a message.
fn request(name: &str, body: serde_json::Value) -> Result<serde_json::Value, (u16, String)> {
    if !backend_configured() {
        return Err((0, "The license server is not configured in this build.".into()));
    }
    let url = format!("{SUPABASE_URL}/functions/v1/{name}");
    let resp = ureq::post(&url)
        .set("apikey", SUPABASE_ANON_KEY)
        .set("Authorization", &format!("Bearer {SUPABASE_ANON_KEY}"))
        .timeout(std::time::Duration::from_secs(15))
        .send_json(body);
    match resp {
        Ok(r) => r.into_json().map_err(|e| (0, e.to_string())),
        Err(ureq::Error::Status(code, r)) => Err((code, r.into_json::<serde_json::Value>().ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from)).unwrap_or_else(|| format!("server returned {code}")))),
        Err(e) => Err((0, format!("network error: {e}"))),
    }
}

/// License calls: a 404 from these functions means there is no license.
fn call_function(name: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    request(name, body).map_err(|(code, msg)| if code == 404 { "no license".into() } else { msg })
}

/// Call another ScreenForge backend function (e.g. the `unsplash` proxy).
pub fn call_backend(name: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    request(name, body).map_err(|(code, msg)| if code == 404 { format!("the {name} service is not available yet") } else { msg })
}

/// Ask the backend for the newest key issued to `email`. Verifies and saves it on success.
pub fn activate(email: &str) -> Result<Status, String> {
    let email = email.trim();
    if email.is_empty() || !email.contains('@') {
        return Err("Enter the email you used at checkout.".into());
    }
    let v = call_function("license-refresh", serde_json::json!({ "email": email }))?;
    let key = v.get("key").and_then(|k| k.as_str()).ok_or_else(|| "no license".to_string())?;
    let status = verify(key);
    if status.is_valid() {
        save(key).map_err(|e| e.to_string())?;
        let _ = save_email(email);
    }
    Ok(status)
}

/// Start a Stripe checkout for `plan` (only "lifetime" is sold) and return the URL to open.
pub fn checkout_url(email: &str, plan: &str) -> Result<String, String> {
    let v = call_function("create-checkout", serde_json::json!({ "email": email.trim(), "plan": plan }))?;
    v.get("url").and_then(|u| u.as_str()).map(String::from).ok_or_else(|| "no checkout url".to_string())
}

fn email_file() -> Option<std::path::PathBuf> {
    key_file().map(|p| p.with_file_name("email"))
}

pub fn save_email(email: &str) -> std::io::Result<()> {
    let p = email_file().ok_or_else(|| std::io::Error::other("no home dir"))?;
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, email.trim())
}

pub fn load_email() -> Option<String> {
    std::fs::read_to_string(email_file()?).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub email: String,
    /// `monthly` | `lifetime`
    pub plan: String,
    pub expires: Option<NaiveDate>,
    pub issued: NaiveDate,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Valid(Payload),
    Expired(Payload),
    Invalid(String),
}

impl Status {
    pub fn is_valid(&self) -> bool {
        matches!(self, Status::Valid(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("malformed key")]
    Malformed,
    #[error("bad signature")]
    BadSignature,
    #[error("{0}")]
    Other(String),
}

fn issuer_key(b64: &str) -> Result<VerifyingKey, Error> {
    let bytes = B64.decode(b64).map_err(|_| Error::Other("bad issuer key".into()))?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| Error::Other("bad issuer key length".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|_| Error::Other("bad issuer key".into()))
}

/// Verify a key against the embedded issuer key.
pub fn verify(key: &str) -> Status {
    verify_with(key, ISSUER_PUBLIC_KEY_B64)
}

pub fn verify_with(key: &str, issuer_b64: &str) -> Status {
    let key = key.trim();
    let parse = || -> Result<Payload, Error> {
        let mut parts = key.split('.');
        if parts.next() != Some("SF1") {
            return Err(Error::Malformed);
        }
        let payload_b64 = parts.next().ok_or(Error::Malformed)?;
        let sig_b64 = parts.next().ok_or(Error::Malformed)?;
        let payload = B64.decode(payload_b64).map_err(|_| Error::Malformed)?;
        let sig = B64.decode(sig_b64).map_err(|_| Error::Malformed)?;
        let sig = Signature::from_slice(&sig).map_err(|_| Error::Malformed)?;
        issuer_key(issuer_b64)?.verify(&payload, &sig).map_err(|_| Error::BadSignature)?;
        serde_json::from_slice(&payload).map_err(|_| Error::Malformed)
    };
    match parse() {
        Err(e) => Status::Invalid(e.to_string()),
        Ok(p) => match p.expires {
            Some(d) if d < Utc::now().date_naive() => Status::Expired(p),
            _ => Status::Valid(p),
        },
    }
}

/// Issue a key (backend side). `signing_key_b64` is the 32-byte Ed25519 secret, base64url.
pub fn sign(signing_key_b64: &str, payload: &Payload) -> Result<String, Error> {
    let bytes = B64.decode(signing_key_b64).map_err(|_| Error::Other("bad signing key".into()))?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| Error::Other("bad signing key length".into()))?;
    let sk = SigningKey::from_bytes(&arr);
    let json = serde_json::to_vec(payload).map_err(|e| Error::Other(e.to_string()))?;
    let sig = sk.sign(&json);
    Ok(format!("SF1.{}.{}", B64.encode(&json), B64.encode(sig.to_bytes())))
}

/// Generate an issuer key pair: `(secret_b64, public_b64)`.
pub fn keygen() -> (String, String) {
    let sk = SigningKey::generate(&mut rand::rngs::OsRng);
    (B64.encode(sk.to_bytes()), B64.encode(sk.verifying_key().to_bytes()))
}

/// Where the app stores the entered key.
pub fn key_file() -> Option<std::path::PathBuf> {
    if cfg!(target_os = "windows") {
        let appdata = std::env::var_os("APPDATA")?;
        return Some(std::path::PathBuf::from(appdata).join("ScreenForge").join("license.key"));
    }
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join("Library/Application Support/ScreenForge/license.key"))
}

/// Exports allowed without a license.
///
/// The count is kept in two places: a file next to the key (works offline) and a row on the
/// backend keyed by a hash of the machine's hardware UUID ([`device_id`]). Deleting the app or
/// its support folder resets the file but not the backend row, so the next sync restores the
/// count. The higher of the two wins.
pub const FREE_EXPORTS: u32 = 3;

/// Count the backend reports for this device (0 until [`sync_free_exports`] has run).
static SERVER_FREE_EXPORTS: AtomicU32 = AtomicU32::new(0);

fn free_exports_file() -> Option<std::path::PathBuf> {
    key_file().map(|p| p.with_file_name("free-exports"))
}

fn local_free_exports_used() -> u32 {
    free_exports_file().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn write_local_free_exports(n: u32) -> std::io::Result<()> {
    let p = free_exports_file().ok_or_else(|| std::io::Error::other("no home dir"))?;
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, n.to_string())
}

/// How many free exports have been used on this machine: the local file or the backend's
/// count for this device, whichever is higher.
pub fn free_exports_used() -> u32 {
    local_free_exports_used().max(SERVER_FREE_EXPORTS.load(Ordering::Relaxed))
}

/// Free exports still available.
pub fn free_exports_left() -> u32 {
    FREE_EXPORTS.saturating_sub(free_exports_used())
}

/// Stable, anonymous identifier for this machine: SHA-256 of the platform UUID
/// (`IOPlatformUUID` on macOS, `/etc/machine-id` on Linux, `MachineGuid` on Windows), hex.
/// The raw UUID never leaves the machine. `None` when the platform offers none.
pub fn device_id() -> Option<String> {
    static ID: OnceLock<Option<String>> = OnceLock::new();
    ID.get_or_init(|| platform_uuid().map(|u| hex(&Sha256::digest(u.trim().to_lowercase().as_bytes())))).clone()
}

fn platform_uuid() -> Option<String> {
    let raw = if cfg!(target_os = "macos") {
        let out = std::process::Command::new("ioreg").args(["-rd1", "-c", "IOPlatformExpertDevice"]).output().ok()?;
        parse_ioreg_uuid(&String::from_utf8_lossy(&out.stdout))?
    } else if cfg!(target_os = "windows") {
        let out = std::process::Command::new("reg").args(["query", r"HKLM\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines().find(|l| l.contains("MachineGuid"))?.split_whitespace().last()?.to_string()
    } else {
        std::fs::read_to_string("/etc/machine-id").or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id")).ok()?
    };
    let raw = raw.trim().to_string();
    (!raw.is_empty()).then_some(raw)
}

fn parse_ioreg_uuid(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("\"IOPlatformUUID\""))?;
    let value = line.split('=').nth(1)?.trim().trim_matches('"');
    (!value.is_empty()).then(|| value.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Ask the backend how many free exports this device has used and reconcile with the local
/// file. Blocking (network); call from a background thread. Silently a no-op offline or when
/// the device has no identifier, in which case the local file alone counts.
pub fn sync_free_exports() {
    let Some(device) = device_id() else { return };
    let Ok(v) = call_function("free-export", serde_json::json!({ "device": device })) else { return };
    if let Some(used) = v.get("used").and_then(|u| u.as_u64()) {
        reconcile_server_count(used as u32);
    }
}

fn reconcile_server_count(server: u32) {
    SERVER_FREE_EXPORTS.fetch_max(server, Ordering::Relaxed);
    if server > local_free_exports_used() {
        let _ = write_local_free_exports(server);
    }
}

/// Count one more free export: the local file right away, the backend row for this device in
/// the background. Fails only when the local file cannot be written.
pub fn record_free_export() -> std::io::Result<()> {
    let n = free_exports_used() + 1;
    write_local_free_exports(n)?;
    if let Some(device) = device_id() {
        std::thread::spawn(move || {
            if let Ok(v) = call_function("free-export", serde_json::json!({ "device": device, "record": true })) {
                if let Some(used) = v.get("used").and_then(|u| u.as_u64()) {
                    reconcile_server_count(used as u32);
                }
            }
        });
    }
    Ok(())
}

pub fn load_saved() -> Option<String> {
    std::fs::read_to_string(key_file()?).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn save(key: &str) -> std::io::Result<()> {
    let p = key_file().ok_or_else(|| std::io::Error::other("no home dir"))?;
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, key.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let (sk, pk) = keygen();
        let p = Payload { email: "a@b.c".into(), plan: "monthly".into(), expires: Some(Utc::now().date_naive() + chrono::Duration::days(30)), issued: Utc::now().date_naive() };
        let key = sign(&sk, &p).unwrap();
        assert!(matches!(verify_with(&key, &pk), Status::Valid(_)));
        let expired = Payload { expires: Some(Utc::now().date_naive() - chrono::Duration::days(1)), ..p.clone() };
        assert!(matches!(verify_with(&sign(&sk, &expired).unwrap(), &pk), Status::Expired(_)));
        assert!(matches!(verify_with("SF1.xx.yy", &pk), Status::Invalid(_)));
        let (_, other) = keygen();
        assert!(matches!(verify_with(&key, &other), Status::Invalid(_)));
    }

    #[test]
    fn ioreg_uuid_parses() {
        let text = "+-o MacBookPro18,3  <class IOPlatformExpertDevice>\n  {\n    \"IOPlatformUUID\" = \"5A3C1F2E-0B6D-4E8A-9C7B-1234567890AB\"\n    \"model\" = <\"x\">\n  }\n";
        assert_eq!(parse_ioreg_uuid(text).as_deref(), Some("5A3C1F2E-0B6D-4E8A-9C7B-1234567890AB"));
        assert_eq!(parse_ioreg_uuid("nothing here"), None);
    }

    #[test]
    fn device_hash_is_stable_and_opaque() {
        let a = hex(&Sha256::digest("abc".as_bytes()));
        assert_eq!(a.len(), 64);
        assert_eq!(a, hex(&Sha256::digest("abc".as_bytes())));
        assert_ne!(a, hex(&Sha256::digest("abd".as_bytes())));
    }

    #[test]
    fn server_count_wins_when_higher() {
        SERVER_FREE_EXPORTS.store(0, Ordering::Relaxed);
        SERVER_FREE_EXPORTS.fetch_max(2, Ordering::Relaxed);
        assert!(free_exports_used() >= 2);
        assert_eq!(free_exports_left(), FREE_EXPORTS.saturating_sub(free_exports_used()));
    }
}
