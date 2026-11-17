use super::*;

fn rule(id: &str, from: u64, until: Option<u64>) -> PricingRule {
    PricingRule {
        id: id.into(),
        code: "803".into(),
        name: "Other E-waste".into(),
        unit: Some("lb".into()),
        rate_cents: Some(40),
        minimum_cents: Some(500),
        effective_from: from,
        effective_until: until,
        provisional: false,
        classification_required: false,
        blank_allowed: true,
    }
}

#[test]
fn default_training_rules_preserve_supported_fixtures_and_flag_other_materials() {
    let catalog = Catalog::default();
    catalog.validate().unwrap();
    let tv = catalog.rule_for("801", 0).unwrap();
    assert_eq!(tv.rate_cents, Some(500));
    assert_eq!(tv.unit.as_deref(), Some("each"));
    assert!(tv.classification_required);
    assert!(!tv.provisional);
    assert!(tv.placeholder_note().is_none());
    let ewaste = catalog.rule_for("803", u64::MAX).unwrap();
    assert_eq!(ewaste.rate_cents, Some(40));
    assert_eq!(ewaste.minimum_cents, Some(500));
    assert!(ewaste.blank_allowed);
    assert!(!ewaste.provisional);
    assert!(ewaste.placeholder_note().is_some());
    assert!(
        catalog
            .rules
            .iter()
            .filter(|r| !["801", "803"].contains(&r.code.as_str()))
            .all(|r| r.provisional)
    );
    assert!(catalog.rule_for("absent", 0).is_none());
    assert!(
        catalog
            .rules
            .iter()
            .filter(|rule| rule.provisional)
            .all(|rule| rule.placeholder_note().is_some())
    );
    assert!(TRAINING_NOTICE.contains("per line"));
}

#[test]
fn adjacent_effective_dates_and_gaps_select_one_rule() {
    let catalog = Catalog {
        rules: vec![
            rule("old", 10, Some(20)),
            rule("new", 20, Some(30)),
            rule("later", 40, None),
        ],
    };
    catalog.validate().unwrap();
    assert!(catalog.rule_for("803", 9).is_none());
    assert_eq!(catalog.rule_for("803", 10).unwrap().id, "old");
    assert_eq!(catalog.rule_for("803", 19).unwrap().id, "old");
    assert_eq!(catalog.rule_for("803", 20).unwrap().id, "new");
    assert!(catalog.rule_for("803", 30).is_none());
    assert_eq!(catalog.rule_for("803", 40).unwrap().id, "later");
}

#[test]
fn overlaps_duplicate_ids_and_invalid_intervals_are_rejected() {
    for invalid in [
        Catalog {
            rules: vec![rule("same", 0, Some(10)), rule("same", 10, None)],
        },
        Catalog {
            rules: vec![rule("old", 0, Some(20)), rule("overlap", 19, None)],
        },
        Catalog {
            rules: vec![rule("open", 0, None), rule("overlap", 100, Some(200))],
        },
        Catalog {
            rules: vec![rule("invalid", 20, Some(20))],
        },
        Catalog {
            rules: vec![rule("invalid", 20, Some(10))],
        },
    ] {
        assert!(invalid.validate().is_err());
    }
    let separate = Catalog {
        rules: vec![
            rule("ewaste", 0, None),
            PricingRule {
                code: "801".into(),
                ..rule("tv", 0, None)
            },
        ],
    };
    separate.validate().unwrap();
}

#[test]
fn immutable_edits_keep_id_and_validate_the_whole_schedule() {
    let original = Catalog {
        rules: vec![rule("old", 0, Some(20))],
    };
    let updated = original.save_rule(&rule("new", 20, None)).unwrap();
    assert_eq!(original.rules.len(), 1);
    assert_eq!(updated.rules.len(), 2);
    let changed = PricingRule {
        rate_cents: Some(50),
        ..rule("new", 20, None)
    };
    let revised = updated.save_rule(&changed).unwrap();
    assert_eq!(revised.rule_for("803", 20).unwrap().rate_cents, Some(50));
    assert_eq!(updated.rule_for("803", 20).unwrap().rate_cents, Some(40));
    assert!(updated.save_rule(&rule("overlap", 1, Some(5))).is_err());
    assert!(
        updated
            .save_rule(&PricingRule {
                code: "801".into(),
                ..changed
            })
            .is_err()
    );
}

#[test]
fn invalid_prices_units_and_unbounded_fields_are_rejected() {
    for invalid in [
        PricingRule {
            id: "".into(),
            ..rule("valid", 0, None)
        },
        PricingRule {
            name: "bad\nname".into(),
            ..rule("valid", 0, None)
        },
        PricingRule {
            unit: Some("kg".into()),
            ..rule("valid", 0, None)
        },
        PricingRule {
            rate_cents: Some(-1),
            ..rule("valid", 0, None)
        },
        PricingRule {
            minimum_cents: Some(-1),
            ..rule("valid", 0, None)
        },
        PricingRule {
            unit: None,
            ..rule("valid", 0, None)
        },
        PricingRule {
            rate_cents: None,
            ..rule("valid", 0, None)
        },
        PricingRule {
            code: "x".repeat(33),
            ..rule("valid", 0, None)
        },
    ] {
        assert!(
            Catalog {
                rules: vec![invalid]
            }
            .validate()
            .is_err()
        );
    }
    Catalog {
        rules: vec![PricingRule {
            unit: None,
            rate_cents: None,
            minimum_cents: None,
            ..rule("manual", 0, None)
        }],
    }
    .validate()
    .unwrap();
    Catalog {
        rules: vec![PricingRule {
            rate_cents: Some(0),
            minimum_cents: Some(0),
            ..rule("free", 0, None)
        }],
    }
    .validate()
    .unwrap();
}

#[test]
fn serialized_catalog_retains_dated_rules() {
    let catalog = Catalog::default();
    let restored: Catalog =
        serde_json::from_str(&serde_json::to_string(&catalog).unwrap()).unwrap();
    assert_eq!(restored, catalog);
    restored.validate().unwrap();
}
