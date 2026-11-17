use super::*;
use crate::{
    auth::{OperatorSnapshot, Role},
    customers::Customer,
    domain::Payment,
    intake::{
        DeliveryMode, IntakeReason, MaterialClass, MaterialCondition, MaterialIntake,
        SourceCategory, SourceIntake,
    },
    intake_review::{ReviewDisposition, ReviewSnapshot},
    inventory::{InventoryLocation, PartySnapshot, ReceiveInput, ShipmentInput},
};
use std::collections::HashMap;

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "supervisor".into(),
        display_name: "Synthetic Supervisor".into(),
        role: Role::Supervisor,
        revision: 1,
    }
}
fn ticket() -> Ticket {
    let mut t = Ticket::new(1);
    t.created_at = 100;
    t.add_line("801", "1", "").unwrap();
    t.line_intake(0, "Flat screen", "1", "PRIVATE-BILLING-NOTE")
        .unwrap();
    t.add_weight_line("803", "20", "0").unwrap();
    t.select_customer(&Customer {
        id: 1,
        number: "C1".into(),
        name: "Counter Customer".into(),
        ..Default::default()
    })
    .unwrap();
    t = t
        .with_source_intake(&SourceIntake {
            category: SourceCategory::Residential,
            reason: IntakeReason::EmptyingGarage,
            delivery: DeliveryMode::DropOff,
            source_name: "Pinned Source".into(),
            source_address: "7 Private Source Lane".into(),
            organization: "Private Source Organization".into(),
            contact_name: "Private Source Contact".into(),
            phone: "555-0199".into(),
            circumstances: "PRIVATE-CIRCUMSTANCES".into(),
            notes: "PRIVATE-SOURCE-NOTES".into(),
            ..Default::default()
        })
        .unwrap();
    for (index, class, count, weight) in [
        (0, MaterialClass::NonCrtDisplay, 1, None),
        (1, MaterialClass::OtherElectronics, 2, Some(5_000)),
    ] {
        t = t
            .with_material_intake(
                index,
                &MaterialIntake {
                    class,
                    condition: MaterialCondition::Intact,
                    reporting_units: Some(count),
                    actual_weight_thousandths: weight,
                    notes: "PRIVATE-MATERIAL-NOTES".into(),
                },
            )
            .unwrap();
    }
    t.confirm_cash("20").unwrap();
    t.complete(Payment::Cash).unwrap();
    t.completed_at = Some(110);
    t
}
fn reviews(ticket: &Ticket) -> ReviewLedger {
    let mut snapshot = ReviewSnapshot::from_ticket(ticket).unwrap();
    snapshot.disposition = ReviewDisposition::Reviewed;
    ReviewLedger::default()
        .record(
            ticket,
            &snapshot,
            "Synthetic Supervisor",
            "PRIVATE-REVIEW-REASON",
            "review-1",
            115,
        )
        .unwrap()
        .0
}
fn receive(
    ledger: &InventoryLedger,
    ticket: &Ticket,
    reviews: &ReviewLedger,
    dir: &InventoryDirectory,
    index: usize,
    at: u64,
) -> InventoryLedger {
    ledger
        .receive(
            std::slice::from_ref(ticket),
            reviews,
            dir,
            &ReceiveInput {
                ticket_id: ticket.id,
                line_identity: LineIdentity::from_ticket(ticket, index).unwrap(),
                review_event_id: Some(1),
                location_id: 1,
                container: format!("Container-{}", index + 1),
                received_at: Some(105),
                first_accumulation_at: None,
                reason: "PRIVATE-RECEIVE-REASON".into(),
                confirmed: true,
            },
            &actor(),
            &format!("receive-{index}"),
            at,
        )
        .unwrap()
        .0
}

// Minimal RFC-4180 reader keeps tests independent of comma/newline/quote content.
fn csv_rows(csv: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = csv.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                row.push(std::mem::take(&mut field));
            }
            '\r' if !quoted && chars.peek() == Some(&'\n') => {
                chars.next();
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    assert!(!quoted);
    assert!(field.is_empty());
    assert!(row.is_empty());
    rows
}
fn records(csv: &str) -> Vec<HashMap<String, String>> {
    let rows = csv_rows(csv);
    let header = &rows[0];
    rows[1..]
        .iter()
        .map(|row| {
            assert_eq!(row.len(), header.len());
            header.iter().cloned().zip(row.iter().cloned()).collect()
        })
        .collect()
}

