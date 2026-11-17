use super::receipt_delivery_compat_tests::{
    PrinterTestAccess, printer_test_access, printer_test_actor, printer_test_reopen_access,
};
use super::*;
use crate::{
    domain::Payment,
    printing::CopyKind,
    receipt_dispatch::{DispatchOutcome, DispatchState},
    store::PrepareDispatch,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);
struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "ewaste-dispatch-{}-{}-{}.sqlite3",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
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
fn paid(store: &Store, access: &PrinterTestAccess) -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    store.test_printer_ticket(access, &ticket).unwrap()
}
fn submit(store: &Store, access: &PrinterTestAccess, key: &str) -> super::PreparedDispatch {
    match store
        .test_prepare_dispatch(access, 1, CopyKind::Customer, key, "Chosen printer")
        .unwrap()
    {
        PrepareDispatch::Submit(prepared) => prepared,
        PrepareDispatch::Existing(_) => {
            panic!("new request must return saved submission capability")
        }
    }
}
fn state_rows(store: &Store) -> BTreeMap<String, String> {
    let statement = store
        .statement("SELECT key,payload FROM app_state ORDER BY key")
        .unwrap();
    let mut result = BTreeMap::new();
    while statement.step().unwrap() == ROW {
        result.insert(
            statement.read_text(0).unwrap(),
            statement.read_text(1).unwrap(),
        );
    }
    result
}

#[test]
fn explicit_queue_cas_and_attribution_do_not_change_ticket_or_legacy_preview_log() {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let ticket = paid(&store, &access);
    assert_eq!(store.receipt_printer_queue().unwrap(), None);
    assert!(
        store
            .test_prepare_dispatch(&access, 1, CopyKind::Customer, "first", "Chosen printer")
            .is_err()
    );
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    assert!(
        store
            .test_printer_queue(&access, None, Some("Another printer"))
            .is_err()
    );
    assert!(
        store
            .test_printer_queue(&access, Some("Chosen printer"), Some("Bad\nPrinter"))
            .is_err()
    );
    let prepared = submit(&store, &access, "first");
    assert_eq!(prepared.ticket_id(), ticket.id);
    assert_eq!(prepared.attempt_id(), 1);
    assert_eq!(prepared.copy(), CopyKind::Customer);
    assert_eq!(prepared.queue(), "Chosen printer");
    assert_eq!(
        prepared.receipt(),
        format!("CUSTOMER RECEIPT\n{}", ticket.receipt())
    );
    assert_eq!(store.list().unwrap(), vec![ticket]);
    assert!(store.print_log().unwrap().jobs.is_empty());
    assert_eq!(
        store
            .receipt_dispatch_log()
            .unwrap()
            .job(prepared.job_id())
            .unwrap()
            .state(),
        &DispatchState::Dispatching
    );
    let ledger = store.receipt_dispatch_log().unwrap();
    assert_eq!(ledger.jobs[0].actor, printer_test_actor(&access));
    assert_eq!(
        store.security_audit().unwrap().events.last().unwrap().actor,
        Some(printer_test_actor(&access))
    );
    #[cfg(feature = "extended-workflows")]
    {
        access.revoke();
        assert!(
            store
                .test_printer_queue(&access, Some("Chosen printer"), None)
                .is_err()
        );
        assert!(
            store
                .test_prepare_dispatch(&access, 1, CopyKind::Customer, "unsigned", "Chosen printer")
                .is_err()
        );
    }
}

#[test]
fn saved_intent_survives_restart_without_automatic_resubmission_or_different_key_bypass() {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let ticket = paid(&store, &access);
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    let prepared = submit(&store, &access, "physical-customer:1");
    let before = store.receipt_dispatch_log().unwrap();
    drop(store);
    let reopened = db.open();
    let access = printer_test_reopen_access(&reopened);
    match reopened
        .test_prepare_dispatch(
            &access,
            1,
            CopyKind::Customer,
            "physical-customer:1",
            "Chosen printer",
        )
        .unwrap()
    {
        PrepareDispatch::Existing(job) => assert_eq!(job.id, prepared.job_id()),
        PrepareDispatch::Submit(_) => {
            panic!("restart cannot repeat a potentially submitted request")
        }
    }
    assert!(
        reopened
            .test_prepare_dispatch(
                &access,
                1,
                CopyKind::Customer,
                "changed-key",
                "Chosen printer"
            )
            .is_err()
    );
    assert!(
        reopened
            .test_retry_dispatch(&access, prepared.job_id(), "retry")
            .is_err()
    );
    assert!(
        reopened
            .record_receipt_dispatch_result(
                prepared,
                DispatchOutcome::Accepted { windows_job_id: 9 }
            )
            .is_err()
    );
    assert_eq!(reopened.receipt_dispatch_log().unwrap(), before);
    assert_eq!(reopened.list().unwrap(), vec![ticket]);
}

