use crate::{customers::Customer, domain::*, tender::CardOutcome};

fn blank_ticket() -> Ticket {
    let mut t = Ticket::new(1);
    t.add_weight_line("803", "4", "0").unwrap();
    t.choose_blank().unwrap();
    t
}
#[test]
fn customer_change_preserves_lines_and_completed_snapshot() {
    let mut t = blank_ticket();
    let c = Customer {
        id: 7,
        number: "C000007".into(),
        name: "Example Person".into(),
        phone: "555-0100".into(),
        ..Default::default()
    };
    t.select_customer(&c).unwrap();
    assert_eq!(t.lines.len(), 1);
    t.confirm_cash("10.00").unwrap();
    t.complete(Payment::Cash).unwrap();
    assert_eq!(t.customer_record.as_ref().unwrap().id, 7);
    assert!(
        t.select_customer(&Customer {
            name: "Changed".into(),
            ..c
        })
        .is_err()
    );
    assert!(t.receipt().contains("Example Person"));
    assert!(t.receipt().contains("Change $5.00"));
    assert!(t.receipt().contains("Phone #: 555-0100"));
}
#[test]
fn explicit_blank_classification_and_weight_guards() {
    let mut t = Ticket::new(1);
    t.add_line("801", "1", "").unwrap();
    assert!(t.choose_blank().is_err());
    assert!(t.complete(Payment::Cash).is_err());
    t.select_customer(&Customer {
        id: 1,
        number: "C000001".into(),
        name: "Example".into(),
        ..Default::default()
    })
    .unwrap();
    assert!(t.confirm_cash("5").is_err()); // classification required
    t.line_intake(0, "LCD", "1", "").unwrap();
    #[cfg(not(feature = "extended-workflows"))]
    {
        assert!(t.confirm_cash("5").is_err());
        t.source_intake.category = crate::intake::SourceCategory::Residential;
    }
    t.confirm_cash("5").unwrap();
    t.complete(Payment::Cash).unwrap();
    let mut t = blank_ticket();
    assert_eq!(t.lines[0].gross_thousandths, Some(4000));
    assert!(t.add_weight_line("803", "1", "2").is_err());
    assert!(t.add_weight_line("803", "1", "1").is_err());
    assert_eq!(t.lines.len(), 1);
    t.confirm_cash("5").unwrap();
    t.complete(Payment::Cash).unwrap();
    assert!(t.receipt().contains("$1.60"));
    assert!(t.receipt().contains("$3.40"));
}
#[test]
fn unknown_payment_survives_serialization_and_cannot_switch_tender() {
    let mut t = blank_ticket();
    t.start_card("5", "TEST-UNKNOWN").unwrap();
    t.card_result(CardOutcome::Unknown, false, false).unwrap();
    let mut t: Ticket = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
    assert!(t.confirm_cash("5").is_err());
    assert!(t.start_card("5", "TEST-RETRY").is_err());
    assert!(t.add_line("801", "1", "").is_err());
    assert!(
        t.complete(Payment::ExternalCard {
            reference: "TEST-UNKNOWN".into()
        })
        .is_err()
    );
    t.card_result(CardOutcome::Approved, true, true).unwrap();
    t.complete(Payment::ExternalCard {
        reference: "TEST-UNKNOWN".into(),
    })
    .unwrap();
    let saved = t.clone();
    t.complete(Payment::ExternalCard {
        reference: "TEST-UNKNOWN".into(),
    })
    .unwrap();
    assert_eq!(t, saved);
}