#[test]
fn reporting_weights_and_dates_are_independent_and_unknowns_stay_empty() {
    let ticket = ticket();
    let reviews = reviews(&ticket);
    let dir = InventoryDirectory::default();
    let ledger = receive(&InventoryLedger::default(), &ticket, &reviews, &dir, 0, 120);
    let ledger = receive(&ledger, &ticket, &reviews, &dir, 1, 121);
    let csv = export_inventory(
        std::slice::from_ref(&ticket),
        &reviews,
        &dir,
        &ledger,
        false,
    )
    .unwrap();
    let rows = records(&csv.lots);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["training_actual_weight_lb"], "");
    assert_eq!(rows[1]["training_actual_weight_lb"], "5.000");
    assert_eq!(rows[0]["training_reporting_unit_count"], "1");
    assert_eq!(rows[1]["training_reporting_unit_count"], "2");
    assert_eq!(rows[0]["training_received_at_unix_seconds"], "105");
    assert_eq!(rows[0]["training_first_accumulation_at_unix_seconds"], "");
    assert_eq!(rows[0]["training_recorded_at_unix_seconds"], "120");
    assert_eq!(rows[0]["training_origin_review_event_id"], "1");
    assert!(csv_rows(&csv.shipments)[1..].is_empty());
    assert_eq!(records(&csv.movements).len(), 2);
}

#[test]
fn receiving_pins_exact_review_history_instead_of_later_classification_or_contacts() {
    let ticket = ticket();
    let first = reviews(&ticket);
    let dir = InventoryDirectory::default();
    let ledger = receive(&InventoryLedger::default(), &ticket, &first, &dir, 0, 120);
    let mut later = first.latest(ticket.id).unwrap().snapshot.clone();
    later.source.source_name = "Later Source Must Not Replace Pin".into();
    later.materials[0].details.class = MaterialClass::OtherElectronics;
    later.materials[0].details.actual_weight_thousandths = Some(123_000);
    later.disposition = ReviewDisposition::NotForClaim;
    let newer = first
        .record(
            &ticket,
            &later,
            "Synthetic Supervisor",
            "PRIVATE-LATER-REASON",
            "review-later",
            125,
        )
        .unwrap()
        .0;
    let csv = export_inventory(&[ticket], &newer, &dir, &ledger, true).unwrap();
    let rows = records(&csv.lots);
    assert_eq!(rows[0]["training_origin_review_event_id"], "1");
    assert_eq!(rows[0]["training_documentary_disposition"], "Reviewed");
    assert_eq!(rows[0]["training_material_class"], "NonCrtDisplay");
    assert_eq!(rows[0]["training_actual_weight_lb"], "");
    assert_eq!(rows[0]["training_source_name"], "Pinned Source");
    assert!(!csv.lots.contains("Later Source Must Not Replace Pin"));
}

#[test]
fn whole_lot_movement_and_shipment_lineage_is_exported_without_weight_estimates() {
    let ticket = ticket();
    let reviews = reviews(&ticket);
    let dir = InventoryDirectory {
        locations: vec![
            InventoryLocation {
                id: 1,
                name: "Training staging".into(),
                active: true,
            },
            InventoryLocation {
                id: 2,
                name: "Bay B".into(),
                active: true,
            },
        ],
        next_id: 3,
    };
    let ledger = receive(&InventoryLedger::default(), &ticket, &reviews, &dir, 0, 120);
    let ledger = receive(&ledger, &ticket, &reviews, &dir, 1, 121);
    let ledger = ledger
        .move_lot(
            &dir,
            1,
            2,
            "TV-container",
            "PRIVATE-MOVE-REASON",
            &actor(),
            "move-1",
            130,
        )
        .unwrap()
        .0;
    let (ledger, shipment) = ledger
        .ship(
            &dir,
            &ShipmentInput {
                lot_ids: vec![2, 1],
                destination: PartySnapshot {
                    name: "Synthetic destination".into(),
                    address: "8 Private Destination Road".into(),
                },
                carrier: PartySnapshot {
                    name: "Synthetic carrier".into(),
                    address: "9 Private Carrier Road".into(),
                },
                evidence_reference: "EVIDENCE-1".into(),
                shipped_at: Some(145),
                actual_weight_thousandths: None,
                reason: "PRIVATE-SHIP-REASON".into(),
                confirmed: true,
            },
            &actor(),
            "ship-1",
            150,
        )
        .unwrap();
    let csv = export_inventory(
        std::slice::from_ref(&ticket),
        &reviews,
        &dir,
        &ledger,
        false,
    )
    .unwrap();
    let lots = records(&csv.lots);
    let movements = records(&csv.movements);
    let shipments = records(&csv.shipments);
    assert!(
        lots.iter()
            .all(|row| row["training_lot_status"] == "Shipped")
    );
    assert_eq!(lots[0]["training_location_id"], "2");
    assert_eq!(lots[0]["training_container"], "TV-container");
    assert!(
        lots.iter()
            .all(|row| row["training_shipment_id"] == shipment.id.to_string())
    );
    assert_eq!(movements.len(), 5);
    let moved = movements
        .iter()
        .find(|row| row["training_event_type"] == "Moved")
        .unwrap();
    assert_eq!(moved["training_from_location_id"], "1");
    assert_eq!(moved["training_to_location_id"], "2");
    assert_eq!(shipments[0]["training_lot_ids"], "1|2");
    assert_eq!(shipments[0]["training_actual_weight_lb"], "");
    assert_eq!(shipments[0]["training_shipped_at_unix_seconds"], "145");
    assert_eq!(shipments[0]["training_recorded_at_unix_seconds"], "150");
    assert_eq!(shipments[0]["training_evidence_reference"], "EVIDENCE-1");
    assert!(!csv.shipments.contains("8 Private Destination Road"));
    let contacts = export_inventory(&[ticket], &reviews, &dir, &ledger, true).unwrap();
    assert!(contacts.shipments.contains("8 Private Destination Road"));
    assert!(contacts.shipments.contains("9 Private Carrier Road"));
}

