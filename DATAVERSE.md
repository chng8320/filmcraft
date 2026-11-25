# This fork: FilmCraft for the Dataverse

This is the Dataverse's fork of [FilmCraft](https://github.com/storytold/filmcraft) by the ArtCraft team
(MIT OR Apache-2.0). The Dataverse runs its command line on the main server, headless, as the `filmcraft-engine`
package.

## Branches

| Branch | What it is |
|---|---|
| `main` | Exactly ArtCraft's `main`. Never commit here; it is only updated from them. |
| `dataverse` | `main` plus our own commits on top. This is what the Dataverse builds. |

Each of our changes is one commit on `dataverse`, kept small and in its own place, so it replays cleanly
on every new FilmCraft and can be offered back as a pull request. FilmCraft's rules for contributors
(`AGENTS.md`) apply to our commits too.

## Keeping up with FilmCraft

`.github/workflows/dataverse.yml` runs every Monday (and on demand from the Actions tab): it updates `main`
from ArtCraft, replays our commits on `dataverse` (opening an issue when one no longer applies), builds the
command line, runs its tests and attaches the program as `dataverse-film-cli` (Linux x86-64).

## Our changes

| Commit | What | Offered upstream? |
|---|---|---|
| stream mode | `crates/stream` + `filmcraft-cli stream`: a chain of video effects (colour grading, looks, blurs, keys…) on live raw RGBA video, live parameter changes over a control FIFO | not yet |
| speech on the server | `FILMCRAFT_MODELS`, and the command line built with `whisper` and `neural-voices` | not yet |

## Name

The ArtCraft name and logos are trademarks, not covered by the code licence; modified versions must not use
them. Our build is called `dataverse-film-cli` and is described as "based on FilmCraft by the ArtCraft team".

## Speech models and devices

`transcript.generate` (and the sequence transcription command) take `model` (`whisper-tiny`, `whisper-base`,
`whisper-small`, `whisper-large-v3-turbo`) and `device` (`auto`, `cpu`, `gpu`, `cuda[:N]`, `metal[:N]`);
a server sets its default device with `FILMCRAFT_DEVICE`. The result says what ran it (`"device"`).
CI builds `dataverse-film-cli-linux-x86_64` (CPU) and `dataverse-film-cli-linux-x86_64-cuda` (NVIDIA GPUs).

## Any speech model (the model catalogue)

Speech models beyond the built-in Whisper list come from catalogue files (`<FILMCRAFT_MODELS>/catalog.json`,
`FILMCRAFT_CATALOG`), the same files craft-ai reads: entries with `"task": "speech-to-text"` and either
`dir` (a Whisper folder in the Hugging Face layout, run here on CPU or GPU) or `endpoint` + `remote_model`
(any OpenAI-compatible `/audio/transcriptions` server; key from `token_env`, default
`FILMCRAFT_SPEECH_TOKEN`). `transcript.models` lists them; `transcript.generate` takes their ids.

## Speech to text now comes from craft-ai

Whisper, the speech model catalogue, the device choice, endpoints and speaker labelling moved to craft-ai
(`craft_ai::speech` in the Dataverse fork of craft-libs) so every engine shares them. `filmcraft-speech` is
now FilmCraft's side only: the `Transcriber` trait in FilmCraft's types and the exact sample-to-tick conversion.
The moved tests (Whisper end to end, diarization, word tightening, catalogue, endpoint) run in craft-libs.
Settings are craft-ai's: `CRAFT_AI_DEVICE` (was `FILMCRAFT_DEVICE`), `CRAFT_AI_CATALOG` (was
`FILMCRAFT_CATALOG`), `CRAFT_AI_TOKEN` for endpoints (was `FILMCRAFT_SPEECH_TOKEN`); models stay in
`FILMCRAFT_MODELS`.

## Live mode

`filmcraft-cli live --chain CHAIN --in URL --out URL --width W --height H [--fps N] [--control tcp:PORT|FIFO]`:
FilmCraft's video effects on a live stream end to end through craft-live (FFmpeg), the input's audio carried,
the same as EffectCraft's live mode. Live captions are craft-ai's (`craft-ai-cli live`).
