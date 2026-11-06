//! Updates: the newest GitHub release compared with this build's version, downloaded and
//! installed in place.
//!
//! The check is one HTTPS GET of the public releases API (no account, nothing about the user is
//! sent). It runs on launch at most once a day while `Prefs::check_updates` is on, and whenever
//! "Check for updates now" is pressed. When a newer release is out and this build knows how it
//! was installed, the matching file is downloaded in the background, checked against the
//! SHA-256 GitHub publishes for it (and on macOS against the running app's signing team), and
//! staged. "Restart to update" then quits; a small helper waits for the process to exit, puts the
//! new build in place (or runs the installer) and opens it again.
//!
//! Builds that cannot update themselves (development builds, the portable Windows zip, a folder
//! the user cannot write to) fall back to opening the release page.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const LATEST_URL: &str = "https://api.github.com/repos/mehfuzh/screenforge/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/mehfuzh/screenforge/releases/latest";
/// Automatic checks run at most this often.
pub const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;
const USER_AGENT: &str = concat!("ScreenForge/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Checking,
    /// A newer release this build cannot install itself: its version (no `v`) and page.
    Available { version: String, url: String },
    /// The newer release's file is being fetched (`total` is 0 when unknown).
    Downloading { version: String, done: u64, total: u64 },
    /// Downloaded and verified; installed on the next restart.
    Ready { version: String },
    /// The download or its checks failed; the release page is the way out.
    InstallFailed { version: String, url: String, error: String },
    UpToDate,
    Failed(String),
}

impl State {
    /// A newer version is known (the settings button shows a dot).
    pub fn has_update(&self) -> bool {
        matches!(self, State::Available { .. } | State::Downloading { .. } | State::Ready { .. } | State::InstallFailed { .. })
    }
}

/// One file of a release.
#[derive(Debug, Clone, PartialEq)]
struct Asset {
    url: String,
    /// Lowercase hex, from the API's `digest` (`sha256:<hex>`).
    sha256: String,
    size: u64,
}

/// How this copy of ScreenForge was installed, which decides the file to fetch and how to put
/// it in place.
// each platform builds only its own variants
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
enum Install {
    /// `…/ScreenForge.app`; replaced by the app inside `ScreenForge-macOS.zip`.
    MacBundle { app: PathBuf },
    /// The portable Linux folder; replaced by the one inside the `.tar.gz`.
    LinuxFolder { dir: PathBuf },
    /// `/opt/screenforge` from the `.deb`; reinstalled with `pkexec apt-get`.
    LinuxDeb,
    /// Installed by the Inno Setup installer; it runs again silently.
    WindowsSetup { exe: PathBuf },
}

impl Install {
    fn asset_name(&self) -> &'static str {
        match self {
            Install::MacBundle { .. } => "ScreenForge-macOS.zip",
            Install::LinuxFolder { .. } => "ScreenForge-linux-x86_64.tar.gz",
            Install::LinuxDeb => "screenforge_amd64.deb",
            Install::WindowsSetup { .. } => "ScreenForge-windows-x86_64-setup.exe",
        }
    }
}

/// What "Restart to update" runs once the app has quit.
#[derive(Debug, Clone)]
enum Pending {
    /// Swap `new` into `current` (both on one volume), drop `stage`, open `current`.
    Replace { current: PathBuf, new: PathBuf, stage: PathBuf, mac: bool },
    Deb { file: PathBuf },
    Setup { file: PathBuf, exe: PathBuf },
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::Idle))
}

fn pending() -> &'static Mutex<Option<Pending>> {
    static P: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

static QUIT: AtomicBool = AtomicBool::new(false);

fn set(next: State) {
    if let Ok(mut s) = state().lock() {
        *s = next;
    }
}

