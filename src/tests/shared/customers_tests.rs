use super::*;

fn person(name: &str) -> Customer {
    Customer {
        name: name.into(),
        phone: "555-0100".into(),
        address: "1 Example Lane".into(),
        ..Default::default()
    }
}

#[test]
fn creates_and_updates_without_mutating_original() {
    let original = Directory::default();
    let (created, first) = original.save_customer(&person("Example Person")).unwrap();
    assert!(original.customers.is_empty());
    assert_eq!(first.number, "C000001");
    let (second, other) = created.save_customer(&person("Example Person")).unwrap();
    assert_ne!(first.id, other.id);
    let changed = Customer {
        phone: "555-0101".into(),
        ..first.clone()
    };
    let (updated, saved) = second.save_customer(&changed).unwrap();
    assert_eq!(saved.id, first.id);
    assert_eq!(updated.customers.len(), 2);
    assert_eq!(second.customers[0].phone, "555-0100");
}

#[test]
fn searches_all_approved_fields_and_distinguishes_duplicate_names() {
    let (directory, first) = Directory::default()
        .save_customer(&person("Example Person"))
        .unwrap();
    let (directory, _) = directory
        .save_customer(&Customer {
            address: "2 Example Lane".into(),
            ..person("Example Person")
        })
        .unwrap();
    assert_eq!(directory.search_customers("EXAMPLE person").len(), 2);
    assert_eq!(directory.search_customers(&first.number).len(), 1);
    assert_eq!(directory.search_customers("5550100").len(), 2);
    assert_eq!(directory.search_customers("2 Example Lane").len(), 1);
    assert!(directory.search_customers("absent").is_empty());
    assert!(directory.search_customers("").is_empty());
}

#[test]
fn invalid_updates_and_duplicate_numbers_preserve_directory() {
    let (directory, first) = Directory::default()
        .save_customer(&person("Example"))
        .unwrap();
    for invalid in [
        Customer {
            name: "".into(),
            ..first.clone()
        },
        Customer {
            name: "Bad\nName".into(),
            ..first.clone()
        },
        Customer {
            phone: "x".repeat(41),
            ..first.clone()
        },
        Customer {
            id: 999,
            ..first.clone()
        },
    ] {
        assert!(directory.save_customer(&invalid).is_err());
    }
    assert!(
        directory
            .save_customer(&Customer {
                number: first.number.clone(),
                ..person("Other")
            })
            .is_err()
    );
    assert_eq!(directory.customers.len(), 1);
}

