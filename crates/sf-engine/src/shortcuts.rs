//! Keyboard shortcut overlay: which key caps to show at a source time.
//!
//! Key-downs from the input log become labels like `⌘ ⇧ S`. A press with ⌘, ⌃ or ⌥ (or ⇧ with
//! a non-character key such as ⇧⇥) is a shortcut and always shown; a key pressed on its own
//! (a letter, ⏎, ⎋, an arrow…) only with `showShortcutsWithSingleLetters`. Each label stays up
//! for [`HOLD_MS`] and is replaced by the next press.

/// How long a label stays visible after its key-down.
pub const HOLD_MS: f64 = 1200.0;
/// Fade in / fade out at the ends of that window.
const FADE_IN_MS: f64 = 80.0;
const FADE_OUT_MS: f64 = 250.0;

/// One key-down from the input log (source time).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyPress {
    pub time_ms: f64,
    /// `shift` | `control` | `option` | `command`, as recorded.
    pub modifiers: Vec<String>,
    pub character: Option<String>,
    /// macOS virtual key code, when the recorder wrote one.
    pub key_code: Option<u32>,
    pub is_repeat: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shortcut {
    pub time_ms: f64,
    /// Key caps left to right: modifiers (⌃ ⌥ ⇧ ⌘, Apple's order), then the key.
    pub keys: Vec<String>,
}

impl Shortcut {
    pub fn label(&self) -> String {
        self.keys.join(" ")
    }
}

/// The shortcuts worth showing, in time order. Auto-repeats extend nothing and are skipped.
pub fn build(presses: &[KeyPress], single_keys: bool) -> Vec<Shortcut> {
    let mut out: Vec<Shortcut> = presses
        .iter()
        .filter(|p| !p.is_repeat)
        .filter_map(|p| {
            let has = |m: &str| p.modifiers.iter().any(|x| x == m);
            let (cmd, ctrl, opt, shift) = (has("command"), has("control"), has("option"), has("shift"));
            let (key, special) = key_name(p, ctrl || opt)?;
            let is_shortcut = cmd || ctrl || opt || (shift && special);
            if !is_shortcut && !single_keys {
                return None;
            }
            let mut keys = vec![];
            if is_shortcut {
                for (on, sym) in [(ctrl, "⌃"), (opt, "⌥"), (shift, "⇧"), (cmd, "⌘")] {
                    if on {
                        keys.push(sym.to_string());
                    }
                }
            }
            keys.push(key);
            Some(Shortcut { time_ms: p.time_ms, keys })
        })
        .collect();
    out.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    out
}

/// The label to show at `t` and its opacity.
pub fn frame_at(shortcuts: &[Shortcut], t: f64) -> Option<(&Shortcut, f32)> {
    let i = shortcuts.partition_point(|s| s.time_ms <= t);
    let s = shortcuts.get(i.checked_sub(1)?)?;
    let age = t - s.time_ms;
    if age >= HOLD_MS {
        return None;
    }
    // a following press replaces the label at once, so only the first one of a burst fades in
    let fade_in = if i >= 2 && s.time_ms - shortcuts[i - 2].time_ms < HOLD_MS { 1.0 } else { (age / FADE_IN_MS).min(1.0) };
    let fade_out = ((HOLD_MS - age) / FADE_OUT_MS).min(1.0);
    Some((s, (fade_in * fade_out).clamp(0.0, 1.0) as f32))
}

/// Cap text for the pressed key and whether it is a non-character key (⏎, ⇥, arrows, F-keys…).
/// With ⌃ or ⌥ held the recorded character is a control code or a composed letter (⌥A = å),
/// so the key code is preferred then.
fn key_name(p: &KeyPress, mangled: bool) -> Option<(String, bool)> {
    let from_code = p.key_code.and_then(mac_key_code);
    if let Some((name, true)) = &from_code {
        return Some((name.clone(), true));
    }
    if mangled {
        if let Some(k) = from_code {
            return Some(k);
        }
    }
    let ch = p.character.as_deref().and_then(|s| s.chars().next());
    // ⌃A … ⌃Z arrive as 0x01 … 0x1a when no key code was recorded (⌃H, ⌃I, ⌃M read as ⌫ ⇥ ⏎)
    if let Some(c @ '\u{1}'..='\u{1a}') = ch.filter(|c| mangled && !matches!(c, '\u{8}' | '\t' | '\r')) {
        return Some((((c as u8 - 1 + b'A') as char).to_string(), false));
    }
    if let Some(k) = ch.and_then(special_char) {
        return Some((k.to_string(), true));
    }
    match ch {
        Some(c) if !c.is_control() => Some((c.to_uppercase().collect(), false)),
        _ => from_code,
    }
}

