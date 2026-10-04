use super::*;

#[test]
fn same_cell_regions_must_be_disjoint_before_rendering() {
    let panel = |id, rect| {
        serde_json::from_value::<Panel>(json!({
            "cell_id":id,"edit_rect":rect,"clear":[0,0,0,0],
            "outline":[0,0,0,255],"centered":true,"lines":[]
        }))
        .unwrap()
    };
    assert!(validate_regions(&[panel(1, [0, 0, 20, 16]), panel(1, [20, 0, 20, 16])]).is_ok());
    assert!(validate_regions(&[panel(1, [0, 0, 20, 16]), panel(1, [19, 0, 20, 16])]).is_err());
    assert!(validate_regions(&[panel(1, [0, 0, 20, 16]), panel(2, [0, 0, 20, 16])]).is_ok());
    assert!(validate_regions(&[panel(1, [usize::MAX, 0, 20, 16])]).is_err());
}
