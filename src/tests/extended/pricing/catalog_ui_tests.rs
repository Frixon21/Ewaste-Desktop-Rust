use super::*;

#[test]
fn utc_dates_validate_leap_years_and_supported_bounds() {
    assert_eq!(parse_utc_date("1970-01-01").unwrap(), 0);
    assert_eq!(parse_utc_date("1970-01-02").unwrap(), 86_400);
    assert_eq!(parse_utc_date("2000-02-29").unwrap(), 951_782_400);
    for invalid in [
        "1969-12-31",
        "2026-02-29",
        "2100-02-29",
        "2026-04-31",
        "2026-00-01",
        "2026-13-01",
        "2026-01-00",
        "2026-1-01",
        "2026/01/01",
        "10000-01-01",
        "2026-01-01x",
    ] {
        assert!(parse_utc_date(invalid).is_err(), "{invalid}");
    }
    for date in [
        "1970-01-01",
        "2000-02-29",
        "2026-11-16",
        "2400-02-29",
        "9999-12-31",
    ] {
        assert_eq!(
            format_utc_date(parse_utc_date(date).unwrap()).unwrap(),
            date
        );
    }
    assert!(format_utc_date(u64::MAX).is_err());
}

#[test]
fn editor_preserves_non_midnight_timestamps_unless_date_changes() {
    let mut rule = Catalog::default().rules[0].clone();
    rule.effective_from = parse_utc_date("2026-11-16").unwrap() + 123;
    let mut form = RuleForm::from_rule(&rule, false);
    assert_eq!(
        form.candidate().unwrap().effective_from,
        rule.effective_from
    );
    form.from = "2026-11-17".into();
    assert_eq!(
        form.candidate().unwrap().effective_from,
        parse_utc_date("2026-11-17").unwrap()
    );
}

#[test]
fn headless_catalog_window_renders_without_saving_and_acknowledges_failure() {
    let context = egui::Context::default();
    let catalog = Catalog::default();
    let mut editor = CatalogUi {
        open: true,
        ..Default::default()
    };
    let output = context.run(Default::default(), |context| {
        assert!(editor.show(context, &catalog).is_none());
    });
    assert!(!output.shapes.is_empty());
    let before = editor.form.as_ref().unwrap().rule.clone();
    editor.error("Simulated persistence failure".into());
    assert!(editor.open);
    assert!(editor.message.is_some());
    assert_eq!(editor.form.as_ref().unwrap().rule, before);
    editor.saved();
    assert!(!editor.open);
    assert!(editor.message.is_none());
}

#[test]
fn new_rule_and_invalid_dates_are_reviewable_without_changing_catalog() {
    let catalog = Catalog::default();
    let mut form = RuleForm::new();
    assert!(form.candidate().is_err());
    form.rule.id = "training-new-v1".into();
    form.rule.code = "NEW".into();
    form.rule.name = "Synthetic material".into();
    let candidate = form.candidate().unwrap();
    let revised = catalog.save_rule(&candidate).unwrap();
    assert_eq!(revised.rules.len(), catalog.rules.len() + 1);
    assert_eq!(catalog.rules.len(), 9);
    form.until = "2026-02-29".into();
    assert!(form.candidate().is_err());
}
