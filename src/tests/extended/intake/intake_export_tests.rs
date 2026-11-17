use super::*;
use crate::{customers::Customer, domain::Payment, intake::*, intake_review::ReviewDisposition};

fn completed(code: &str) -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.created_at = 90;
    ticket
        .add_line(code, if code == "803" { "4" } else { "1" }, "")
        .unwrap();
    if code == "803" {
        ticket.choose_blank().unwrap();
    } else {
        ticket
            .select_customer(&Customer {
                id: 1,
                number: "C1".into(),
                name: "PRIVATE_CUSTOMER".into(),
                ..Default::default()
            })
            .unwrap();
        ticket
            .line_intake(0, "Training classification", "1", "INTERNAL_LINE_NOTE")
            .unwrap();
    }
    ticket = ticket
        .with_source_intake(&SourceIntake {
            category: SourceCategory::Residential,
            reason: IntakeReason::EmptyingGarage,
            delivery: DeliveryMode::DropOff,
            source_name: "PRIVATE_SOURCE".into(),
            source_address: "PRIVATE_ADDRESS".into(),
            organization: "PRIVATE_ORG".into(),
            contact_name: "PRIVATE_CONTACT".into(),
            phone: "555-0109".into(),
            circumstances: "INTERNAL_CIRCUMSTANCES".into(),
            reason_detail: "INTERNAL_REASON".into(),
            notes: "INTERNAL_SOURCE_NOTE".into(),
            ..Default::default()
        })
        .unwrap();
    ticket = ticket
        .with_material_intake(
            0,
            &MaterialIntake {
                class: MaterialClass::NonCrtDisplay,
                condition: MaterialCondition::Intact,
                reporting_units: Some(1),
                notes: "INTERNAL_MATERIAL_NOTE".into(),
                ..Default::default()
            },
        )
        .unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.completed_at = Some(100);
    ticket
}

fn field(csv: &str, header: &str) -> String {
    let mut lines = csv.split("\r\n");
    let index = lines
        .next()
        .unwrap()
        .split(',')
        .position(|value| value == header)
        .unwrap();
    lines
        .next()
        .unwrap()
        .split(',')
        .nth(index)
        .unwrap()
        .to_owned()
}

#[test]
fn default_export_is_training_only_and_excludes_contacts_and_internal_prose() {
    let ticket = completed("803");
    let csv = export_intake(&[ticket], &ReviewLedger::default(), false).unwrap();
    assert!(csv.starts_with("training_ticket_id,"));
    assert!(!csv.contains("PRIVATE_"));
    assert!(!csv.contains("555-0109"));
    assert!(!csv.contains("INTERNAL_"));
    assert_eq!(field(&csv, "training_ticket_state"), "Completed");
    assert_eq!(field(&csv, "training_tender_disposition"), "CashReceived");
    assert_eq!(field(&csv, "training_billing_line_total_cents"), "500");
    assert_eq!(
        field(&csv, "training_documentary_disposition"),
        "PendingReview"
    );
    assert_eq!(field(&csv, "training_actual_weight_lb"), "");
}

#[test]
fn contacts_are_opt_in_and_internal_fields_stay_excluded() {
    let csv = export_intake(&[completed("801")], &ReviewLedger::default(), true).unwrap();
    assert!(csv.contains("PRIVATE_SOURCE"));
    assert!(csv.contains("PRIVATE_ADDRESS"));
    assert!(csv.contains("PRIVATE_CONTACT"));
    assert!(csv.contains("555-0109"));
    assert!(!csv.contains("PRIVATE_CUSTOMER"));
    assert!(!csv.contains("INTERNAL_"));
}