/// This build's version; `SF_FAKE_VERSION` pretends to be another one, to see the notice.
pub fn version() -> String {
    std::env::var("SF_FAKE_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

pub fn current() -> State {
    state().lock().map(|s| s.clone()).unwrap_or(State::Idle)
}

/// Ask GitHub for the newest release in the background, and fetch it when this build can
/// install it; `ctx` is repainted as it goes.
pub fn check(ctx: &egui::Context) {
    {
        let Ok(mut s) = state().lock() else { return };
        // a download in flight or a staged update stays as it is
        if matches!(*s, State::Checking | State::Downloading { .. } | State::Ready { .. }) {
            return;
        }
        *s = State::Checking;
    }
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let install = detect();
        let latest = fetch_latest(install.as_ref().map(Install::asset_name));
        match latest {
            Err(e) => {
                tracing::warn!("update check failed: {e}");
                set(State::Failed(e));
            }
            Ok(r) if !is_newer(&r.tag, &version()) => set(State::UpToDate),
            Ok(r) => {
                let version = r.tag.trim_start_matches(['v', 'V']).to_string();
                match (install, r.asset) {
                    (Some(install), Some(asset)) => {
                        set(State::Downloading { version: version.clone(), done: 0, total: asset.size });
                        ctx.request_repaint();
                        match prepare(&install, &asset, &version, &ctx) {
                            Ok(p) => {
                                if let Ok(mut slot) = pending().lock() {
                                    *slot = Some(p);
                                }
                                set(State::Ready { version });
                            }
                            Err(error) => {
                                tracing::warn!("update to v{version} failed: {error}");
                                set(State::InstallFailed { version, url: r.page, error });
                            }
                        }
                    }
                    _ => set(State::Available { version, url: r.page }),
                }
            }
        }
        ctx.request_repaint();
    });
}

/// Quit and install the staged update. On success the app should close (see [`take_quit`]).
pub fn restart() {
    let Some(p) = pending().lock().ok().and_then(|p| p.clone()) else { return };
    match spawn_installer(&p) {
        Ok(()) => QUIT.store(true, Ordering::SeqCst),
        Err(error) => {
            let version = match current() {
                State::Ready { version } => version,
                _ => String::new(),
            };
            tracing::warn!("could not start the update: {error}");
            set(State::InstallFailed { version, url: RELEASES_URL.into(), error });
        }
    }
}

/// Whether [`restart`] asked the app to quit (reset once read).
pub fn take_quit() -> bool {
    QUIT.swap(false, Ordering::SeqCst)
}

/// Whether an automatic check is due (`last` = unix seconds of the previous one).
pub fn due(last: Option<u64>, now: u64) -> bool {
    last.is_none_or(|t| now.saturating_sub(t) >= CHECK_EVERY_SECS)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

struct Latest {
    tag: String,
    page: String,
    /// The file named `want`, when it is there with a SHA-256.
    asset: Option<Asset>,
}

/// The newest published release (drafts and pre-releases excluded by the API).
fn fetch_latest(want: Option<&str>) -> Result<Latest, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    let body = agent
        .get(LATEST_URL)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    parse_release(&body, want)
}

fn parse_release(body: &str, want: Option<&str>) -> Result<Latest, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("no tag_name in the release")?.to_string();
    let page = v["html_url"].as_str().unwrap_or(RELEASES_URL).to_string();
    let asset = want.and_then(|name| {
        let a = v["assets"].as_array()?.iter().find(|a| a["name"].as_str() == Some(name))?;
        let sha256 = a["digest"].as_str()?.strip_prefix("sha256:")?.to_ascii_lowercase();
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(Asset { url: a["browser_download_url"].as_str()?.to_string(), sha256, size: a["size"].as_u64().unwrap_or(0) })
    });
    Ok(Latest { tag, page, asset })
}

/// How the running copy was installed, or `None` when it cannot update itself.
fn detect() -> Option<Install> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    #[cfg(target_os = "macos")]
    {
        // …/ScreenForge.app/Contents/MacOS/screenforge
        let app = exe.parent()?.parent()?.parent()?;
        (app.extension().is_some_and(|e| e == "app") && writable(app.parent()?)).then(|| Install::MacBundle { app: app.to_path_buf() })
    }
    #[cfg(target_os = "linux")]
    {
        let dir = exe.parent()?;
        if dir == Path::new("/opt/screenforge") {
            return on_path("pkexec").then_some(Install::LinuxDeb);
        }
        // the portable folder (bundle-linux.sh) carries ffmpeg's license and the assets
        let portable = dir.join("FFMPEG-LICENSE.txt").is_file() && dir.join("assets").is_dir();
        (portable && writable(dir.parent()?)).then(|| Install::LinuxFolder { dir: dir.to_path_buf() })
    }
    #[cfg(target_os = "windows")]
    {
        // the Inno Setup uninstaller sits next to an installed copy, not the portable zip
        exe.parent()?.join("unins000.exe").is_file().then(|| Install::WindowsSetup { exe })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = exe;
        None
    }
}

