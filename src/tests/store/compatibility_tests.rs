use super::*;
use crate::{
    domain::Payment,
    intake::{MaterialClass, MaterialCondition, MaterialIntake, SourceCategory, SourceIntake},
    operator_snapshot::{OperatorSnapshot, Role},
    price_overrides::PriceApproval,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);
struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "ewaste-basic-compat-{}-{stamp}-{}.sqlite3",
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
    }
}
fn historical_actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 17,
        login: "retained-supervisor".into(),
        display_name: "Retained named operator".into(),
        role: Role::Supervisor,
        revision: 1,
    }
}
fn raw_state(store: &Store) -> BTreeMap<String, String> {
    let rows = store
        .statement("SELECT key,payload FROM app_state ORDER BY key")
        .unwrap();
    let mut result = BTreeMap::new();
    while rows.step().unwrap() == ROW {
        result.insert(rows.read_text(0).unwrap(), rows.read_text(1).unwrap());
    }
    result
}
fn raw_ticket_payload(store: &Store, id: u64) -> String {
    let mut rows = store
        .statement("SELECT payload FROM tickets WHERE id=?1")
        .unwrap();
    rows.text(1, &format!("{id:020}")).unwrap();
    assert_eq!(rows.step().unwrap(), ROW);
    rows.read_text(0).unwrap()
}
fn raw_ticket(store: &Store, id: u64) -> Value {
    serde_json::from_str(&raw_ticket_payload(store, id)).unwrap()
}
fn insert_ticket(store: &Store, ticket: &Ticket) -> Value {
    if let Some(approval) = &ticket.lines[0].price_approval {
        let line = &ticket.lines[0];
        let basis = json!({"code":line.code,"name":line.name,"quantity":line.quantity,"unit":line.unit,"gross_thousandths":line.gross_thousandths,"tare_thousandths":line.tare_thousandths,"rule_snapshot":line.rule_snapshot,"rate_cents":line.rate_cents,"base_cents":line.base_cents,"minimum_cents":line.minimum_cents,"total_cents":line.total_cents});
        store.write_state("price_overrides",&json!({"events":[{"id":1,"request_key":"retained-approval","ticket_id":ticket.id,"line_id":line.id,"ticket_revision":0,"at":ticket.created_at,"actor":ticket.created_by,"reason":"Retained original approval reason","before":{"basis":basis,"approval":null},"after":{"basis":basis,"approval":approval},"action":{"Apply":{"approved_total_cents":125}}}],"next_id":2})).unwrap();
    }
    let mut raw = serde_json::to_value(ticket).unwrap();
    raw["future_ticket_extension"] = json!({"keep":"original ticket metadata"});
    raw["lines"][0]["future_line_extension"] = json!({"keep":"original line metadata"});
    raw["source_intake"]["future_source_extension"] = json!({"keep":"original source metadata"});
    let mut statement = store
        .statement("INSERT INTO tickets(id,created_at,payload) VALUES(?1,?2,?3)")
        .unwrap();
    statement.text(1, &format!("{:020}", ticket.id)).unwrap();
    statement
        .text(2, &format!("{:020}", ticket.created_at))
        .unwrap();
    statement
        .text(3, &serde_json::to_string(&raw).unwrap())
        .unwrap();
    statement.finish().unwrap();
    raw
}
fn priced_ticket(id: u64, actor: &OperatorSnapshot, completed: bool) -> Ticket {
    let mut ticket = Ticket::new(id);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket = ticket
        .with_source_intake(&SourceIntake {
            category: SourceCategory::Residential,
            source_name: "Retained source snapshot".into(),
            phone: "555-0188".into(),
            notes: "Retained original source note".into(),
            ..Default::default()
        })
        .unwrap();
    ticket.lines[0].material_intake = MaterialIntake {
        class: MaterialClass::OtherElectronics,
        condition: MaterialCondition::Intact,
        reporting_units: Some(2),
        actual_weight_thousandths: Some(7_500),
        notes: "Retained independent material snapshot".into(),
    };
    ticket.created_by = Some(actor.clone());
    ticket.revision = 1;
    if completed {
        ticket.confirm_cash("5").unwrap();
        ticket.complete(Payment::Cash).unwrap();
        ticket.completed_by = Some(actor.clone());
        let cash = ticket.tender_evidence.cash.as_mut().unwrap();
        cash.amount_due_cents = 125;
        cash.received_cents = 125;
        cash.change_cents = 0;
    }
    ticket.lines[0].price_approval = Some(PriceApproval {
        event_id: 1,
        approved_total_cents: 125,
    });
    if completed {
        ticket.receipt_snapshot = Some(ticket.render_receipt());
    }
    ticket
}
fn preserved_extensions(store: &Store) {
    store
        .write_state(
            "adjustments",
            &json!({"events":[],"next_id":1,"future_marker":"retained adjustment extension"}),
        )
        .unwrap();
    store.write_state("inventory_directory",&json!({"locations":[{"id":1,"name":"Retained storage snapshot","active":false}],"next_id":2})).unwrap();
    store.write_state("inventory_ledger",&json!({"events":[],"next_id":1,"next_lot_id":1,"next_shipment_id":1,"future_marker":"retained inventory extension"})).unwrap();
    store
        .write_state(
            "intake_reviews",
            &json!({"events":[],"next_id":1,"future_marker":"retained documentary extension"}),
        )
        .unwrap();
    store
        .write_state("closeouts", &Vec::<Value>::new())
        .unwrap();
    store
        .write_state("future_module", &json!({"keep":"unknown existing module"}))
        .unwrap();
}

