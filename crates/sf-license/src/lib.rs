//! License keys gate video export. Recording and editing are free and open source; a key
//! (US$29 once, lifetime) is checked offline against the embedded Ed25519 public key.
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

/// Public key of the ScreenForge license issuer (replace with yours before shipping;
/// `sf license keygen` prints a matching pair).
pub const ISSUER_PUBLIC_KEY_B64: &str = "fq9T9G8fJeZSPJtnjCsgO1V0BzXYds8UxAqAEjir4CM";

pub const PRICE_LABEL: &str = "US$29 once";
pub const PRICE_LIFETIME: &str = "US$29 once";
pub const PURCHASE_URL: &str = "https://screenforge.studio/pricing.html";

/// Supabase project hosting the edge functions in `backend/` (see backend/README.md).
/// Placeholders until the project exists; activation reports a clear error meanwhile.
pub const SUPABASE_URL: &str = "https://kxpppjdcqkxpybdawxum.supabase.co";
pub const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6Imt4cHBwamRjcWt4cHliZGF3eHVtIiwicm9sZSI6ImFub24iLCJpYXQiOjE3OTE4MTc3NjAsImV4cCI6MjEwNzM5Mzc2MH0.FT7EFB8DG8WW49NllafP4v6GfrGQ4ELYB-8NuvY5Jmw";

fn backend_configured() -> bool {
    !SUPABASE_URL.contains("SUPABASE_PROJECT_REF")
}

fn call_function(name: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    if !backend_configured() {
        return Err("The license server is not configured in this build.".into());
    }
    let url = format!("{SUPABASE_URL}/functions/v1/{name}");
    let resp = ureq::post(&url)
        .set("apikey", SUPABASE_ANON_KEY)
        .set("Authorization", &format!("Bearer {SUPABASE_ANON_KEY}"))
        .timeout(std::time::Duration::from_secs(15))
        .send_json(body);
    match resp {
        Ok(r) => r.into_json().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(404, _)) => Err("no license".into()),
        Err(ureq::Error::Status(code, r)) => Err(r.into_json::<serde_json::Value>().ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from)).unwrap_or_else(|| format!("server returned {code}"))),
        Err(e) => Err(format!("network error: {e}")),
    }
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

/// Exports allowed without a license (an honour-system counter kept next to the key file).
pub const FREE_EXPORTS: u32 = 3;

fn free_exports_file() -> Option<std::path::PathBuf> {
    key_file().map(|p| p.with_file_name("free-exports"))
}

/// How many free exports have been used on this machine.
pub fn free_exports_used() -> u32 {
    free_exports_file().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// Free exports still available.
pub fn free_exports_left() -> u32 {
    FREE_EXPORTS.saturating_sub(free_exports_used())
}

/// Count one more free export.
pub fn record_free_export() -> std::io::Result<()> {
    let p = free_exports_file().ok_or_else(|| std::io::Error::other("no home dir"))?;
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, (free_exports_used() + 1).to_string())
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
}
