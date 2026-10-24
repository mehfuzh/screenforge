//! English text to Kokoro phonemes, from misaki's pronunciation dictionaries (the ones Kokoro
//! was trained with): `us_gold.json` first, then `us_silver.json`. Words missing from both are
//! built from what is there: plural, past and `-ing` endings on a known stem, compounds of
//! two known words (`ScreenForge` = screen + forge), and as a last resort letter by letter.

use std::collections::HashMap;
use std::ops::Range;

/// Phonemes that start with a vowel sound (for `the` and `to`).
const VOWELS: &str = "AIOQWYaiuæɑɒɔəɛɜɪʊʌᵻ";
/// Punctuation Kokoro reads as pauses; it is kept in the phonemes.
const PAUSES: &str = ".,!?;:—…";

pub struct Dict {
    words: HashMap<String, String>,
}

/// A sentence as phonemes, with where each word's phonemes are (for caption timing).
#[derive(Debug, Clone, PartialEq)]
pub struct Phrase {
    pub phonemes: String,
    /// Caption text of each word (punctuation after it included) and its char range in
    /// `phonemes`.
    pub words: Vec<(String, Range<usize>)>,
}

impl Phrase {
    pub fn len(&self) -> usize {
        self.phonemes.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.phonemes.is_empty()
    }
}

impl Dict {
    /// Dictionaries as misaki ships them: `word: phonemes` or `word: {DEFAULT: phonemes, …}`
    /// (by part of speech; the default reading is used).
    pub fn from_json(gold: &str, silver: &str) -> anyhow::Result<Dict> {
        let mut words = HashMap::new();
        // silver first, so gold wins
        for src in [silver, gold] {
            let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(src)?;
            for (k, v) in map {
                let p = match &v {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Object(o) => o.get("DEFAULT").and_then(|d| d.as_str()).map(str::to_string),
                    _ => None,
                };
                if let Some(p) = p.filter(|p| !p.is_empty()) {
                    words.insert(k, p);
                }
            }
        }
        Ok(Dict { words })
    }

    fn get(&self, w: &str) -> Option<&str> {
        self.words.get(w).map(String::as_str)
    }

    /// The word as written, then lower case, then capitalised (names are stored capitalised).
    fn lookup(&self, w: &str) -> Option<String> {
        let lower = w.to_lowercase();
        let mut cap = lower.clone();
        if let Some(f) = cap.get_mut(0..1) {
            f.make_ascii_uppercase();
        }
        self.get(w).or_else(|| self.get(&lower)).or_else(|| self.get(&cap)).map(str::to_string)
    }

    /// Phonemes of one word (letters and apostrophes only).
    pub fn word(&self, w: &str) -> Option<String> {
        self.word_depth(w, 0)
    }

    fn word_depth(&self, w: &str, depth: u8) -> Option<String> {
        if w.is_empty() {
            return None;
        }
        if let Some(p) = self.lookup(w) {
            return Some(p);
        }
        // a short all-caps word nobody wrote down is an acronym: spelled out
        if w.len() <= 5 && w.len() > 1 && w.chars().all(|c| c.is_ascii_uppercase()) {
            return self.spell(w);
        }
        if let Some(p) = self.suffixed(w) {
            return Some(p);
        }
        if depth < 2 {
            // camelCase and PascalCase: each part
            let parts = camel_parts(w);
            if parts.len() > 1 {
                if let Some(ps) = parts.iter().map(|p| self.word_depth(p, depth + 1)).collect::<Option<Vec<_>>>() {
                    return Some(ps.concat());
                }
            }
            // a compound of two known words, the longest first part first
            let lower = w.to_lowercase();
            let chars: Vec<(usize, char)> = lower.char_indices().collect();
            for k in (3..chars.len().saturating_sub(2)).rev() {
                let at = chars[k].0;
                if let (Some(a), Some(b)) = (self.lookup(&lower[..at]).or_else(|| self.suffixed(&lower[..at])), self.word_depth(&lower[at..], depth + 1)) {
                    return Some(a + &b);
                }
            }
        }
        None
    }

