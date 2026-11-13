//! ElevenLabs voices, read online through the backend's `voice` function (which holds the key
//! and charges credits): the voices the editor offers, and turning a reply (16-bit PCM at
//! 24 kHz and ElevenLabs' character timing) into a [`Speech`] with its words for captions.

use anyhow::{bail, Result};

use crate::{Speech, SpokenWord, PEAK};

/// An ElevenLabs voice. Its id is the app's (`el_brian`); the backend maps it to ElevenLabs'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudVoice {
    pub id: &'static str,
    pub name: &'static str,
    pub female: bool,
    /// How it sounds, for the agent and tooltips.
    pub style: &'static str,
}

/// Keep in step with VOICES in backend/supabase/functions/voice. The first is the one picked
/// when switching to ElevenLabs (and the agent's choice for studio quality).
pub const CLOUD_VOICES: [CloudVoice; 8] = [
    CloudVoice { id: "el_eric", name: "Eric", female: false, style: "smooth, trustworthy" },
    CloudVoice { id: "el_brian", name: "Brian", female: false, style: "deep, resonant narrator" },
    CloudVoice { id: "el_george", name: "George", female: false, style: "warm British storyteller" },
    CloudVoice { id: "el_daniel", name: "Daniel", female: false, style: "steady British broadcaster" },
    CloudVoice { id: "el_sarah", name: "Sarah", female: true, style: "mature, confident" },
    CloudVoice { id: "el_matilda", name: "Matilda", female: true, style: "knowledgeable, professional" },
    CloudVoice { id: "el_alice", name: "Alice", female: true, style: "clear British educator" },
    CloudVoice { id: "el_jessica", name: "Jessica", female: true, style: "playful, bright" },
];

pub fn cloud_voice(id: &str) -> Option<&'static CloudVoice> {
    CLOUD_VOICES.iter().find(|v| v.id == id)
}

/// Credits a reading of `chars` characters costs (ElevenLabs v4 at US$0.08 per 1,000
/// characters, 1 credit = US$0.01); the backend charges what it is set to, this is for the panel.
pub fn credits_for(chars: usize) -> f64 {
    chars as f64 * 0.008
}

/// ElevenLabs' character timing of a reading (the `alignment` of a `with-timestamps` reply).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Alignment {
    pub characters: Vec<String>,
    pub starts: Vec<f64>,
    pub ends: Vec<f64>,
}

impl Alignment {
    pub fn from_json(v: &serde_json::Value) -> Alignment {
        let list = |k: &str| v.get(k).and_then(|a| a.as_array()).cloned().unwrap_or_default();
        Alignment {
            characters: list("characters").iter().map(|c| c.as_str().unwrap_or_default().to_string()).collect(),
            starts: list("character_start_times_seconds").iter().map(|t| t.as_f64().unwrap_or(0.0)).collect(),
            ends: list("character_end_times_seconds").iter().map(|t| t.as_f64().unwrap_or(0.0)).collect(),
        }
    }

    /// The words, each from its first character's start to its last one's end (ms).
    pub fn words(&self) -> Vec<SpokenWord> {
        let mut out: Vec<SpokenWord> = vec![];
        let mut current: Option<SpokenWord> = None;
        let n = self.characters.len().min(self.starts.len()).min(self.ends.len());
        for i in 0..n {
            let c = &self.characters[i];
            if c.trim().is_empty() {
                out.extend(current.take());
                continue;
            }
            let (s, e) = (self.starts[i] * 1000.0, self.ends[i] * 1000.0);
            match current.as_mut() {
                Some(w) => {
                    w.text.push_str(c);
                    w.end_ms = e.max(w.end_ms);
                }
                None => current = Some(SpokenWord { start_ms: s, end_ms: e, text: c.clone() }),
            }
        }
        out.extend(current);
        out
    }
}

/// A reading from 16-bit little-endian mono PCM at [`crate::SAMPLE_RATE`], brought to the same
/// peak level as Kokoro's takes.
pub fn speech_from_pcm(pcm: &[u8], alignment: &Alignment) -> Result<Speech> {
    if pcm.len() < 2 {
        bail!("ElevenLabs returned no audio");
    }
    let mut samples: Vec<f32> = pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let gain = PEAK / peak;
        samples.iter_mut().for_each(|s| *s *= gain);
    }
    Ok(Speech { samples, words: alignment.words() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_make_words_between_spaces() {
        let chars = "Hi, there  you".chars().map(String::from).collect::<Vec<_>>();
        let n = chars.len();
        let a = Alignment { characters: chars, starts: (0..n).map(|i| i as f64 * 0.1).collect(), ends: (0..n).map(|i| i as f64 * 0.1 + 0.1).collect() };
        let w = a.words();
        assert_eq!(w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["Hi,", "there", "you"]);
        assert!(w[0].start_ms == 0.0 && (w[0].end_ms - 300.0).abs() < 1e-6);
        assert!((w[2].start_ms - 1100.0).abs() < 1e-6);
    }

    #[test]
    fn alignment_is_read_from_the_reply() {
        let v = serde_json::json!({ "characters": ["O", "k"], "character_start_times_seconds": [0.0, 0.2], "character_end_times_seconds": [0.2, 0.5] });
        let a = Alignment::from_json(&v);
        assert_eq!(a.words(), vec![SpokenWord { start_ms: 0.0, end_ms: 500.0, text: "Ok".into() }]);
    }

    #[test]
    fn pcm_is_decoded_and_levelled() {
        let pcm: Vec<u8> = [0i16, 8192, -16384].iter().flat_map(|s| s.to_le_bytes()).collect();
        let s = speech_from_pcm(&pcm, &Alignment::default()).unwrap();
        assert_eq!(s.samples.len(), 3);
        assert!((s.samples[2] + PEAK).abs() < 1e-6 && (s.samples[1] - PEAK / 2.0).abs() < 1e-6);
        assert!(speech_from_pcm(&[], &Alignment::default()).is_err());
    }

    #[test]
    fn ids_do_not_clash_with_kokoro() {
        assert!(CLOUD_VOICES.iter().all(|c| crate::voice(c.id).is_none()));
        assert!((credits_for(1000) - 8.0).abs() < 1e-9);
    }
}
