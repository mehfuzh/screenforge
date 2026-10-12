//! Process-relative and wall clocks shared by every recorder.
//!
//! polyrecorder stamps every event and session with two times: `processTimeMs`
//! (milliseconds since its own process started, from a monotonic clock) and
//! `unixTimeMs` (epoch milliseconds). All recorders in one recording must use the
//! same origin, so the origin is captured once per process in [`init`] and
//! read through [`process_time_ms`].

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

#[cfg(target_os = "macos")]
extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> libc::c_int;
}

struct Clock {
    start: Instant,
    #[cfg(target_os = "macos")]
    start_mach: u64,
}

static CLOCK: OnceLock<Clock> = OnceLock::new();

fn clock() -> &'static Clock {
    CLOCK.get_or_init(|| Clock {
        start: Instant::now(),
        #[cfg(target_os = "macos")]
        start_mach: unsafe { mach_absolute_time() },
    })
}

/// Capture the process-time origin. Called automatically by the first clock read;
/// call it early (e.g. in `main`) so `processTimeMs` is measured from process start.
pub fn init() {
    let _ = clock();
}

/// Milliseconds since [`init`] (approximately: since the process started).
pub fn process_time_ms() -> f64 {
    clock().start.elapsed().as_secs_f64() * 1000.0
}

/// Epoch milliseconds (wall clock).
pub fn unix_time_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// Both clocks sampled at the same instant: `(process_time_ms, unix_time_ms)`.
pub fn now_pair() -> (f64, f64) {
    (process_time_ms(), unix_time_ms())
}

/// Convert a `CGEventTimestamp` (nanoseconds of uptime, the `mach_absolute_time` clock
/// converted to ns) into process milliseconds on the same origin as [`process_time_ms`].
#[cfg(target_os = "macos")]
pub fn process_time_ms_from_event_timestamp(ts_ns: u64) -> f64 {
    use std::sync::OnceLock as Once;
    static TIMEBASE: Once<(u32, u32)> = Once::new();
    let (numer, denom) = *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        unsafe { mach_timebase_info(&mut info) };
        (info.numer.max(1), info.denom.max(1))
    });
    let c = clock();
    let start_ns = c.start_mach as f64 * numer as f64 / denom as f64;
    (ts_ns as f64 - start_ns) / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_agree() {
        init();
        let (p, u) = now_pair();
        assert!((0.0..60_000.0).contains(&p));
        assert!(u > 1.6e12);
    }

    /// `CGEventTimestamp` is uptime in nanoseconds (`CLOCK_UPTIME_RAW`); converting one taken
    /// "now" must land on the current process time.
    #[cfg(target_os = "macos")]
    #[test]
    fn event_timestamp_conversion_matches_process_time() {
        init();
        extern "C" {
            fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
        }
        const CLOCK_UPTIME_RAW: libc::clockid_t = 8;
        let now_ns = unsafe { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) };
        let a = process_time_ms_from_event_timestamp(now_ns);
        let b = process_time_ms();
        assert!((a - b).abs() < 50.0, "converted {a} ms vs process {b} ms");
    }
}