    /// A known stem with a plural/possessive, past or `-ing` ending.
    fn suffixed(&self, w: &str) -> Option<String> {
        let lower = w.to_lowercase().replace('’', "'");
        let stem = |s: &str| self.lookup(s);
        if let Some(base) = lower.strip_suffix("'s") {
            return stem(base).map(|p| add_s(&p));
        }
        if let Some(base) = lower.strip_suffix("ies").filter(|b| b.len() >= 2) {
            if let Some(p) = stem(&format!("{base}y")) {
                return Some(add_s(&p));
            }
        }
        if let Some(base) = lower.strip_suffix('s').filter(|b| b.len() >= 2 && !b.ends_with('s')) {
            if let Some(p) = stem(base).or_else(|| base.strip_suffix('e').and_then(stem)) {
                return Some(add_s(&p));
            }
        }
        if let Some(base) = lower.strip_suffix("ed").filter(|b| b.len() >= 2) {
            let undoubled = undouble(base);
            if let Some(p) = stem(base).or_else(|| stem(&format!("{base}e"))).or_else(|| undoubled.as_deref().and_then(stem)) {
                return Some(add_ed(&p));
            }
            if let Some(p) = base.strip_suffix('i').and_then(|b| stem(&format!("{b}y"))) {
                return Some(add_ed(&p));
            }
        }
        if let Some(base) = lower.strip_suffix("ing").filter(|b| b.len() >= 2) {
            let undoubled = undouble(base);
            if let Some(p) = stem(base).or_else(|| stem(&format!("{base}e"))).or_else(|| undoubled.as_deref().and_then(stem)) {
                return Some(p + "ɪŋ");
            }
        }
        if let Some(base) = lower.strip_suffix("ly").filter(|b| b.len() >= 3) {
            if let Some(p) = stem(base) {
                return Some(p + "li");
            }
        }
        None
    }

    /// Letter by letter, by the letters' names.
    fn spell(&self, w: &str) -> Option<String> {
        w.chars().filter(|c| c.is_alphanumeric()).map(|c| self.get(&c.to_uppercase().to_string()).map(str::to_string)).collect::<Option<Vec<_>>>().map(|v| v.concat())
    }

    /// A word nothing else could read: its letters spelled, or nothing for symbols.
    fn fallback(&self, w: &str) -> Option<String> {
        self.spell(w)
    }

    /// One sentence (or clause) as phonemes.
    pub fn phrase(&self, text: &str) -> Phrase {
        // (caption text, phonemes or None for punctuation, the punctuation itself)
        let mut items: Vec<Item> = vec![];
        for tok in tokens(text) {
            match tok {
                Token::Pause(c) => {
                    // after the word's caption (its first piece when it is read as several)
                    let last = items.iter_mut().rev().take_while(|i| matches!(i, Item::Word { .. })).find(|i| matches!(i, Item::Word { caption, .. } if !caption.is_empty()));
                    if let Some(Item::Word { caption, .. }) = last {
                        caption.push(c);
                    }
                    items.push(Item::Pause(c));
                }
                Token::Word(w) => {
                    let first = items.len();
                    for spoken in expand(&w) {
                        let p = spoken.split('-').filter(|s| !s.is_empty()).filter_map(|s| self.word(s).or_else(|| self.fallback(s))).collect::<Vec<_>>().join(" ");
                        if !p.is_empty() {
                            items.push(Item::Word { caption: String::new(), phonemes: p, spoken });
                        }
                    }
                    // the caption shows the word as written, on the first spoken piece
                    if let Some(Item::Word { caption, .. }) = items.get_mut(first) {
                        *caption = w.clone();
                    }
                }
            }
        }
        // function words whose reading depends on the next sound
        for i in 0..items.len() {
            let next_vowel = items[i + 1..].iter().find_map(|it| match it {
                Item::Word { phonemes, .. } => Some(phonemes.chars().find(|c| !"ˈˌ".contains(*c)).is_some_and(|c| VOWELS.contains(c))),
                Item::Pause(_) => None,
            });
            if let Item::Word { spoken, phonemes, .. } = &mut items[i] {
                match spoken.to_lowercase().as_str() {
                    "a" => *phonemes = "ɐ".into(),
                    "the" => *phonemes = if next_vowel == Some(true) { "ði" } else { "ðə" }.into(),
                    "to" => *phonemes = if next_vowel == Some(true) { "tʊ" } else { "tə" }.into(),
                    _ => {}
                }
            }
        }
        let mut phonemes = String::new();
        let mut words = vec![];
        let mut len = 0usize;
        for it in items {
            match it {
                Item::Word { caption, phonemes: p, .. } => {
                    if len > 0 {
                        phonemes.push(' ');
                        len += 1;
                    }
                    let n = p.chars().count();
                    phonemes.push_str(&p);
                    // a word split in pieces (a number read out) shows once, over all of them
                    if caption.is_empty() {
                        if let Some((_, r)) = words.last_mut() {
                            let r: &mut Range<usize> = r;
                            r.end = len + n;
                        }
                    } else {
                        words.push((caption, len..len + n));
                    }
                    len += n;
                }
                Item::Pause(c) => {
                    phonemes.push(c);
                    len += 1;
                }
            }
        }
        Phrase { phonemes, words }
    }
}

