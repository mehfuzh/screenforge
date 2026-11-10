//! License keys gate video export. Recording and editing are free and open source; a key
//! (US$49 once, lifetime) is checked offline against the embedded Ed25519 public key.
//!
//! Key format: `SF1.<base64url payload>.<base64url signature>`, payload JSON:
//! `{ "email": "...", "plan": "monthly" | "lifetime", "expires": "2026-11-12" | null, "issued": "2026-10-12", "device": "<sha-256 hex>" }`
//!
//! One license works on one computer at a time. The backend signs a key for the computer it
//! activates (`device`, see [`device_id`]); [`verify`] only accepts a key bound to this
//! computer, so a copied key file does not export elsewhere, even offline. Keys without a
//! device (issued before device activation) need activating once.
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
use std::sync::OnceLock;

/// Public key of the ScreenForge license issuer (replace with yours before shipping;
/// `sf license keygen` prints a matching pair).
pub const ISSUER_PUBLIC_KEY_B64: &str = "fq9T9G8fJeZSPJtnjCsgO1V0BzXYds8UxAqAEjir4CM";

pub const PRICE_LABEL: &str = "US$49 once";
pub const PRICE_LIFETIME: &str = "US$49 once";
/// Pro: export and the AI agent (a monthly key, renewed while subscribed).
pub const PRICE_PRO: &str = "US$20/month";
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
    request_within(name, body, 15)
}

fn request_within(name: &str, body: serde_json::Value, secs: u64) -> Result<serde_json::Value, (u16, String)> {
    if !backend_configured() {
        return Err((0, "The license server is not configured in this build.".into()));
    }
    let url = format!("{SUPABASE_URL}/functions/v1/{name}");
    let resp = ureq::post(&url)
        .set("apikey", SUPABASE_ANON_KEY)
        .set("Authorization", &format!("Bearer {SUPABASE_ANON_KEY}"))
        .timeout(std::time::Duration::from_secs(secs))
        .send_json(body);
    match resp {
        Ok(r) => r.into_json().map_err(|e| (0, e.to_string())),
        Err(ureq::Error::Status(code, r)) => Err((code, r.into_json::<serde_json::Value>().ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from)).unwrap_or_else(|| format!("server returned {code}")))),
        // the transport error text repeats itself ("Network Error: Network Error: …"): say
        // what happened, keep the cause short
        Err(ureq::Error::Transport(t)) if t.kind() == ureq::ErrorKind::Io || t.kind() == ureq::ErrorKind::ConnectionFailed || t.kind() == ureq::ErrorKind::Dns => {
            Err((0, "Could not reach the license server. Check your connection and try again.".into()))
        }
        Err(e) => Err((0, format!("The license server did not answer ({}). Try again in a moment.", e.kind()))),
    }
}

/// License calls: a 404 from these functions means there is no license.
fn call_function(name: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    request(name, body).map_err(|(code, msg)| if code == 404 { "no license".into() } else { msg })
}

/// Call another ScreenForge backend function (e.g. the `unsplash` proxy).
pub fn call_backend(name: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    call_backend_within(name, body, 15)
}

/// [`call_backend`] for a function that takes long to answer (an ElevenLabs reading).
pub fn call_backend_within(name: &str, body: serde_json::Value, secs: u64) -> Result<serde_json::Value, String> {
    request_within(name, body, secs).map_err(|(code, msg)| if code == 404 { format!("the {name} service is not available yet") } else { msg })
}

/// Check in with the backend for the key saved on this computer, without emailing anything:
/// a renewed key is saved, and when the license was moved to another computer the saved key is
/// removed (`Ok(Some(false))`). `Ok(None)`: no key or no email saved, nothing to check. A
/// network problem is an `Err` and changes nothing, so the key keeps working offline.
pub fn check_in() -> Result<Option<bool>, String> {
    let (Some(_), Some(email)) = (load_saved(), load_email()) else { return Ok(None) };
    match activate(&email, false)? {
        Activation::Done(status) => Ok(Some(status.is_valid())),
        Activation::NeedsCode | Activation::CodeSent => {
            forget().map_err(|e| e.to_string())?;
            Ok(Some(false))
        }
    }
}

/// Outcome of [`activate`].
#[derive(Debug, Clone, PartialEq)]
pub enum Activation {
    /// This computer is activated: the key was fetched, verified and saved.
    Done(Status),
    /// A 6-digit code was emailed to the license address; finish with [`confirm_code`].
    CodeSent,
    /// This computer is not activated yet and `send_code` was false (nothing was emailed).
    NeedsCode,
}

