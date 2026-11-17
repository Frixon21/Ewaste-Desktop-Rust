use super::*;

#[test]
fn capture_is_bounded_clearable_and_debug_never_contains_payload() {
    let mut capture = CaptureSession::new();
    capture.append_text("SYNTHETIC-PRIVATE-ID").unwrap();
    assert_eq!(capture.len(), 20);
    assert!(!format!("{capture:?}").contains("SYNTHETIC-PRIVATE-ID"));
    capture.backspace();
    assert_eq!(capture.len(), 19);
    capture.clear();
    assert!(capture.is_empty());
    capture.append_text(&"x".repeat(MAX_CAPTURE_BYTES)).unwrap();
    assert_eq!(capture.append_text("x"), Err(ScanError::TooLong));
    assert!(capture.is_empty());
}

#[test]
fn backspace_keeps_complete_unicode_scalars_and_review_always_clears() {
    let mut capture = CaptureSession::new();
    capture.append_text("É🙂").unwrap();
    capture.backspace();
    assert_eq!(capture.len(), "É".len());
    assert!(capture.review().is_err());
    assert!(capture.is_empty());
    capture
        .append_text(&super::aamva::tests::fixture(
            12,
            "DL",
            &super::aamva::tests::fields(),
        ))
        .unwrap();
    let candidate = capture.review().unwrap();
    assert_eq!(candidate.name, "JANE SAMPLE");
    assert!(capture.is_empty());
    assert!(capture.review().is_err());
}

#[test]
fn candidate_validation_rejects_controls_missing_and_oversized_details() {
    for candidate in [
        ScanCandidate {
            name: " ".into(),
            address: "100 SYNTHETIC WAY".into(),
        },
        ScanCandidate {
            name: "JANE".into(),
            address: "".into(),
        },
        ScanCandidate {
            name: "JANE\nSAMPLE".into(),
            address: "100 SYNTHETIC WAY".into(),
        },
        ScanCandidate {
            name: "x".repeat(121),
            address: "100 SYNTHETIC WAY".into(),
        },
        ScanCandidate {
            name: "JANE".into(),
            address: "x".repeat(241),
        },
    ] {
        assert!(candidate.validate().is_err());
    }
}
