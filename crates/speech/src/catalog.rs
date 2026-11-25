//! Speech models beyond the built-in catalogue ([`crate::models`]), from the same catalogue files
//! the other engines read (the Dataverse SDK's `sdk/models.md`): entries whose `task` is
//! `speech-to-text`, so a new model is a few lines of JSON, not a new build.
//!
//! ```json
//! { "models": [
//!   { "task": "speech-to-text", "id": "phowhisper-large", "dir": "phowhisper-large", "device": "gpu" },
//!   { "task": "speech-to-text", "id": "big-asr", "endpoint": "http://asr:8000/v1", "remote_model": "openai/whisper-large-v3" }
//! ] }
//! ```
//!
//! - **Folder** (`dir`): a Whisper model in the Hugging Face layout (`config.json`,
//!   `generation_config.json`, `tokenizer.json`, `model.safetensors`), run here on candle.
//!   Relative to the catalogue file. Not checksummed: the operator put it there.
//! - **Endpoint** (`endpoint`): any server with the OpenAI-compatible `/audio/transcriptions`
//!   call (vLLM, faster-whisper and whisper.cpp servers, cloud APIs) — any speech model, on its
//!   hardware. The key comes from the variable `token_env` names (default `FILMCRAFT_SPEECH_TOKEN`).
//!
//! Files: `<models dir>/catalog.json`, then each path in `FILMCRAFT_CATALOG`. A later entry
//! replaces an earlier one with the same id, also a built-in one.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{DeviceChoice, SpeechError};

/// The task speech-to-text models carry in a catalogue.
pub const TASK: &str = "speech-to-text";

/// How a catalogue speech model runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Folder(PathBuf),
    Endpoint { endpoint: String, model: String, token_env: String },
}

/// One speech model from a catalogue file.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeechModel {
    pub id: String,
    pub name: String,
    pub source: Source,
    pub device: Option<DeviceChoice>,
    pub origin: String,
}

#[derive(Deserialize)]
struct File {
    models: Vec<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct Entry {
    id: String,
    name: Option<String>,
    dir: Option<String>,
    multilingual: Option<bool>,
    endpoint: Option<String>,
    remote_model: Option<String>,
    token_env: Option<String>,
    device: Option<String>,
    description: Option<String>,
}

/// The catalogue files to read under `models_dir`.
pub fn default_files(models_dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let own = models_dir.join("catalog.json");
    if own.is_file() {
        v.push(own);
    }
    if let Some(list) = std::env::var_os("FILMCRAFT_CATALOG") {
        v.extend(std::env::split_paths(&list).filter(|p| !p.as_os_str().is_empty()));
    }
    v
}

/// The speech models of `files`, later ids replacing earlier ones.
pub fn load(files: &[PathBuf]) -> Result<Vec<SpeechModel>, SpeechError> {
    let mut out: Vec<SpeechModel> = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(f).map_err(|e| SpeechError::Model(format!("{}: {e}", f.display())))?;
        let parsed: File = serde_json::from_str(&text).map_err(|e| SpeechError::Model(format!("{}: {e}", f.display())))?;
        let base = f.parent().unwrap_or_else(|| Path::new("."));
        for mut v in parsed.models {
            if v.get("task").and_then(|t| t.as_str()) != Some(TASK) {
                continue; // another engine's model
            }
            if let Some(o) = v.as_object_mut() {
                o.remove("task");
            }
            let e: Entry = serde_json::from_value(v).map_err(|e| SpeechError::Model(format!("{}: {e}", f.display())))?;
            let m = entry(e, base, f).map_err(|e| SpeechError::Model(format!("{}: {e}", f.display())))?;
            out.retain(|x| x.id != m.id);
            out.push(m);
        }
    }
    Ok(out)
}

/// The catalogue model `id` under `models_dir`, if a catalogue file lists it.
pub fn find(models_dir: &Path, id: &str) -> Result<Option<SpeechModel>, SpeechError> {
    Ok(load(&default_files(models_dir))?.into_iter().find(|m| m.id == id))
}

fn entry(e: Entry, base: &Path, file: &Path) -> Result<SpeechModel, String> {
    let id = e.id.trim().to_string();
    if id.is_empty() {
        return Err("a speech model without an id".into());
    }
    let at = |what: &str| format!("speech model {id:?}: {what}");
    let source = match (e.dir, e.endpoint) {
        (Some(d), None) => Source::Folder(if Path::new(&d).is_absolute() { PathBuf::from(d) } else { base.join(d) }),
        (None, Some(endpoint)) => Source::Endpoint {
            endpoint,
            model: e.remote_model.ok_or_else(|| at("an endpoint needs remote_model"))?,
            token_env: e.token_env.unwrap_or_else(|| "FILMCRAFT_SPEECH_TOKEN".into()),
        },
        _ => return Err(at("give exactly one of dir (a Whisper folder) or endpoint")),
    };
    let device = e.device.as_deref().map(str::parse::<DeviceChoice>).transpose().map_err(|m| at(&m))?;
    let _ = (e.multilingual, e.description);
    Ok(SpeechModel { name: e.name.unwrap_or_else(|| id.clone()), id, source, device, origin: file.display().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_entries_are_read_and_other_tasks_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("catalog.json");
        std::fs::write(
            &f,
            r#"{"models": [
              {"id": "chat-model", "url": "https://x/a.gguf", "tokenizer_url": "https://x/t.json"},
              {"task": "speech-to-text", "id": "pho", "dir": "pho", "device": "gpu"},
              {"task": "speech-to-text", "id": "big", "endpoint": "http://asr:8000/v1", "remote_model": "w3"}
            ]}"#,
        )
        .unwrap();
        let m = load(&[f.clone()]).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].source, Source::Folder(dir.path().join("pho")));
        assert_eq!(m[0].device, Some(DeviceChoice::Gpu));
        assert!(matches!(&m[1].source, Source::Endpoint { token_env, .. } if token_env == "FILMCRAFT_SPEECH_TOKEN"));
        std::fs::write(&f, r#"{"models": [{"task": "speech-to-text", "id": "x"}]}"#).unwrap();
        assert!(load(&[f]).is_err(), "neither dir nor endpoint");
    }
}