fn special_char(c: char) -> Option<&'static str> {
    Some(match c {
        ' ' => "Space",
        '\r' | '\n' | '\u{3}' => "⏎",
        '\t' | '\u{19}' => "⇥",
        '\u{8}' | '\u{7f}' => "⌫",
        '\u{1b}' => "⎋",
        // AppKit's function-key range (NSUpArrowFunctionKey …)
        '\u{f700}' => "↑",
        '\u{f701}' => "↓",
        '\u{f702}' => "←",
        '\u{f703}' => "→",
        '\u{f704}'..='\u{f70f}' => return None, // F-keys: named from the key code below
        '\u{f728}' => "⌦",
        '\u{f729}' => "↖",
        '\u{f72b}' => "↘",
        '\u{f72c}' => "⇞",
        '\u{f72d}' => "⇟",
        _ => return None,
    })
}

/// macOS virtual key codes (ANSI layout for the letters; non-character keys flagged `true`).
fn mac_key_code(code: u32) -> Option<(String, bool)> {
    let special = match code {
        36 | 76 => Some("⏎"),
        48 => Some("⇥"),
        49 => Some("Space"),
        51 => Some("⌫"),
        53 => Some("⎋"),
        117 => Some("⌦"),
        115 => Some("↖"),
        119 => Some("↘"),
        116 => Some("⇞"),
        121 => Some("⇟"),
        123 => Some("←"),
        124 => Some("→"),
        125 => Some("↓"),
        126 => Some("↑"),
        _ => None,
    };
    if let Some(s) = special {
        return Some((s.to_string(), true));
    }
    let f = [(122, 1), (120, 2), (99, 3), (118, 4), (96, 5), (97, 6), (98, 7), (100, 8), (101, 9), (109, 10), (103, 11), (111, 12)];
    if let Some((_, n)) = f.iter().find(|(c, _)| *c == code) {
        return Some((format!("F{n}"), true));
    }
    const ANSI: [&str; 51] = [
        "A", "S", "D", "F", "H", "G", "Z", "X", "C", "V", "", "B", "Q", "W", "E", "R", "Y", "T", "1", "2", "3", "4", "6", "5", "=", "9", "7", "-", "8", "0", "]", "O", "U", "[", "I", "P", "", "L", "J", "'", "K", ";", "\\", ",", "/", "N", "M", ".", "", "", "`",
    ];
    ANSI.get(code as usize).filter(|s| !s.is_empty()).map(|s| (s.to_string(), false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(t: f64, mods: &[&str], ch: &str, code: Option<u32>) -> KeyPress {
        KeyPress { time_ms: t, modifiers: mods.iter().map(|m| m.to_string()).collect(), character: Some(ch.into()), key_code: code, is_repeat: false }
    }

    fn labels(v: &[Shortcut]) -> Vec<String> {
        v.iter().map(|s| s.label()).collect()
    }

    #[test]
    fn modifier_combos_in_apple_order() {
        let p = [press(0.0, &["command", "shift"], "s", None), press(1.0, &["option", "control"], "\u{1}", Some(0))];
        assert_eq!(labels(&build(&p, false)), ["⇧ ⌘ S", "⌃ ⌥ A"]);
    }

    #[test]
    fn plain_typing_needs_single_keys() {
        let p = [press(0.0, &[], "a", None), press(1.0, &["shift"], "?", None), press(2.0, &[], "\r", None), press(3.0, &[], "\u{f702}", None)];
        assert!(build(&p, false).is_empty());
        assert_eq!(labels(&build(&p, true)), ["A", "?", "⏎", "←"]);
    }

    #[test]
    fn shift_with_special_key_is_a_shortcut() {
        let p = [press(0.0, &["shift"], "\t", None)];
        assert_eq!(labels(&build(&p, false)), ["⇧ ⇥"]);
    }

    #[test]
    fn key_code_beats_mangled_characters() {
        // ⌥E types a dead key: the key code names it
        let p = [press(0.0, &["option"], "´", Some(14)), press(1.0, &["command"], "", Some(122))];
        assert_eq!(labels(&build(&p, false)), ["⌥ E", "⌘ F1"]);
        // ⌃ without a key code falls back to the control character
        assert_eq!(labels(&build(&[press(0.0, &["control"], "\u{3}", None), press(1.0, &["control"], "\t", None)], false)), ["⌃ C", "⌃ ⇥"]);
    }

    #[test]
    fn repeats_are_skipped() {
        let mut r = press(100.0, &["command"], "z", None);
        r.is_repeat = true;
        assert_eq!(build(&[press(0.0, &["command"], "z", None), r], false).len(), 1);
    }

    #[test]
    fn label_holds_then_fades() {
        let s = build(&[press(1000.0, &["command"], "c", None), press(1500.0, &["command"], "v", None)], false);
        assert!(frame_at(&s, 999.0).is_none());
        let (a, o) = frame_at(&s, 1040.0).unwrap();
        assert_eq!(a.label(), "⌘ C");
        assert!(o > 0.4 && o < 0.6, "fading in: {o}");
        // the next press replaces it without a fade-in
        let (b, o) = frame_at(&s, 1501.0).unwrap();
        assert_eq!((b.label().as_str(), o), ("⌘ V", 1.0));
        assert!(frame_at(&s, 1500.0 + HOLD_MS - 50.0).unwrap().1 < 0.25);
        assert!(frame_at(&s, 1500.0 + HOLD_MS).is_none());
    }
}