#[allow(dead_code)]
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".screenforge-write-test-{}", std::process::id()));
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[allow(dead_code)]
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// Download, verify and stage the update for `install`.
fn prepare(install: &Install, asset: &Asset, version: &str, ctx: &egui::Context) -> Result<Pending, String> {
    match install {
        Install::MacBundle { app } | Install::LinuxFolder { dir: app } => {
            let mac = matches!(install, Install::MacBundle { .. });
            // staged next to the install so the final swap is a rename on one volume
            let stage = app.parent().ok_or("no parent folder")?.join(".ScreenForge-update");
            let _ = std::fs::remove_dir_all(&stage);
            std::fs::create_dir_all(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
            let result = (|| {
                let archive = stage.join(if mac { "update.zip" } else { "update.tar.gz" });
                download(asset, &archive, version, ctx)?;
                let out = stage.join("new");
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                if mac {
                    run(std::process::Command::new("ditto").args(["-x", "-k"]).arg(&archive).arg(&out))?;
                } else {
                    run(std::process::Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&out))?;
                }
                let _ = std::fs::remove_file(&archive);
                let new = out.join(if mac { "ScreenForge.app" } else { "ScreenForge" });
                if !new.exists() {
                    return Err(format!("{} is not in the download", new.file_name().unwrap_or_default().to_string_lossy()));
                }
                if mac {
                    verify_signature(app, &new)?;
                }
                Ok(Pending::Replace { current: app.clone(), new, stage: stage.clone(), mac })
            })();
            if result.is_err() {
                let _ = std::fs::remove_dir_all(&stage);
            }
            result
        }
        Install::LinuxDeb | Install::WindowsSetup { .. } => {
            let dir = std::env::temp_dir().join("screenforge-update");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let file = dir.join(install.asset_name());
            download(asset, &file, version, ctx)?;
            Ok(match install {
                Install::WindowsSetup { exe } => Pending::Setup { file, exe: exe.clone() },
                _ => Pending::Deb { file },
            })
        }
    }
}

/// Stream `asset` to `dest`, reporting progress, and check its SHA-256.
fn download(asset: &Asset, dest: &Path, version: &str, ctx: &egui::Context) -> Result<(), String> {
    use sha2::Digest;
    // no overall timeout: the files are 100+ MB; a stalled read still gives up
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(60)).build();
    let resp = agent.get(&asset.url).set("User-Agent", USER_AGENT).call().map_err(|e| e.to_string())?;
    let total = if asset.size > 0 { asset.size } else { resp.header("Content-Length").and_then(|l| l.parse().ok()).unwrap_or(0) };
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let (mut done, mut shown) = (0u64, 0u64);
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - shown >= 1 << 20 {
            shown = done;
            set(State::Downloading { version: version.to_string(), done, total });
            ctx.request_repaint();
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got != asset.sha256 {
        let _ = std::fs::remove_file(dest);
        return Err("the download does not match the release's SHA-256".into());
    }
    Ok(())
}

fn run(cmd: &mut std::process::Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.lines().last().unwrap_or("failed").trim().to_string())
    }
}

/// The new app must carry a valid signature, from the same team as the running one.
fn verify_signature(current: &Path, new: &Path) -> Result<(), String> {
    run(std::process::Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(new)).map_err(|e| format!("the new app's signature is not valid: {e}"))?;
    let team = |p: &Path| run(std::process::Command::new("codesign").args(["-dv", "--verbose=2"]).arg(p)).ok().and_then(|t| team_id(&t));
    match (team(current), team(new)) {
        (Some(a), Some(b)) if a != b => Err(format!("the new app is signed by another team ({b})")),
        (Some(_), None) => Err("the new app has no signing team".into()),
        _ => Ok(()),
    }
}

/// `TeamIdentifier=…` from `codesign -dv` (`not set` for ad-hoc signatures counts as none).
fn team_id(codesign_output: &str) -> Option<String> {
    let t = codesign_output.lines().find_map(|l| l.strip_prefix("TeamIdentifier="))?.trim();
    (!t.is_empty() && t != "not set").then(|| t.to_string())
}

/// Waits for pid `$1` to exit, then moves `$3` to `$2`, keeping the old `$2` until the move
/// worked; `$4` (the staging folder) is removed on success.
const SWAP_SCRIPT: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
if mv "$2" "$4/previous"; then
  if mv "$3" "$2"; then rm -rf "$4"; else mv "$4/previous" "$2"; fi
fi"#;