enum Item {
    Word { caption: String, phonemes: String, spoken: String },
    Pause(char),
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Pause(char),
}

fn tokens(text: &str) -> Vec<Token> {
    let mut out = vec![];
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        let in_word = c.is_alphanumeric()
            || ((c == '\'' || c == '’' || c == '-') && !cur.is_empty() && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric()))
            // 1.5, 1,000 and 0:30 stay one number
            || ((c == '.' || c == ',' || c == ':') && cur.chars().last().is_some_and(|l| l.is_ascii_digit()) && next_digit)
            || (c == '%' || c == '×') && cur.chars().last().is_some_and(|l| l.is_ascii_digit());
        if in_word {
            cur.push(c);
            continue;
        }
        if !cur.is_empty() {
            out.push(Token::Word(std::mem::take(&mut cur)));
        }
        let pause = match c {
            '"' | '“' | '”' | '(' | ')' => None,
            '-' => Some('—'),
            c if PAUSES.contains(c) => Some(c),
            _ => None,
        };
        if let Some(p) = pause {
            // "..." is one pause
            if !(p == '.' && matches!(out.last(), Some(Token::Pause('.') | Token::Pause('…')))) {
                out.push(Token::Pause(p));
            }
        }
    }
    if !cur.is_empty() {
        out.push(Token::Word(cur));
    }
    out
}

/// A token as the words to say: numbers read out, `2×` as "two times", `50%` as "fifty
/// percent"; anything else as it is.
fn expand(w: &str) -> Vec<String> {
    let lower = w.to_lowercase();
    if !w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return vec![w.to_string()];
    }
    let (num, rest) = lower.split_at(lower.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',' || c == ':')).unwrap_or(lower.len()));
    let mut out: Vec<String> = vec![];
    if let Some((m, s)) = num.split_once(':') {
        // a time like 1:30
        out.extend(number_words(m));
        if s.trim_start_matches('0').is_empty() {
            out.push("o'clock".into());
        } else {
            if s.starts_with('0') {
                out.push("oh".into());
            }
            out.extend(number_words(s));
        }
    } else {
        out.extend(number_words(&num.replace(',', "")));
    }
    match rest {
        "" => {}
        "x" | "×" => out.push("times".into()),
        "%" => out.push("percent".into()),
        "st" | "nd" | "rd" | "th" => {
            if let Some(last) = out.pop() {
                out.push(ordinal(&last));
            }
        }
        "s" => {
            // 1990s
            if let Some(last) = out.pop() {
                out.push(if let Some(b) = last.strip_suffix('y') { format!("{b}ies") } else { format!("{last}s") });
            }
        }
        r => out.push(r.to_string()),
    }
    out
}

const ONES: [&str; 20] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen"];
const TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

