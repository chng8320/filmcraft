//! `filmcraft-cli live`: FilmCraft's video effects on a live stream end to end. craft-live
//! (FFmpeg) reads the input (RTMP, SRT, RTSP, HLS, a device or a file) into raw RGBA frames, the
//! chain processes them frame by frame as in stream mode, and craft-live encodes them (H.264) to
//! the live output with the input's audio carried alongside. Control lines change the chain while
//! it runs.

use std::io::Write;

use crate::{Chain, Control, StreamConfig, StreamError, StreamStats, load_chain, parse_control, run};

/// Everything `effectcraft-cli live` takes.
#[derive(Debug, Clone)]
pub struct LiveRun {
    /// The chain: a JSON text or a file holding one.
    pub chain: String,
    /// Where the stream comes from: a URL, a device or a file.
    pub input: String,
    /// FFmpeg's input format for a device (`v4l2`, `dshow`, `avfoundation`).
    pub input_format: Option<String>,
    /// Where it goes: rtmp(s)://, srt://, rtsp://, a .m3u8 path (HLS) or a file.
    pub output: String,
    /// Control lines: `tcp:PORT`, or a file or FIFO.
    pub control: Option<String>,
    /// The size and rate the chain runs at (the input is scaled to it).
    pub config: StreamConfig,
    /// Stop after this many seconds; 0 runs until the input ends.
    pub seconds: u32,
    /// Carry the input's audio to the output (default).
    pub carry_audio: bool,
    pub video_bitrate_kbps: Option<u32>,
}

/// Run until the input ends (or `seconds`): reports the chain as one JSON line on `log` when it
/// starts, control errors as they happen, and the totals at the end.
pub fn live(opts: &LiveRun, log: &mut dyn Write) -> Result<StreamStats, StreamError> {
    let spec = load_chain(&opts.chain)?;
    let mut chain = Chain::new(&spec, &opts.config)?;
    let raw = craft_live::Raw::Video { width: opts.config.width, height: opts.config.height, fps: opts.config.fps };
    let mut lo = craft_live::LiveOptions::new(opts.input.clone(), Some(opts.output.clone()), raw);
    lo.input_format = opts.input_format.clone();
    lo.seconds = opts.seconds;
    lo.carry_other = opts.carry_audio;
    if let Some(k) = opts.video_bitrate_kbps {
        lo.video_bitrate_kbps = k;
    }
    let rx = match &opts.control {
        Some(spec) => {
            let lines = craft_live::control_lines(spec).map_err(|e| StreamError::Live(format!("control {spec}: {e}")))?;
            let (tx, rx) = std::sync::mpsc::channel::<Control>();
            std::thread::spawn(move || {
                for line in lines {
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
            Some(rx)
        }
        None => None,
    };
    let mut l = craft_live::Live::start(&lo).map_err(|e| StreamError::Live(e.to_string()))?;
    let ids: Vec<&str> = spec.items().iter().map(|i| i.id.as_str()).collect();
    let _ = writeln!(
        log,
        "{}",
        serde_json::json!({"live": "started", "input": opts.input, "output": opts.output, "effects": ids,
            "width": opts.config.width, "height": opts.config.height, "fps": opts.config.fps})
    );
    let mut out = l.out.take().ok_or_else(|| StreamError::Live("the output did not start".into()))?;
    let mut errs = Vec::new();
    let result = run(&mut chain, &mut l.frames, &mut out, rx.as_ref(), |e| errs.push(e));
    drop(out);
    for e in &errs {
        let _ = writeln!(log, "{}", serde_json::json!({"control_error": e}));
    }
    if result.is_err() {
        l.stop();
    }
    let finished = l.finish().map_err(|e| StreamError::Live(e.to_string()));
    let stats = result?;
    finished?;
    let _ = writeln!(log, "{}", serde_json::json!({"live": "ended", "stats": stats}));
    Ok(stats)
}
