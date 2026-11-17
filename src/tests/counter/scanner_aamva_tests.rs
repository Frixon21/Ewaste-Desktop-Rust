use super::*;

pub(crate) fn fields() -> String {
    "DAQSYNTHETIC-PRIVATE-ID\nDBB01011990\nDCSSAMPLE\nDACJANE\nDDEN\nDDFN\nDDGN\nDAG100 SYNTHETIC WAY\nDAITESTVILLE\nDAJCA\nDAK000000000\nDCGUSA".into()
}

pub(crate) fn fixture(version: u8, kind: &str, fields: &str) -> String {
    let body = format!("{kind}{fields}\r");
    format!(
        "@\n\u{1e}\rANSI 636000{version:02}0001{kind}0031{:04}{body}",
        body.chars().count()
    )
}

#[test]
fn supports_declared_versions_and_only_returns_reviewed_name_address() {
    for version in 9..=12 {
        for kind in ["DL", "ID"] {
            let candidate = decode_aamva_keyboard(&fixture(version, kind, &fields())).unwrap();
            assert_eq!(candidate.name, "JANE SAMPLE");
            assert_eq!(
                candidate.address,
                "100 SYNTHETIC WAY, TESTVILLE, CA 000000000, USA"
            );
            assert!(!format!("{candidate:?}").contains("SYNTHETIC-PRIVATE-ID"));
            assert!(!format!("{candidate:?}").contains("01011990"));
        }
    }
}

#[test]
fn latin1_name_and_optional_middle_suffix_address_line_are_preserved() {
    let fields = fields()
        .replace("DACJANE", "DACJOSÉ\nDADMÁRIE\nDCUJR")
        .replace("DAG100 SYNTHETIC WAY", "DAG100 SYNTHETIC WAY\nDAHAPT 2");
    let candidate = decode_aamva_keyboard(&fixture(12, "DL", &fields)).unwrap();
    assert_eq!(candidate.name, "JOSÉ MÁRIE SAMPLE JR");
    assert!(candidate.address.contains("WAY, APT 2, TESTVILLE"));
}

#[test]
fn documented_keyboard_newline_translation_reconstructs_original_byte_offsets() {
    let canonical = fixture(12, "DL", &fields());
    let normalized = canonical
        .replacen("@\n\u{1e}\r", "@\n\n", 1)
        .replace('\n', "\r\n")
        .replace('\r', "\n");
    // Explicit keyboard profile: RS dropped, Enter emits LF, field line endings CRLF.
    let prefix = "@\n\nANSI ";
    let translated = format!("{prefix}{}", &canonical[9..]).replace("\nD", "\r\nD");
    let translated = translated.strip_suffix('\r').unwrap().to_owned() + "\n";
    assert_eq!(
        decode_aamva_keyboard(&canonical),
        decode_aamva_keyboard(&translated)
    );
    assert!(decode_aamva_keyboard(&normalized).is_err());
}

#[test]
fn malformed_header_version_offsets_lengths_and_attached_scans_fail_without_echo() {
    let valid = fixture(12, "DL", &fields());
    for invalid in [
        valid[..20].to_owned(),
        valid.replacen("636000", "ABCDEF", 1),
        fixture(8, "DL", &fields()),
        fixture(13, "DL", &fields()),
        valid.replacen("DL0031", "DL0030", 1),
        valid.replacen("DL0031", "DL9999", 1),
        valid[..valid.len() - 1].to_owned(),
        format!("{valid}{valid}"),
        valid.replace("ANSI ", "ANSI\t"),
        valid.replacen("0001DL", "0000DL", 1),
    ] {
        let error = decode_aamva_keyboard(&invalid).unwrap_err();
        assert!(!error.to_string().contains("SYNTHETIC-PRIVATE-ID"));
        assert!(!format!("{error:?}").contains("01011990"));
    }
}

#[test]
fn field_ids_only_at_boundaries_and_duplicate_or_truncated_selected_values_fail() {
    for fields in [
        fields().replace("DCSSAMPLE\n", "ZZZembeddedDCSSAMPLE\n"),
        format!("{}\nDCSOTHER", fields()),
        format!("{}\nDACJANE", fields()),
        format!("{}\nDDET", fields()),
        fields().replace("DAG100 SYNTHETIC WAY", "DAG"),
        fields().replace("DACJANE", "DAC🙂"),
        fields().replace("DAJCA", "DAJCAL"),
    ] {
        assert!(decode_aamva_keyboard(&fixture(12, "DL", &fields)).is_err());
    }
}

#[test]
fn required_fields_and_unknown_keyboard_separators_never_guess() {
    assert!(decode_aamva_keyboard(&fixture(12, "DL", &fields().replace("\nDACJANE", ""))).is_err());
    assert!(decode_aamva_keyboard(&fixture(12, "DL", &fields()).replace('\n', "\t")).is_err());
    assert!(decode_aamva_keyboard(&"x".repeat(8_193)).is_err());
    assert!(decode_aamva_keyboard("").is_err());
}