/// "1.5" as one point five, "2026" as twenty twenty-six, otherwise in thousands.
fn number_words(s: &str) -> Vec<String> {
    if let Some((int, frac)) = s.split_once('.') {
        let mut out = number_words(int);
        out.push("point".into());
        out.extend(frac.chars().filter_map(|d| d.to_digit(10)).map(|d| ONES[d as usize].to_string()));
        return out;
    }
    let Ok(n) = s.parse::<u64>() else { return s.chars().filter_map(|d| d.to_digit(10)).map(|d| ONES[d as usize].to_string()).collect() };
    // years read in pairs: 1999, 2026 (but 2000–2009 as two thousand …)
    if (1100..2100).contains(&n) && !(2000..2010).contains(&n) && n % 1000 >= 10 && s.len() == 4 {
        let (hi, lo) = (n / 100, n % 100);
        let mut out = below_hundred(hi);
        if lo == 0 {
            out.push("hundred".into());
        } else {
            if lo < 10 {
                out.push("oh".into());
            }
            out.extend(below_hundred(lo));
        }
        return out;
    }
    words_for(n)
}

fn words_for(n: u64) -> Vec<String> {
    if n < 100 {
        return below_hundred(n);
    }
    for (unit, name) in [(1_000_000_000_000, "trillion"), (1_000_000_000, "billion"), (1_000_000, "million"), (1_000, "thousand"), (100, "hundred")] {
        if n >= unit {
            let mut out = words_for(n / unit);
            out.push(name.into());
            if n % unit > 0 {
                out.extend(words_for(n % unit));
            }
            return out;
        }
    }
    unreachable!()
}

fn below_hundred(n: u64) -> Vec<String> {
    match n {
        0..=19 => vec![ONES[n as usize].into()],
        _ if n % 10 == 0 => vec![TENS[(n / 10) as usize].into()],
        _ => vec![format!("{}-{}", TENS[(n / 10) as usize], ONES[(n % 10) as usize])],
    }
}

fn ordinal(w: &str) -> String {
    let (head, last) = w.rsplit_once('-').map_or(("", w), |(h, l)| (h, l));
    let o = match last {
        "one" => "first".into(),
        "two" => "second".into(),
        "three" => "third".into(),
        "five" => "fifth".into(),
        "eight" => "eighth".into(),
        "nine" => "ninth".into(),
        "twelve" => "twelfth".into(),
        l if l.ends_with('y') => format!("{}ieth", &l[..l.len() - 1]),
        l => format!("{l}th"),
    };
    if head.is_empty() { o } else { format!("{head}-{o}") }
}

/// `stopped` → `stop`: the doubled consonant before an ending.
fn undouble(base: &str) -> Option<String> {
    let b = base.as_bytes();
    (b.len() >= 3 && b[b.len() - 1] == b[b.len() - 2]).then(|| base[..base.len() - 1].to_string())
}

/// `cats` /s/, `dogs` /z/, `boxes` /ᵻz/.
fn add_s(p: &str) -> String {
    match p.chars().last() {
        Some(c) if "szʃʒʧʤ".contains(c) => format!("{p}ᵻz"),
        Some(c) if "ptkfθ".contains(c) => format!("{p}s"),
        _ => format!("{p}z"),
    }
}

/// `stopped` /t/, `played` /d/, `edited` /ᵻd/.
fn add_ed(p: &str) -> String {
    match p.chars().last() {
        Some(c) if "td".contains(c) => format!("{p}ᵻd"),
        Some(c) if "pkfθʃsʧ".contains(c) => format!("{p}t"),
        _ => format!("{p}d"),
    }
}

