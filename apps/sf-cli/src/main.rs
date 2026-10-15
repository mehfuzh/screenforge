use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use sf_engine::stage::OutputSize;
use sf_media::VideoCodec;
use sf_render::{Compositor, Gpu};
use sf_session::Session;

#[derive(Parser)]
#[command(name = "sf", version, about = "ScreenForge – open source screen recorder and editor")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Codec {
    H264,
    Hevc,
    Prores,
    Gif,
    Vp9,
}

impl From<Codec> for VideoCodec {
    fn from(c: Codec) -> Self {
        match c {
            Codec::H264 => VideoCodec::H264,
            Codec::Hevc => VideoCodec::Hevc,
            Codec::Prores => VideoCodec::ProRes4444,
            Codec::Gif => VideoCodec::Gif,
            Codec::Vp9 => VideoCodec::Vp9,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Print a summary of a .screenstudio package (migrating old formats in memory)
    Inspect {
        path: PathBuf,
        /// Dump the (migrated) project.json
        #[arg(long)]
        json: bool,
    },
    /// Write QuickLook/Preview.png for a project (the recent-projects thumbnail)
    Preview { project: PathBuf },
    /// Render a single output frame to PNG
    Frame {
        path: PathBuf,
        /// Output time in milliseconds
        #[arg(long, default_value_t = 0.0)]
        at_ms: f64,
        #[arg(short, long, default_value = "frame.png")]
        out: PathBuf,
        #[arg(long, default_value_t = 1080)]
        height: u32,
    },
    /// Export the project to a video file
    Export {
        path: PathBuf,
        #[arg(short, long, default_value = "export.mp4")]
        out: PathBuf,
        #[arg(long, default_value_t = 60.0)]
        fps: f64,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        #[arg(long, value_enum, default_value_t = Codec::H264)]
        codec: Codec,
        #[arg(long, default_value_t = 0.85)]
        quality: f64,
        #[arg(long)]
        no_audio: bool,
        /// Export only [start,end] ms of the output timeline
        #[arg(long, num_args = 2, value_names = ["START_MS", "END_MS"])]
        range: Option<Vec<f64>>,
    },
    /// Re-save a package (useful to migrate v3 → v4 on disk). Writes project.json.bak first.
    Resave { path: PathBuf },
    /// List presets in the presets folder (~/Documents/Screen Studio Presets)
    Presets {
        #[arg(long)]
        folder: Option<PathBuf>,
    },
    /// Save a project's configuration as a .screenstudiopreset package
    PresetCreate { project: PathBuf, out: PathBuf },
    /// Apply a preset to a project (keeps the crop) and save it
    PresetApply { project: PathBuf, preset: PathBuf },
    /// License key tools (issuer side): keygen, sign, verify
    License {
        #[command(subcommand)]
        cmd: LicenseCmd,
    },
}

#[derive(Subcommand)]
enum LicenseCmd {
    /// Generate an issuer key pair (keep the secret in your payment backend)
    Keygen,
    /// Sign a key: --email, --plan lifetime|monthly, --days (monthly validity); secret from SF_LICENSE_SIGNING_KEY
    Sign {
        #[arg(long)]
        email: String,
        #[arg(long, default_value = "lifetime")]
        plan: String,
        #[arg(long, default_value_t = 31)]
        days: i64,
    },
    /// Verify a key against the embedded (or SF_LICENSE_PUBLIC_KEY) issuer key
    Verify { key: String },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    match Cli::parse().cmd {
        Cmd::Inspect { path, json } => inspect(&path, json),
        Cmd::Preview { project } => {
            sf_session::preview::write_quicklook_preview(&project)?;
            println!("wrote {}", project.join(sf_project::package::QUICKLOOK_PREVIEW).display());
            Ok(())
        }
        Cmd::Frame { path, at_ms, out, height } => frame(&path, at_ms, &out, height),
        Cmd::Export { path, out, fps, height, codec, quality, no_audio, range } => {
            let gpu = Gpu::headless()?;
            let mut session = Session::open(&path, gpu.clone())?;
            let mut comp = Compositor::new(gpu);
            let opts = sf_export::ExportOptions {
                output: out,
                fps,
                max_height: height,
                codec: codec.into(),
                quality,
                include_audio: !no_audio,
                range_ms: range.map(|r| (r[0], r[1])),
            };
            let started = std::time::Instant::now();
            let result = sf_export::export(&mut session, &mut comp, &opts, |p| {
                if p.frame % 30 == 0 || p.frame == p.total_frames {
                    eprint!("\r{:>6}/{} frames  {:5.1}s", p.frame, p.total_frames, started.elapsed().as_secs_f64());
                    let _ = std::io::stderr().flush();
                }
            })?;
            eprintln!("\nwrote {}", result.display());
            Ok(())
        }
        Cmd::Presets { folder } => {
            let folder = folder.or_else(sf_project::Preset::default_folder).ok_or_else(|| anyhow::anyhow!("no presets folder"))?;
            for p in sf_project::Preset::list(&folder) {
                println!("{:<30} bg={} cursor={} radius={}  {}", p.name(), p.meta.project_config.styles.background.kind, p.meta.project_config.cursor.set.id, p.meta.project_config.styles.screen_border_radius, p.path.display());
            }
            Ok(())
        }
        Cmd::PresetCreate { project, out } => {
            let pkg = sf_project::ProjectPackage::open(&project)?;
            let name = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let assets = pkg.path.join("assets");
            sf_project::Preset::create(&out, &name, &pkg.project.config, assets.is_dir().then_some(assets.as_path()))?;
            println!("wrote {}", out.display());
            Ok(())
        }
        Cmd::PresetApply { project, preset } => {
            let mut pkg = sf_project::ProjectPackage::open(&project)?;
            let pr = sf_project::Preset::open(&preset)?;
            let dir = pkg.path.clone();
            pr.apply(&mut pkg.project, &dir)?;
            pkg.save()?;
            println!("applied {} to {}", pr.name(), project.display());
            Ok(())
        }
        Cmd::License { cmd } => {
            match cmd {
                LicenseCmd::Keygen => {
                    let (sk, pk) = sf_license::keygen();
                    println!("SF_LICENSE_SIGNING_KEY={sk}\nISSUER_PUBLIC_KEY_B64={pk}");
                }
                LicenseCmd::Sign { email, plan, days } => {
                    let sk = std::env::var("SF_LICENSE_SIGNING_KEY").map_err(|_| anyhow::anyhow!("set SF_LICENSE_SIGNING_KEY"))?;
                    let today = chrono::Utc::now().date_naive();
                    let payload = sf_license::Payload { email, expires: if plan == "lifetime" { None } else { Some(today + chrono::Duration::days(days)) }, plan, issued: today };
                    println!("{}", sf_license::sign(&sk, &payload)?);
                }
                LicenseCmd::Verify { key } => {
                    let pk = std::env::var("SF_LICENSE_PUBLIC_KEY").unwrap_or_else(|_| sf_license::ISSUER_PUBLIC_KEY_B64.to_string());
                    println!("{:?}", sf_license::verify_with(&key, &pk));
                }
            }
            Ok(())
        }
        Cmd::Resave { path } => {
            let pkg = sf_project::ProjectPackage::open(&path)?;
            pkg.save()?;
            println!("saved {} (migrated: {})", path.display(), pkg.migrated);
            Ok(())
        }
    }
}

fn inspect(path: &std::path::Path, json: bool) -> Result<()> {
    let pkg = sf_project::ProjectPackage::open(path)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&pkg.project)?);
        return Ok(());
    }
    let p = &pkg.project;
    println!("{}  ({})", p.name, p.id);
    println!("  format: v{}{}   writer: {}", p.version, if pkg.migrated { " (migrated from v3)" } else { "" }, pkg.meta.version);
    println!("  created: {}   saved: {}", p.created_at, p.last_saved_at.map(|d| d.to_string()).unwrap_or_else(|| "never".into()));
    let c = &p.config;
    println!("  output: aspect={} padding={} | background={} {} | cursor={} size={} | layout={}", c.output.aspect_ratio.label(), c.output.padding_ratio01, c.styles.background.kind, c.styles.background.system_name.as_deref().unwrap_or(&c.styles.background.color), c.cursor.set.id, c.cursor.size, c.default_layout.kind);
    for s in &p.scenes {
        let tl = sf_engine::timeline::Timeline::from_scene(s);
        println!("  scene '{}' [{}]  slices={} zooms={} layouts={} masks={} voiceovers={}  duration={:.1}s", s.name, s.kind, s.slices.len(), s.zooms.len(), s.layouts.len(), s.masks.len(), s.voice_overs.len(), tl.duration_ms() / 1000.0);
        for (i, sl) in s.slices.iter().enumerate() {
            println!("    #{i:<2} src {:>9.1} → {:>9.1} ms  speed ×{:<4}  vol {}  {}", sl.source_start_ms, sl.source_end_ms, 1.0 / sl.time_scale, sl.volume, if sl.hide_cursor { "hide-cursor" } else { "" });
        }
    }
    if let Some(r) = &pkg.recording {
        println!("  recording: polyrecorder {} state={} duration={:.1}s", r.polyrecorder_version.as_deref().unwrap_or("?"), r.state.as_deref().unwrap_or("?"), r.duration_ms() / 1000.0);
        for rec in &r.recorders {
            let files: Vec<String> = rec.sessions.iter().filter_map(|s| s.output_filename.clone().or(s.mouse_moves_filename.clone())).collect();
            println!("    {:<22} {:<12} sessions={} {}", rec.id, rec.kind, rec.sessions.len(), files.join(","));
        }
        if let Ok(src) = sf_session::SourceInfo::from_package(&pkg) {
            println!("  video: {}x{} @ {:.0}fps, {:.1}s, bounds {}x{} pt at ({},{})", src.native_width, src.native_height, src.fps, src.duration_ms / 1000.0, src.bounds.width, src.bounds.height, src.bounds.x, src.bounds.y);
        }
    }
    Ok(())
}

fn frame(path: &std::path::Path, at_ms: f64, out: &std::path::Path, height: u32) -> Result<()> {
    let gpu = Gpu::headless()?;
    let mut session = Session::open(path, gpu.clone())?;
    let mut comp = Compositor::new(gpu);
    let size: OutputSize = session.output_size(height);
    // warm the springs a little before the requested time so the frame looks like playback
    let fps = 60.0;
    let warm = (at_ms - 2000.0).max(0.0);
    let mut t = warm;
    let mut desc = None;
    while t <= at_ms + 1e-6 {
        desc = Some(session.frame_desc_at(t, size)?);
        t += 1000.0 / fps;
    }
    comp.render(&desc.unwrap());
    let px = comp.read_pixels()?;
    image::save_buffer(out, &px, size.width, size.height, image::ColorType::Rgba8)?;
    println!("wrote {} ({}x{})", out.display(), size.width, size.height);
    Ok(())
}