#[test]
fn reviewed_classification_overlays_reporting_without_changing_ticket_or_charge() {
    let ticket = completed("801");
    let original = ticket.clone();
    let mut snapshot = ReviewSnapshot::from_ticket(&ticket).unwrap();
    snapshot.materials[0].details.class = MaterialClass::LooseBattery;
    snapshot.materials[0].details.reporting_units = Some(17);
    snapshot.disposition = ReviewDisposition::Reviewed;
    let (ledger, _) = ReviewLedger::default()
        .record(
            &ticket,
            &snapshot,
            "Operator",
            "Synthetic review",
            "export-review",
            110,
        )
        .unwrap();
    let csv = export_intake(std::slice::from_ref(&ticket), &ledger, false).unwrap();
    assert_eq!(field(&csv, "training_material_class"), "LooseBattery");
    assert_eq!(field(&csv, "training_reporting_unit_count"), "17");
    assert_eq!(field(&csv, "training_billing_quantity"), "1");
    assert_eq!(field(&csv, "training_billing_line_total_cents"), "500");
    assert_eq!(field(&csv, "training_actual_weight_lb"), "");
    assert_eq!(field(&csv, "training_documentary_issue_count"), "0");
    assert_eq!(ticket, original);
    assert_eq!(ticket.receipt(), original.receipt());
}

#[test]
fn draft_held_pending_and_legacy_identity_are_explicit_and_reproducible() {
    let mut held = Ticket::new(2);
    held.add_line("901", "", "").unwrap();
    held.hold().unwrap();
    let before = held.clone();
    let csv = export_intake(&[held.clone()], &ReviewLedger::default(), false).unwrap();
    assert_eq!(field(&csv, "training_ticket_state"), "Held");
    assert_eq!(field(&csv, "training_tender_disposition"), "Pending");
    assert_eq!(field(&csv, "training_billing_line_total_cents"), "");
    assert_eq!(
        export_intake(&[held.clone()], &ReviewLedger::default(), false).unwrap(),
        csv
    );
    assert_eq!(held, before);
    let mut legacy = completed("803");
    legacy.lines[0].id = 0;
    let csv = export_intake(&[legacy], &ReviewLedger::default(), false).unwrap();
    assert_eq!(field(&csv, "training_line_identity"), "legacy-ordinal:0");
    let empty = export_intake(&[Ticket::new(3)], &ReviewLedger::default(), false).unwrap();
    assert_eq!(
        empty.split("\r\n").filter(|line| !line.is_empty()).count(),
        1
    );
}

#[test]
fn spreadsheet_formulas_are_neutralized_and_csv_quotes_are_escaped() {
    for dangerous in [
        "=1+1",
        "+SUM(A1)",
        "-cmd",
        "@function",
        "  =1+1",
        "\tformula",
    ] {
        assert!(csv_cell(dangerous).contains('\''));
    }
    assert_eq!(
        csv_cell("first,\r\n\"second\""),
        "\"first,\r\n\"\"second\"\"\""
    );
    let mut ticket = completed("803");
    ticket.source_intake.source_name = "=HYPERLINK(\"https://example.invalid\",\"x\")".into();
    let csv = export_intake(&[ticket], &ReviewLedger::default(), true).unwrap();
    assert!(csv.contains("\"'=HYPERLINK(\"\"https://example.invalid\"\",\"\"x\"\")\""));
}

#[test]
fn corrupt_ticket_or_review_identity_is_rejected_instead_of_skipped() {
    let ticket = completed("803");
    assert!(
        export_intake(
            &[ticket.clone(), ticket.clone()],
            &ReviewLedger::default(),
            false
        )
        .is_err()
    );
    let snapshot = ReviewSnapshot::from_ticket(&ticket).unwrap();
    let (ledger, _) = ReviewLedger::default()
        .record(
            &ticket,
            &snapshot,
            "Operator",
            "Synthetic review",
            "review",
            110,
        )
        .unwrap();
    assert!(export_intake(&[], &ledger, false).is_err());
    let mut bad_ledger = ledger.clone();
    bad_ledger.events[0].snapshot.materials[0].identity = LineIdentity::Stable(999);
    assert!(export_intake(std::slice::from_ref(&ticket), &bad_ledger, false).is_err());
    let mut corrupt = ticket.clone();
    corrupt.lines[0].total_cents = Some(-1);
    assert!(export_intake(&[corrupt], &ReviewLedger::default(), false).is_err());
    let mut corrupt = ticket;
    corrupt.source_intake.source_name = "bad\nname".into();
    assert!(export_intake(&[corrupt], &ReviewLedger::default(), true).is_err());
}
