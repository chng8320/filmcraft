//! Speech-to-text for FilmCraft's Text panel ▸ Transcript, on craft-ai's speech to text
//! (`craft_ai::speech` in craft-libs: Whisper on the CPU or a GPU, the model catalogue, endpoints,
//! speaker labelling). This crate keeps FilmCraft's side: the [`Transcriber`] trait in FilmCraft's
//! own types (`Tick`s; one 16 kHz sample is exactly 15 876 000 ticks) and the exact conversion
//! from craft-ai's sample times.
//!
//! See `docs/transcripts.md` for the user-facing behaviour and accuracy numbers.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

use std::sync::Arc;

use filmcraft_project::{Speaker, Transcript, Word};
use filmcraft_time::{TICKS_PER_SECOND, Tick};

pub use craft_ai::DeviceChoice;
pub use craft_ai::speech::{Options, ProgressFn, SAMPLE_RATE, SpeechError, catalog, downmix, models, word_error_rate};

/// The processors this build can use.
pub mod device {
    pub use craft_ai::compiled_backends;
}

/// Ticks per 16 kHz sample (exact).
pub const TICKS_PER_SAMPLE: i64 = TICKS_PER_SECOND / SAMPLE_RATE as i64;

/// Media time of a 16 kHz sample index.
pub fn sample_tick(n: i64) -> Tick {
    Tick(n * TICKS_PER_SAMPLE)
}

/// Media time of `s` seconds (rounded to the nearest 16 kHz sample).
pub fn seconds_tick(s: f64) -> Tick {
    sample_tick((s * SAMPLE_RATE as f64).round() as i64)
}

/// A speech recogniser.
pub trait Transcriber: Send + Sync {
    /// Stable id of the model (`"whisper-base"`), stored in the transcript's `source`.
    fn id(&self) -> String;
    /// Transcribe mono 16 kHz `audio`. Word times are relative to the first sample.
    fn transcribe(&self, audio: &[f32], opts: &Options, progress: ProgressFn) -> Result<Transcript, SpeechError>;
    /// What the model runs on (`"CPU"`, `"NVIDIA GPU 0 (CUDA)"`, an endpoint).
    fn device(&self) -> String {
        "CPU".into()
    }
}

/// A craft-ai transcript in FilmCraft's types: sample times become ticks exactly.
pub fn to_project(t: craft_ai::speech::Transcript) -> Transcript {
    Transcript {
        language: t.language,
        source: t.source,
        speakers: t.speakers.into_iter().map(|s| Speaker { name: s.name }).collect(),
        words: t
            .words
            .into_iter()
            .map(|w| {
                let mut x = Word::new(w.text, sample_tick(w.start.0), sample_tick(w.end.0));
                x.speaker = w.speaker;
                x.confidence = w.confidence;
                x
            })
            .collect(),
    }
}

/// A craft-ai transcriber behind FilmCraft's trait.
struct Shared(Arc<dyn craft_ai::speech::Transcriber>);

impl Transcriber for Shared {
    fn id(&self) -> String {
        self.0.id()
    }
    fn transcribe(&self, audio: &[f32], opts: &Options, progress: ProgressFn) -> Result<Transcript, SpeechError> {
        self.0.transcribe(audio, opts, progress).map(to_project)
    }
    fn device(&self) -> String {
        self.0.device()
    }
}

/// A transcriber that returns a fixed transcript (tests, demos and agents that bring their own
/// transcript); words beyond the audio's length are dropped.
pub struct FixedTranscriber {
    pub transcript: Transcript,
    pub id: String,
}

impl Transcriber for FixedTranscriber {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn transcribe(&self, audio: &[f32], _: &Options, progress: ProgressFn) -> Result<Transcript, SpeechError> {
        if !progress(0.5, "Transcribing") {
            return Err(SpeechError::Cancelled);
        }
        let end = sample_tick(audio.len() as i64);
        let mut t = self.transcript.clone();
        t.words.retain(|w| w.start < end);
        t.source = self.id.clone();
        progress(1.0, "Done");
        Ok(t)
    }
}

/// Whether `id` is a speech model here: built in, or in a model catalogue file.
pub fn known(models_dir: &std::path::Path, id: &str) -> bool {
    craft_ai::speech::known(models_dir, id)
}

/// Load speech model `id` (craft-ai: a catalogue entry, else a built-in Whisper model in
/// `models_dir`). The processor: `device`, else the catalogue entry's, else `CRAFT_AI_DEVICE`,
/// else auto.
pub fn load_with(models_dir: &std::path::Path, id: &str, device: Option<DeviceChoice>) -> Result<Arc<dyn Transcriber>, SpeechError> {
    Ok(Arc::new(Shared(craft_ai::speech::load(models_dir, id, device)?)))
}

/// Whether this build can run speech models here.
pub const fn available() -> bool {
    cfg!(feature = "whisper")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_ticks_are_exact() {
        assert_eq!(TICKS_PER_SAMPLE * SAMPLE_RATE as i64, TICKS_PER_SECOND);
        assert_eq!(seconds_tick(1.5), Tick(TICKS_PER_SECOND * 3 / 2));
    }

    #[test]
    fn craft_ai_times_become_the_same_instants_in_ticks() {
        let mut t = craft_ai::speech::Transcript { language: "vi".into(), ..Default::default() };
        let mut w = craft_ai::speech::Word::new("xin", craft_ai::speech::Samples(16_000), craft_ai::speech::Samples(24_000));
        w.speaker = Some(0);
        t.words.push(w);
        t.normalize();
        let p = to_project(t);
        assert_eq!((p.words[0].start, p.words[0].end), (seconds_tick(1.0), seconds_tick(1.5)));
        assert_eq!((p.words[0].speaker, p.speakers.len(), p.language.as_str()), (Some(0), 1, "vi"));
    }

    #[test]
    fn fixed_transcriber_trims_and_cancels() {
        let mut t = Transcript::default();
        t.words.push(Word::new("hi", seconds_tick(0.1), seconds_tick(0.3)));
        t.words.push(Word::new("late", seconds_tick(5.0), seconds_tick(5.5)));
        let f = FixedTranscriber { transcript: t, id: "fixed".into() };
        let out = f.transcribe(&vec![0.0; 16_000], &Options::default(), &mut |_, _| true).unwrap();
        assert_eq!(out.words.len(), 1);
        assert_eq!(out.source, "fixed");
        assert_eq!(f.transcribe(&[], &Options::default(), &mut |_, _| false), Err(SpeechError::Cancelled));
    }
}