/// `ScreenForge` → screen, forge; `macOS` → mac, OS; `iPhone` → i, phone.
fn camel_parts(w: &str) -> Vec<String> {
    let chars: Vec<char> = w.chars().collect();
    let mut parts: Vec<String> = vec![];
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let boundary = c.is_uppercase() && (prev.is_some_and(|p| p.is_lowercase()) || (prev.is_some_and(|p| p.is_uppercase()) && next.is_some_and(|n| n.is_lowercase())));
        if boundary && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Dict {
        let gold = r#"{"a": "A", "A": "ˈA", "the": "ði", "to": "tu", "screen": "skɹˈin", "forge": "fˈɔɹʤ", "video": "vˈɪdiO", "edit": "ˈɛdɪt",
            "stop": "stˈɑp", "box": "bˈɑks", "play": "plˈA", "is": "ɪz", "AI": "ˈAˌI", "U": "jˈu", "I": "ˈI", "one": "wˈʌn", "five": "fˈIv", "point": "pˈYnt",
            "times": "tˈImz", "two": "tˈu", "S": "ˈɛs", "B": "bˈi", "C": "sˈi", "end": "ˈɛnd", "minute": {"DEFAULT": "mˈɪnət", "ADJ": "mInˈut"}, "twenty": "twˈɛnti",
            "six": "sˈɪks", "twenty-six": "twˈɛnti sˈɪks", "apple": "ˈæpᵊl"}"#;
        Dict::from_json(gold, r#"{"prompt": "pɹˈɑmpt", "video": "WRONG"}"#).unwrap()
    }

    #[test]
    fn gold_wins_and_part_of_speech_entries_use_the_default() {
        let d = dict();
        assert_eq!(d.word("video").unwrap(), "vˈɪdiO");
        assert_eq!(d.word("minute").unwrap(), "mˈɪnət");
        assert_eq!(d.word("Prompt").unwrap(), "pɹˈɑmpt");
    }

    #[test]
    fn endings_on_known_stems() {
        let d = dict();
        assert_eq!(d.word("videos").unwrap(), "vˈɪdiOz");
        assert_eq!(d.word("prompts").unwrap(), "pɹˈɑmpts");
        assert_eq!(d.word("boxes").unwrap(), "bˈɑksᵻz");
        assert_eq!(d.word("edited").unwrap(), "ˈɛdɪtᵻd");
        assert_eq!(d.word("stopped").unwrap(), "stˈɑpt");
        assert_eq!(d.word("played").unwrap(), "plˈAd");
        assert_eq!(d.word("editing").unwrap(), "ˈɛdɪtɪŋ");
    }

    #[test]
    fn names_and_acronyms() {
        let d = dict();
        assert_eq!(d.word("ScreenForge").unwrap(), "skɹˈinfˈɔɹʤ");
        assert_eq!(d.word("screenforge").unwrap(), "skɹˈinfˈɔɹʤ");
        assert_eq!(d.word("SBC").unwrap(), "ˈɛsbˈisˈi");
    }

    #[test]
    fn function_words_follow_the_next_sound() {
        let d = dict();
        assert_eq!(d.phrase("the apple").phonemes, "ði ˈæpᵊl");
        assert_eq!(d.phrase("the video").phonemes, "ðə vˈɪdiO");
        assert_eq!(d.phrase("to edit a video").phonemes, "tʊ ˈɛdɪt ɐ vˈɪdiO");
    }

    #[test]
    fn numbers_are_read_out_and_captioned_as_written() {
        let d = dict();
        let p = d.phrase("1.5× the video, 2026.");
        assert_eq!(p.phonemes, "wˈʌn pˈYnt fˈIv tˈImz ðə vˈɪdiO, twˈɛnti twˈɛnti sˈɪks.");
        let captions: Vec<&str> = p.words.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(captions, ["1.5×", "the", "video,", "2026."]);
        // each caption covers its phonemes
        let chars: Vec<char> = p.phonemes.chars().collect();
        let first: String = chars[p.words[0].1.clone()].iter().collect();
        assert_eq!(first, "wˈʌn pˈYnt fˈIv tˈImz");
    }

    #[test]
    fn number_words() {
        assert_eq!(super::number_words("1999").join(" "), "nineteen ninety-nine");
        assert_eq!(super::number_words("2005").join(" "), "two thousand five");
        assert_eq!(super::number_words("1200").join(" "), "twelve hundred");
        assert_eq!(super::number_words("345").join(" "), "three hundred forty-five");
        assert_eq!(super::number_words("1000000").join(" "), "one million");
        assert_eq!(expand("3rd").join(" "), "third");
        assert_eq!(expand("21st").join(" "), "twenty-first");
        assert_eq!(expand("50%").join(" "), "fifty percent");
        assert_eq!(expand("0:30").join(" "), "zero thirty");
    }

    #[test]
    fn tokens_keep_numbers_and_contractions_whole() {
        assert_eq!(tokens("It's 1,000 videos... wow"), vec![Token::Word("It's".into()), Token::Word("1,000".into()), Token::Word("videos".into()), Token::Pause('.'), Token::Word("wow".into())]);
        assert_eq!(tokens("AI-powered, done!"), vec![Token::Word("AI-powered".into()), Token::Pause(','), Token::Word("done".into()), Token::Pause('!')]);
    }
}