#[test]
fn existing_v6_receipt_source_actors_and_opaque_records_survive_basic_reprint_and_backup() {
    let db = Database::new();
    let store = db.open();
    let actor = historical_actor();
    preserved_extensions(&store);
    let ticket = priced_ticket(1, &actor, true);
    let original = insert_ticket(&store, &ticket);
    let original_payload = raw_ticket_payload(&store, 1);
    let before = raw_state(&store);
    let receipt = ticket.receipt();
    drop(store);
    let store = db.open();
    assert!(
        raw_ticket_payload(&store, 1) == original_payload,
        "Opening the basic build rewrote its existing ticket payload"
    );
    assert_eq!(raw_ticket(&store, 1), original);
    let loaded = store.list().unwrap()[0].clone();
    assert_eq!(loaded, ticket);
    assert_eq!(loaded.total(), Some(125));
    assert_eq!(loaded.receipt(), receipt);
    assert_eq!(loaded.source_intake.category, SourceCategory::Residential);
    assert_eq!(
        loaded.lines[0].material_intake.actual_weight_thousandths,
        Some(7_500)
    );
    let mut reprinted = loaded;
    reprinted.reprint().unwrap();
    reprinted.revision += 1;
    let saved = store.save_counter_ticket(&reprinted).unwrap();
    assert_eq!(saved.receipt(), receipt);
    let raw = raw_ticket(&store, 1);
    for key in [
        "future_ticket_extension",
        "receipt_snapshot",
        "created_by",
        "completed_by",
        "source_intake",
    ] {
        assert_eq!(raw[key], original[key]);
    }
    assert_eq!(raw["lines"][0], original["lines"][0]);
    let after = raw_state(&store);
    for (key, payload) in before {
        if key != "security_audit" {
            assert!(
                after.get(&key) == Some(&payload),
                "Basic reprint rewrote unrelated metadata: {key}"
            );
        }
    }
    let backup = db.0.with_extension("backup.sqlite3");
    store.backup(&backup).unwrap();
    let restored = Store::open(&backup).unwrap();
    assert_eq!(raw_ticket(&restored, 1), raw);
    assert_eq!(restored.list().unwrap()[0].receipt(), receipt);
    drop(restored);
    std::fs::remove_file(backup).unwrap();
}

#[test]
fn basic_counter_refuses_new_price_authority_and_keeps_existing_approved_drafts_read_only() {
    let db = Database::new();
    let store = db.open();
    let actor = historical_actor();
    let mut fresh = Ticket::new(1);
    fresh.add_weight_line("803", "4", "0").unwrap();
    fresh.choose_blank().unwrap();
    fresh.lines[0].price_approval = Some(PriceApproval {
        event_id: 99,
        approved_total_cents: 1,
    });
    assert!(store.save_counter_ticket(&fresh).is_err());
    assert!(store.list().unwrap().is_empty());
    let old = priced_ticket(2, &actor, false);
    let original = insert_ticket(&store, &old);
    let before = raw_state(&store);
    for mode in 0..3 {
        let mut candidate = old.clone();
        candidate.revision += 1;
        match mode {
            0 => candidate.lines[0].notes = "An ordinary detail edit".into(),
            1 => candidate.lines[0].price_approval = None,
            _ => candidate.lines.clear(),
        };
        assert!(store.save_counter_ticket(&candidate).is_err());
    }
    assert_eq!(raw_ticket(&store, 2), original);
    assert!(
        raw_state(&store) == before,
        "Denied price edits changed metadata"
    );
}