#[test]
fn business_identity_is_separate_and_inactive_accounts_remain_findable() {
    let directory = Directory::default();
    let (directory, business) = directory
        .save_account(&BusinessAccount {
            name: "Example Business".into(),
            active: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(business.number, "A000001");
    assert!(directory.customers.is_empty());
    assert_eq!(directory.search_accounts("EXAMPLE").len(), 1);
    let (directory, disabled) = directory
        .save_account(&BusinessAccount {
            active: false,
            ..business.clone()
        })
        .unwrap();
    assert!(!disabled.active);
    assert_eq!(directory.search_accounts(&business.number).len(), 1);
    assert!(
        directory
            .save_account(&BusinessAccount {
                id: 99,
                ..business.clone()
            })
            .is_err()
    );
    assert!(
        directory
            .save_account(&BusinessAccount {
                number: business.number.clone(),
                ..Default::default()
            })
            .is_err()
    );
    assert!(directory.search_accounts("").is_empty());
}

#[test]
fn scanner_always_returns_review_and_never_changes_directory() {
    let directory = Directory::default();
    let review = directory.scan_fixture(ScanFixture::Success).unwrap();
    assert_eq!(review.customer.id, 0);
    assert!(review.matches.is_empty());
    assert!(directory.customers.is_empty());
    let (directory, saved) = directory.save_customer(&review.customer).unwrap();
    let duplicate = directory.scan_fixture(ScanFixture::Duplicate).unwrap();
    assert_eq!(duplicate.matches, vec![saved.id]);
    assert!(directory.scan_fixture(ScanFixture::Unreadable).is_err());
    assert!(directory.scan_fixture(ScanFixture::Disconnected).is_err());
    assert_eq!(directory.customers.len(), 1);
    let json = serde_json::to_string(&duplicate).unwrap();
    assert!(!json.contains("license"));
    assert!(!json.contains("dob"));
    assert!(!json.contains("barcode"));
}

#[test]
fn directory_roundtrip_retains_sequence_and_supports_missing_sequence_fields() {
    let (directory, _) = Directory::default()
        .save_customer(&person("First"))
        .unwrap();
    let restored: Directory =
        serde_json::from_str(&serde_json::to_string(&directory).unwrap()).unwrap();
    let (_, second) = restored.save_customer(&person("Second")).unwrap();
    assert_eq!(second.number, "C000002");
    let restored: Directory = serde_json::from_str("{\"customers\":[],\"accounts\":[]}").unwrap();
    assert_eq!(restored, Directory::default());
}

#[test]
fn sequence_collision_unknown_id_and_exhaustion_fail_safely() {
    let (directory, first) = Directory::default()
        .save_customer(&person("First"))
        .unwrap();
    let stale = Directory {
        next_customer_id: 1,
        ..directory.clone()
    };
    let (_, second) = stale.save_customer(&person("Second")).unwrap();
    assert_ne!(second.id, first.id);
    let exhausted = Directory {
        next_customer_id: u64::MAX,
        ..Directory::default()
    };
    assert!(exhausted.save_customer(&person("No room")).is_err());
    assert!(
        directory
            .save_customer(&Customer {
                number: "different".into(),
                ..first
            })
            .is_err()
    );
}

#[test]
fn restored_directory_rejects_corrupt_identity_and_fields() {
    let (directory, customer) = Directory::default()
        .save_customer(&person("Example"))
        .unwrap();
    directory.validate().unwrap();
    for invalid in [
        Directory {
            customers: vec![customer.clone(), customer.clone()],
            ..Directory::default()
        },
        Directory {
            customers: vec![Customer {
                id: 0,
                ..customer.clone()
            }],
            ..Directory::default()
        },
        Directory {
            customers: vec![Customer {
                name: "bad\nname".into(),
                ..customer.clone()
            }],
            ..Directory::default()
        },
        Directory {
            customers: vec![Customer {
                number: "".into(),
                ..customer.clone()
            }],
            ..Directory::default()
        },
        Directory {
            customers: vec![
                customer.clone(),
                Customer {
                    id: 2,
                    number: customer.number.to_lowercase(),
                    ..customer.clone()
                },
            ],
            ..Directory::default()
        },
        Directory {
            accounts: vec![BusinessAccount {
                id: 1,
                number: "A1".into(),
                name: "Bad\rBusiness".into(),
                active: true,
            }],
            ..Directory::default()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn customer_boundaries_trim_validate_lengths_and_numbers() {
    let original = Directory::default();
    let (_, saved) = original.save_customer(&person("  Example  ")).unwrap();
    assert_eq!(saved.name, "Example");
    for invalid in [
        Customer {
            number: "not a number".into(),
            ..person("Example")
        },
        Customer {
            number: "x".repeat(33),
            ..person("Example")
        },
        Customer {
            name: "x".repeat(121),
            ..person("Example")
        },
        Customer {
            address: "x".repeat(241),
            ..person("Example")
        },
        Customer {
            notice: "x".repeat(501),
            ..person("Example")
        },
    ] {
        assert!(original.save_customer(&invalid).is_err());
    }
    assert!(original.customers.is_empty());
    assert!(original.search_customers(&"x".repeat(241)).is_empty());
}

#[test]
fn account_numbers_are_unique_and_sequences_survive_stale_metadata() {
    let account = BusinessAccount {
        name: "Example".into(),
        active: true,
        ..Default::default()
    };
    let (directory, first) = Directory::default().save_account(&account).unwrap();
    let stale = Directory {
        next_account_id: 1,
        ..directory.clone()
    };
    let (directory, second) = stale.save_account(&account).unwrap();
    assert_eq!(second.number, "A000002");
    directory.validate().unwrap();
    assert!(
        directory
            .save_account(&BusinessAccount {
                number: first.number.to_lowercase(),
                ..account.clone()
            })
            .is_err()
    );
    assert!(
        directory
            .save_account(&BusinessAccount {
                number: "changed".into(),
                ..first.clone()
            })
            .is_err()
    );
    assert!(directory.search_accounts(&"x".repeat(121)).is_empty());
    let exhausted = Directory {
        next_account_id: u64::MAX,
        ..Directory::default()
    };
    assert!(exhausted.save_account(&account).is_err());
    for invalid in [
        Directory {
            accounts: vec![first.clone(), first.clone()],
            ..Directory::default()
        },
        Directory {
            accounts: vec![BusinessAccount {
                id: 0,
                ..first.clone()
            }],
            ..Directory::default()
        },
        Directory {
            accounts: vec![BusinessAccount {
                number: "".into(),
                ..first.clone()
            }],
            ..Directory::default()
        },
        Directory {
            accounts: vec![
                first.clone(),
                BusinessAccount {
                    id: 2,
                    number: first.number.to_lowercase(),
                    ..first.clone()
                },
            ],
            ..Directory::default()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
}
