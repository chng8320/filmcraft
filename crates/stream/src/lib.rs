#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]
//! FilmCraft stream mode.
//!
//! Runs a chain of FilmCraft's video effects (the same ids and parameters as the Effects panel and
//! the engine's `effect.*` commands: Lumetri colour, looks and LUTs, blurs, sharpen, keys, …) over
//! a live video stream: raw 8-bit sRGB RGBA frames in on one reader, the processed frames out on
//! one writer, frame by frame, so FFmpeg or GStreamer sit on either side. Parameters change while
//! it runs through control messages, one JSON object per line, as in SoundCraft's and EffectCraft's
//! stream modes. Each effect sees one frame at the stream's time and no project: effects that need
//! other clips or frames get none.

use std::io::{self, Read, Write};
use std::sync::mpsc::{Receiver, TryRecvError};

use filmcraft_project::{EffectDef, EffectInstance, EffectKind, Param, ParamValue, find_effect};
use filmcraft_render::Image;
use filmcraft_render::effects::FxCtx;
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};

mod live;
pub use live::{LiveRun, live};

/// One effect of a chain: its id and the parameters to set.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ChainItem {
    pub id: String,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub bypass: bool,
}

/// A chain: `{"effects": [...]}` or just the list.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum ChainSpec {
    Object { effects: Vec<ChainItem> },
    List(Vec<ChainItem>),
}

