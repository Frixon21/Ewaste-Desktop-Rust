use super::*;
use crate::{
    adjustments::{AdjustmentKind, AdjustmentOutcome},
    auth::{Permission, Role, Session},
    domain::Payment,
    intake::{MaterialClass, MaterialCondition, MaterialIntake, SourceCategory, SourceIntake},
    intake_review::{LineIdentity, ReviewSnapshot},
    inventory::{
        InventoryAction, InventoryDirectory, InventoryLocation, LotStatus, PartySnapshot,
        ReceiveInput, ShipmentInput,
    },
    security_audit::AuditAction,
};
use std::sync::atomic::{AtomicU64, Ordering};

const PASSWORD: &str = "Synthetic inventory operator password 2026";
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);
struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "ewaste-inventory-store-{}-{stamp}-{}.sqlite3",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn open(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}-journal", self.0.display()));
    }
}
fn administrator(store: &Store) -> Session {
    store
        .bootstrap_administrator("admin", "Synthetic inventory administrator", PASSWORD)
        .unwrap()
}
fn operator(store: &Store, admin: &Session, login: &str, role: Role) -> Session {
    store
        .create_operator_as(
            admin,
            &store.auth_directory().unwrap(),
            login,
            "Synthetic inventory operator",
            role,
            PASSWORD,
        )
        .unwrap();
    store.sign_in(login, PASSWORD).unwrap()
}
fn completed(store: &Store, session: &Session, id: u64) -> Ticket {
    let mut ticket = Ticket::new(id);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket = ticket
        .with_source_intake(&SourceIntake {
            category: SourceCategory::Residential,
            source_name: "PRIVATE_INVENTORY_SOURCE".into(),
            source_address: "PRIVATE_INVENTORY_ADDRESS".into(),
            contact_name: "PRIVATE_INVENTORY_CONTACT".into(),
            phone: "PRIVATE_INVENTORY_PHONE".into(),
            circumstances: "PRIVATE_SOURCE_CIRCUMSTANCES".into(),
            notes: "PRIVATE_SOURCE_PROSE".into(),
            ..Default::default()
        })
        .unwrap();
    ticket = ticket
        .with_material_intake(
            0,
            &MaterialIntake {
                class: MaterialClass::OtherElectronics,
                condition: MaterialCondition::Intact,
                notes: "PRIVATE_MATERIAL_PROSE".into(),
                ..Default::default()
            },
        )
        .unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    store.save_ticket_as(session, &ticket).unwrap()
}
fn receive(ticket: &Ticket) -> ReceiveInput {
    ReceiveInput {
        ticket_id: ticket.id,
        line_identity: LineIdentity::from_ticket(ticket, 0).unwrap(),
        review_event_id: None,
        location_id: 1,
        container: "Synthetic container A".into(),
        received_at: None,
        first_accumulation_at: None,
        reason: "PRIVATE_RECEIVE_REASON".into(),
        confirmed: true,
    }
}
fn shipment(ids: Vec<u64>) -> ShipmentInput {
    ShipmentInput {
        lot_ids: ids,
        destination: PartySnapshot {
            name: "Synthetic downstream recipient".into(),
            address: "PRIVATE_DESTINATION_ADDRESS".into(),
        },
        carrier: PartySnapshot {
            name: "Synthetic transporter".into(),
            address: "PRIVATE_CARRIER_ADDRESS".into(),
        },
        evidence_reference: "TRAINING-SHIPMENT-001".into(),
        shipped_at: None,
        actual_weight_thousandths: None,
        reason: "PRIVATE_SHIPMENT_REASON".into(),
        confirmed: true,
    }
}