/// Start the helper that installs `p` once this process has exited.
fn spawn_installer(p: &Pending) -> Result<(), String> {
    let pid = std::process::id().to_string();
    match p {
        Pending::Replace { current, new, stage, mac } => {
            let open = if *mac { r#"open "$2""# } else { r#"cd "$2" && exec ./screenforge"# };
            let script = format!("{SWAP_SCRIPT}\n{open}");
            std::process::Command::new("/bin/sh").arg("-c").arg(script).arg("screenforge-update").arg(&pid).arg(current).arg(new).arg(stage).spawn().map_err(|e| e.to_string())?;
        }
        Pending::Deb { file } => {
            let script = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
pkexec apt-get install -y "$2"
exec /opt/screenforge/screenforge"#;
            std::process::Command::new("/bin/sh").arg("-c").arg(script).arg("screenforge-update").arg(&pid).arg(file).spawn().map_err(|e| e.to_string())?;
        }
        Pending::Setup { file, exe } => spawn_setup(&pid, file, exe)?,
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn spawn_setup(pid: &str, file: &Path, exe: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
    // a per-user install (the default, under %LOCALAPPDATA%) stays per-user; anything else
    // asks for admin rights as the original install did
    let local = std::env::var_os("LOCALAPPDATA").is_some_and(|l| exe.starts_with(l));
    let scope = if local { "/CURRENTUSER" } else { "/ALLUSERS" };
    let script = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; \
         Start-Process -FilePath {setup} -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','{scope}' -Wait; \
         Start-Process -FilePath {exe}",
        setup = q(file),
        exe = q(exe),
    );
    std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script]).creation_flags(CREATE_NO_WINDOW).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn spawn_setup(_pid: &str, _file: &Path, _exe: &Path) -> Result<(), String> {
    Err("the Windows installer only runs on Windows".into())
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

    const RELEASE: &str = r#"{"tag_name":"v0.1.48","html_url":"https://github.com/mehfuzh/screenforge/releases/tag/v0.1.48","assets":[
        {"name":"ScreenForge-macOS.zip","size":91889348,"digest":"sha256:DA85D7FDA1B128E4C4B06ACAC84BD7C9A24FA3A22CCB6A0E9BF4A7F5917DBCFD","browser_download_url":"https://github.com/mehfuzh/screenforge/releases/download/v0.1.48/ScreenForge-macOS.zip"},
        {"name":"screenforge_amd64.deb","size":10,"digest":null,"browser_download_url":"https://example.invalid/x.deb"}]}"#;

    #[test]
    fn picks_the_platform_file_with_its_sha256() {
        let r = parse_release(RELEASE, Some("ScreenForge-macOS.zip")).unwrap();
        assert_eq!(r.tag, "v0.1.48");
        let a = r.asset.unwrap();
        assert_eq!(a.sha256, "da85d7fda1b128e4c4b06acac84bd7c9a24fa3a22ccb6a0e9bf4a7f5917dbcfd");
        assert_eq!(a.size, 91_889_348);
        assert!(a.url.ends_with("/v0.1.48/ScreenForge-macOS.zip"));
    }

    #[test]
    fn no_digest_or_no_file_means_no_self_update() {
        assert!(parse_release(RELEASE, Some("screenforge_amd64.deb")).unwrap().asset.is_none());
        assert!(parse_release(RELEASE, Some("ScreenForge-windows-x86_64-setup.exe")).unwrap().asset.is_none());
        assert!(parse_release(RELEASE, None).unwrap().asset.is_none());
    }

    #[test]
    fn reads_the_signing_team() {
        assert_eq!(team_id("Identifier=studio.screenforge\nTeamIdentifier=ABCDE12345\n").as_deref(), Some("ABCDE12345"));
        assert_eq!(team_id("TeamIdentifier=not set\n"), None);
        assert_eq!(team_id("Signature=adhoc\n"), None);
    }

    #[cfg(unix)]
    #[test]
    fn swap_script_replaces_the_install_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("ScreenForge");
        let stage = tmp.path().join(".ScreenForge-update");
        let new = stage.join("new/ScreenForge");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("v"), "old").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("v"), "new").unwrap();
        // an exited pid, and no relaunch
        let st = std::process::Command::new("/bin/sh").arg("-c").arg(SWAP_SCRIPT).arg("t").arg("999999999").arg(&current).arg(&new).arg(&stage).status().unwrap();
        assert!(st.success());
        assert_eq!(std::fs::read_to_string(current.join("v")).unwrap(), "new");
        assert!(!stage.exists());
    }
}

#[cfg(test)]
mod network {
    /// Talks to GitHub: `cargo test -p studio -- --ignored latest_release`.
    #[test]
    #[ignore]
    fn latest_release_has_a_version_tag_and_platform_files() {
        for name in ["ScreenForge-macOS.zip", "ScreenForge-linux-x86_64.tar.gz", "screenforge_amd64.deb", "ScreenForge-windows-x86_64-setup.exe"] {
            let r = super::fetch_latest(Some(name)).unwrap();
            assert!(super::parse(&r.tag).is_some(), "{}", r.tag);
            assert!(r.page.starts_with("https://github.com/"), "{}", r.page);
            assert!(r.asset.is_some(), "{name} missing or without a digest");
        }
    }
}