#[test]
fn catalog_803_floor_boundary_and_neighboring_cent_transitions_keep_weight_evidence() {
    let catalog = crate::catalog::Catalog::default();
    let rule = catalog.rule_for("803", 10).unwrap().clone();
    assert_eq!(rule.rate_cents, Some(40));
    assert_eq!(rule.minimum_cents, Some(500));
    // Literal products at $0.40/lb: 499.48/499.52 cents and
    // 500.48/500.52 cents straddle the nearby cent-rounding transitions.
    // 12.499/12.500/12.501 lb produce 499.96/500.00/500.04 cents:
    // all three round to $5.00 before any minimum adjustment.
    for (gross, gross_thousandths, net, base, adjustment, due, change, receipt_total) in [
        ("13.737", 13_737, "12.487", 499, 1, 500, 500, "Total  $5.00"),
        ("13.738", 13_738, "12.488", 500, 0, 500, 500, "Total  $5.00"),
        ("13.749", 13_749, "12.499", 500, 0, 500, 500, "Total  $5.00"),
        ("13.750", 13_750, "12.5", 500, 0, 500, 500, "Total  $5.00"),
        ("13.751", 13_751, "12.501", 500, 0, 500, 500, "Total  $5.00"),
        ("13.762", 13_762, "12.512", 500, 0, 500, 500, "Total  $5.00"),
        ("13.763", 13_763, "12.513", 501, 0, 501, 499, "Total  $5.01"),
    ] {
        let mut ticket = Ticket::new(1);
        ticket
            .add_catalog_line(&catalog, "803", gross, "1.250", "", 10)
            .unwrap();
        let line = &ticket.lines[0];
        assert_eq!(line.rule_snapshot.as_ref(), Some(&rule));
        assert_eq!(line.rate_cents, Some(40));
        assert_eq!(line.unit.as_deref(), Some("lb"));
        assert_eq!(line.gross_thousandths, Some(gross_thousandths));
        assert_eq!(line.tare_thousandths, Some(1_250));
        assert_eq!(line.quantity, net);
        assert_eq!(line.base_cents, Some(base), "Wrong base for {net} lb");
        assert_eq!(
            line.minimum_cents,
            Some(adjustment),
            "Wrong adjustment for {net} lb"
        );
        assert_eq!(line.total_cents, Some(due));
        assert_eq!(ticket.total(), Some(due));
        ticket.choose_blank().unwrap();
        ticket.confirm_cash("10").unwrap();
        ticket.complete(Payment::Cash).unwrap();
        assert_eq!(ticket.state, State::Completed);
        let tender = ticket.tender_evidence.cash.as_ref().unwrap();
        assert_eq!(tender.amount_due_cents, due);
        assert_eq!(tender.change_cents, change);
        assert!(ticket.receipt().contains(receipt_total));
        assert_eq!(
            ticket.receipt_snapshot.as_deref(),
            Some(ticket.receipt().as_str())
        );
    }
}

#[test]
fn catalog_803_fixture_minimum_applies_to_each_line_before_ticket_total() {
    let catalog = crate::catalog::Catalog::default();
    let mut ticket = Ticket::new(1);
    ticket
        .add_catalog_line(&catalog, "803", "5", "1", "", 10)
        .unwrap();
    ticket
        .add_catalog_line(&catalog, "803", "4", "0", "", 10)
        .unwrap();
    for line in &ticket.lines {
        assert_eq!(line.quantity, "4");
        assert_eq!(
            line.rule_snapshot.as_ref().unwrap().minimum_cents,
            Some(500)
        );
        assert_eq!(line.base_cents, Some(160));
        assert_eq!(line.minimum_cents, Some(340));
        assert_eq!(line.total_cents, Some(500));
    }
    assert_eq!(ticket.lines[0].gross_thousandths, Some(5_000));
    assert_eq!(ticket.lines[0].tare_thousandths, Some(1_000));
    assert_eq!(ticket.lines[1].gross_thousandths, Some(4_000));
    assert_eq!(ticket.lines[1].tare_thousandths, Some(0));
    assert_eq!(ticket.total(), Some(1_000));
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("10").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    assert!(ticket.receipt().contains("Total  $10.00"));
}