#[test]
fn inventory_requires_live_supervisor_authority_and_stamps_the_signed_in_actor() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let cashier = operator(&store, &admin, "cashier", Role::Cashier);
    let supervisor = operator(&store, &admin, "supervisor", Role::Supervisor);
    assert!(
        store
            .require_session(&supervisor, Permission::ManageInventory)
            .is_ok()
    );
    assert!(
        store
            .require_session(&cashier, Permission::ManageInventory)
            .is_err()
    );
    let ticket = completed(&store, &cashier, 1);
    let input = receive(&ticket);
    let directory = store.inventory_directory().unwrap();
    let before = store.inventory_ledger().unwrap();
    let audit = store.security_audit().unwrap();
    assert!(
        store
            .receive_inventory_lot_as(&cashier, &before, &directory, &input, "cashier-receive")
            .is_err()
    );
    assert!(
        store
            .save_inventory_location_as(
                &cashier,
                &directory,
                &InventoryLocation {
                    id: 0,
                    name: "Denied location".into(),
                    active: true
                }
            )
            .is_err()
    );
    assert!(store.prepare_inventory_export_as(&cashier, false).is_err());
    assert_eq!(store.inventory_ledger().unwrap(), before);
    assert_eq!(store.security_audit().unwrap(), audit);
    let (ledger, lot) = store
        .receive_inventory_lot_as(
            &supervisor,
            &before,
            &directory,
            &input,
            "supervisor-receive",
        )
        .unwrap();
    assert_eq!(lot.received_by, *supervisor.actor());
    assert_eq!(ledger.events[0].actor, *supervisor.actor());
    assert_eq!(
        store
            .security_audit()
            .unwrap()
            .events
            .last()
            .unwrap()
            .action,
        AuditAction::InventoryRecorded
    );
    let audit = store.security_audit().unwrap();
    assert!(
        store
            .move_inventory_lot_as(
                &cashier,
                &ledger,
                &directory,
                lot.id,
                1,
                "Denied cashier container",
                "Not a supervisor",
                "cashier-move"
            )
            .is_err()
    );
    assert!(
        store
            .ship_inventory_lots_as(
                &cashier,
                &ledger,
                &directory,
                &shipment(vec![lot.id]),
                "cashier-ship"
            )
            .is_err()
    );
    assert_eq!(store.inventory_ledger().unwrap(), ledger);
    assert_eq!(store.security_audit().unwrap(), audit);
    assert_eq!(store.list().unwrap(), vec![ticket]);
}

#[test]
fn completion_and_refund_never_receive_stock_or_invent_reporting_weight() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let ticket = completed(&store, &admin, 1);
    let empty = store.inventory_ledger().unwrap();
    assert!(empty.events.is_empty());
    let (pending, attempt) = store
        .start_adjustment_as(
            &admin,
            &store.adjustments().unwrap(),
            1,
            AdjustmentKind::Refund,
            100,
            "Synthetic refund does not move stock",
            "no-stock-refund",
            "",
        )
        .unwrap();
    store
        .record_adjustment_result_as(
            &admin,
            &pending,
            1,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "",
            "no-stock-refund-result",
            false,
        )
        .unwrap();
    assert_eq!(store.inventory_ledger().unwrap(), empty);
    let directory = store.inventory_directory().unwrap();
    let mut input = receive(&ticket);
    input.confirmed = false;
    let before = store.security_audit().unwrap();
    assert!(
        store
            .receive_inventory_lot_as(&admin, &empty, &directory, &input, "unconfirmed-receive")
            .is_err()
    );
    assert_eq!(store.inventory_ledger().unwrap(), empty);
    assert_eq!(store.security_audit().unwrap(), before);
    input.confirmed = true;
    let (ledger, lot) = store
        .receive_inventory_lot_as(&admin, &empty, &directory, &input, "confirmed-receive")
        .unwrap();
    assert_eq!(ticket.lines[0].gross_thousandths, Some(4_000));
    assert_eq!(lot.origin.material.actual_weight_thousandths, None);
    assert_eq!(lot.origin.material.reporting_units, None);
    assert_eq!(lot.received_at, None);
    assert_eq!(lot.first_accumulation_at, None);
    assert_eq!(ledger.project().unwrap().lots.len(), 1);
    assert_eq!(store.list().unwrap(), vec![ticket]);
}