#[test]
fn result_capability_is_idempotent_and_cannot_cross_registers() {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let ticket = paid(&store, &access);
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    let prepared = submit(&store, &access, "first");
    let second = Database::new();
    let other = second.open();
    let _ = printer_test_access(&other);
    assert!(
        other
            .record_receipt_dispatch_result(
                prepared.clone(),
                DispatchOutcome::Accepted { windows_job_id: 9 }
            )
            .is_err()
    );
    #[cfg(feature = "extended-workflows")]
    store.sign_out(&access).unwrap();
    let accepted = store
        .record_receipt_dispatch_result(
            prepared.clone(),
            DispatchOutcome::Accepted { windows_job_id: 9 },
        )
        .unwrap();
    assert_eq!(
        accepted.state(),
        &DispatchState::Accepted { windows_job_id: 9 }
    );
    let rows = state_rows(&store);
    assert_eq!(
        store
            .record_receipt_dispatch_result(
                prepared.clone(),
                DispatchOutcome::Accepted { windows_job_id: 9 }
            )
            .unwrap(),
        accepted
    );
    assert_eq!(state_rows(&store), rows);
    assert!(
        store
            .record_receipt_dispatch_result(
                prepared,
                DispatchOutcome::Unknown {
                    detail: "Cannot change known result".into()
                }
            )
            .is_err()
    );
    assert_eq!(store.list().unwrap(), vec![ticket]);
}

#[test]
fn proven_no_submission_needs_explicit_retry_and_old_capability_cannot_change_later_attempt() {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let _ = paid(&store, &access);
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    let first = submit(&store, &access, "first");
    store
        .record_receipt_dispatch_result(
            first.clone(),
            DispatchOutcome::NotSubmitted {
                detail: "OpenPrinter failed before submission".into(),
            },
        )
        .unwrap();
    let retry = match store
        .test_retry_dispatch(&access, first.job_id(), "explicit-retry")
        .unwrap()
    {
        PrepareDispatch::Submit(prepared) => prepared,
        _ => panic!("explicit retry should save a new attempt"),
    };
    assert_eq!(retry.job_id(), first.job_id());
    assert_eq!(retry.attempt_id(), 2);
    assert!(matches!(
        store
            .test_retry_dispatch(&access, first.job_id(), "explicit-retry")
            .unwrap(),
        PrepareDispatch::Existing(_)
    ));
    assert!(
        store
            .record_receipt_dispatch_result(first, DispatchOutcome::Accepted { windows_job_id: 5 })
            .is_err()
    );
    store
        .record_receipt_dispatch_result(
            retry,
            DispatchOutcome::Unknown {
                detail: "Write result uncertain".into(),
            },
        )
        .unwrap();
    assert!(store.test_retry_dispatch(&access, 1, "another").is_err());
    assert!(
        store
            .test_prepare_dispatch(&access, 1, CopyKind::Customer, "bypass", "Chosen printer")
            .is_err()
    );
}

#[test]
fn audit_failure_rolls_back_intent_and_result_without_losing_retry_capability() {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let _ = paid(&store, &access);
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    store.execute("CREATE TRIGGER deny_dispatch_audit BEFORE UPDATE ON app_state WHEN NEW.key='security_audit' BEGIN SELECT RAISE(ABORT,'synthetic dispatch audit failure'); END").unwrap();
    assert!(
        store
            .test_prepare_dispatch(&access, 1, CopyKind::Customer, "first", "Chosen printer")
            .is_err()
    );
    assert!(store.receipt_dispatch_log().unwrap().jobs.is_empty());
    store.execute("DROP TRIGGER deny_dispatch_audit").unwrap();
    let prepared = submit(&store, &access, "first");
    store.execute("CREATE TRIGGER deny_dispatch_audit BEFORE UPDATE ON app_state WHEN NEW.key='security_audit' BEGIN SELECT RAISE(ABORT,'synthetic dispatch audit failure'); END").unwrap();
    assert!(
        store
            .record_receipt_dispatch_result(
                prepared.clone(),
                DispatchOutcome::Accepted { windows_job_id: 8 }
            )
            .is_err()
    );
    assert_eq!(
        store.receipt_dispatch_log().unwrap().jobs[0].state(),
        &DispatchState::Dispatching
    );
    store.execute("DROP TRIGGER deny_dispatch_audit").unwrap();
    assert!(
        store
            .record_receipt_dispatch_result(
                prepared,
                DispatchOutcome::Accepted { windows_job_id: 8 }
            )
            .is_ok()
    );
}