#[test]
fn catalog_803_no_floor_fee_and_blank_eligibility_are_independent() {
    let original = crate::catalog::Catalog::default();
    for (minimum, adjustment, due) in [(None, 0, 160), (Some(0), 0, 160), (Some(500), 340, 500)] {
        for blank_allowed in [true, false] {
            let rule = crate::catalog::PricingRule {
                minimum_cents: minimum,
                blank_allowed,
                ..original.rule_for("803", 10).unwrap().clone()
            };
            let catalog = original.save_rule(&rule).unwrap();
            let mut ticket = Ticket::new(1);
            ticket
                .add_catalog_line(&catalog, "803", "4", "0", "", 10)
                .unwrap();
            assert_eq!(ticket.lines[0].rule_snapshot.as_ref(), Some(&rule));
            assert_eq!(ticket.lines[0].base_cents, Some(160));
            assert_eq!(ticket.lines[0].minimum_cents, Some(adjustment));
            assert_eq!(ticket.lines[0].total_cents, Some(due));
            assert_eq!(ticket.total(), Some(due));
            if blank_allowed {
                ticket.choose_blank().unwrap();
            } else {
                let before = ticket.clone();
                assert!(ticket.choose_blank().is_err());
                assert_eq!(ticket, before);
                assert!(ticket.confirm_cash("10").is_err());
                ticket
                    .select_customer(&Customer {
                        id: 3,
                        number: "C000003".into(),
                        name: "Synthetic pricing customer".into(),
                        ..Default::default()
                    })
                    .unwrap();
            }
            ticket.confirm_cash("10").unwrap();
            ticket.complete(Payment::Cash).unwrap();
            assert_eq!(ticket.total(), Some(due));
            assert_eq!(
                ticket
                    .tender_evidence
                    .cash
                    .as_ref()
                    .unwrap()
                    .amount_due_cents,
                due
            );
        }
    }
}

#[test]
fn changed_catalog_affects_new_lines_but_preserves_saved_rules_and_frozen_receipts() {
    let original = crate::catalog::Catalog::default();
    let mut paid = Ticket::new(1);
    paid.add_catalog_line(&original, "803", "4", "0", "", 10)
        .unwrap();
    paid.choose_blank().unwrap();
    paid.confirm_cash("5").unwrap();
    paid.complete(Payment::Cash).unwrap();
    let frozen = paid.clone();
    let receipt = paid.receipt();
    let mut draft = Ticket::new(2);
    draft
        .add_catalog_line(&original, "803", "4", "0", "", 10)
        .unwrap();
    let saved_line = draft.lines[0].clone();
    let changed_rule = crate::catalog::PricingRule {
        rate_cents: Some(75),
        minimum_cents: Some(800),
        blank_allowed: false,
        ..original.rule_for("803", 10).unwrap().clone()
    };
    let changed = original.save_rule(&changed_rule).unwrap();
    draft
        .add_catalog_line(&changed, "803", "4", "0", "", 10)
        .unwrap();
    assert_eq!(draft.lines[0], saved_line);
    assert_eq!(
        draft.lines[0].rule_snapshot.as_ref().unwrap().rate_cents,
        Some(40)
    );
    assert_eq!(draft.lines[0].total_cents, Some(500));
    assert_eq!(draft.lines[1].rule_snapshot.as_ref(), Some(&changed_rule));
    assert_eq!(draft.lines[1].base_cents, Some(300));
    assert_eq!(draft.lines[1].minimum_cents, Some(500));
    assert_eq!(draft.lines[1].total_cents, Some(800));
    assert_eq!(draft.total(), Some(1_300));
    assert_eq!(original.rule_for("803", 10).unwrap().rate_cents, Some(40));
    assert_eq!(paid, frozen);
    let restored: Ticket = serde_json::from_str(&serde_json::to_string(&paid).unwrap()).unwrap();
    assert_eq!(restored, frozen);
    assert_eq!(restored.receipt(), receipt);
    assert_eq!(restored.receipt_snapshot.as_ref(), Some(&receipt));
}

fn contact_ticket(id: u64) -> Ticket {
    let mut ticket = Ticket::new(id);
    ticket
        .add_catalog_line(&crate::catalog::Catalog::default(), "803", "4", "0", "", 10)
        .unwrap();
    ticket
}