#[test]
fn stale_directory_and_ledger_are_rejected_but_exact_retry_survives_location_changes() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let first = completed(&store, &admin, 1);
    let second = completed(&store, &admin, 2);
    let initial_directory = store.inventory_directory().unwrap();
    let initial_ledger = store.inventory_ledger().unwrap();
    let input = receive(&first);
    let (received, lot) = store
        .receive_inventory_lot_as(
            &admin,
            &initial_ledger,
            &initial_directory,
            &input,
            "received-once",
        )
        .unwrap();
    let directory = store
        .save_inventory_location_as(
            &admin,
            &initial_directory,
            &InventoryLocation {
                id: 0,
                name: "Second training area".into(),
                active: true,
            },
        )
        .unwrap();
    let audit = store.security_audit().unwrap();
    assert!(
        store
            .receive_inventory_lot_as(
                &admin,
                &received,
                &initial_directory,
                &receive(&second),
                "stale-directory"
            )
            .is_err()
    );
    assert!(
        store
            .receive_inventory_lot_as(
                &admin,
                &initial_ledger,
                &directory,
                &receive(&second),
                "stale-ledger"
            )
            .is_err()
    );
    assert_eq!(store.security_audit().unwrap(), audit);
    let _ = store
        .save_inventory_location_as(
            &admin,
            &directory,
            &InventoryLocation {
                id: 1,
                name: "Renamed inactive training area".into(),
                active: false,
            },
        )
        .unwrap();
    let before_retry = store.security_audit().unwrap();
    let (retried, same) = store
        .receive_inventory_lot_as(
            &admin,
            &initial_ledger,
            &initial_directory,
            &input,
            "received-once",
        )
        .unwrap();
    assert_eq!(retried, received);
    assert_eq!(same, lot);
    assert_eq!(store.security_audit().unwrap(), before_retry);
    admin.revoke();
    assert!(
        store
            .receive_inventory_lot_as(
                &admin,
                &received,
                &initial_directory,
                &input,
                "received-once"
            )
            .is_err()
    );
    assert_eq!(store.inventory_ledger().unwrap(), received);
}

