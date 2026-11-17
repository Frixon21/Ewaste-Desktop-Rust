use super::*;
use crate::customers::Customer;
use crate::domain::{Payment, State};

fn customer(id: u64) -> Customer {
    Customer {
        id,
        number: format!("C{id}"),
        name: format!("Synthetic Person {id}"),
        address: "1 Example Lane".into(),
        phone: "555-0100".into(),
        ..Default::default()
    }
}

fn documented_source() -> SourceIntake {
    SourceIntake {
        category: SourceCategory::Residential,
        reason: IntakeReason::EmptyingGarage,
        delivery: DeliveryMode::DropOff,
        source_name: "Synthetic Source".into(),
        source_address: "2 Example Lane".into(),
        ..Default::default()
    }
}

fn material() -> MaterialIntake {
    MaterialIntake {
        class: MaterialClass::NonCrtDisplay,
        condition: MaterialCondition::Intact,
        reporting_units: Some(1),
        ..Default::default()
    }
}

#[test]
fn old_empty_payloads_deserialize_as_unreviewed_without_invented_data() {
    let source: SourceIntake = serde_json::from_str("{}").unwrap();
    let material: MaterialIntake = serde_json::from_str("{}").unwrap();
    assert_eq!(source, SourceIntake::default());
    assert_eq!(material, MaterialIntake::default());
    assert_eq!(source.category, SourceCategory::Unspecified);
    assert_eq!(material.actual_weight_thousandths, None);
    source.validate().unwrap();
    material.validate().unwrap();
}