fn receipt_copies(ticket: &Ticket) -> crate::receipt_dispatch::ReceiptDispatchLedger {
    use crate::{
        operator_snapshot::{OperatorSnapshot, Role},
        printing::CopyKind,
        receipt_dispatch::{DispatchOutcome, ReceiptDispatchLedger},
    };
    let actor = OperatorSnapshot {
        id: 1,
        login: "receipt-contact-test".into(),
        display_name: "Synthetic receipt operator".into(),
        role: Role::Cashier,
        revision: 1,
    };
    let at = ticket.completed_at.unwrap() + 1;
    let mut ledger = ReceiptDispatchLedger::default();
    for (copy, key) in [
        (CopyKind::Customer, "contact-customer-copy"),
        (CopyKind::Internal, "contact-internal-copy"),
    ] {
        let (next, job, submit) = ledger
            .prepare(
                ticket,
                copy,
                key,
                "Synthetic contact receipt queue",
                &actor,
                at,
            )
            .unwrap();
        assert!(submit);
        assert_eq!(job.ticket_id, ticket.id);
        assert_eq!(job.copy, copy);
        assert!(job.receipt.starts_with(match copy {
            CopyKind::Customer => "CUSTOMER RECEIPT\n",
            CopyKind::Internal => "INTERNAL COPY\n",
        }));
        ledger = next
            .record(
                job.id,
                1,
                &DispatchOutcome::Accepted {
                    windows_job_id: job.id as u32,
                },
                at,
            )
            .unwrap()
            .0;
    }
    ledger
        .validate_against(std::slice::from_ref(ticket))
        .unwrap();
    ledger
}

fn assert_customer_contact_block(body: &str, customer: &Customer) {
    let name = body.find(&format!("Customer: {}", customer.name)).unwrap();
    let address = body
        .find(&customer.address)
        .expect("Saved customer address missing from receipt");
    let phone = body
        .find(&format!("Phone #: {}", customer.phone))
        .expect("Saved customer phone missing from receipt");
    let item = body.find("803 Other E-waste").unwrap();
    assert!(
        name < address && address < phone && phone < item,
        "Customer contact block must precede material lines"
    );
    assert_eq!(body.matches(&customer.address).count(), 1);
    assert_eq!(
        body.matches(&format!("Phone #: {}", customer.phone))
            .count(),
        1
    );
}