#[test]
fn whole_lot_moves_and_multi_lot_shipping_preserve_dates_and_refuse_duplicate_dispatch() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let first = completed(&store, &admin, 1);
    let second = completed(&store, &admin, 2);
    let initial_directory = store.inventory_directory().unwrap();
    let directory = store
        .save_inventory_location_as(
            &admin,
            &initial_directory,
            &InventoryLocation {
                id: 0,
                name: "Shipping training area".into(),
                active: true,
            },
        )
        .unwrap();
    let destination = directory
        .locations
        .iter()
        .find(|location| location.id != 1)
        .unwrap()
        .id;
    let mut input = receive(&first);
    input.received_at = Some(crate::domain::now().saturating_sub(86_400));
    input.first_accumulation_at = Some(crate::domain::now().saturating_sub(172_800));
    let (ledger, lot1) = store
        .receive_inventory_lot_as(
            &admin,
            &store.inventory_ledger().unwrap(),
            &directory,
            &input,
            "first-lot",
        )
        .unwrap();
    let (ledger, lot2) = store
        .receive_inventory_lot_as(&admin, &ledger, &directory, &receive(&second), "second-lot")
        .unwrap();
    let (moved, lot1) = store
        .move_inventory_lot_as(
            &admin,
            &ledger,
            &directory,
            lot1.id,
            destination,
            "Synthetic shipping container",
            "Whole-lot location move",
            "move-first-lot",
        )
        .unwrap();
    assert_eq!(lot1.received_at, input.received_at);
    assert_eq!(lot1.first_accumulation_at, input.first_accumulation_at);
    assert_eq!(lot1.location.id, destination);
    let audit = store.security_audit().unwrap();
    for ids in [vec![lot1.id, lot1.id], vec![lot1.id, 999]] {
        assert!(
            store
                .ship_inventory_lots_as(
                    &admin,
                    &moved,
                    &directory,
                    &shipment(ids),
                    "invalid-multi-lot"
                )
                .is_err()
        );
        assert_eq!(store.inventory_ledger().unwrap(), moved);
        assert_eq!(store.security_audit().unwrap(), audit);
    }
    let ship = shipment(vec![lot1.id, lot2.id]);
    let (shipped, dispatch) = store
        .ship_inventory_lots_as(&admin, &moved, &directory, &ship, "whole-lot-dispatch")
        .unwrap();
    assert_eq!(dispatch.actual_weight_thousandths, None);
    assert_eq!(dispatch.shipped_at, None);
    let state = shipped.project().unwrap();
    assert_eq!(state.lots.len(), 2);
    assert!(
        state
            .lots
            .iter()
            .all(|lot| lot.status == LotStatus::Shipped && lot.shipment_id == Some(dispatch.id))
    );
    let audit = store.security_audit().unwrap();
    let (retried, same) = store
        .ship_inventory_lots_as(&admin, &moved, &directory, &ship, "whole-lot-dispatch")
        .unwrap();
    assert_eq!(retried, shipped);
    assert_eq!(same, dispatch);
    assert_eq!(store.security_audit().unwrap(), audit);
    assert!(
        store
            .ship_inventory_lots_as(&admin, &shipped, &directory, &ship, "second-dispatch")
            .is_err()
    );
    assert!(
        store
            .receive_inventory_lot_as(
                &admin,
                &shipped,
                &directory,
                &input,
                "receive-shipped-origin-again"
            )
            .is_err()
    );
    assert_eq!(store.inventory_ledger().unwrap(), shipped);
    assert_eq!(store.list().unwrap().len(), 2);
}

#[test]
fn source_review_is_pinned_and_later_documentary_corrections_do_not_rewrite_the_lot() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let ticket = completed(&store, &admin, 1);
    let initial = store.intake_reviews().unwrap();
    let mut snapshot = ReviewSnapshot::from_ticket(&ticket).unwrap();
    snapshot.source.source_name = "PINNED_REVIEW_SOURCE".into();
    snapshot.materials[0].details.reporting_units = Some(2);
    snapshot.materials[0].details.actual_weight_thousandths = Some(9_000);
    let (candidate, _) = initial
        .record(
            &ticket,
            &snapshot,
            &admin.actor().display_name,
            "First documentary evidence",
            "first-review",
            crate::domain::now(),
        )
        .unwrap();
    let saved = store
        .save_intake_reviews_as(&admin, &initial, &candidate)
        .unwrap();
    let event = saved.events.last().unwrap();
    let mut input = receive(&ticket);
    input.review_event_id = Some(event.id);
    let directory = store.inventory_directory().unwrap();
    let (ledger, lot) = store
        .receive_inventory_lot_as(
            &admin,
            &store.inventory_ledger().unwrap(),
            &directory,
            &input,
            "receive-pinned-review",
        )
        .unwrap();
    assert_eq!(lot.origin.review_event_id, Some(event.id));
    assert_eq!(lot.origin.source, snapshot.source);
    assert_eq!(lot.origin.material, snapshot.materials[0].details);
    let mut corrected = snapshot;
    corrected.source.source_name = "NEWER_REVIEW_SOURCE".into();
    corrected.materials[0].details.actual_weight_thousandths = Some(11_000);
    let (candidate, _) = saved
        .record(
            &ticket,
            &corrected,
            &admin.actor().display_name,
            "Later documentary correction",
            "later-review",
            crate::domain::now(),
        )
        .unwrap();
    let corrected = store
        .save_intake_reviews_as(&admin, &saved, &candidate)
        .unwrap();
    assert_eq!(store.inventory_ledger().unwrap(), ledger);
    assert_eq!(ledger.project().unwrap().lot(lot.id), Some(&lot));
    assert!(
        !ledger
            .issues(&store.list().unwrap(), &corrected, &directory)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.list().unwrap(), vec![ticket]);
}