#[test]
fn contact_fields_require_opt_in_and_private_prose_never_enters_any_export() {
    let ticket = ticket();
    let reviews = reviews(&ticket);
    let dir = InventoryDirectory::default();
    let ledger = receive(&InventoryLedger::default(), &ticket, &reviews, &dir, 0, 120);
    for include_contacts in [false, true] {
        let csv = export_inventory(
            std::slice::from_ref(&ticket),
            &reviews,
            &dir,
            &ledger,
            include_contacts,
        )
        .unwrap();
        for document in [&csv.lots, &csv.movements, &csv.shipments] {
            assert!(!document.contains("PRIVATE-"));
            assert!(!document.contains("Counter Customer"));
            assert!(
                csv_rows(document)[0]
                    .iter()
                    .all(|header| header.starts_with("training_"))
            );
        }
        assert_eq!(csv.lots.contains("Pinned Source"), include_contacts);
        assert_eq!(csv.lots.contains("555-0199"), include_contacts);
        assert_eq!(csv.lots.contains("7 Private Source Lane"), include_contacts);
    }
}

#[test]
fn spreadsheet_formulas_bom_controls_and_csv_quotes_are_protected() {
    for input in [
        "=1+1",
        "+SUM(A1)",
        "-1+2",
        "@SUM(A1)",
        " \t=1",
        "\u{feff} =1",
        "\r=1",
    ] {
        let decoded = csv_rows(&format!("field\r\n{}\r\n", csv_cell(input)));
        assert!(decoded[1][0].starts_with('\''), "{input:?}");
    }
    assert_eq!(csv_cell("plain, \"quoted\""), "\"plain, \"\"quoted\"\"\"");
    assert_eq!(csv_cell("line\ntext"), "\"line\ntext\"");
    let mut ticket = ticket();
    ticket.source_intake.source_name = "=SUM(1,2)".into();
    ticket.source_intake.source_address = "Address, \"quoted\"".into();
    let reviews = reviews(&ticket);
    let dir = InventoryDirectory::default();
    let ledger = receive(&InventoryLedger::default(), &ticket, &reviews, &dir, 0, 120);
    let csv = export_inventory(&[ticket], &reviews, &dir, &ledger, true).unwrap();
    let rows = records(&csv.lots);
    assert_eq!(rows[0]["training_source_name"], "'=SUM(1,2)");
    assert_eq!(rows[0]["training_source_address"], "Address, \"quoted\"");
}

#[test]
fn corrupt_or_orphaned_lineage_fails_before_any_csv_is_returned() {
    let ticket = ticket();
    let reviews = reviews(&ticket);
    let dir = InventoryDirectory::default();
    let ledger = receive(&InventoryLedger::default(), &ticket, &reviews, &dir, 0, 120);
    assert!(export_inventory(&[], &reviews, &dir, &ledger, false).is_err());
    let mut corrupt = ledger.clone();
    if let InventoryAction::Received { lot } = &mut corrupt.events[0].action {
        lot.origin.review_event_id = Some(999);
    }
    assert!(
        export_inventory(
            std::slice::from_ref(&ticket),
            &reviews,
            &dir,
            &corrupt,
            false
        )
        .is_err()
    );
    let mut corrupt = ledger.clone();
    corrupt.events[0].id = 99;
    assert!(
        export_inventory(
            std::slice::from_ref(&ticket),
            &reviews,
            &dir,
            &corrupt,
            false
        )
        .is_err()
    );
    let one = export_inventory(
        std::slice::from_ref(&ticket),
        &reviews,
        &dir,
        &ledger,
        false,
    )
    .unwrap();
    let two = export_inventory(&[ticket], &reviews, &dir, &ledger, false).unwrap();
    assert_eq!(one, two);
}
