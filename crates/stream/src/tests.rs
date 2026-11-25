use super::*;

fn cfg(w: u32, h: u32) -> StreamConfig {
    StreamConfig { width: w, height: h, fps: 25.0 }
}

fn chain(json: &str, c: StreamConfig) -> Result<Chain, StreamError> {
    let spec: ChainSpec = serde_json::from_str(json).map_err(|e| StreamError::BadSize(e.to_string()))?;
    Chain::new(&spec, &c)
}

fn frames(px: [u8; 4], n: usize, w: u32, h: u32) -> Vec<u8> {
    px.iter().copied().cycle().take(w as usize * h as usize * 4 * n).collect()
}

#[test]
fn a_chain_naming_what_doesnt_exist_or_isnt_video_is_refused() {
    assert!(matches!(chain(r#"[{"id":"nope"}]"#, cfg(4, 4)), Err(StreamError::UnknownEffect(_))));
    assert!(matches!(chain(r#"[{"id":"volume"}]"#, cfg(4, 4)), Err(StreamError::NotVideo(_))));
    assert!(matches!(chain(r#"[{"id":"tint","params":{"nope":1}}]"#, cfg(4, 4)), Err(StreamError::UnknownParam { .. })));
    assert!(matches!(chain(r#"[{"id":"gaussian_blur","params":{"blurriness":"lots"}}]"#, cfg(4, 4)), Err(StreamError::BadValue { .. })));
    assert!(matches!(chain("[]", cfg(4, 0)), Err(StreamError::BadSize(_))));
}

#[test]
fn every_whole_frame_comes_out_processed_and_a_partial_one_is_dropped() {
    let (w, h) = (8, 4);
    let mut c = chain(r#"{"effects":[{"id":"tint"}]}"#, cfg(w, h)).unwrap_or_else(|e| panic!("{e}"));
    let mut input = frames([230, 40, 40, 255], 2, w, h);
    input.extend_from_slice(&[9, 9]);
    let mut out = Vec::new();
    let stats = run(&mut c, &mut input.as_slice(), &mut out, None, |_| {}).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(stats.frames, 2);
    assert_eq!(out.len(), w as usize * h as usize * 4 * 2);
    let (r, g, b) = (out[0] as i32, out[1] as i32, out[2] as i32);
    assert!((r - g).abs() <= 2 && (g - b).abs() <= 2, "tinted to grey: {:?}", &out[..4]);
}

#[test]
fn a_control_message_changes_the_next_frame_and_a_bad_one_is_reported_not_fatal() {
    let (w, h) = (4, 4);
    let mut c = chain(r#"[{"id":"brightness_contrast"}]"#, cfg(w, h)).unwrap_or_else(|e| panic!("{e}"));
    let input = frames([100, 100, 100, 255], 1, w, h);
    let mut before = Vec::new();
    run(&mut c, &mut input.as_slice(), &mut before, None, |_| {}).unwrap_or_else(|e| panic!("{e}"));
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(Control::Set { effect: EffectRef::Id("brightness_contrast".into()), param: "brightness".into(), value: serde_json::json!(60) });
    let _ = tx.send(Control::Bypass { effect: EffectRef::Index(7), on: true });
    let (mut after, mut errors) = (Vec::new(), Vec::new());
    let stats = run(&mut c, &mut input.as_slice(), &mut after, Some(&rx), |e| errors.push(e)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((stats.controls, errors.len()), (2, 1));
    assert!(after[0] > before[0], "brighter after the change: {} then {}", before[0], after[0]);
}

#[test]
fn plain_json_values_take_the_parameters_kinds() {
    assert_eq!(coerce(&ParamValue::Color([0.0; 4]), &serde_json::json!("#00ff00")).ok(), Some(ParamValue::Color([0.0, 1.0, 0.0, 1.0])));
    assert_eq!(coerce(&ParamValue::Choice(0), &serde_json::json!(2)).ok(), Some(ParamValue::Choice(2)));
    assert_eq!(coerce(&ParamValue::Curve(vec![]), &serde_json::json!([[0, 0], [1, 1]])).ok(), Some(ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]])));
    assert!(coerce(&ParamValue::Float(0.0), &serde_json::json!(true)).is_err());
}

#[test]
fn the_list_has_the_video_effects_and_none_of_the_audio_ones() {
    let ids: Vec<&str> = chainable().iter().map(|d| d.id).collect();
    assert!(ids.contains(&"lumetri") && ids.contains(&"gaussian_blur"));
    assert!(!ids.contains(&"volume"));
}

#[test]
fn live_mode_runs_a_live_input_through_the_chain_to_a_live_output() {
    // FFmpeg on both sides (the dataverse workflow installs it): one second of test pattern in,
    // the chain, and an encoded file out.
    let dir = std::env::temp_dir().join(format!("filmcraft-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.mkv");
    let run = LiveRun {
        chain: "[]".into(),
        input: "testsrc=size=64x36:rate=10:duration=1".into(),
        input_format: Some("lavfi".into()),
        output: out.to_string_lossy().into_owned(),
        control: None,
        config: StreamConfig { width: 64, height: 36, fps: 10.0 },
        seconds: 0,
        carry_audio: false,
        video_bitrate_kbps: None,
    };
    let mut log = Vec::new();
    let stats = live(&run, &mut log).unwrap();
    assert!(stats.frames >= 9 && stats.frames <= 11, "{stats:?}");
    assert!(std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) > 500, "the output was written");
    let _ = std::fs::remove_dir_all(&dir);
}