#[test]
fn audit_failure_rolls_back_inventory_and_location_writes_atomically() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let ticket = completed(&store, &admin, 1);
    let directory = store.inventory_directory().unwrap();
    let ledger = store.inventory_ledger().unwrap();
    let audit = store.security_audit().unwrap();
    store.execute("CREATE TRIGGER fail_inventory_audit BEFORE UPDATE ON app_state WHEN NEW.key='security_audit' BEGIN SELECT RAISE(ABORT,'Synthetic inventory audit failure'); END").unwrap();
    assert!(
        store
            .receive_inventory_lot_as(
                &admin,
                &ledger,
                &directory,
                &receive(&ticket),
                "atomic-receive"
            )
            .is_err()
    );
    assert!(
        store
            .save_inventory_location_as(
                &admin,
                &directory,
                &InventoryLocation {
                    id: 0,
                    name: "Atomic training area".into(),
                    active: true
                }
            )
            .is_err()
    );
    assert_eq!(store.inventory_ledger().unwrap(), ledger);
    assert_eq!(store.inventory_directory().unwrap(), directory);
    assert_eq!(store.security_audit().unwrap(), audit);
    store.execute("DROP TRIGGER fail_inventory_audit").unwrap();
    assert!(
        store
            .receive_inventory_lot_as(
                &admin,
                &ledger,
                &directory,
                &receive(&ticket),
                "atomic-receive"
            )
            .is_ok()
    );
}

#[test]
fn failed_shipment_audit_leaves_every_lot_on_hand_and_the_same_request_can_retry() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let first = completed(&store, &admin, 1);
    let second = completed(&store, &admin, 2);
    let directory = store.inventory_directory().unwrap();
    let (ledger, first) = store
        .receive_inventory_lot_as(
            &admin,
            &store.inventory_ledger().unwrap(),
            &directory,
            &receive(&first),
            "atomic-ship-first",
        )
        .unwrap();
    let (ledger, second) = store
        .receive_inventory_lot_as(
            &admin,
            &ledger,
            &directory,
            &receive(&second),
            "atomic-ship-second",
        )
        .unwrap();
    let input = shipment(vec![first.id, second.id]);
    let audit = store.security_audit().unwrap();
    store.execute("CREATE TRIGGER fail_shipment_audit BEFORE UPDATE ON app_state WHEN NEW.key='security_audit' BEGIN SELECT RAISE(ABORT,'Synthetic shipment audit failure'); END").unwrap();
    assert!(
        store
            .ship_inventory_lots_as(&admin, &ledger, &directory, &input, "atomic-shipment")
            .is_err()
    );
    assert_eq!(store.inventory_ledger().unwrap(), ledger);
    assert_eq!(store.security_audit().unwrap(), audit);
    assert!(
        store
            .inventory_ledger()
            .unwrap()
            .project()
            .unwrap()
            .lots
            .iter()
            .all(|lot| lot.status == LotStatus::OnHand)
    );
    store.execute("DROP TRIGGER fail_shipment_audit").unwrap();
    let (saved, shipment) = store
        .ship_inventory_lots_as(&admin, &ledger, &directory, &input, "atomic-shipment")
        .unwrap();
    assert_eq!(shipment.lot_ids.len(), 2);
    assert!(
        saved
            .project()
            .unwrap()
            .lots
            .iter()
            .all(|lot| lot.status == LotStatus::Shipped && lot.shipment_id == Some(shipment.id))
    );
}

#[path = "store_inventory_recovery_tests.rs"]
mod recovery;
