use super::shift_into_rows;

#[test]
fn display_fit_moves_complete_ink_without_trimming() {
    let mut cell = [0; 196];
    cell[14 + 2] = 17;
    cell[12 * 14 + 3] = 239;
    shift_into_rows(&mut cell, 12).unwrap();
    assert_eq!(cell[2], 17);
    assert_eq!(cell[11 * 14 + 3], 239);
    assert_eq!(cell.iter().map(|&a| u32::from(a)).sum::<u32>(), 256);
    let fitted = cell;
    shift_into_rows(&mut cell, 12).unwrap();
    assert_eq!(cell, fitted);
}

#[test]
fn display_fit_rejects_ink_taller_than_sprite() {
    let mut cell = [0; 196];
    cell[2] = 1;
    cell[12 * 14 + 3] = 255;
    let original = cell;
    assert!(shift_into_rows(&mut cell, 12).is_err());
    assert_eq!(cell, original);
}