#[test]
fn basic_open_fails_closed_for_missing_core_or_required_compatibility_metadata() {
    for key in [
        "security_audit",
        "adjustments",
        "price_overrides",
        "inventory_directory",
        "inventory_ledger",
        "receipt_dispatches",
        "printer_settings",
    ] {
        let db = Database::new();
        let store = db.open();
        let mut delete = store
            .statement("DELETE FROM app_state WHERE key=?1")
            .unwrap();
        delete.text(1, key).unwrap();
        delete.finish().unwrap();
        drop(delete);
        drop(store);
        assert!(
            Store::open(&db.0).is_err(),
            "Missing required metadata reopened a reset register: {key}"
        );
    }
}

#[test]
fn basic_counter_ignores_missing_or_invalid_legacy_login_records_without_rewriting_them() {
    for invalid in [false, true] {
        let db = Database::new();
        let store = db.open();
        for key in ["auth", "auth_setup", "login_throttle"] {
            let sql = if invalid {
                "UPDATE app_state SET payload=?2 WHERE key=?1"
            } else {
                "DELETE FROM app_state WHERE key=?1"
            };
            let mut statement = store.statement(sql).unwrap();
            statement.text(1, key).unwrap();
            if invalid {
                statement
                    .text(2, "opaque invalid legacy login metadata retained verbatim")
                    .unwrap();
            }
            statement.finish().unwrap();
        }
        let before = raw_state(&store);
        drop(store);
        let reopened = db.open();
        let mut ticket = Ticket::new(1);
        ticket.add_weight_line("803", "4", "0").unwrap();
        ticket.choose_blank().unwrap();
        let saved = reopened.save_counter_ticket(&ticket).unwrap();
        assert_eq!(saved.created_by, Some(OperatorSnapshot::local_counter()));
        let after = raw_state(&reopened);
        for key in ["auth", "auth_setup", "login_throttle"] {
            assert_eq!(
                after.get(key),
                before.get(key),
                "Basic action changed legacy login metadata {key}"
            );
        }
    }
}

#[test]
fn migration_preserves_existing_extension_payloads_instead_of_reinitializing_them() {
    let db = Database::new();
    let store = db.open();
    preserved_extensions(&store);
    store.execute("PRAGMA user_version=5").unwrap();
    let before = raw_state(&store);
    drop(store);
    let store = db.open();
    assert!(
        raw_state(&store) == before,
        "Migration reset an existing opaque extension record"
    );
}

#[test]
fn ordinary_billing_edits_preserve_existing_source_and_independent_reporting_snapshots() {
    let db = Database::new();
    let store = db.open();
    let actor = historical_actor();
    let mut original = priced_ticket(1, &actor, false);
    original.lines[0].price_approval = None;
    original.revision = 0;
    let mut insert = store
        .statement("INSERT INTO tickets(id,created_at,payload) VALUES(?1,?2,?3)")
        .unwrap();
    insert.text(1, &format!("{:020}", original.id)).unwrap();
    insert
        .text(2, &format!("{:020}", original.created_at))
        .unwrap();
    insert
        .text(3, &serde_json::to_string(&original).unwrap())
        .unwrap();
    insert.finish().unwrap();
    drop(insert);
    let mut revised = original.clone();
    revised.edit_weight_line(0, "20", "0").unwrap();
    revised.revision += 1;
    let saved = store.save_counter_ticket(&revised).unwrap();
    assert_eq!(saved.total(), Some(800));
    assert_eq!(saved.lines[0].gross_thousandths, Some(20_000));
    assert_eq!(
        saved.lines[0].material_intake,
        original.lines[0].material_intake
    );
    assert_eq!(
        saved.lines[0].material_intake.actual_weight_thousandths,
        Some(7_500)
    );
    assert_eq!(saved.source_intake, original.source_intake);
    assert_eq!(saved.created_by, original.created_by);
}

