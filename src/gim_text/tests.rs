use super::*;

#[test]
fn colours_come_from_the_rectangle_bands() {
    let dark = [10, 10, 10, 255];
    let light = [250, 250, 250, 255];
    let clear = [0, 0, 0, 0];
    let cols = vec![(dark, 6), (light, 4), (clear, 3)];
    assert_eq!(pick(&cols, None, true).unwrap(), dark);
    assert_eq!(pick(&cols, None, false).unwrap(), light);
    assert_eq!(pick(&cols, Some([240, 240, 240]), true).unwrap(), light);
    assert_eq!(first_max(&[(dark, 2), (light, 2)]), Some(dark));
}
