use super::*;
#[test]
fn protected_pixels_and_bounds() {
    assert!(check_pixels(&[0; 8], &[1; 8], &[true, false]).is_err());
    assert_eq!(
        check_pixels(&[0; 8], &[1, 1, 1, 1, 0, 0, 0, 0], &[true, false]).unwrap(),
        1
    );
    assert!(rect(&json!({"rect_xywh":[3,3,2,2]}), 4, 4).is_err());
}
