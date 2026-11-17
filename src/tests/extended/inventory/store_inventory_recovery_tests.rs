use super::*;
use std::collections::BTreeMap;

fn raw_state(store: &Store) -> BTreeMap<String, String> {
    let statement = store
        .statement("SELECT key,payload FROM app_state ORDER BY key")
        .unwrap();
    let mut data = BTreeMap::new();
    while statement.step().unwrap() == ROW {
        data.insert(
            statement.read_text(0).unwrap(),
            statement.read_text(1).unwrap(),
        );
    }
    data
}
fn raw_tickets(store: &Store) -> Vec<(String, String, String)> {
    let statement = store
        .statement("SELECT id,created_at,payload FROM tickets ORDER BY id")
        .unwrap();
    let mut data = vec![];
    while statement.step().unwrap() == ROW {
        data.push((
            statement.read_text(0).unwrap(),
            statement.read_text(1).unwrap(),
            statement.read_text(2).unwrap(),
        ));
    }
    data
}

#[test]
fn schema_five_upgrade_preserves_security_price_adjustments_and_raw_tickets_without_backfilling_stock()
 {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let mut ticket = Ticket::new(1);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    let draft = store.save_ticket_as(&admin, &ticket).unwrap();
    let mut approved = store
        .approve_line_price_as(
            &admin,
            draft.revision,
            1,
            draft.lines[0].id,
            125,
            "Migration price fixture",
            "migration-price",
        )
        .unwrap();
    approved.confirm_cash("1.25").unwrap();
    approved.complete(Payment::Cash).unwrap();
    approved.revision += 1;
    let ticket = store.save_ticket_as(&admin, &approved).unwrap();
    let (pending, attempt) = store
        .start_adjustment_as(
            &admin,
            &store.adjustments().unwrap(),
            1,
            AdjustmentKind::Refund,
            25,
            "Unknown refund must survive inventory migration",
            "migration-refund",
            "",
        )
        .unwrap();
    let (unknown, _) = store
        .record_adjustment_result_as(
            &admin,
            &pending,
            1,
            attempt.id,
            AdjustmentOutcome::Unknown,
            false,
            "",
            "migration-refund-unknown",
            false,
        )
        .unwrap();
    let auth = store.auth_directory().unwrap();
    let prices = store.price_overrides().unwrap();
    store
        .execute("DELETE FROM app_state WHERE key IN ('inventory_directory','inventory_ledger')")
        .unwrap();
    store.execute("PRAGMA user_version=5").unwrap();
    let state = raw_state(&store);
    let tickets = raw_tickets(&store);
    drop(store);
    let store = db.open();
    let version = store.statement("PRAGMA user_version").unwrap();
    assert_eq!(version.step().unwrap(), ROW);
    assert_eq!(
        unsafe { (store.api.column_int)(version.stmt.as_ptr(), 0) },
        7
    );
    drop(version);
    let migrated = raw_state(&store);
    for (key, value) in state {
        assert!(
            migrated.get(&key) == Some(&value),
            "Existing app metadata changed during schema-six migration: {key}"
        );
    }
    assert!(
        raw_tickets(&store) == tickets,
        "Migration changed an original ticket payload"
    );
    assert_eq!(store.auth_directory().unwrap(), auth);
    assert_eq!(store.price_overrides().unwrap(), prices);
    assert_eq!(store.adjustments().unwrap(), unknown);
    assert!(store.inventory_ledger().unwrap().events.is_empty());
    assert_eq!(
        store.inventory_directory().unwrap(),
        InventoryDirectory::default()
    );
    let signed_in = store.sign_in("admin", PASSWORD).unwrap();
    assert_eq!(store.adjustments().unwrap(), unknown);
    let input = receive(&ticket);
    assert!(
        store
            .receive_inventory_lot_as(
                &signed_in,
                &store.inventory_ledger().unwrap(),
                &store.inventory_directory().unwrap(),
                &input,
                "after-migration-receive"
            )
            .is_ok()
    );
}

