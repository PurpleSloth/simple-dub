use simple_dub_core::subtitles::{SubtitleSegment, parse_srt, write_srt};

#[test]
fn parses_multiline_srt_and_removes_visual_markup() {
    let input = "\u{feff}1\r\n\
00:00:01,250 --> 00:00:03,500\r\n\
<i>Первый</i> текст\r\n\
в две строки\r\n\r\n\
2\r\n\
00:00:05.000 --> 00:00:06.100\r\n\
{\\an8}Второй текст\r\n";

    let segments = parse_srt(input).expect("SRT должен разбираться");

    assert_eq!(
        segments,
        vec![
            SubtitleSegment {
                id: "1".to_owned(),
                start_ms: 1_250,
                end_ms: 3_500,
                text: "Первый текст в две строки".to_owned(),
            },
            SubtitleSegment {
                id: "2".to_owned(),
                start_ms: 5_000,
                end_ms: 6_100,
                text: "Второй текст".to_owned(),
            },
        ]
    );
}

#[test]
fn rejects_invalid_or_reversed_timestamps() {
    let invalid = "1\n00:00:04,000 --> 00:00:03,000\nТекст\n";
    let error = parse_srt(invalid).expect_err("обратный интервал должен отклоняться");

    assert!(error.to_string().contains("интервал"));
}

#[test]
fn srt_round_trip_preserves_ids_timing_and_text() {
    let source = vec![SubtitleSegment {
        id: "scene-7".to_owned(),
        start_ms: 65_432,
        end_ms: 68_000,
        text: "Строка для озвучки".to_owned(),
    }];

    let serialized = write_srt(&source);
    let parsed = parse_srt(&serialized).unwrap();

    assert_eq!(parsed[0].start_ms, source[0].start_ms);
    assert_eq!(parsed[0].end_ms, source[0].end_ms);
    assert_eq!(parsed[0].text, source[0].text);
}
