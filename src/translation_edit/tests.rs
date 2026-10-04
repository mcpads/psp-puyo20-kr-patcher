use super::*;

#[test]
fn story_lines_follow_source_breaks_and_keep_end_span_empty() {
    let mut e = json!({
        "source_text": "あ<F881:0005><FFFD>い<F880:0001>う<F813>",
        "segments": ["ああ", "", "いい", "うう", ""],
        "expected_text": "ああ\nいいうう"
    });
    apply_story(&mut e, "첫 줄/둘째 줄이야").unwrap();
    let segs = strings(&e["segments"]).unwrap();
    assert_eq!(segs.len(), 5);
    assert_eq!(segs[4], "");
    assert_eq!(e["expected_text"], "첫 줄\n둘째 줄이야");
    assert_eq!(segs[2..4].concat(), "둘째 줄이야");
    assert!(apply_story(&mut e, "한 줄뿐").is_err());
}

#[test]
fn academy_edits_stay_inside_colour_spans() {
    let mut e = json!({
        "source_text": "a<FFFD><F800:0001>b<F801><FFFF>",
        "segments": ["예를 들어", "", "동시에", "", ""],
        "expected_text": "예를 들어<FFFD><F800:0001>동시에<F801><FFFF>"
    });
    apply_academy(&mut e, "예를 들어/동시에", "예를 들어/동시 지우기").unwrap();
    assert_eq!(
        e["expected_text"],
        "예를 들어<FFFD><F800:0001>동시 지우기<F801><FFFF>"
    );
    assert!(apply_academy(&mut e, "예를 들어/동시 지우기", "예를 들어/동시 지우기/더").is_err());
}

#[test]
fn deletions_work_without_inline_source() {
    let mut e = json!({"segments": ["그", " ", "사건은."], "expected_text": "그 사건은."});
    delete_only(&mut e, "그 사건은").unwrap();
    assert_eq!(e["expected_text"], "그 사건은");
    assert_eq!(strings(&e["segments"]).unwrap().concat(), "그 사건은");
    assert!(delete_only(&mut e, "그 사건이").is_err());
}

#[test]
fn lines_keep_indentation_and_trailing_empty_lines() {
    let mut e = json!({"lines": ["  제한 시간 안에", "  연쇄의 씨앗을", ""]});
    apply_lines(&mut e, "제한 시간 안에/연쇄 씨앗을").unwrap();
    assert_eq!(e["lines"], json!(["  제한 시간 안에", "  연쇄 씨앗을", ""]));
}

#[test]
fn layout_and_key_order_are_preserved() {
    let text = "{\n \"z\": 1,\n \"a\": [\n  \"x\"\n ]\n}\n";
    let mut doc: Value = serde_json::from_str(text).unwrap();
    doc["a"][0] = json!("y");
    assert_eq!(
        dump_like(text, &doc).unwrap(),
        "{\n \"z\": 1,\n \"a\": [\n  \"y\"\n ]\n}\n"
    );
}