#[test]
fn legacy_zero_id_reporting_snapshot_cannot_be_changed_but_contact_corrections_preserve_it() {
    let db = Database::new();
    let store = db.open();
    let actor = historical_actor();
    let mut original = priced_ticket(1, &actor, false);
    original.lines[0].price_approval = None;
    original.lines[0].id = 0;
    original.next_line_id = 1;
    original.revision = 0;
    let mut insert = store
        .statement("INSERT INTO tickets(id,created_at,payload) VALUES(?1,?2,?3)")
        .unwrap();
    insert.text(1, &format!("{:020}", original.id)).unwrap();
    insert
        .text(2, &format!("{:020}", original.created_at))
        .unwrap();
    insert
        .text(3, &serde_json::to_string(&original).unwrap())
        .unwrap();
    insert.finish().unwrap();
    drop(insert);
    let before = raw_ticket_payload(&store, 1);
    let mut changed = original.clone();
    changed.revision += 1;
    changed.lines[0].material_intake.reporting_units = Some(3);
    changed.lines[0].material_intake.class = MaterialClass::CrtDisplay;
    changed.lines[0].material_intake.notes = "Changed legacy reporting snapshot".into();
    assert!(store.save_counter_ticket(&changed).is_err());
    assert!(
        raw_ticket_payload(&store, 1) == before,
        "Basic edit changed an immutable legacy reporting snapshot"
    );
    let mut contact = original.clone();
    contact.revision += 1;
    contact.source_intake.phone = "555-0189".into();
    let saved = store.save_counter_ticket(&contact).unwrap();
    assert_eq!(saved.lines, original.lines);
    assert_eq!(saved.source_intake.phone, "555-0189");
}

#[test]
fn unsnapshotted_legacy_tv_receipt_retains_classification_for_internal_copy_and_restart() {
    for code in ["801", "802"] {
        let db = Database::new();
        let store = db.open();
        let directory = store.directory().unwrap();
        let (next, customer) = directory
            .save_customer(&crate::customers::Customer {
                name: "Legacy saved customer".into(),
                ..Default::default()
            })
            .unwrap();
        store.save_counter_directory(&directory, &next).unwrap();
        let mut ticket = Ticket::new(1);
        if code == "801" {
            ticket.add_line(code, "1", "").unwrap();
        } else {
            ticket.add_weight_line(code, "4", "0").unwrap();
        }
        ticket.select_customer(&customer).unwrap();
        ticket
            .line_intake(0, "Legacy saved classification", "1", "")
            .unwrap();
        ticket.lines[0].id = 0;
        ticket.next_line_id = 1;
        ticket.state = State::Completed;
        ticket.payment = Some(Payment::Cash);
        ticket.completed_at = Some(crate::domain::now());
        ticket.tender_evidence.cash = Some(crate::tender::CashTender {
            amount_due_cents: 500,
            received_cents: 500,
            change_cents: 0,
        });
        let mut raw = serde_json::to_value(&ticket).unwrap();
        raw.as_object_mut().unwrap().remove("source_intake");
        raw.as_object_mut().unwrap().remove("receipt_snapshot");
        let payload = serde_json::to_string(&raw).unwrap();
        let mut insert = store
            .statement("INSERT INTO tickets(id,created_at,payload) VALUES(?1,?2,?3)")
            .unwrap();
        insert.text(1, &format!("{:020}", ticket.id)).unwrap();
        insert
            .text(2, &format!("{:020}", ticket.created_at))
            .unwrap();
        insert.text(3, &payload).unwrap();
        insert.finish().unwrap();
        drop(insert);
        let legacy = store.list().unwrap().remove(0);
        assert_eq!(legacy.source_intake.category, SourceCategory::Unspecified);
        assert!(legacy.receipt_snapshot.is_none());
        let receipt = legacy.receipt();
        assert!(
            receipt.contains("Classification: Legacy saved classification"),
            "Legacy {code} lost the saved classification on receipt reconstruction"
        );
        assert!(!receipt.contains("Source category: Unspecified"));
        assert!(!receipt.contains("Choose category"));
        let previous = store.print_log().unwrap();
        let (next, job) = previous
            .request(
                legacy.id,
                crate::printing::CopyKind::Internal,
                &receipt,
                "legacy-internal-copy",
                false,
            )
            .unwrap();
        store.save_counter_print_log(&previous, &next).unwrap();
        assert_eq!(job.receipt, receipt);
        let queue = "Synthetic legacy receipt queue";
        store.save_receipt_printer_queue(None, Some(queue)).unwrap();
        let physical = store
            .prepare_receipt_dispatch(
                legacy.id,
                crate::printing::CopyKind::Internal,
                "legacy-physical-internal-copy",
                queue,
            )
            .unwrap();
        let PrepareDispatch::Submit(capability) = physical else {
            panic!("Legacy internal copy must create a physical dispatch intent");
        };
        assert_eq!(capability.copy(), crate::printing::CopyKind::Internal);
        assert_eq!(capability.queue(), queue);
        assert!(capability.receipt().starts_with("INTERNAL COPY"));
        assert!(capability.receipt().ends_with(&receipt));
        let frozen_body = capability.receipt().to_owned();
        let accepted = store
            .record_receipt_dispatch_result(
                capability,
                crate::receipt_dispatch::DispatchOutcome::Accepted { windows_job_id: 71 },
            )
            .unwrap();
        assert_eq!(accepted.receipt, frozen_body);
        assert!(matches!(
            accepted.state(),
            crate::receipt_dispatch::DispatchState::Accepted { windows_job_id: 71 }
        ));
        assert!(
            raw_ticket_payload(&store, 1) == payload,
            "Internal copy rewrote the old ticket payload"
        );
        drop(store);
        let reopened = db.open();
        assert_eq!(reopened.list().unwrap()[0].receipt(), receipt);
        assert_eq!(reopened.print_log().unwrap().jobs[0].receipt, receipt);
        let physical = reopened.receipt_dispatch_log().unwrap();
        assert_eq!(physical.jobs, vec![accepted]);
        assert_eq!(physical.jobs[0].receipt, frozen_body);
        assert!(
            raw_ticket_payload(&reopened, 1) == payload,
            "Restart rewrote the old source-less ticket"
        );
    }
}

