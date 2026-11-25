//! Speech to text on a server: the OpenAI-compatible `POST {endpoint}/audio/transcriptions`
//! call, which vLLM, the faster-whisper and whisper.cpp servers and cloud APIs share. The clip is
//! sent as 16 kHz mono WAV and asked for word times (`verbose_json`); a server that only returns
//! segments gets its words spread over each segment. Speaker labels and the word-edge tightening
//! run here on the audio, as for the local models.

use std::io::Read;

use filmcraft_project::{Transcript, Word};

use crate::{Options, ProgressFn, SpeechError, Transcriber, seconds_tick};

/// A speech model behind an endpoint.
pub struct Remote {
    pub id: String,
    pub endpoint: String,
    pub model: String,
    /// Bearer token, from the environment; never logged.
    pub token: Option<String>,
}

impl Remote {
    fn url(&self) -> String {
        format!("{}/audio/transcriptions", self.endpoint.trim_end_matches('/'))
    }
}

/// 16-bit PCM WAV of mono `audio` at 16 kHz.
pub fn wav(audio: &[f32]) -> Vec<u8> {
    let data = (audio.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&crate::SAMPLE_RATE.to_le_bytes());
    w.extend_from_slice(&(crate::SAMPLE_RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in audio {
        w.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    w
}

fn multipart(boundary: &str, fields: &[(&str, &str)], audio: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(audio.len() + 1024);
    for (k, v) in fields {
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    b.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    b.extend_from_slice(audio);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

/// A language as a two-letter code: servers answer with a code or an English name.
fn language_code(l: &str) -> String {
    let l = l.trim().to_ascii_lowercase();
    const NAMES: &[(&str, &str)] = &[
        ("english", "en"),
        ("vietnamese", "vi"),
        ("chinese", "zh"),
        ("japanese", "ja"),
        ("korean", "ko"),
        ("french", "fr"),
        ("german", "de"),
        ("spanish", "es"),
        ("italian", "it"),
        ("portuguese", "pt"),
        ("russian", "ru"),
        ("thai", "th"),
        ("indonesian", "id"),
        ("arabic", "ar"),
        ("hindi", "hi"),
        ("dutch", "nl"),
        ("turkish", "tr"),
        ("polish", "pl"),
    ];
    NAMES.iter().find(|(n, _)| *n == l).map_or(l, |(_, c)| (*c).to_string())
}

/// Words from a `verbose_json` answer: its `words`, else each segment's text spread over the
/// segment's time.
pub fn words_of(v: &serde_json::Value) -> Vec<Word> {
    let f = |x: &serde_json::Value, k: &str| x[k].as_f64().unwrap_or(0.0);
    if let Some(ws) = v["words"].as_array().filter(|w| !w.is_empty()) {
        return ws
            .iter()
            .filter_map(|w| {
                let t = w["word"].as_str().or_else(|| w["text"].as_str())?.trim();
                (!t.is_empty()).then(|| Word::new(t, seconds_tick(f(w, "start")), seconds_tick(f(w, "end").max(f(w, "start")))))
            })
            .collect();
    }
    let mut out = Vec::new();
    for s in v["segments"].as_array().into_iter().flatten() {
        let parts: Vec<&str> = s["text"].as_str().unwrap_or("").split_whitespace().collect();
        let (a, b) = (f(s, "start"), f(s, "end").max(f(s, "start")));
        let step = (b - a) / parts.len().max(1) as f64;
        for (i, p) in parts.iter().enumerate() {
            out.push(Word::new(*p, seconds_tick(a + step * i as f64), seconds_tick(a + step * (i + 1) as f64)));
        }
    }
    out
}

impl Transcriber for Remote {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn device(&self) -> String {
        format!("endpoint {}", self.endpoint)
    }

    fn transcribe(&self, audio: &[f32], opts: &Options, progress: ProgressFn) -> Result<Transcript, SpeechError> {
        if !progress(0.05, "Sending the audio") {
            return Err(SpeechError::Cancelled);
        }
        let boundary = format!("filmcraft-{:016x}", audio.len() as u64 ^ 0x5eed_f11c_c0de_d00d);
        let mut fields = vec![
            ("model", self.model.as_str()),
            ("response_format", "verbose_json"),
            ("timestamp_granularities[]", "word"),
            ("timestamp_granularities[]", "segment"),
        ];
        let language = opts.language.clone().filter(|l| !l.is_empty() && l != "auto");
        if let Some(l) = &language {
            fields.push(("language", l.as_str()));
        }
        let body = multipart(&boundary, &fields, &wav(audio));
        let mut req = crate::models::agent().post(&self.url()).header("Content-Type", format!("multipart/form-data; boundary={boundary}").as_str());
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}").as_str());
        }
        let mut resp = req.send(&body[..]).map_err(|e| SpeechError::Model(format!("{}: {e}", self.endpoint)))?;
        let mut text = String::new();
        resp.body_mut().as_reader().take(64 << 20).read_to_string(&mut text).map_err(|e| SpeechError::Io(e.to_string()))?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| SpeechError::Model(format!("{}: not a transcription answer: {e}", self.endpoint)))?;
        if !progress(0.9, "Reading the answer") {
            return Err(SpeechError::Cancelled);
        }
        let lang = language.or_else(|| v["language"].as_str().map(language_code)).unwrap_or_else(|| "en".into());
        let mut t = Transcript { language: lang, source: self.id.clone(), speakers: Vec::new(), words: words_of(&v) };
        t.normalize();
        crate::vad::tighten_words(audio, &mut t.words);
        if opts.diarize && !t.words.is_empty() {
            if !progress(0.96, "Labelling speakers") {
                return Err(SpeechError::Cancelled);
            }
            let p = crate::diarize::Params { max_speakers: opts.max_speakers, ..Default::default() };
            crate::diarize::diarize(audio, &mut t, &p);
        }
        progress(1.0, "Done");
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_only_answers_still_give_timed_words() {
        let v = serde_json::json!({"segments": [{"start": 1.0, "end": 2.0, "text": " hello big world"}]});
        let w = words_of(&v);
        assert_eq!(w.len(), 3);
        assert!(w[0].start < w[1].start && w[2].end <= seconds_tick(2.0));
        let v = serde_json::json!({"words": [{"word": " hi", "start": 0.5, "end": 0.7}], "segments": [{"start": 0.0, "end": 9.0, "text": "x y"}]});
        assert_eq!(words_of(&v).len(), 1, "word times win over segments");
    }

    #[test]
    fn the_wav_holds_every_sample() {
        let w = wav(&[0.0; 160]);
        assert_eq!(w.len(), 44 + 320);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(language_code("Vietnamese"), "vi");
    }
}
