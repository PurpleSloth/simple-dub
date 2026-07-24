use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use simple_dub_core::commands::{MuxOptions, build_mux_args};
use simple_dub_core::media::parse_ffprobe_json;

#[test]
fn preserves_existing_mkv_streams_and_adds_dub_audio() {
    let Some(ffmpeg) = find_ffmpeg("ffmpeg.exe") else {
        eprintln!("ffmpeg не найден — интеграционный тест пропущен");
        return;
    };
    let Some(ffprobe) = find_ffmpeg("ffprobe.exe") else {
        eprintln!("ffprobe не найден — интеграционный тест пропущен");
        return;
    };

    let temp = tempfile::tempdir().unwrap();
    let subtitles = temp.path().join("ru.srt");
    let input = temp.path().join("input.mkv");
    let dubbed = temp.path().join("dubbed.m4a");
    let output = temp.path().join("output.dub.ru.mkv");

    fs::write(
        &subtitles,
        "1\n00:00:00,100 --> 00:00:00,800\nТестовая реплика\n",
    )
    .unwrap();

    run(
        &ffmpeg,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=320x180:d=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=660:duration=1",
            "-i",
            subtitles.to_str().unwrap(),
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-map",
            "2:a:0",
            "-map",
            "3:s:0",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            "-c:s",
            "srt",
            "-metadata:s:a:0",
            "language=jpn",
            "-metadata:s:a:1",
            "language=eng",
            "-metadata:s:s:0",
            "language=rus",
            "-y",
            input.to_str().unwrap(),
        ],
    );

    run(
        &ffmpeg,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:duration=1",
            "-c:a",
            "aac",
            "-y",
            dubbed.to_str().unwrap(),
        ],
    );

    let mux_args = build_mux_args(&MuxOptions {
        input_path: &input,
        dubbed_audio_path: &dubbed,
        output_path: &output,
        existing_audio_streams: 2,
        make_default: true,
    });
    let mux_refs: Vec<&str> = mux_args.iter().map(String::as_str).collect();
    run(&ffmpeg, &mux_refs);

    let probe = Command::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_chapters",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(&output)
        .output()
        .unwrap();
    assert!(probe.status.success());

    let media = parse_ffprobe_json(&String::from_utf8(probe.stdout).unwrap()).unwrap();
    assert_eq!(media.audio_streams().len(), 3);
    assert_eq!(media.subtitle_streams().len(), 1);

    let dub = media.audio_streams()[2];
    assert_eq!(dub.language.as_deref(), Some("rus"));
    assert_eq!(dub.title.as_deref(), Some("Русский одноголосый дубляж"));
    assert!(!media.audio_streams()[0].is_default);
    assert!(!media.audio_streams()[1].is_default);
    assert!(dub.is_default);
}

fn find_ffmpeg(name: &str) -> Option<PathBuf> {
    let configured = Path::new(r"C:\ffmpeg\bin").join(name);
    if configured.is_file() {
        return Some(configured);
    }

    let status = Command::new(name).arg("-version").output().ok()?;
    status.status.success().then(|| PathBuf::from(name))
}

fn run(program: &Path, args: &[&str]) {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