/// Activate this computer for the license bought with `email`. The backend only returns a key
/// to a computer already activated for that license; otherwise, with `send_code`, it emails a
/// code to the license address that [`confirm_code`] takes. The silent renewal check passes
/// `send_code: false` so it never sends email by itself.
pub fn activate(email: &str, send_code: bool) -> Result<Activation, String> {
    let email = checked_email(email)?;
    let device = device_id().ok_or_else(|| "ScreenForge cannot identify this computer, so the license cannot be activated.".to_string())?;
    let v = call_function("license-activate", serde_json::json!({ "email": email, "device": device, "send": send_code }))?;
    match v.get("status").and_then(|s| s.as_str()) {
        Some("code_sent") => Ok(Activation::CodeSent),
        Some("verify") => Ok(Activation::NeedsCode),
        _ => store_key(&v, email).map(Activation::Done),
    }
}

/// Finish activation with the emailed `code`: verifies and saves the key on success.
pub fn confirm_code(email: &str, code: &str) -> Result<Status, String> {
    let email = checked_email(email)?;
    let code: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if code.len() != 6 {
        return Err("Enter the 6-digit code from the email.".into());
    }
    let device = device_id().ok_or_else(|| "ScreenForge cannot identify this computer.".to_string())?;
    let v = call_function("license-activate", serde_json::json!({ "email": email, "device": device, "code": code }))?;
    store_key(&v, email)
}

fn checked_email(email: &str) -> Result<&str, String> {
    let email = email.trim();
    if email.is_empty() || !email.contains('@') {
        return Err("Enter the email you used at checkout.".into());
    }
    Ok(email)
}

/// Verify the key in a backend reply and save it (and the email) when valid.
fn store_key(v: &serde_json::Value, email: &str) -> Result<Status, String> {
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

/// Start a checkout for a license `plan` ("pro", "lifetime") that activates this computer by
/// itself once paid: a fresh secret is saved here (see [`pending_claim`]) and only its SHA-256
/// goes to the backend; [`claim_checkout`] then fetches the key. `email` (optional) prefills
/// Checkout, `promo` (optional) is a promotion code; an invalid one is an `Err` with the reason.
pub fn start_checkout(plan: &str, email: Option<&str>, promo: Option<&str>) -> Result<String, String> {
    use rand::RngCore;
    let device = device_id().ok_or_else(|| "ScreenForge cannot identify this computer.".to_string())?;
    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let secret = hex(&secret);
    let mut body = serde_json::json!({ "plan": plan, "device": device, "claim": hex(&Sha256::digest(secret.as_bytes())) });
    if let Some(e) = email.map(str::trim).filter(|e| e.contains('@')) {
        body["email"] = e.into();
    }
    if let Some(p) = promo.map(str::trim).filter(|p| !p.is_empty()) {
        body["promo"] = p.into();
    }
    let v = call_function("create-checkout", body)?;
    let url = v.get("url").and_then(|u| u.as_str()).map(String::from).ok_or_else(|| "no checkout url".to_string())?;
    // kept on disk, so paying after a restart still activates
    if let Some(p) = claim_file() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, &secret);
    }
    Ok(url)
}

fn claim_file() -> Option<std::path::PathBuf> {
    key_file().map(|p| p.with_file_name("checkout-claim"))
}

/// A checkout started here that has not activated this computer yet.
pub fn pending_claim() -> bool {
    claim_file().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|s| s.trim().len() == 64)
}

fn drop_claim() {
    if let Some(p) = claim_file() {
        let _ = std::fs::remove_file(p);
    }
}

/// Outcome of [`claim_checkout`].
#[derive(Debug, Clone, PartialEq)]
pub enum Claim {
    /// Paid: this computer is activated (key and email saved).
    Done(Status),
    /// Not paid yet: ask again in a moment.
    Pending,
    /// Paid with an address that had a license already: sign in with the emailed code.
    Verify(String),
}

/// Ask whether the checkout from [`start_checkout`] was paid, activating this computer when it
/// was. The saved secret is removed once it is used up (or refused).
pub fn claim_checkout() -> Result<Claim, String> {
    let secret = claim_file().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.trim().to_string()).filter(|s| s.len() == 64).ok_or("no checkout")?;
    let device = device_id().ok_or_else(|| "ScreenForge cannot identify this computer.".to_string())?;
    let v = match request("license-claim", serde_json::json!({ "device": device, "claim": secret })) {
        Ok(v) => v,
        Err((410, msg)) => {
            drop_claim();
            // claimed already, by the agent panel or Export watching the same checkout
            let saved = load_saved().map(|k| verify(&k)).filter(Status::is_valid);
            return saved.map(Claim::Done).ok_or(msg);
        }
        Err((_, msg)) => return Err(msg),
    };
    match v.get("status").and_then(|s| s.as_str()) {
        Some("pending") => Ok(Claim::Pending),
        Some("verify") => {
            drop_claim();
            Ok(Claim::Verify(v.get("email").and_then(|e| e.as_str()).unwrap_or_default().to_string()))
        }
        _ => {
            let email = v.get("email").and_then(|e| e.as_str()).unwrap_or_default().to_string();
            let status = store_key(&v, &email)?;
            drop_claim();
            Ok(Claim::Done(status))
        }
    }
}