#[test]
fn schema_six_migrates_insert_only_and_backup_preserves_dispatch_and_old_records() {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let ticket = paid(&store, &access);
    let preview = store.print_log().unwrap();
    let (preview, _) = preview
        .request(
            ticket.id,
            CopyKind::Customer,
            &ticket.receipt(),
            "old-preview",
            false,
        )
        .unwrap();
    store
        .test_printer_preview(&access, &Default::default(), &preview)
        .unwrap();
    store
        .execute("DELETE FROM app_state WHERE key IN ('receipt_dispatches','printer_settings')")
        .unwrap();
    store.execute("PRAGMA user_version=6").unwrap();
    let before = state_rows(&store);
    drop(store);
    let store = db.open();
    for (key, value) in before {
        assert_eq!(state_rows(&store).get(&key), Some(&value));
    }
    assert_eq!(store.print_log().unwrap(), preview);
    assert_eq!(store.list().unwrap(), vec![ticket]);
    assert_eq!(store.receipt_printer_queue().unwrap(), None);
    let access = printer_test_reopen_access(&store);
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    let prepared = submit(&store, &access, "first");
    store
        .record_receipt_dispatch_result(
            prepared,
            DispatchOutcome::Unknown {
                detail: "Keep this saved unknown result".into(),
            },
        )
        .unwrap();
    let log = store.receipt_dispatch_log().unwrap();
    let backup = Database::new();
    store.backup(&backup.0).unwrap();
    let copied = backup.open();
    assert_eq!(copied.receipt_dispatch_log().unwrap(), log);
    assert_eq!(
        copied.receipt_printer_queue().unwrap().as_deref(),
        Some("Chosen printer")
    );
    store
        .execute("DELETE FROM app_state WHERE key='receipt_dispatches'")
        .unwrap();
    drop(store);
    assert!(Store::open(&db.0).is_err());
}

#[test]
fn readonly_v6_backup_upgrades_only_the_copy_and_keeps_original_preview_and_auth_bytes() {
    let source = Database::new();
    let store = source.open();
    let access = printer_test_access(&store);
    let ticket = paid(&store, &access);
    store
        .execute("DELETE FROM app_state WHERE key IN ('receipt_dispatches','printer_settings')")
        .unwrap();
    store.execute("PRAGMA user_version=6").unwrap();
    let before_rows = state_rows(&store);
    drop(store);
    let before_bytes = std::fs::read(&source.0).unwrap();
    let source_store = Store::open_readonly(&source.0).unwrap();
    let output = Database::new();
    source_store.backup(&output.0).unwrap();
    drop(source_store);
    assert_eq!(std::fs::read(&source.0).unwrap(), before_bytes);
    let copy = output.open();
    let copy_rows = state_rows(&copy);
    for (key, data) in before_rows {
        assert_eq!(copy_rows.get(&key), Some(&data));
    }
    assert_eq!(copy.list().unwrap(), vec![ticket]);
    assert_eq!(copy.receipt_printer_queue().unwrap(), None);
    assert!(copy.receipt_dispatch_log().unwrap().jobs.is_empty());
    let _ = printer_test_reopen_access(&copy);
}

#[test]
fn draft_missing_ticket_and_bad_settings_are_rejected_without_saving_intent() {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let draft = store.seed_empty_register().unwrap();
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    assert!(
        store
            .test_prepare_dispatch(
                &access,
                draft.id,
                CopyKind::Customer,
                "draft",
                "Chosen printer"
            )
            .is_err()
    );
    assert!(
        store
            .test_prepare_dispatch(
                &access,
                999,
                CopyKind::Customer,
                "missing",
                "Chosen printer"
            )
            .is_err()
    );
    assert!(store.receipt_dispatch_log().unwrap().jobs.is_empty());
    store
        .write_state(
            "printer_settings",
            &serde_json::json!({"queue":"bad\nprinter"}),
        )
        .unwrap();
    drop(store);
    assert!(Store::open(&db.0).is_err());
    assert!(Store::open_readonly(&db.0).is_err());
}

