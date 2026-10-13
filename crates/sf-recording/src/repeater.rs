//! Frame repeater shared by the stdin-fed display recorders (ScreenCaptureKit on macOS, the
//! ScreenCast portal on Wayland): capture callbacks keep the latest frame in a [`FrameStore`]
//! and a thread feeds ffmpeg at a constant frame rate. If the encoder falls behind, missed
//! ticks are caught up with duplicate frames so the media duration always equals wall time
//! (and therefore the input event timeline).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;

use crate::display::hls_raw_args;
use crate::ffmpeg::FfmpegProc;
use crate::log::RecLog;
use crate::Result;

/// Latest captured frame (tightly packed, `width * 4` bytes per row).
pub(crate) struct FrameStore {
    pub latest: Mutex<Option<Arc<Vec<u8>>>>,
    /// Expected frame size; the macOS callback drops frames of any other size.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub width: usize,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub height: usize,
    pub received: AtomicU64,
}

impl FrameStore {
    pub fn new(width: usize, height: usize) -> Self {
        FrameStore { latest: Mutex::new(None), width, height, received: AtomicU64::new(0) }
    }

    pub fn put(&self, frame: Vec<u8>) {
        *self.latest.lock() = Some(Arc::new(frame));
        self.received.fetch_add(1, Ordering::Relaxed);
    }

    /// Wait up to `timeout` for the first frame.
    pub fn wait_first(&self, timeout: Duration) -> bool {
        let t = Instant::now();
        while self.received.load(Ordering::Relaxed) == 0 && t.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.received.load(Ordering::Relaxed) > 0
    }
}

pub(crate) enum Cmd {
    Begin { index: usize, dir: PathBuf, reply: Sender<Result<()>> },
    End { reply: Sender<Result<u64>> },
    Stop,
}

/// What the stored frames are and how ffmpeg turns them into the output size.
pub(crate) struct RawInput {
    /// ffmpeg pixel format of the stored frames (`bgra`, `bgr0`, `rgb0`, …).
    pub pix_fmt: &'static str,
    pub width: u32,
    pub height: u32,
    /// Optional `-vf` chain (crop/scale to the output size).
    pub filter: Option<String>,
}

pub(crate) struct Repeater {
    pub rx: Receiver<Cmd>,
    pub store: Arc<FrameStore>,
    pub log: RecLog,
    pub component: &'static str,
    pub fps: u32,
    pub input: RawInput,
    pub bitrate: u64,
    pub encoder: &'static str,
}

struct Active {
    proc: FfmpegProc,
    t0: Instant,
    written: u64,
    failed: AtomicBool,
}

impl Repeater {
    pub fn run(self) {
        let mut active: Option<Active> = None;
        let frame_dur = Duration::from_secs_f64(1.0 / self.fps as f64);
        loop {
            let cmd = match &active {
                None => self.rx.recv().ok(),
                Some(a) => {
                    let due = a.t0 + frame_dur * (a.written as u32);
                    let now = Instant::now();
                    if due > now {
                        match self.rx.recv_timeout(due - now) {
                            Ok(c) => Some(c),
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                            Err(_) => break,
                        }
                    } else {
                        match self.rx.try_recv() {
                            Ok(c) => Some(c),
                            Err(crossbeam_channel::TryRecvError::Empty) => None,
                            Err(_) => break,
                        }
                    }
                }
            };
            match cmd {
                Some(Cmd::Begin { index, dir, reply }) => {
                    if let Some(a) = active.take() {
                        let _ = a.proc.finish();
                    }
                    let r = self.spawn(index, &dir).map(|p| {
                        active = Some(Active { proc: p, t0: Instant::now(), written: 0, failed: AtomicBool::new(false) });
                    });
                    let _ = reply.send(r);
                }
                Some(Cmd::End { reply }) => {
                    let r = match active.take() {
                        Some(a) => {
                            let n = a.written;
                            a.proc.finish().map(|_| n)
                        }
                        None => Ok(0),
                    };
                    let _ = reply.send(r);
                }
                Some(Cmd::Stop) => break,
                None => {
                    if let Some(a) = active.as_mut() {
                        if a.failed.load(Ordering::Relaxed) {
                            // ffmpeg died; keep counting ticks so End returns promptly
                            a.written += 1;
                            continue;
                        }
                        let frame = self.store.latest.lock().clone();
                        if let Some(f) = frame {
                            if let Err(e) = a.proc.write(&f) {
                                self.log.error(self.component, format!("Frame write failed: {e}"));
                                a.failed.store(true, Ordering::Relaxed);
                            }
                        }
                        a.written += 1;
                    }
                }
            }
        }
        if let Some(a) = active.take() {
            let _ = a.proc.finish();
        }
    }

    fn spawn(&self, index: usize, dir: &Path) -> Result<FfmpegProc> {
        let args = hls_raw_args(&self.input, self.fps, self.bitrate, self.encoder, index);
        self.log.notice(self.component, format!("Starting video session {index}. file=channel-1-display-{index}.m3u8"));
        FfmpegProc::spawn(&args, dir, self.log.clone(), self.component)
    }
}