#[test]
fn normalized_fields_reject_controls_limits_and_invalid_measurements() {
    let source = SourceIntake {
        source_name: "  Example  ".into(),
        ..Default::default()
    };
    assert_eq!(source.normalized().unwrap().source_name, "Example");
    assert_eq!(source.source_name, "  Example  ");
    for invalid in [
        SourceIntake {
            source_name: "bad\nname".into(),
            ..Default::default()
        },
        SourceIntake {
            source_address: "x".repeat(241),
            ..Default::default()
        },
        SourceIntake {
            phone: "x".repeat(41),
            ..Default::default()
        },
        SourceIntake {
            linked_customer_id: Some(0),
            ..Default::default()
        },
    ] {
        assert!(invalid.normalized().is_err());
    }
    for invalid in [
        MaterialIntake {
            reporting_units: Some(0),
            ..Default::default()
        },
        MaterialIntake {
            reporting_units: Some(1_000_000),
            ..Default::default()
        },
        MaterialIntake {
            actual_weight_thousandths: Some(0),
            ..Default::default()
        },
        MaterialIntake {
            actual_weight_thousandths: Some(-1),
            ..Default::default()
        },
        MaterialIntake {
            actual_weight_thousandths: Some(1_000_000_000),
            ..Default::default()
        },
        MaterialIntake {
            notes: "bad\rnote".into(),
            ..Default::default()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn mixed_load_documentary_units_and_weight_never_reprice_billed_materials() {
    let mut ticket = Ticket::new(1);
    ticket.add_line("801", "2", "").unwrap();
    ticket.add_line("803", "20", "").unwrap();
    let original = ticket.clone();
    let classified = ticket.with_material_intake(0, &material()).unwrap();
    let classified = classified
        .with_material_intake(
            1,
            &MaterialIntake {
                class: MaterialClass::LooseBattery,
                condition: MaterialCondition::Damaged,
                reporting_units: Some(17),
                actual_weight_thousandths: Some(3_500),
                notes: "Synthetic mixed stream review".into(),
            },
        )
        .unwrap();
    assert_eq!(classified.total(), original.total());
    assert_eq!(classified.lines[0].quantity, "2");
    assert_eq!(classified.lines[1].quantity, "20");
    assert_eq!(
        classified.lines[0].piece_count,
        original.lines[0].piece_count
    );
    assert_eq!(
        classified.lines[0]
            .material_intake
            .actual_weight_thousandths,
        None
    );
    assert_eq!(ticket, original);
    assert!(ticket.with_material_intake(99, &material()).is_err());
}

#[test]
fn explicit_customer_copy_links_snapshot_and_customer_switch_requests_review() {
    let first = customer(1);
    let copied = SourceIntake::from_customer(&first);
    assert_eq!(copied.source_name, first.name);
    assert_eq!(copied.source_address, first.address);
    assert_eq!(copied.phone, first.phone);
    assert_eq!(copied.linked_customer_id, Some(1));
    assert_eq!(copied.category, SourceCategory::Unspecified);
    let mut ticket = Ticket::new(1);
    ticket.add_line("803", "4", "").unwrap();
    ticket.select_customer(&first).unwrap();
    assert_eq!(ticket.source_intake, SourceIntake::default());
    let mut ticket = ticket.with_source_intake(&copied).unwrap();
    assert!(
        !ticket
            .intake_issues()
            .iter()
            .any(|issue| issue.code == "linked-customer")
    );
    ticket.select_customer(&customer(2)).unwrap();
    assert!(
        ticket
            .intake_issues()
            .iter()
            .any(|issue| issue.code == "linked-customer")
    );
    assert_eq!(ticket.source_intake.source_name, first.name);
    ticket.choose_blank().unwrap();
    assert!(
        ticket
            .intake_issues()
            .iter()
            .any(|issue| issue.code == "linked-customer")
    );
}

#[test]
fn documentary_review_issues_are_specific_and_actual_weight_is_optional() {
    let mut ticket = Ticket::new(7);
    ticket.add_line("803", "4", "").unwrap();
    let issues = ticket.intake_issues();
    assert!(issues.iter().all(|issue| issue.ticket_id == 7));
    assert!(
        issues
            .iter()
            .any(|issue| issue.code == "material-class" && issue.line_index == Some(0))
    );
    let ticket = ticket
        .with_source_intake(&documented_source())
        .unwrap()
        .with_material_intake(0, &material())
        .unwrap();
    assert!(ticket.intake_issues().is_empty());
    let anonymous = ticket
        .with_source_intake(&SourceIntake {
            category: SourceCategory::Anonymous,
            ..documented_source()
        })
        .unwrap();
    assert!(
        anonymous
            .intake_issues()
            .iter()
            .any(|issue| issue.code == "anonymous-circumstances")
    );
    let business = ticket
        .with_source_intake(&SourceIntake {
            category: SourceCategory::Business,
            ..documented_source()
        })
        .unwrap();
    assert!(
        business
            .intake_issues()
            .iter()
            .any(|issue| issue.code == "organization")
    );
    assert!(
        business
            .intake_issues()
            .iter()
            .any(|issue| issue.code == "contact-name")
    );
    assert!(
        !business
            .intake_issues()
            .iter()
            .any(|issue| issue.message.to_lowercase().contains("eligible"))
    );
}

#[test]
fn intake_edits_preserve_tender_hold_and_finalized_receipt_snapshots() {
    let mut ticket = Ticket::new(1);
    ticket.add_line("803", "4", "").unwrap();
    ticket.choose_blank().unwrap();
    ticket.hold().unwrap();
    let held = ticket.clone();
    assert!(ticket.with_source_intake(&documented_source()).is_err());
    assert_eq!(ticket, held);
    ticket.resume().unwrap();
    ticket.confirm_cash("5").unwrap();
    let paid = ticket.clone();
    assert!(ticket.with_material_intake(0, &material()).is_err());
    assert_eq!(ticket.tender_evidence, paid.tender_evidence);
    ticket.complete(Payment::Cash).unwrap();
    let receipt = ticket.receipt();
    assert!(ticket.with_source_intake(&documented_source()).is_err());
    assert_eq!(ticket.state, State::Completed);
    assert_eq!(ticket.receipt(), receipt);
    let restored: Ticket = serde_json::from_str(&serde_json::to_string(&ticket).unwrap()).unwrap();
    assert_eq!(restored, ticket);
}

#[test]
fn copied_details_become_stale_after_same_customer_id_is_corrected() {
    let original_customer = customer(1);
    let mut ticket = Ticket::new(1);
    ticket.add_line("803", "4", "").unwrap();
    ticket.select_customer(&original_customer).unwrap();
    let copied = SourceIntake::from_customer(&original_customer);
    let ticket = ticket.with_source_intake(&copied).unwrap();
    for corrected in [
        Customer {
            name: "Corrected name".into(),
            ..original_customer.clone()
        },
        Customer {
            address: "Corrected address".into(),
            ..original_customer.clone()
        },
        Customer {
            phone: "555-0101".into(),
            ..original_customer.clone()
        },
    ] {
        let mut edited = ticket.clone();
        edited.select_customer(&corrected).unwrap();
        assert_eq!(edited.source_intake, copied);
        assert_eq!(edited.lines, ticket.lines);
        assert!(
            edited
                .intake_issues()
                .iter()
                .any(|issue| issue.code == "linked-customer-details")
        );
        let detached = edited
            .with_source_intake(&SourceIntake {
                linked_customer_id: None,
                ..copied.clone()
            })
            .unwrap();
        assert!(
            !detached
                .intake_issues()
                .iter()
                .any(|issue| issue.code.starts_with("linked-customer"))
        );
    }
}

#[test]
fn anonymous_identity_contradictions_are_review_issues_without_rewriting_source() {
    let original_customer = customer(1);
    let mut ticket = Ticket::new(1);
    ticket.add_line("803", "4", "").unwrap();
    ticket.select_customer(&original_customer).unwrap();
    let source = SourceIntake {
        category: SourceCategory::Anonymous,
        circumstances: "Synthetic unattended collection".into(),
        ..SourceIntake::from_customer(&original_customer)
    };
    let linked = ticket.with_source_intake(&source).unwrap();
    assert!(
        linked
            .intake_issues()
            .iter()
            .any(|issue| issue.code == "anonymous-identity")
    );
    assert_eq!(linked.source_intake, source);
    let unnamed = linked
        .with_source_intake(&SourceIntake {
            linked_customer_id: None,
            source_name: String::new(),
            ..source
        })
        .unwrap();
    assert!(
        !unnamed
            .intake_issues()
            .iter()
            .any(|issue| issue.code == "anonymous-identity")
    );
}
