use super::*;
#[test]
fn retains_character_text_table_and_symbolic_lookup_without_resolving_calls() {
    let raw = b": main\nMzEntryCharTextData\n55 7 3 63 2 3 ;\nMzGetGameCharID first second\nMzGetCharTextData first message face bubble MzSetText 15 message 0 bubble 0 0\0";
    let events = scan_commands(raw, true).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["table_rows"], json!([[55, 7, 3], [63, 2, 3]]));
    assert_eq!(events[0]["mapping_status"], "lookup_unresolved");
    assert_eq!(events[0]["byte_offset"], 7);
    assert_eq!(events[0]["line"], 2);
    assert_eq!(events[0]["section"], "main");
    assert_eq!(
        events[0]["source_call"],
        "MzEntryCharTextData\n55 7 3 63 2 3 ;"
    );
    assert_eq!(events[1]["arguments"], json!(["first", "second"]));
    assert_eq!(
        events[2]["arguments"],
        json!(["first", "message", "face", "bubble"])
    );
    assert_eq!(scan_calls(raw).unwrap(), events[3..]);
    for malformed in [
        b"MzEntryCharTextData 55 7 3\0".as_slice(),
        b"MzEntryCharTextData 55 7 ;\0",
        b"MzEntryCharTextData ;\0",
        b"MzEntryCharTextData chosen 7 3 ;\0",
        b"MzGetCharTextData first message face\0",
    ] {
        assert!(scan_commands(malformed, true).is_err());
    }
}
#[test]
fn retains_exit_direction_and_following_offscreen_text_calls() {
    let raw = b": scene\nMzExitChr 1\nMzExitChrOpp 0\nMzExitChrFall 1\nMzSetText 2 12 1 4 0 0\0";
    let events = scan_commands(raw, true).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[1]["command"], "MzExitChrOpp");
    assert_eq!(events[1]["arguments"], json!(["0"]));
    assert_eq!(events[2]["command"], "MzExitChrFall");
    assert_eq!(events[2]["arguments"], json!(["1"]));
    assert_eq!(
        events[3]["arguments"],
        json!(["2", "12", "1", "4", "0", "0"])
    );
    assert_eq!(scan_calls(raw).unwrap(), events[3..]);
}
#[test]
fn finds_multiple_calls_per_line_and_preserves_symbolic_arguments() {
    let raw = b": main\r\nMzSetText 0 1 0 3 0 0 MzWaitText MzSetText 2 chosen 0 bubble 0 0\0";
    let calls = scan_calls(raw).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["byte_offset"], 8);
    assert_eq!(calls[1]["arguments"][1], "chosen");
    let mtx = text::Mtx {
        header: vec![],
        group_first_records: vec![0, 1],
        records: vec![
            text::Record {
                offset: 16,
                units: vec![0xffff],
            },
            text::Record {
                offset: 18,
                units: vec![0xffff],
            },
        ],
    };
    assert!(candidate_record(&calls[0], &mtx).is_err()); // group 0 has only record 0
    assert_eq!(candidate_record(&calls[1], &mtx).unwrap(), None);
    assert!(scan_calls(b"MzSetText 0 1\0").is_err());
    assert!(scan_calls(b"MzSetText 0 0 0 0 0 0\0hidden\0").is_err());
}
