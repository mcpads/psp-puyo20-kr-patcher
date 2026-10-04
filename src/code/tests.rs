use super::*;

#[test]
fn uses_psp_addresses_and_rejects_invalid_code_windows() {
    // beql $zero, $zero, +1: branch-likely must retain annulled delay-slot semantics.
    let decoded = inspect_window(&0x5000_0001u32.to_le_bytes(), 0x0880_0000).unwrap();
    assert!(
        decoded[0]["control_flow"]
            .as_str()
            .unwrap()
            .contains("WhenTransferOccurs")
    );
    assert_eq!(decoded[0]["address"], 0x0880_0000);
    assert!(inspect_window(&[0; 4], 0x0880_0001).is_err());
    assert!(inspect_window(&[0; 3], 0x0880_0000).is_err());
    assert!(inspect_window(&[0; 8], 0xffff_fffc).is_err());
    let unknown = inspect_window(&0xffff_ffffu32.to_le_bytes(), 0x0880_0000).unwrap();
    assert!(unknown[0].get("decode_error").is_some());
    assert!(unknown[0].get("instruction").is_none());
}