#[test]
fn checked_new_copy_reroutes_an_unknown_request_with_explicit_attributed_link_and_no_payment_change()
 {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let ticket = paid(&store, &access);
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    let original = submit(&store, &access, "first");
    let prior = store
        .record_receipt_dispatch_result(
            original.clone(),
            DispatchOutcome::Unknown {
                detail: "Queue result uncertain".into(),
            },
        )
        .unwrap();
    store
        .test_printer_queue(&access, Some("Chosen printer"), Some("Replacement printer"))
        .unwrap();
    assert!(
        store
            .test_checked_copy(
                &access,
                prior.id,
                1,
                "checked-copy",
                "Replacement printer",
                false
            )
            .is_err()
    );
    assert!(
        store
            .test_checked_copy(
                &access,
                prior.id,
                1,
                "checked-copy",
                "Unsaved printer",
                true
            )
            .is_err()
    );
    let child = match store
        .test_checked_copy(
            &access,
            prior.id,
            1,
            "checked-copy",
            "Replacement printer",
            true,
        )
        .unwrap()
    {
        PrepareDispatch::Submit(prepared) => prepared,
        _ => panic!("checked copy requires a new saved intent"),
    };
    assert_eq!(child.receipt(), original.receipt());
    assert_eq!(child.queue(), "Replacement printer");
    let log = store.receipt_dispatch_log().unwrap();
    assert_eq!(log.jobs[0], prior);
    let link = log.jobs[1].checked_copy.as_ref().unwrap();
    assert_eq!(link.prior_job_id, prior.id);
    assert_eq!(link.prior_attempt_id, 1);
    assert_eq!(&link.checked_by, &printer_test_actor(&access));
    assert!(link.queue_and_paper_checked);
    let before = state_rows(&store);
    assert!(matches!(
        store
            .test_checked_copy(
                &access,
                prior.id,
                1,
                "checked-copy",
                "Replacement printer",
                true
            )
            .unwrap(),
        PrepareDispatch::Existing(_)
    ));
    assert_eq!(state_rows(&store), before);
    assert!(
        store
            .test_checked_copy(
                &access,
                prior.id,
                1,
                "duplicate-child",
                "Replacement printer",
                true
            )
            .is_err()
    );
    store
        .record_receipt_dispatch_result(child, DispatchOutcome::Accepted { windows_job_id: 91 })
        .unwrap();
    assert!(matches!(
        store
            .test_prepare_dispatch(
                &access,
                1,
                CopyKind::Customer,
                "deliberate-next-copy",
                "Replacement printer"
            )
            .unwrap(),
        PrepareDispatch::Submit(_)
    ));
    assert_eq!(store.list().unwrap(), vec![ticket]);
}

#[test]
fn late_original_result_survives_recovery_and_failed_recovery_audit_leaves_no_successor() {
    let db = Database::new();
    let store = db.open();
    let access = printer_test_access(&store);
    let _ = paid(&store, &access);
    store
        .test_printer_queue(&access, None, Some("Chosen printer"))
        .unwrap();
    let original = submit(&store, &access, "first");
    store.execute("CREATE TRIGGER deny_recovery_audit BEFORE UPDATE ON app_state WHEN NEW.key='security_audit' BEGIN SELECT RAISE(ABORT,'synthetic checked copy audit failure'); END").unwrap();
    assert!(
        store
            .test_checked_copy(
                &access,
                original.job_id(),
                1,
                "checked-copy",
                "Chosen printer",
                true
            )
            .is_err()
    );
    assert_eq!(store.receipt_dispatch_log().unwrap().jobs.len(), 1);
    store.execute("DROP TRIGGER deny_recovery_audit").unwrap();
    let child = match store
        .test_checked_copy(
            &access,
            original.job_id(),
            1,
            "checked-copy",
            "Chosen printer",
            true,
        )
        .unwrap()
    {
        PrepareDispatch::Submit(prepared) => prepared,
        _ => panic!("new checked intent expected"),
    };
    store
        .record_receipt_dispatch_result(original, DispatchOutcome::Accepted { windows_job_id: 71 })
        .unwrap();
    store
        .record_receipt_dispatch_result(child, DispatchOutcome::Accepted { windows_job_id: 72 })
        .unwrap();
    let log = store.receipt_dispatch_log().unwrap();
    assert_eq!(log.jobs.len(), 2);
    assert!(matches!(
        log.jobs[0].state(),
        DispatchState::Accepted { windows_job_id: 71 }
    ));
    drop(store);
    let reopened = db.open();
    assert_eq!(reopened.receipt_dispatch_log().unwrap(), log);
}
