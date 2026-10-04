use super::*;
#[test]
fn rejects_overlap_and_protected_writes() {
    let w = ExpectedWrite {
        offset: 4,
        before: vec![1, 2],
        after: vec![3, 4],
    };
    assert!(validate(&[w.clone(), w.clone()], 10, std::slice::from_ref(&(0..10))).is_err());
    assert!(validate(std::slice::from_ref(&w), 10, std::slice::from_ref(&(0..4))).is_err());
    assert!(validate(std::slice::from_ref(&w), 10, &[0..5, 6..10]).is_err());
    assert!(validate(&[w], 10, &[0..2, 4..6]).is_ok());
}
#[test]
fn audits_real_write_and_rejects_bad_expected() {
    let mut src = tempfile::tempfile().unwrap();
    src.write_all(b"abcdefgh").unwrap();
    let mut dst = tempfile::tempfile().unwrap();
    let p = SourceProfile {
        size_bytes: 8,
        sha256: sha256(b"abcdefgh"),
    };
    let mut w = ExpectedWrite {
        offset: 2,
        before: b"cd".to_vec(),
        after: b"XY".to_vec(),
    };
    let (h, _) = apply_and_audit(
        &mut src,
        &mut dst,
        &p,
        &[w.clone()],
        std::slice::from_ref(&(2..4)),
        8,
    )
    .unwrap();
    assert_eq!(h, sha256(b"abXYefgh"));
    // A shorter output keeps the audited prefix and drops the verified tail.
    let (h, _) = apply_and_audit(
        &mut src,
        &mut dst,
        &p,
        &[w.clone()],
        std::slice::from_ref(&(2..4)),
        6,
    )
    .unwrap();
    assert_eq!(h, sha256(b"abXYef"));
    assert!(
        apply_and_audit(
            &mut src,
            &mut dst,
            &p,
            &[w.clone()],
            std::slice::from_ref(&(2..4)),
            3
        )
        .is_err()
    );
    w.before = b"zz".to_vec();
    assert!(
        apply_and_audit(
            &mut src,
            &mut dst,
            &p,
            &[w],
            std::slice::from_ref(&(2..4)),
            8
        )
        .is_err()
    );
}