#[test]
fn both_receipt_copies_use_saved_customer_contact_above_items_and_keep_it_after_directory_changes()
{
    let (directory, customer) = crate::customers::Directory::default()
        .save_customer(&Customer {
            name: "Synthetic receipt customer".into(),
            address: "17 Synthetic Customer Way".into(),
            phone: "555-0177".into(),
            ..Default::default()
        })
        .unwrap();
    let mut ticket = contact_ticket(1);
    ticket.select_customer(&customer).unwrap();
    ticket.source_intake.source_name = "Separate source person".into();
    ticket.source_intake.source_address = "88 Source Address Only".into();
    ticket.source_intake.phone = "555-0188".into();
    ticket
        .select_account(&crate::customers::BusinessAccount {
            id: 9,
            number: "CONTACT-BUSINESS".into(),
            name: "Separate business contact".into(),
            active: true,
        })
        .unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    let frozen = ticket.clone();
    let body = ticket.receipt();
    let copies = receipt_copies(&ticket);
    assert_eq!(copies.jobs.len(), 2);
    for copy in &copies.jobs {
        assert_customer_contact_block(&copy.receipt, &customer);
        assert!(copy.receipt.ends_with(&body));
        for other_contact in [
            "Separate source person",
            "88 Source Address Only",
            "555-0188",
            "Separate business contact",
        ] {
            assert!(!copy.receipt.contains(other_contact));
        }
    }
    let (directory, edited) = directory
        .save_customer(&Customer {
            address: "99 Changed Directory Address".into(),
            phone: "555-0199".into(),
            ..customer.clone()
        })
        .unwrap();
    assert_eq!(edited.id, customer.id);
    let (_, alternate) = directory
        .save_customer(&Customer {
            name: "Different selected customer".into(),
            address: "42 Alternate Address".into(),
            phone: "555-0142".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(ticket.select_customer(&alternate).is_err());
    assert_eq!(ticket, frozen);
    assert_eq!(ticket.receipt(), body);
    let restarted: Ticket = serde_json::from_str(&serde_json::to_string(&ticket).unwrap()).unwrap();
    let restored_copies: crate::receipt_dispatch::ReceiptDispatchLedger =
        serde_json::from_str(&serde_json::to_string(&copies).unwrap()).unwrap();
    restored_copies
        .validate_against(std::slice::from_ref(&restarted))
        .unwrap();
    assert_eq!(restored_copies, copies);
    assert_eq!(restarted.receipt(), body);
    for copy in &receipt_copies(&restarted).jobs {
        assert_customer_contact_block(&copy.receipt, &customer);
        assert!(!copy.receipt.contains("99 Changed Directory Address"));
        assert!(!copy.receipt.contains("555-0199"));
        assert!(!copy.receipt.contains("42 Alternate Address"));
    }
}

#[test]
fn old_frozen_receipt_without_contacts_stays_byte_identical_on_both_copy_types() {
    let (_, customer) = crate::customers::Directory::default()
        .save_customer(&Customer {
            name: "Legacy frozen customer".into(),
            address: "71 Existing Saved Address".into(),
            phone: "555-0171".into(),
            ..Default::default()
        })
        .unwrap();
    let mut ticket = contact_ticket(1);
    ticket.select_customer(&customer).unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    let old_body = "E-WASTE COUNTER\nTicket #000001\nCustomer: Legacy frozen customer\n\n803 Other E-waste (4 lb)  $5.00\n\nTotal  $5.00\nCash recorded\n";
    ticket.receipt_snapshot = Some(old_body.into());
    ticket.reprint().unwrap();
    let restarted: Ticket = serde_json::from_str(&serde_json::to_string(&ticket).unwrap()).unwrap();
    assert_eq!(restarted.receipt().as_bytes(), old_body.as_bytes());
    for copy in receipt_copies(&restarted).jobs {
        let header = match copy.copy {
            crate::printing::CopyKind::Customer => "CUSTOMER RECEIPT\n",
            crate::printing::CopyKind::Internal => "INTERNAL COPY\n",
        };
        assert_eq!(
            copy.receipt.strip_prefix(header).unwrap().as_bytes(),
            old_body.as_bytes()
        );
        assert!(!copy.receipt.contains(&customer.address));
        assert!(!copy.receipt.contains("Phone #:"));
    }
}

#[test]
fn blank_legacy_and_partial_customer_records_never_invent_missing_receipt_contacts() {
    for (kind, address, phone) in [
        ("blank", "", ""),
        ("legacy-name-only", "", ""),
        ("missing-fields", "", ""),
        ("address-only", "21 Partial Customer Way", ""),
        ("phone-only", "", "555-0121"),
    ] {
        let mut ticket = contact_ticket(1);
        if matches!(kind, "blank" | "legacy-name-only") {
            ticket.choose_blank().unwrap();
        } else {
            let (_, customer) = crate::customers::Directory::default()
                .save_customer(&Customer {
                    name: "Synthetic partial customer".into(),
                    address: address.into(),
                    phone: phone.into(),
                    ..Default::default()
                })
                .unwrap();
            ticket.select_customer(&customer).unwrap();
        }
        ticket.source_intake.source_address = "Source must never supply receipt address".into();
        ticket.source_intake.phone = "555-0999".into();
        ticket.confirm_cash("5").unwrap();
        ticket.complete(Payment::Cash).unwrap();
        if kind == "legacy-name-only" {
            ticket.customer = Some("Historical name without saved record".into());
            ticket.customer_choice = crate::workflow::CustomerChoice::Selected;
            ticket.customer_record = None;
            ticket.receipt_snapshot = None;
        }
        for copy in receipt_copies(&ticket).jobs {
            let item = copy.receipt.find("803 Other E-waste").unwrap();
            if !address.is_empty() {
                assert!(
                    copy.receipt
                        .find(address)
                        .expect("Available saved address missing")
                        < item
                );
            }
            if phone.is_empty() {
                assert!(
                    !copy.receipt.contains("Phone #:"),
                    "Missing phone generated a label for {kind}"
                );
            } else {
                assert!(
                    copy.receipt
                        .find(&format!("Phone #: {phone}"))
                        .expect("Available saved phone missing")
                        < item
                );
            }
            assert!(
                !copy
                    .receipt
                    .lines()
                    .any(|line| matches!(line.trim(), "Address:" | "Phone #:"))
            );
            assert!(
                !copy
                    .receipt
                    .contains("Source must never supply receipt address")
            );
            assert!(!copy.receipt.contains("555-0999"));
        }
    }
}
