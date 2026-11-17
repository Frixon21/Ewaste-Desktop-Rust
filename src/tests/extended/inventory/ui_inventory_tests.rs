use super::*;
use crate::{
    intake::{MaterialClass, MaterialIntake, SourceIntake},
    intake_review::LineIdentity,
    inventory::{InventoryLocation, LotStatus, PartySnapshot, ReceiveInput, ShipmentInput},
};
struct Db(std::path::PathBuf);
impl Db {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ewaste-native-inventory-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn app(&self) -> RegisterApp {
        RegisterApp::new(Store::open(&self.0.join("register.sqlite3")).unwrap()).unwrap()
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        if let (Ok(path), Ok(temp)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize())
            && path.starts_with(temp)
            && path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("ewaste-native-inventory-")
            })
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
fn completed(db: &Db) -> RegisterApp {
    let mut app = db.app();
    app.test_sign_in();
    app.save_action(|ticket| {
        ticket.add_weight_line("803", "4", "0")?;
        ticket.choose_blank()?;
        *ticket = ticket.with_source_intake(&SourceIntake {
            source_name: "PRIVATE_STOCK_SOURCE".into(),
            notes: "PRIVATE_STOCK_PROSE".into(),
            ..Default::default()
        })?;
        *ticket = ticket.with_material_intake(
            0,
            &MaterialIntake {
                class: MaterialClass::OtherElectronics,
                ..Default::default()
            },
        )?;
        ticket.confirm_cash("5")
    })
    .unwrap();
    app.run(Act::Complete).unwrap();
    app
}
fn receive(ticket: &Ticket) -> InventoryUiAction {
    InventoryUiAction::Receive {
        input: ReceiveInput {
            ticket_id: ticket.id,
            line_identity: LineIdentity::from_ticket(ticket, 0).unwrap(),
            review_event_id: None,
            location_id: 1,
            container: "Training bin A".into(),
            received_at: None,
            first_accumulation_at: None,
            reason: "PRIVATE_STOCK_RECEIVE_REASON".into(),
            confirmed: true,
        },
        request_key: "native-receive".into(),
    }
}

#[test]
fn native_whole_lot_receive_move_ship_and_masked_export_preserve_financial_ticket() {
    let db = Db::new();
    let mut app = completed(&db);
    let original = app.active().clone();
    let prints = app.wf.prints.clone();
    let receive = receive(&original);
    app.run_inventory_action(receive.clone()).unwrap();
    app.run_inventory_action(receive).unwrap();
    assert_eq!(app.inventory.ledger.events.len(), 1);
    app.run_inventory_action(InventoryUiAction::SaveLocation {
        location: InventoryLocation {
            id: 0,
            name: "Training outbound staging".into(),
            active: true,
        },
    })
    .unwrap();
    app.run_inventory_action(InventoryUiAction::Move {
        lot_id: 1,
        to_location_id: 2,
        container: "Training bin B".into(),
        reason: "PRIVATE_STOCK_MOVE_REASON".into(),
        request_key: "native-move".into(),
    })
    .unwrap();
    app.run_inventory_action(InventoryUiAction::Ship {
        input: ShipmentInput {
            lot_ids: vec![1],
            destination: PartySnapshot {
                name: "Synthetic downstream".into(),
                address: "PRIVATE_DESTINATION".into(),
            },
            carrier: Default::default(),
            evidence_reference: "TRAINING-OUT-001".into(),
            shipped_at: None,
            actual_weight_thousandths: Some(7000),
            reason: "PRIVATE_STOCK_SHIP_REASON".into(),
            confirmed: true,
        },
        request_key: "native-ship".into(),
    })
    .unwrap();
    let state = app.inventory.ledger.project().unwrap();
    let lot = state.lot(1).unwrap();
    assert_eq!(lot.status, LotStatus::Shipped);
    assert_eq!(lot.location.id, 2);
    assert_eq!(lot.origin.material.actual_weight_thousandths, None);
    assert_eq!(lot.received_at, None);
    assert_eq!(
        state.shipment(1).unwrap().actual_weight_thousandths,
        Some(7000)
    );
    assert_eq!(app.active(), &original);
    assert_eq!(app.wf.prints, prints);
    app.run_inventory_action(InventoryUiAction::Export {
        include_contacts: false,
    })
    .unwrap();
    let export_root = db.0.join("exports");
    let output = std::fs::read_dir(&export_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    for name in ["lots.csv", "movements.csv", "shipments.csv"] {
        let csv = std::fs::read_to_string(output.join(name)).unwrap();
        assert!(csv.starts_with("training_"));
        assert!(!csv.contains("PRIVATE_"));
    }
    drop(app);
    let app = db.app();
    assert!(app.security.session.is_none());
    assert_eq!(app.inventory.ledger.project().unwrap(), state);
    assert_eq!(app.active(), &original);
}

#[test]
fn unsigned_inventory_handler_preserves_ledger_and_refresh_preserves_new_ticket_order() {
    let db = Db::new();
    let mut app = completed(&db);
    let request = receive(app.active());
    let before = app.store.security_audit().unwrap();
    app.lock_register();
    assert!(app.run_inventory_action(request).is_err());
    assert_eq!(
        app.store.security_audit().unwrap().events.len(),
        before.events.len() + 1
    );
    assert!(app.store.inventory_ledger().unwrap().events.is_empty());
    app.test_sign_in();
    app.run(Act::New).unwrap();
    assert_eq!(app.active, 2);
    app.run(Act::Open(1)).unwrap();
    app.run_inventory_action(InventoryUiAction::Refresh)
        .unwrap();
    app.run(Act::New).unwrap();
    assert_eq!(app.active, 3);
    assert_eq!(app.tickets.len(), 3);
}