/// Free exports per computer without a license (the backend's `export-allowance` has the
/// final say).
pub const FREE_EXPORTS: u32 = 2;

/// Free exports finished while the server could not be told, sent on the next check.
fn unsent_exports_file() -> Option<std::path::PathBuf> {
    key_file().map(|p| p.with_file_name("free-exports-unsent"))
}

fn unsent_exports() -> u32 {
    unsent_exports_file().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn set_unsent_exports(n: u32) {
    if let Some(p) = unsent_exports_file() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = if n == 0 { std::fs::remove_file(p) } else { std::fs::write(p, n.to_string()) };
    }
}

/// Free exports this computer has left. Needs the server: offline it is an `Err` and there
/// is no free export (licensed export works offline). Free exports recorded while offline are
/// sent first.
pub fn free_exports_left() -> Result<u32, String> {
    let _one = FREE_EXPORTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    free_exports_left_locked()
}

/// One check or record at a time: two sending the same unsent export would count it twice.
static FREE_EXPORTS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn free_exports_left_locked() -> Result<u32, String> {
    let device = device_id().ok_or_else(|| "ScreenForge cannot identify this computer.".to_string())?;
    let mut unsent = unsent_exports();
    let mut v = None;
    while unsent > 0 {
        v = Some(call_backend("export-allowance", serde_json::json!({ "device": device, "record": true }))?);
        unsent -= 1;
        set_unsent_exports(unsent);
    }
    let v = match v {
        Some(v) => v,
        None => call_backend("export-allowance", serde_json::json!({ "device": device }))?,
    };
    let used = v.get("used").and_then(|u| u.as_u64()).unwrap_or(u64::from(FREE_EXPORTS)) as u32;
    let free = v.get("free").and_then(|f| f.as_u64()).map(|f| f as u32).unwrap_or(FREE_EXPORTS);
    Ok(free.saturating_sub(used))
}