#[test]
fn inventory_backup_restore_keeps_lineage_and_missing_required_metadata_fails_closed() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let ticket = completed(&store, &admin, 1);
    let directory = store.inventory_directory().unwrap();
    let (ledger, _) = store
        .receive_inventory_lot_as(
            &admin,
            &store.inventory_ledger().unwrap(),
            &directory,
            &receive(&ticket),
            "backup-lot",
        )
        .unwrap();
    let auth = store.auth_directory().unwrap();
    let backup = db.0.with_extension("backup.sqlite3");
    let restored = db.0.with_extension("restored.sqlite3");
    store.backup_as(&admin, &backup).unwrap();
    let original_backup = std::fs::read(&backup).unwrap();
    restore_copy(&backup, &restored).unwrap();
    let copy = Store::open(&restored).unwrap();
    assert_eq!(copy.inventory_ledger().unwrap(), ledger);
    assert_eq!(copy.inventory_directory().unwrap(), directory);
    assert_eq!(copy.auth_directory().unwrap(), auth);
    assert_eq!(copy.list().unwrap(), vec![ticket]);
    assert!(copy.sign_in("admin", PASSWORD).is_ok());
    copy.execute("DELETE FROM app_state WHERE key='inventory_directory'")
        .unwrap();
    drop(copy);
    assert!(Store::open(&restored).is_err());
    store
        .execute("DELETE FROM app_state WHERE key='inventory_ledger'")
        .unwrap();
    drop(store);
    assert!(Store::open(&db.0).is_err());
    assert!(
        std::fs::read(&backup).unwrap() == original_backup,
        "Restore changed its source backup"
    );
    std::fs::remove_file(backup).unwrap();
    std::fs::remove_file(restored).unwrap();
}

#[test]
fn inventory_exports_mask_contacts_and_prose_and_reject_corrupt_pinned_evidence_without_success_audit()
 {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let ticket = completed(&store, &admin, 1);
    let directory = store.inventory_directory().unwrap();
    let (received, lot) = store
        .receive_inventory_lot_as(
            &admin,
            &store.inventory_ledger().unwrap(),
            &directory,
            &receive(&ticket),
            "export-lot",
        )
        .unwrap();
    store
        .ship_inventory_lots_as(
            &admin,
            &received,
            &directory,
            &shipment(vec![lot.id]),
            "export-shipment",
        )
        .unwrap();
    let default = store.prepare_inventory_export_as(&admin, false).unwrap();
    let contacts = store.prepare_inventory_export_as(&admin, true).unwrap();
    let all_default = format!("{}{}{}", default.lots, default.movements, default.shipments);
    let all_contacts = format!(
        "{}{}{}",
        contacts.lots, contacts.movements, contacts.shipments
    );
    for sensitive in [
        "PRIVATE_INVENTORY_SOURCE",
        "PRIVATE_INVENTORY_ADDRESS",
        "PRIVATE_INVENTORY_CONTACT",
        "PRIVATE_INVENTORY_PHONE",
        "PRIVATE_DESTINATION_ADDRESS",
        "PRIVATE_CARRIER_ADDRESS",
    ] {
        assert!(
            !all_default.contains(sensitive),
            "Default inventory export leaked contact field"
        );
        assert!(
            all_contacts.contains(sensitive),
            "Explicit contact export lost its approved field"
        );
    }
    for prose in [
        "PRIVATE_SOURCE_CIRCUMSTANCES",
        "PRIVATE_SOURCE_PROSE",
        "PRIVATE_MATERIAL_PROSE",
        "PRIVATE_RECEIVE_REASON",
        "PRIVATE_SHIPMENT_REASON",
    ] {
        assert!(!all_default.contains(prose));
        assert!(!all_contacts.contains(prose));
    }
    let audit = store.security_audit().unwrap();
    let mut corrupt = store.inventory_ledger().unwrap();
    let InventoryAction::Received { lot } = &mut corrupt.events[0].action else {
        panic!("First inventory fixture event should receive");
    };
    lot.origin.material.actual_weight_thousandths = Some(999);
    store.write_state("inventory_ledger", &corrupt).unwrap();
    assert!(store.inventory_ledger().is_err());
    assert!(store.prepare_inventory_export_as(&admin, false).is_err());
    assert_eq!(store.security_audit().unwrap(), audit);
}

#[test]
fn immutable_legacy_lines_use_ordinal_origin_without_rewriting_the_financial_ticket() {
    let db = Database::new();
    let store = db.open();
    let admin = administrator(&store);
    let mut ticket = completed(&store, &admin, 1);
    ticket.lines[0].id = 0;
    let mut update = store
        .statement("UPDATE tickets SET payload=?1 WHERE id=?2")
        .unwrap();
    update
        .text(1, &serde_json::to_string(&ticket).unwrap())
        .unwrap();
    update.text(2, &format!("{:020}", ticket.id)).unwrap();
    update.finish().unwrap();
    drop(update);
    let input = receive(&ticket);
    assert_eq!(input.line_identity, LineIdentity::LegacyOrdinal(0));
    let (ledger, lot) = store
        .receive_inventory_lot_as(
            &admin,
            &store.inventory_ledger().unwrap(),
            &store.inventory_directory().unwrap(),
            &input,
            "legacy-lot",
        )
        .unwrap();
    assert_eq!(lot.origin.line_identity, LineIdentity::LegacyOrdinal(0));
    assert_eq!(ledger.project().unwrap().lots.len(), 1);
    assert_eq!(store.list().unwrap(), vec![ticket]);
}