fn multiple_segments(segments: &[(&str, &str)]) -> String {
    let mut offset = 21 + segments.len() * 10;
    let mut designators = String::new();
    let mut data = String::new();
    for (kind, fields) in segments {
        let body = format!("{kind}{fields}\r");
        let length = body.chars().count();
        designators.push_str(&format!("{kind}{offset:04}{length:04}"));
        data.push_str(&body);
        offset += length;
    }
    format!(
        "@\n\u{1e}\rANSI 6360001200{:02}{designators}{data}",
        segments.len()
    )
}

#[test]
fn all_declared_subfiles_are_bounded_and_unused_jurisdiction_text_is_not_retained() {
    let payload = multiple_segments(&[
        ("DL", &fields()),
        ("ZV", "ZVASYNTHETIC-JURISDICTION-SECRET"),
    ]);
    let candidate = decode_aamva_keyboard(&payload).unwrap();
    assert_eq!(candidate.name, "JANE SAMPLE");
    assert!(!format!("{candidate:?}").contains("JURISDICTION"));
    let translated = payload
        .replacen("@\n\u{1e}\r", "@\n\u{1e}\n", 1)
        .replace("\nD", "\r\nD")
        .replace("\rZVZ", "\nZVZ");
    assert_eq!(
        decode_aamva_keyboard(&payload),
        decode_aamva_keyboard(&translated)
    );
}

#[test]
fn overlapping_gapped_duplicate_or_competing_identity_subfiles_are_rejected() {
    let payload = multiple_segments(&[("DL", &fields()), ("ZV", "ZVASYNTHETIC")]);
    let mut overlapping = payload.clone().into_bytes();
    overlapping[33..37].copy_from_slice(b"0041");
    let mut gapped = payload.clone().into_bytes();
    gapped[23..27].copy_from_slice(b"0042");
    for invalid in [
        String::from_utf8(overlapping).unwrap(),
        String::from_utf8(gapped).unwrap(),
        multiple_segments(&[("DL", &fields()), ("ID", &fields())]),
        multiple_segments(&[("DL", &fields()), ("DL", &fields())]),
        multiple_segments(&[("ZV", "ZVASYNTHETIC")]),
        multiple_segments(&[("AB", &fields())]),
        payload + "SYNTHETIC-ATTACHED-RAW",
    ] {
        assert!(decode_aamva_keyboard(&invalid).is_err());
    }
}

#[test]
fn every_incomplete_scan_fails_and_errors_never_expose_raw_or_partial_fields() {
    let payload = fixture(12, "DL", &fields());
    for end in 0..payload.len() {
        let error = decode_aamva_keyboard(&payload[..end]).unwrap_err();
        assert!(!error.to_string().contains("JANE"));
        assert!(!format!("{error:?}").contains("PRIVATE-ID"));
    }
    for fields in [
        fields().replace("DCSSAMPLE", "DCSNONE"),
        fields().replace("DACJANE", "DACunavl"),
        fields().replace("DAG100 SYNTHETIC WAY", &format!("DAG{}", "X".repeat(36))),
        fields().replace("DAK000000000", "DAK0000000000"),
        fields().replace("DCGUSA", "DCGZZZ"),
        fields().replace("DACJANE", "DACJA\u{85}NE"),
        fields().replace("DCSSAMPLE", "DCSSAMPLE\nDDFU"),
    ] {
        assert!(decode_aamva_keyboard(&fixture(12, "DL", &fields)).is_err());
    }
}

#[test]
fn accepts_only_the_documented_header_transports_and_preserves_selected_values() {
    let canonical = fixture(12, "DL", &fields());
    for header in [
        "@\n\u{1e}\rANSI ",
        "@\n\u{1e}\nANSI ",
        "@\n\rANSI ",
        "@\n\nANSI ",
    ] {
        let transported = format!("{header}{}", &canonical[9..]);
        assert_eq!(
            decode_aamva_keyboard(&canonical),
            decode_aamva_keyboard(&transported)
        );
    }
    for header in [
        "@ANSI ",
        "@\r\nANSI ",
        "@\tANSI ",
        "ANSI ",
        "@@\n\u{1e}\rANSI ",
    ] {
        assert!(decode_aamva_keyboard(&format!("{header}{}", &canonical[9..])).is_err());
    }
}

#[test]
fn missing_name_truncation_flags_require_manual_review() {
    for missing in ["DDE", "DDF", "DDG"] {
        let incomplete = fields()
            .lines()
            .filter(|line| !line.starts_with(missing))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            decode_aamva_keyboard(&fixture(12, "DL", &incomplete)).is_err(),
            "{missing}"
        );
    }
}
