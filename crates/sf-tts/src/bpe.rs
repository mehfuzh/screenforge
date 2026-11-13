//! The byte-level BPE tokenizer Chatterbox Turbo reads text with (GPT-2's, from the model's
//! `tokenizer.json`), giving the same ids as Hugging Face's `tokenizers`: tags such as
//! `[chuckle]` are tokens of their own, text is split the way GPT-2's pattern splits it, each
//! piece's UTF-8 bytes are mapped to GPT-2's printable characters and merged by rank, and two
//! `<|endoftext|>` close the text.

use std::collections::HashMap;

use anyhow::{Context, Result};

pub struct Bpe {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    /// Tokens matched as they are (`<|endoftext|>`, `[chuckle]`, …), longest first.
    added: Vec<(String, i64)>,
    end: i64,
    byte_char: [char; 256],
}

impl Bpe {
    pub fn from_json(json: &str) -> Result<Bpe> {
        let v: serde_json::Value = serde_json::from_str(json).context("reading tokenizer.json")?;
        let model = &v["model"];
        let vocab: HashMap<String, i64> = model["vocab"].as_object().context("tokenizer.json has no vocab")?.iter().filter_map(|(k, id)| Some((k.clone(), id.as_i64()?))).collect();
        let mut ranks = HashMap::new();
        for (i, m) in model["merges"].as_array().context("tokenizer.json has no merges")?.iter().enumerate() {
            // ["a", "b"] in newer files, "a b" in older ones
            let pair = match m {
                serde_json::Value::Array(p) => (p[0].as_str().unwrap_or_default().to_string(), p[1].as_str().unwrap_or_default().to_string()),
                serde_json::Value::String(s) => match s.split_once(' ') {
                    Some((a, b)) => (a.to_string(), b.to_string()),
                    None => continue,
                },
                _ => continue,
            };
            ranks.insert(pair, i);
        }
        let mut added: Vec<(String, i64)> = v["added_tokens"].as_array().into_iter().flatten().filter_map(|t| Some((t["content"].as_str()?.to_string(), t["id"].as_i64()?))).collect();
        added.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        let end = added.iter().find(|(s, _)| s == "<|endoftext|>").map(|t| t.1).or_else(|| vocab.get("<|endoftext|>").copied()).context("no <|endoftext|> token")?;
        Ok(Bpe { vocab, ranks, added, end, byte_char: byte_chars() })
    }

    /// The ids of `text`, closed by two `<|endoftext|>` as the model was trained.
    pub fn encode(&self, text: &str) -> Vec<i64> {
        let mut ids = vec![];
        let mut rest = text;
        while !rest.is_empty() {
            // the nearest added token, if any
            let next = self.added.iter().filter_map(|(s, id)| rest.find(s.as_str()).map(|at| (at, s.len(), *id))).min_by_key(|(at, len, _)| (*at, std::cmp::Reverse(*len)));
            let (plain, after) = match next {
                Some((at, len, id)) => {
                    let plain = &rest[..at];
                    self.encode_plain(plain, &mut ids);
                    ids.push(id);
                    (None, &rest[at + len..])
                }
                None => (Some(rest), ""),
            };
            if let Some(p) = plain {
                self.encode_plain(p, &mut ids);
            }
            rest = after;
        }
        ids.extend([self.end, self.end]);
        ids
    }

    fn encode_plain(&self, text: &str, ids: &mut Vec<i64>) {
        for piece in pieces(text) {
            let mapped: Vec<String> = piece.bytes().map(|b| self.byte_char[b as usize].to_string()).collect();
            for part in self.merge(mapped) {
                if let Some(id) = self.vocab.get(&part) {
                    ids.push(*id);
                }
            }
        }
    }

    /// Merge `parts` by rank until no pair has one.
    fn merge(&self, mut parts: Vec<String>) -> Vec<String> {
        while parts.len() > 1 {
            let best = (0..parts.len() - 1).filter_map(|i| self.ranks.get(&(parts[i].clone(), parts[i + 1].clone())).map(|r| (*r, i))).min();
            let Some((_, i)) = best else { break };
            let joined = format!("{}{}", parts[i], parts[i + 1]);
            parts.splice(i..i + 2, [joined]);
        }
        parts
    }
}

/// GPT-2's map from bytes to printable characters.
fn byte_chars() -> [char; 256] {
    let mut out = ['\0'; 256];
    let mut extra = 0u32;
    for b in 0..256u32 {
        let printable = (33..=126).contains(&b) || (161..=172).contains(&b) || (174..=255).contains(&b);
        out[b as usize] = if printable {
            char::from_u32(b).unwrap()
        } else {
            extra += 1;
            char::from_u32(255 + extra).unwrap()
        };
    }
    out
}

#[derive(PartialEq, Clone, Copy)]
enum Class {
    Letter,
    Number,
    Other,
    Space,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphabetic() {
        Class::Letter
    } else if c.is_numeric() {
        Class::Number
    } else {
        Class::Other
    }
}

/// `text` split as GPT-2's pattern does:
/// `'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`
fn pieces(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte_at = |i: usize| chars.get(i).map_or(text.len(), |c| c.0);
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        let rest = &text[byte_at(i)..];
        if let Some(c) = ["'s", "'t", "'re", "'ve", "'m", "'ll", "'d"].iter().find(|c| rest.starts_with(**c)) {
            let n = c.chars().count();
            out.push(&text[byte_at(i)..byte_at(i + n)]);
            i += n;
            continue;
        }
        let c = chars[i].1;
        // a single space joins the run of letters, numbers or other marks after it
        let (start, run_class) = if c == ' ' && chars.get(i + 1).is_some_and(|n| class(n.1) != Class::Space) {
            (i + 1, class(chars[i + 1].1))
        } else {
            (i, class(c))
        };
        let mut j = start;
        while j < chars.len() && class(chars[j].1) == run_class {
            j += 1;
        }
        if run_class == Class::Space && j < chars.len() && j - i > 1 {
            // whitespace before a word leaves its last space to that word
            j -= 1;
        }
        out.push(&text[byte_at(i)..byte_at(j)]);
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_gpt2() {
        assert_eq!(pieces("We're 25 ok!!  Yes\n\nNo it's"), ["We", "'re", " 25", " ok", "!!", " ", " Yes", "\n", "\n", "No", " it", "'s"]);
    }

    #[test]
    fn bytes_map_to_printable_characters() {
        let m = byte_chars();
        assert_eq!(m[b'a' as usize], 'a');
        assert_eq!(m[b' ' as usize], 'Ġ');
        assert_eq!(m[b'\n' as usize], 'Ċ');
    }

    /// Against Hugging Face's tokenizer, with the model's `tokenizer.json`
    /// (`SF_TURBO_TOKENIZER=… cargo test -p sf-tts -- --ignored`).
    #[test]
    #[ignore]
    fn matches_the_reference_tokenizer() {
        let path = std::env::var("SF_TURBO_TOKENIZER").expect("SF_TURBO_TOKENIZER");
        let bpe = Bpe::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(bpe.encode("We're provisioning our personal GPU box with a single command."), [1135, 821, 8287, 278, 674, 2614, 11362, 3091, 351, 257, 2060, 3141, 13, 50256, 50256]);
        assert_eq!(
            bpe.encode("Numbers like 3, 25 and 1,000 — café  ok?\n\nYes [chuckle] it's 10%!"),
            [49601, 588, 513, 11, 1679, 290, 352, 11, 830, 851, 40304, 220, 12876, 30, 198, 198, 5297, 220, 50274, 340, 338, 838, 4, 0, 50256, 50256]
        );
    }
}