#[test]
fn schema_six_receipt_dispatch_upgrade_adds_only_missing_print_metadata() {
    let db = Database::new();
    let store = db.open();
    preserved_extensions(&store);
    let mut ticket = Ticket::new(1);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    store.save_counter_ticket(&ticket).unwrap();
    store
        .execute("DELETE FROM app_state WHERE key IN ('receipt_dispatches','printer_settings')")
        .unwrap();
    store.execute("PRAGMA user_version=6").unwrap();
    let state = raw_state(&store);
    let payload = raw_ticket_payload(&store, 1);
    drop(store);
    let reopened = db.open();
    let after = raw_state(&reopened);
    assert_eq!(after.len(), state.len() + 2);
    for (key, value) in state {
        assert!(
            after.get(&key) == Some(&value),
            "Print migration changed existing metadata: {key}"
        );
    }
    assert!(
        raw_ticket_payload(&reopened, 1) == payload,
        "Print migration rewrote an existing ticket"
    );
    assert!(reopened.receipt_dispatch_log().unwrap().jobs.is_empty());
    assert_eq!(reopened.receipt_printer_queue().unwrap(), None);
    let version = reopened.statement("PRAGMA user_version").unwrap();
    assert_eq!(version.step().unwrap(), ROW);
    assert_eq!(
        unsafe { (reopened.api.column_int)(version.stmt.as_ptr(), 0) },
        7
    );
}

#[test]
fn native_counter_ticket_saves_keep_price_tender_stale_and_frozen_receipt_guards() {
    let db = Database::new();
    let store = db.open();
    let mut input = Ticket::new(1);
    input.add_weight_line("803", "4", "0").unwrap();
    input.choose_blank().unwrap();
    let saved = store.save_counter_ticket(&input).unwrap();
    assert_eq!(saved.created_by, Some(OperatorSnapshot::local_counter()));
    let before = raw_state(&store);
    let payload = raw_ticket_payload(&store, 1);
    let mut forged = saved.clone();
    forged.revision += 1;
    forged.lines[0].total_cents = Some(1);
    assert!(store.save_counter_ticket(&forged).is_err());
    let mut unpaid = saved.clone();
    unpaid.revision += 1;
    unpaid.state = State::Completed;
    unpaid.payment = Some(Payment::Cash);
    assert!(store.save_counter_ticket(&unpaid).is_err());
    let mut fake_receipt = saved.clone();
    fake_receipt.revision += 1;
    fake_receipt.receipt_snapshot = Some("Forged draft receipt".into());
    assert!(store.save_counter_ticket(&fake_receipt).is_err());
    assert_eq!(raw_ticket_payload(&store, 1), payload);
    assert_eq!(raw_state(&store), before);
    let mut updated = saved.clone();
    updated.edit_weight_line(0, "5", "0").unwrap();
    updated.revision += 1;
    let updated = store.save_counter_ticket(&updated).unwrap();
    let mut stale = saved;
    stale.revision += 1;
    stale.line_intake(0, "", "", "Stale draft note").unwrap();
    assert!(store.save_counter_ticket(&stale).is_err());
    let mut paid = updated;
    paid.confirm_cash("5").unwrap();
    paid.revision += 1;
    let mut paid = store.save_counter_ticket(&paid).unwrap();
    paid.complete(Payment::Cash).unwrap();
    paid.revision += 1;
    let completed = store.save_counter_ticket(&paid).unwrap();
    assert_eq!(
        completed.completed_by,
        Some(OperatorSnapshot::local_counter())
    );
    assert_eq!(completed.receipt_snapshot, Some(completed.receipt()));
    let mut changed = completed.clone();
    changed.revision += 1;
    changed.lines[0].notes = "Changed final data".into();
    assert!(store.save_counter_ticket(&changed).is_err());
    assert_eq!(store.list().unwrap(), vec![completed]);
}