impl ChainSpec {
    pub fn items(&self) -> &[ChainItem] {
        match self {
            Self::Object { effects } | Self::List(effects) => effects,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StreamConfig {
    pub width: u32,
    pub height: u32,
    /// Frames per second: the time each effect sees (frame n is at n / fps seconds).
    pub fps: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum EffectRef {
    Index(usize),
    Id(String),
}

/// A live change: `{"set": {"effect": 0 | "lumetri", "param": "exposure", "value": 0.5}}`,
/// `{"bypass": {"effect": 1, "on": true}}` or `{"reset": true}`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Control {
    Set { effect: EffectRef, param: String, value: serde_json::Value },
    Bypass { effect: EffectRef, on: bool },
    Reset(bool),
}

#[derive(Debug)]
pub enum StreamError {
    UnknownEffect(String),
    NotVideo(String),
    UnknownParam {
        effect: String,
        param: String,
    },
    BadValue {
        effect: String,
        param: String,
        why: String,
    },
    BadSize(String),
    Io(io::Error),
    /// The live input or output (FFmpeg) failed.
    Live(String),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEffect(id) => write!(f, "unknown effect `{id}` (filmcraft-cli stream --list)"),
            Self::NotVideo(id) => write!(f, "`{id}` is not a video effect (stream mode runs video effects; audio goes through soundcraft-cli stream)"),
            Self::UnknownParam { effect, param } => write!(f, "{effect} has no parameter `{param}`"),
            Self::BadValue { effect, param, why } => write!(f, "{effect} {param}: {why}"),
            Self::BadSize(m) => write!(f, "{m}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Live(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for StreamError {}

impl From<io::Error> for StreamError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// A JSON value as the parameter's kind, going by its default: numbers for sliders, popup indexes
/// (0-based) for choices, `[x, y]` for points, `[r, g, b]`/`[r, g, b, a]` (0–1) or `"#rrggbb"`
/// for colours, booleans, strings, `[[x, y], …]` for curves; or FilmCraft's own `{"Float": 1.0}`.
pub fn coerce(default: &ParamValue, j: &serde_json::Value) -> Result<ParamValue, String> {
    use serde_json::Value as J;
    if let J::Object(_) = j {
        return serde_json::from_value(j.clone()).map_err(|e| e.to_string());
    }
    let nums = |n: usize| -> Option<Vec<f64>> {
        let a = j.as_array()?;
        (a.len() == n).then(|| a.iter().filter_map(J::as_f64).collect::<Vec<_>>()).filter(|v| v.len() == n)
    };
    let out = match default {
        ParamValue::Float(_) => j.as_f64().map(ParamValue::Float),
        ParamValue::Choice(_) => j.as_u64().and_then(|n| u32::try_from(n).ok()).map(ParamValue::Choice),
        ParamValue::Bool(_) => j.as_bool().map(ParamValue::Bool),
        ParamValue::Text(_) => j.as_str().map(|s| ParamValue::Text(s.to_string())),
        ParamValue::Vec2(_) => nums(2).map(|v| ParamValue::Vec2(filmcraft_geom::Vec2::new(v[0], v[1]))),
        ParamValue::Color(_) => match j {
            J::String(s) => hex_color(s).map(ParamValue::Color),
            _ => nums(4)
                .map(|v| ParamValue::Color([v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]))
                .or_else(|| nums(3).map(|v| ParamValue::Color([v[0] as f32, v[1] as f32, v[2] as f32, 1.0]))),
        },
        ParamValue::Curve(_) => j
            .as_array()
            .and_then(|a| {
                a.iter()
                    .map(|p| p.as_array().filter(|q| q.len() == 2).and_then(|q| Some([q[0].as_f64()? as f32, q[1].as_f64()? as f32])))
                    .collect::<Option<Vec<_>>>()
            })
            .map(ParamValue::Curve),
        ParamValue::Path(_) => None,
    };
    out.ok_or_else(|| format!("{j} doesn't fit this parameter (its default is {})", serde_json::to_string(default).unwrap_or_default()))
}

fn hex_color(s: &str) -> Option<[f32; 4]> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|b| f32::from(b) / 255.0);
    match h.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 1.0]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

struct Slot {
    def: &'static EffectDef,
    fx: EffectInstance,
    initial: EffectInstance,
}

pub struct Chain {
    slots: Vec<Slot>,
    cfg: StreamConfig,
}

impl Chain {
    pub fn new(spec: &ChainSpec, cfg: &StreamConfig) -> Result<Self, StreamError> {
        if cfg.width == 0 || cfg.height == 0 || cfg.width > 16384 || cfg.height > 16384 {
            return Err(StreamError::BadSize(format!("frame size {}x{} (1 to 16384 each)", cfg.width, cfg.height)));
        }
        let mut slots = Vec::new();
        for item in spec.items() {
            let def = find_effect(&item.id).ok_or_else(|| StreamError::UnknownEffect(item.id.clone()))?;
            if def.kind != EffectKind::Video {
                return Err(StreamError::NotVideo(item.id.clone()));
            }
            let mut fx = def.instance();
            for (k, v) in &item.params {
                let d = def.param(k).ok_or_else(|| StreamError::UnknownParam { effect: item.id.clone(), param: k.clone() })?;
                let val = coerce(&d.default, v).map_err(|why| StreamError::BadValue { effect: item.id.clone(), param: k.clone(), why })?;
                fx.params.insert(k.clone(), Param::new(val));
            }
            fx.enabled = !item.bypass;
            slots.push(Slot { def, initial: fx.clone(), fx });
        }
        Ok(Self { slots, cfg: *cfg })
    }

    /// One frame (straight sRGB RGBA8, the stream's size) through every effect not bypassed.
    pub fn process(&self, rgba: &[u8], time: f64) -> Vec<u8> {
        let mut img = from_srgb8(self.cfg.width as usize, self.cfg.height as usize, rgba);
        let cx = FxCtx {
            t: Tick::from_seconds_f64(time),
            px_scale: 1.0,
            mask_scale: 1.0,
            seconds: time,
            timecode: "",
            clip_name: "",
            project: None,
            env: None,
            frame_rate: self.cfg.fps,
            working: filmcraft_color::WorkingSpace::default(),
        };
        for s in &self.slots {
            filmcraft_render::effects::apply(&mut img, &s.fx, &cx);
        }
        img.to_rgba8()
    }

    pub fn apply(&mut self, c: &Control) -> Result<(), String> {
        match c {
            Control::Set { effect, param, value } => {
                let s = self.slot(effect).ok_or_else(|| format!("no effect {effect:?} in the chain"))?;
                let d = s.def.param(param).ok_or_else(|| format!("{} has no parameter `{param}`", s.def.id))?;
                let v = coerce(&d.default, value).map_err(|e| format!("{} {param}: {e}", s.def.id))?;
                s.fx.params.insert(param.clone(), Param::new(v));
            }
            Control::Bypass { effect, on } => self.slot(effect).ok_or_else(|| format!("no effect {effect:?} in the chain"))?.fx.enabled = !*on,
            Control::Reset(_) => {
                for s in &mut self.slots {
                    s.fx = s.initial.clone();
                }
            }
        }
        Ok(())
    }

    fn slot(&mut self, r: &EffectRef) -> Option<&mut Slot> {
        match r {
            EffectRef::Index(i) => self.slots.get_mut(*i),
            EffectRef::Id(id) => self.slots.iter_mut().find(|s| s.def.id == id),
        }
    }
}

/// Straight sRGB RGBA8 to the compositor's premultiplied linear light.
fn from_srgb8(w: usize, h: usize, rgba: &[u8]) -> Image {
    let lut = filmcraft_color::srgb_u8_to_linear_table();
    let mut img = Image::new(w, h);
    for (o, s) in img.px.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let a = f32::from(s[3]) / 255.0;
        o[0] = lut[s[0] as usize] * a;
        o[1] = lut[s[1] as usize] * a;
        o[2] = lut[s[2] as usize] * a;
        o[3] = a;
    }
    img
}

/// The video effects a stream can run: every video effect FilmCraft has.
pub fn chainable() -> Vec<&'static EffectDef> {
    filmcraft_project::effect_defs().iter().filter(|d| d.kind == EffectKind::Video && !d.intrinsic).collect()
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct StreamStats {
    pub frames: u64,
    pub controls: u64,
    pub seconds_of_video: f64,
}

/// Read frames from `input` until it ends, run each through `chain`, write it to `output`.
/// Control messages waiting on `controls` apply between frames; an invalid one goes to `on_error`
/// and the stream goes on. A partial last frame is dropped.
pub fn run(
    chain: &mut Chain,
    input: &mut dyn Read,
    output: &mut dyn Write,
    controls: Option<&Receiver<Control>>,
    mut on_error: impl FnMut(String),
) -> Result<StreamStats, StreamError> {
    let mut raw = vec![0u8; chain.cfg.width as usize * chain.cfg.height as usize * 4];
    let mut stats = StreamStats::default();
    loop {
        if let Some(rx) = controls {
            loop {
                match rx.try_recv() {
                    Ok(c) => {
                        stats.controls += 1;
                        if let Err(e) = chain.apply(&c) {
                            on_error(e);
                        }
                    }
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
        }
        if read_full(input, &mut raw)? < raw.len() {
            break;
        }
        let time = if chain.cfg.fps > 0.0 { stats.frames as f64 / chain.cfg.fps } else { 0.0 };
        output.write_all(&chain.process(&raw, time))?;
        stats.frames += 1;
    }
    output.flush()?;
    stats.seconds_of_video = if chain.cfg.fps > 0.0 { stats.frames as f64 / chain.cfg.fps } else { 0.0 };
    Ok(stats)
}

fn read_full(r: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

pub fn parse_control(line: &str) -> Option<Result<Control, String>> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    Some(serde_json::from_str(t).map_err(|e| format!("control {t:?}: {e}")))
}

pub struct ServeOptions {
    /// The chain: a JSON text or a file holding one.
    pub chain: String,
    pub config: StreamConfig,
    /// A file or FIFO of control messages, one per line.
    pub control: Option<String>,
}

/// Frames on standard input, frames on standard output, a JSON line to `log` when it starts and
/// when it ends.
pub fn serve(opts: &ServeOptions, log: &mut dyn Write) -> Result<StreamStats, StreamError> {
    let spec = load_chain(&opts.chain)?;
    let mut chain = Chain::new(&spec, &opts.config)?;
    let ids: Vec<&str> = spec.items().iter().map(|i| i.id.as_str()).collect();
    let _ = writeln!(
        log,
        "{}",
        serde_json::json!({"stream": "started", "effects": ids, "width": opts.config.width, "height": opts.config.height, "fps": opts.config.fps})
    );
    let rx = opts.control.as_ref().map(|path| {
        let (tx, rx) = std::sync::mpsc::channel();
        let path = path.clone();
        std::thread::spawn(move || {
            let Ok(f) = std::fs::File::open(&path) else { return };
            for line in io::BufRead::lines(io::BufReader::new(f)).map_while(Result::ok) {
                match parse_control(&line) {
                    Some(Ok(c)) => {
                        if tx.send(c).is_err() {
                            break;
                        }
                    }
                    Some(Err(e)) => log::warn!("{e}"),
                    None => {}
                }
            }
        });
        rx
    });
    let stdin = io::stdin();
    let stdout = io::stdout();
    let (mut i, mut o) = (stdin.lock(), stdout.lock());
    let mut errs = Vec::new();
    let stats = run(&mut chain, &mut i, &mut o, rx.as_ref(), |e| errs.push(e))?;
    for e in &errs {
        let _ = writeln!(log, "{}", serde_json::json!({"control_error": e}));
    }
    let _ = writeln!(log, "{}", serde_json::json!({"stream": "ended", "stats": stats}));
    Ok(stats)
}

/// A chain from a JSON text or a file holding one.
pub fn load_chain(chain: &str) -> Result<ChainSpec, StreamError> {
    let text = if chain.trim_start().starts_with(['[', '{']) { chain.to_string() } else { std::fs::read_to_string(chain)? };
    serde_json::from_str(&text).map_err(|e| StreamError::Io(io::Error::new(io::ErrorKind::InvalidData, format!("chain: {e}"))))
}

/// The effects a stream can run, with their parameters and defaults (for `--list`).
pub fn list_json() -> serde_json::Value {
    let fx: Vec<serde_json::Value> = chainable()
        .iter()
        .map(|d| {
            serde_json::json!({
                "id": d.id, "name": d.name, "category": d.category,
                "params": d.params.iter().map(|p| serde_json::json!({"id": p.id, "name": p.label, "default": p.default})).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({ "effects": fx })
}

#[cfg(test)]
mod tests;
