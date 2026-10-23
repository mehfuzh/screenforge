//! What happened in the recording, from its mouse and keyboard log: typing sections, clicks and
//! idle stretches, so the agent can act on "the typing part" or "where nothing happens". Times
//! are source ms here; the `get_activity` tool reports them on the timeline.

/// Key presses closer together than this belong to one typing section.
const TYPING_GAP_MS: f64 = 1500.0;
/// A typing section needs at least this many keys…
const TYPING_MIN_KEYS: usize = 3;
/// …and stretches this far before its first key and after its last one.
const TYPING_LEAD_MS: f64 = 200.0;
const TYPING_TAIL_MS: f64 = 400.0;
/// No key, click or mouse movement for this long is idle.
const IDLE_MIN_MS: f64 = 2500.0;
/// Idle stretches keep this much of the quiet around the events on each side.
const IDLE_MARGIN_MS: f64 = 500.0;

/// Typing, clicks and idle stretches of a recording, in source ms.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Activity {
    /// `(start, end, keys pressed)`.
    pub typing: Vec<(f64, f64, usize)>,
    pub clicks: Vec<f64>,
    pub idle: Vec<(f64, f64)>,
}

impl Activity {
    /// From the log's event times: key downs, mouse-button downs and pointer movements, over a
    /// recording `duration_ms` long.
    pub fn from_events(key_downs: &[f64], clicks: &[f64], moves: &[f64], duration_ms: f64) -> Self {
        let mut keys = key_downs.to_vec();
        keys.sort_by(f64::total_cmp);
        let mut typing = vec![];
        let mut i = 0;
        while i < keys.len() {
            let mut j = i;
            while j + 1 < keys.len() && keys[j + 1] - keys[j] <= TYPING_GAP_MS {
                j += 1;
            }
            let n = j - i + 1;
            if n >= TYPING_MIN_KEYS {
                typing.push(((keys[i] - TYPING_LEAD_MS).max(0.0), (keys[j] + TYPING_TAIL_MS).min(duration_ms), n));
            }
            i = j + 1;
        }

        let mut events: Vec<f64> = keys.iter().chain(clicks).chain(moves).copied().filter(|t| t.is_finite()).collect();
        events.sort_by(f64::total_cmp);
        let mut idle = vec![];
        let mut prev = 0.0;
        for t in events.iter().copied().chain(std::iter::once(duration_ms)) {
            if t - prev >= IDLE_MIN_MS {
                let (a, b) = (if prev > 0.0 { prev + IDLE_MARGIN_MS } else { 0.0 }, if t < duration_ms { t - IDLE_MARGIN_MS } else { duration_ms });
                if b - a >= IDLE_MIN_MS - 2.0 * IDLE_MARGIN_MS {
                    idle.push((a, b));
                }
            }
            prev = prev.max(t);
        }

        let mut clicks = clicks.to_vec();
        clicks.sort_by(f64::total_cmp);
        Activity { typing, clicks, idle }
    }

    pub fn is_empty(&self) -> bool {
        self.typing.is_empty() && self.clicks.is_empty() && self.idle.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_sections_group_close_key_presses() {
        // a burst of 5 keys, a lone key, then 3 keys after a pause
        let keys = [1000.0, 1200.0, 1500.0, 2000.0, 2600.0, 6000.0, 9000.0, 9400.0, 9900.0];
        let a = Activity::from_events(&keys, &[], &[], 20_000.0);
        assert_eq!(a.typing, vec![(800.0, 3000.0, 5), (8800.0, 10_300.0, 3)]);
    }

    #[test]
    fn idle_is_time_with_nothing_happening() {
        let moves: Vec<f64> = (0..20).map(|i| i as f64 * 100.0).collect(); // moving until 1.9 s
        let a = Activity::from_events(&[], &[8000.0], &moves, 12_000.0);
        assert_eq!(a.clicks, vec![8000.0]);
        // 1.9 → 8 s quiet, then 8 → 12 s (to the end)
        assert_eq!(a.idle, vec![(2400.0, 7500.0), (8500.0, 12_000.0)]);
        assert!(Activity::from_events(&[], &[], &[], 0.0).is_empty());
    }
}