#[test]
fn native_counter_directory_and_preview_saves_keep_stale_and_saved_receipt_validation() {
    let db = Database::new();
    let store = db.open();
    let directory = store.directory().unwrap();
    let (next, customer) = directory
        .save_customer(&crate::customers::Customer {
            name: "Native counter customer".into(),
            phone: "555-0110".into(),
            ..Default::default()
        })
        .unwrap();
    store.save_counter_directory(&directory, &next).unwrap();
    let (mut stale, _) = directory
        .save_customer(&crate::customers::Customer {
            name: "Stale customer".into(),
            ..Default::default()
        })
        .unwrap();
    stale.next_customer_id += 1;
    assert!(store.save_counter_directory(&directory, &stale).is_err());
    assert_eq!(store.directory().unwrap(), next);
    let mut input = Ticket::new(1);
    input.add_weight_line("803", "4", "0").unwrap();
    input.select_customer(&customer).unwrap();
    input.confirm_cash("5").unwrap();
    input.complete(Payment::Cash).unwrap();
    let paid = store.save_counter_ticket(&input).unwrap();
    let current = store.print_log().unwrap();
    let (forged, _) = current
        .request(
            1,
            crate::printing::CopyKind::Customer,
            "Different receipt body",
            "forged-preview",
            false,
        )
        .unwrap();
    assert!(store.save_counter_print_log(&current, &forged).is_err());
    let (preview, _) = current
        .request(
            1,
            crate::printing::CopyKind::Customer,
            &paid.receipt(),
            "native-preview",
            false,
        )
        .unwrap();
    store.save_counter_print_log(&current, &preview).unwrap();
    assert_eq!(store.print_log().unwrap(), preview);
    let audit = store.security_audit().unwrap();
    assert!(
        audit
            .events
            .iter()
            .all(|event| event.actor.as_ref() == Some(&OperatorSnapshot::local_counter()))
    );
}

#[test]
fn native_counter_audit_failures_roll_back_ticket_directory_and_preview_writes() {
    let db = Database::new();
    let store = db.open();
    let mut paid = Ticket::new(1);
    paid.add_weight_line("803", "4", "0").unwrap();
    paid.choose_blank().unwrap();
    paid.confirm_cash("5").unwrap();
    paid.complete(Payment::Cash).unwrap();
    let paid = store.save_counter_ticket(&paid).unwrap();
    let directory = store.directory().unwrap();
    let (next_directory, _) = directory
        .save_customer(&crate::customers::Customer {
            name: "Rollback customer".into(),
            ..Default::default()
        })
        .unwrap();
    let log = store.print_log().unwrap();
    let (next_log, _) = log
        .request(
            1,
            crate::printing::CopyKind::Internal,
            &paid.receipt(),
            "rollback-preview",
            false,
        )
        .unwrap();
    let mut draft = Ticket::new(2);
    draft.add_weight_line("803", "4", "0").unwrap();
    draft.choose_blank().unwrap();
    let before = raw_state(&store);
    store.execute("CREATE TRIGGER deny_counter_audit BEFORE UPDATE ON app_state WHEN NEW.key='security_audit' BEGIN SELECT RAISE(ABORT,'synthetic counter audit failure'); END").unwrap();
    assert!(store.save_counter_ticket(&draft).is_err());
    assert!(
        store
            .save_counter_directory(&directory, &next_directory)
            .is_err()
    );
    assert!(store.save_counter_print_log(&log, &next_log).is_err());
    assert_eq!(store.list().unwrap(), vec![paid]);
    assert_eq!(store.directory().unwrap(), directory);
    assert_eq!(store.print_log().unwrap(), log);
    assert_eq!(raw_state(&store), before);
}