/// A free export finished: count it (kept here until the server can be told). Returns what
/// is left, as far as this computer knows.
pub fn record_free_export() -> Result<u32, String> {
    let _one = FREE_EXPORTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_unsent_exports(unsent_exports() + 1);
    free_exports_left_locked()
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
    /// The computer the key is for ([`device_id`]); `None` on keys from before activation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
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

/// Verify a key against the embedded issuer key, for this computer: a key bound to another
/// computer, or to none, is not valid here.
pub fn verify(key: &str) -> Status {
    for_device(verify_with(key, ISSUER_PUBLIC_KEY_B64), device_id().as_deref())
}

/// `status` for the computer `device`: only a key bound to it stays valid or expired.
pub fn for_device(status: Status, device: Option<&str>) -> Status {
    let payload = match &status {
        Status::Valid(p) | Status::Expired(p) => p,
        Status::Invalid(_) => return status,
    };
    match (payload.device.as_deref(), device) {
        (Some(bound), Some(here)) if bound == here => status,
        (Some(_), _) => Status::Invalid("This license is activated on another computer. Press Activate to move it here.".into()),
        (None, _) => Status::Invalid("Licenses now work on one computer at a time. Press Activate to activate this computer.".into()),
    }
}

/// Verify a key's signature and dates against `issuer_b64` (any computer).
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

pub fn load_saved() -> Option<String> {
    std::fs::read_to_string(key_file()?).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Sign this computer out of its license: the backend forgets the activation (so the computer
/// stops using the license's agent credits and the license is free to activate elsewhere), then
/// the key and email are removed here. Offline, the computer is still signed out here and the
/// error says the server was not told.
pub fn sign_out() -> Result<(), String> {
    let remote = match (load_email(), device_id()) {
        (Some(email), Some(device)) => call_backend("license-deactivate", serde_json::json!({ "email": email, "device": device })).map(|_| ()),
        _ => Ok(()),
    };
    forget().map_err(|e| e.to_string())?;
    if let Some(p) = email_file().filter(|p| p.exists()) {
        std::fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    remote.map_err(|e| format!("Signed out on this computer, but the license server was not told ({e}). Sign in again and out when online to free the license."))
}

/// A link to Stripe's customer portal for the signed-in Pro subscription (cancel, change the
/// card, invoices).
pub fn billing_portal_url() -> Result<String, String> {
    let email = load_email().ok_or("Sign in first.")?;
    let device = device_id().ok_or("ScreenForge cannot identify this computer.")?;
    let v = call_backend("billing-portal", serde_json::json!({ "email": email, "device": device }))?;
    v.get("url").and_then(|u| u.as_str()).map(String::from).ok_or_else(|| "no portal url".to_string())
}

/// Bumped whenever the key on this computer is saved or removed (activated, renewed, signed
/// out, moved away), so every part of the app that shows the plan can refresh: the agent
/// panel and Export each check [`license_generation`] and re-read when it changed.
static LICENSE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn license_generation() -> u64 {
    LICENSE_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

fn license_changed() {
    LICENSE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Remove the saved key (the license moved to another computer).
pub fn forget() -> std::io::Result<()> {
    match key_file() {
        Some(p) if p.exists() => {
            std::fs::remove_file(p)?;
            license_changed();
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn save(key: &str) -> std::io::Result<()> {
    let p = key_file().ok_or_else(|| std::io::Error::other("no home dir"))?;
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, key.trim())?;
    license_changed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let (sk, pk) = keygen();
        let p = Payload { email: "a@b.c".into(), plan: "monthly".into(), expires: Some(Utc::now().date_naive() + chrono::Duration::days(30)), issued: Utc::now().date_naive(), device: None };
        let key = sign(&sk, &p).unwrap();
        assert!(matches!(verify_with(&key, &pk), Status::Valid(_)));
        let expired = Payload { expires: Some(Utc::now().date_naive() - chrono::Duration::days(1)), ..p.clone() };
        assert!(matches!(verify_with(&sign(&sk, &expired).unwrap(), &pk), Status::Expired(_)));
        assert!(matches!(verify_with("SF1.xx.yy", &pk), Status::Invalid(_)));
        let (_, other) = keygen();
        assert!(matches!(verify_with(&key, &other), Status::Invalid(_)));
    }

    #[test]
    fn a_key_only_works_on_its_own_computer() {
        let (sk, pk) = keygen();
        let here = "a".repeat(64);
        let p = Payload { email: "a@b.c".into(), plan: "lifetime".into(), expires: None, issued: Utc::now().date_naive(), device: Some(here.clone()) };
        let key = sign(&sk, &p).unwrap();
        assert!(for_device(verify_with(&key, &pk), Some(&here)).is_valid());
        assert!(!for_device(verify_with(&key, &pk), Some(&"b".repeat(64))).is_valid());
        assert!(!for_device(verify_with(&key, &pk), None).is_valid());
        // keys from before activation are bound to no computer: activate once
        let old = sign(&sk, &Payload { device: None, ..p }).unwrap();
        assert!(matches!(for_device(verify_with(&old, &pk), Some(&here)), Status::Invalid(m) if m.contains("Activate")));
    }

    #[test]
    fn keys_from_the_backend_parse_with_or_without_a_device() {
        let old: Payload = serde_json::from_str(r#"{"email":"a@b.c","plan":"lifetime","expires":null,"issued":"2026-10-12"}"#).unwrap();
        assert_eq!(old.device, None);
        let new: Payload = serde_json::from_str(r#"{"email":"a@b.c","plan":"lifetime","expires":null,"issued":"2026-10-12","device":"ab"}"#).unwrap();
        assert_eq!(new.device.as_deref(), Some("ab"));
    }

    #[test]
    fn ioreg_uuid_parses() {
        let text = "+-o MacBookPro18,3  <class IOPlatformExpertDevice>\n  {\n    \"IOPlatformUUID\" = \"5A3C1F2E-0B6D-4E8A-9C7B-1234567890AB\"\n    \"model\" = <\"x\">\n  }\n";
        assert_eq!(parse_ioreg_uuid(text).as_deref(), Some("5A3C1F2E-0B6D-4E8A-9C7B-1234567890AB"));
        assert_eq!(parse_ioreg_uuid("nothing here"), None);
    }

    #[test]
    fn activation_checks_input_before_calling_the_server() {
        assert!(activate("not-an-email", true).unwrap_err().contains("email"));
        assert!(confirm_code("you@example.com", "12 34").unwrap_err().contains("6-digit"));
        assert!(confirm_code("you@example.com", "1234567").unwrap_err().contains("6-digit"));
    }

    #[test]
    fn a_reply_without_a_key_is_no_license() {
        assert_eq!(store_key(&serde_json::json!({ "status": "verify" }), "you@example.com").unwrap_err(), "no license");
    }

    #[test]
    fn device_hash_is_stable_and_opaque() {
        let a = hex(&Sha256::digest("abc".as_bytes()));
        assert_eq!(a.len(), 64);
        assert_eq!(a, hex(&Sha256::digest("abc".as_bytes())));
        assert_ne!(a, hex(&Sha256::digest("abd".as_bytes())));
    }
}
