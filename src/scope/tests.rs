use super::*;
#[test]
fn visible_equivalence_does_not_hide_slot_mapping_uncertainty() {
    assert_eq!(
        member_kind(
            "a.snt",
            Some(&json!({"pixel_changes":[{"visible_equal":true}]})),
            false
        ),
        "graphics_visible_equal"
    );
    assert_eq!(
        member_kind("a.snt", Some(&json!({"versions":{}})), false),
        "graphics_slot_mapping_review"
    );
    assert_eq!(
        member_kind(
            "a.snt",
            Some(&json!({"pixel_changes":[{"visible_equal":true},{"visible_equal":false}]})),
            false
        ),
        "graphics_visible_changed"
    );
}
