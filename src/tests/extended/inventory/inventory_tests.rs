use super::*;
use crate::{
    auth::{OperatorSnapshot, Role},
    domain::{Payment, Ticket},
    intake::*,
    intake_review::{LineIdentity, ReviewDisposition, ReviewLedger, ReviewSnapshot},
};

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "supervisor".into(),
        display_name: "Synthetic Supervisor".into(),
        role: Role::Supervisor,
        revision: 1,
    }
}

fn ticket(id: u64) -> Ticket {
    let mut ticket = Ticket::new(id);
    ticket.created_at = 90;
    ticket.add_line("803", "4", "").unwrap();
    ticket.choose_blank().unwrap();
    ticket = ticket
        .with_source_intake(&SourceIntake {
            category: SourceCategory::Residential,
            reason: IntakeReason::EmptyingGarage,
            delivery: DeliveryMode::DropOff,
            source_name: "Synthetic source".into(),
            source_address: "1 Example Lane".into(),
            ..Default::default()
        })
        .unwrap();
    ticket = ticket
        .with_material_intake(
            0,
            &MaterialIntake {
                class: MaterialClass::OtherElectronics,
                condition: MaterialCondition::Intact,
                reporting_units: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.completed_at = Some(100);
    ticket
}

fn receive_input(ticket_id: u64) -> ReceiveInput {
    ReceiveInput {
        ticket_id,
        line_identity: LineIdentity::Stable(1),
        review_event_id: None,
        location_id: 1,
        container: "TEST-CONTAINER-A".into(),
        received_at: Some(110),
        first_accumulation_at: Some(105),
        reason: "Received synthetic whole lot".into(),
        confirmed: true,
    }
}

fn shipment(lot_ids: Vec<u64>) -> ShipmentInput {
    ShipmentInput {
        lot_ids,
        destination: PartySnapshot {
            name: "Synthetic downstream".into(),
            address: "2 Example Road".into(),
        },
        carrier: PartySnapshot::default(),
        evidence_reference: "TEST-SHIPMENT-1".into(),
        shipped_at: Some(220),
        actual_weight_thousandths: None,
        reason: "Recorded synthetic outbound".into(),
        confirmed: true,
    }
}

fn received(ticket: &Ticket) -> (InventoryLedger, InventoryLot) {
    InventoryLedger::default()
        .receive(
            std::slice::from_ref(ticket),
            &ReviewLedger::default(),
            &InventoryDirectory::default(),
            &receive_input(ticket.id),
            &actor(),
            "receive-1",
            200,
        )
        .unwrap()
}

#[test]
fn receive_pins_reporting_data_without_pricing_or_inventing_weight() {
    let original = ticket(1);
    let copy = original.clone();
    let (ledger, lot) = received(&original);
    assert_eq!(lot.origin.material.reporting_units, Some(2));
    assert_eq!(lot.origin.material.actual_weight_thousandths, None);
    assert_eq!(lot.origin.source, original.source_intake);
    assert_eq!(lot.received_at, Some(110));
    assert_eq!(lot.first_accumulation_at, Some(105));
    assert_eq!(lot.status, LotStatus::OnHand);
    assert_eq!(original, copy);
    assert_eq!(original.total(), Some(500));
    assert_eq!(original.lines[0].quantity, "4");
    ledger
        .validate_against(
            &[original],
            &ReviewLedger::default(),
            &InventoryDirectory::default(),
        )
        .unwrap();
}

#[test]
fn exact_receive_retry_and_forever_unique_ticket_line_prevent_duplicate_receipts() {
    let original = ticket(1);
    let (ledger, lot) = received(&original);
    let (retry, existing) = ledger
        .receive(
            std::slice::from_ref(&original),
            &ReviewLedger::default(),
            &InventoryDirectory::default(),
            &receive_input(1),
            &actor(),
            "receive-1",
            999,
        )
        .unwrap();
    assert_eq!(retry, ledger);
    assert_eq!(existing, lot);
    assert!(
        ledger
            .receive(
                std::slice::from_ref(&original),
                &ReviewLedger::default(),
                &InventoryDirectory::default(),
                &receive_input(1),
                &actor(),
                "different-key",
                210
            )
            .is_err()
    );
    let (shipped, _) = ledger
        .ship(
            &InventoryDirectory::default(),
            &shipment(vec![lot.id]),
            &actor(),
            "ship",
            230,
        )
        .unwrap();
    assert!(
        shipped
            .receive(
                std::slice::from_ref(&original),
                &ReviewLedger::default(),
                &InventoryDirectory::default(),
                &receive_input(1),
                &actor(),
                "new-receive",
                240
            )
            .is_err()
    );
    let (retry, existing) = shipped
        .receive(
            &[original],
            &ReviewLedger::default(),
            &InventoryDirectory::default(),
            &receive_input(1),
            &actor(),
            "receive-1",
            240,
        )
        .unwrap();
    assert_eq!(retry, shipped);
    assert_eq!(existing.status, LotStatus::Shipped);
}

#[test]
fn moves_preserve_dates_and_history_while_location_renames_do_not_invalidate_it() {
    let original = ticket(1);
    let (ledger, lot) = received(&original);
    let directory = InventoryDirectory::default();
    let (directory, location) = directory
        .save_location(&InventoryLocation {
            id: 0,
            name: "Training bay B".into(),
            active: true,
        })
        .unwrap();
    let (moved, current) = ledger
        .move_lot(
            &directory,
            lot.id,
            location.id,
            "TEST-CONTAINER-B",
            "Moved whole lot",
            &actor(),
            "move",
            210,
        )
        .unwrap();
    assert_eq!(current.first_accumulation_at, lot.first_accumulation_at);
    assert_eq!(current.origin, lot.origin);
    assert_eq!(current.location, location);
    assert_eq!(current.container, "TEST-CONTAINER-B");
    assert_eq!(ledger.project().unwrap().lot(lot.id), Some(&lot));
    assert!(
        moved
            .move_lot(
                &directory,
                lot.id,
                location.id,
                "TEST-CONTAINER-B",
                "No physical change",
                &actor(),
                "noop",
                211
            )
            .is_err()
    );
    let (directory, _) = directory
        .save_location(&InventoryLocation {
            active: false,
            name: "Renamed training bay".into(),
            ..location
        })
        .unwrap();
    moved
        .validate_against(&[original], &ReviewLedger::default(), &directory)
        .unwrap();
    let (retry, _) = moved
        .move_lot(
            &directory,
            lot.id,
            2,
            "TEST-CONTAINER-B",
            "Moved whole lot",
            &actor(),
            "move",
            240,
        )
        .unwrap();
    assert_eq!(retry, moved);
    assert!(
        moved
            .move_lot(
                &directory,
                lot.id,
                2,
                "TEST-C",
                "New move to inactive",
                &actor(),
                "inactive",
                241
            )
            .is_err()
    );
    let (_, same_area) = moved
        .move_lot(
            &directory,
            lot.id,
            1,
            "TEST-CONTAINER-C",
            "Return to staging",
            &actor(),
            "return",
            242,
        )
        .unwrap();
    assert_eq!(same_area.first_accumulation_at, Some(105));
}

#[test]
fn multi_lot_shipping_is_atomic_and_cannot_repeat_or_move_shipped_stock() {
    let originals = vec![ticket(1), ticket(2)];
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default()
        .receive(
            &originals,
            &ReviewLedger::default(),
            &directory,
            &receive_input(1),
            &actor(),
            "receive-1",
            200,
        )
        .unwrap()
        .0;
    let ledger = ledger
        .receive(
            &originals,
            &ReviewLedger::default(),
            &directory,
            &receive_input(2),
            &actor(),
            "receive-2",
            201,
        )
        .unwrap()
        .0;
    assert!(
        ledger
            .ship(
                &directory,
                &shipment(vec![1, 999]),
                &actor(),
                "bad-ship",
                230
            )
            .is_err()
    );
    assert!(
        ledger
            .ship(
                &directory,
                &shipment(vec![1, 1]),
                &actor(),
                "duplicate-ship",
                230
            )
            .is_err()
    );
    assert!(
        ledger
            .project()
            .unwrap()
            .lots
            .iter()
            .all(|lot| lot.status == LotStatus::OnHand)
    );
    let (shipped, recorded) = ledger
        .ship(&directory, &shipment(vec![1, 2]), &actor(), "ship", 230)
        .unwrap();
    let state = shipped.project().unwrap();
    assert_eq!(state.shipments, vec![recorded.clone()]);
    assert!(
        state
            .lots
            .iter()
            .all(|lot| lot.status == LotStatus::Shipped && lot.shipment_id == Some(recorded.id))
    );
    assert_eq!(recorded.actual_weight_thousandths, None);
    assert!(
        shipped
            .move_lot(
                &directory,
                1,
                1,
                "OTHER",
                "Cannot move dispatched stock",
                &actor(),
                "late-move",
                240
            )
            .is_err()
    );
    assert!(
        shipped
            .ship(&directory, &shipment(vec![1]), &actor(), "second-ship", 240)
            .is_err()
    );
    assert_eq!(
        shipped
            .ship(&directory, &shipment(vec![1, 2]), &actor(), "ship", 999)
            .unwrap()
            .0,
        shipped
    );
    assert_eq!(ledger.events.len(), 2);
}

#[test]
fn selected_review_is_pinned_and_later_documentary_changes_only_create_issues() {
    let original = ticket(1);
    let mut snapshot = ReviewSnapshot::from_ticket(&original).unwrap();
    snapshot.disposition = ReviewDisposition::Reviewed;
    let (reviews, _) = ReviewLedger::default()
        .record(
            &original,
            &snapshot,
            "Supervisor",
            "Reviewed fixture",
            "review-1",
            150,
        )
        .unwrap();
    let input = ReceiveInput {
        review_event_id: Some(1),
        ..receive_input(1)
    };
    let (ledger, pinned) = InventoryLedger::default()
        .receive(
            std::slice::from_ref(&original),
            &reviews,
            &InventoryDirectory::default(),
            &input,
            &actor(),
            "receive",
            200,
        )
        .unwrap();
    snapshot.source.source_name = "Corrected synthetic source".into();
    let (changed, _) = reviews
        .record(
            &original,
            &snapshot,
            "Supervisor",
            "Source correction",
            "review-2",
            210,
        )
        .unwrap();
    ledger
        .validate_against(
            std::slice::from_ref(&original),
            &changed,
            &InventoryDirectory::default(),
        )
        .unwrap();
    assert_eq!(
        ledger.project().unwrap().lot(1).unwrap().origin,
        pinned.origin
    );
    assert!(
        ledger
            .issues(&[original], &changed, &InventoryDirectory::default())
            .unwrap()
            .iter()
            .any(|issue| issue.code == "source-review-changed")
    );
}

#[test]
fn unknown_dates_measures_and_classification_remain_explicit_and_legacy_lines_work() {
    let mut original = ticket(1);
    original.lines[0].id = 0;
    original.lines[0].material_intake = MaterialIntake::default();
    let input = ReceiveInput {
        line_identity: LineIdentity::LegacyOrdinal(0),
        container: String::new(),
        received_at: None,
        first_accumulation_at: None,
        ..receive_input(1)
    };
    let (ledger, lot) = InventoryLedger::default()
        .receive(
            std::slice::from_ref(&original),
            &ReviewLedger::default(),
            &InventoryDirectory::default(),
            &input,
            &actor(),
            "legacy",
            200,
        )
        .unwrap();
    assert_eq!(lot.origin.line_identity, LineIdentity::LegacyOrdinal(0));
    assert_eq!(lot.origin.material.actual_weight_thousandths, None);
    assert_eq!(lot.origin.material.reporting_units, None);
    assert_eq!(lot.first_accumulation_at, None);
    let issues = ledger
        .issues(
            &[original],
            &ReviewLedger::default(),
            &InventoryDirectory::default(),
        )
        .unwrap();
    for code in [
        "received-date-unknown",
        "accumulation-date-unknown",
        "container-unknown",
        "reporting-units-unknown",
        "weight-unknown",
        "material-review",
    ] {
        assert!(issues.iter().any(|issue| issue.code == code), "{code}");
    }
}

#[test]
fn directory_edits_are_immutable_and_ids_never_ambiguous() {
    let original = InventoryDirectory::default();
    original.validate().unwrap();
    let (next, location) = original
        .save_location(&InventoryLocation {
            id: 0,
            name: " Training bay B ".into(),
            active: true,
        })
        .unwrap();
    assert_eq!(location.id, 2);
    assert_eq!(location.name, "Training bay B");
    assert_eq!(original.locations.len(), 1);
    assert!(
        next.save_location(&InventoryLocation {
            id: 999,
            ..location.clone()
        })
        .is_err()
    );
    assert!(
        next.save_location(&InventoryLocation {
            id: 0,
            name: "TRAINING BAY B".into(),
            active: true
        })
        .is_err()
    );
    let mut corrupt = next.clone();
    corrupt.next_id = 1;
    assert!(corrupt.validate().is_err());
    let mut corrupt = next;
    corrupt.locations.push(location);
    assert!(corrupt.validate().is_err());
}

#[cfg(test)]
#[path = "inventory_replay_tests.rs"]
mod replay;
