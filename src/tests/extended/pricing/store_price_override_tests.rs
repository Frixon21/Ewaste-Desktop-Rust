#![cfg(feature = "extended-workflows")]
use super::*;
use crate::{
    adjustments::{AdjustmentKind, AdjustmentOutcome},
    auth::{Permission, Role, Session},
    domain::Payment,
    price_overrides::PriceApproval,
    tender::CardOutcome,
};
use std::sync::atomic::{AtomicU64, Ordering};

const PASSWORD: &str = "Synthetic price approval password 2026";
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

struct Db(std::path::PathBuf);
impl Db {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "ewaste-price-store-{}-{stamp}-{}.sqlite3",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn open(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}-journal", self.0.display()));
    }
}
fn admin(store: &Store) -> Session {
    store
        .bootstrap_administrator("admin", "Synthetic administrator", PASSWORD)
        .unwrap()
}
fn operator(store: &Store, administrator: &Session, login: &str, role: Role) -> Session {
    let current = store.auth_directory().unwrap();
    store
        .create_operator_as(
            administrator,
            &current,
            login,
            "Synthetic price operator",
            role,
            PASSWORD,
        )
        .unwrap();
    store.sign_in(login, PASSWORD).unwrap()
}
fn draft(store: &Store, session: &Session, id: u64) -> Ticket {
    let mut ticket = Ticket::new(id);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    store.save_ticket_as(session, &ticket).unwrap()
}
fn save_change(store: &Store, session: &Session, mut ticket: Ticket) -> Ticket {
    ticket.revision += 1;
    store.save_ticket_as(session, &ticket).unwrap()
}

#[test]
fn supervisor_approval_preserves_baseline_and_cashier_cannot_forge_or_clear_it() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let supervisor = operator(&store, &administrator, "supervisor", Role::Supervisor);
    let cashier = operator(&store, &administrator, "cashier", Role::Cashier);
    assert!(
        store
            .require_session(&supervisor, Permission::OverridePrices)
            .is_ok()
    );
    assert!(
        store
            .require_session(&cashier, Permission::OverridePrices)
            .is_err()
    );
    let before = draft(&store, &cashier, 1);
    let line_id = before.lines[0].id;
    let original_ledger = store.price_overrides().unwrap();
    let original_audit = store.security_audit().unwrap();
    assert!(
        store
            .approve_line_price_as(
                &cashier,
                before.revision,
                1,
                line_id,
                125,
                "Synthetic waiver",
                "cashier-denied"
            )
            .is_err()
    );
    assert_eq!(store.price_overrides().unwrap(), original_ledger);
    assert_eq!(store.security_audit().unwrap(), original_audit);
    for session in [&cashier, &administrator] {
        let mut forged = before.clone();
        forged.revision += 1;
        forged.lines[0].price_approval = Some(PriceApproval {
            event_id: 999,
            approved_total_cents: 1,
        });
        assert!(store.save_ticket_as(session, &forged).is_err());
    }
    let approved = store
        .approve_line_price_as(
            &supervisor,
            before.revision,
            1,
            line_id,
            125,
            " Synthetic waiver ",
            "approved",
        )
        .unwrap();
    assert_eq!(approved.revision, before.revision + 1);
    assert_eq!(approved.total(), Some(125));
    assert_eq!(approved.lines[0].effective_total_cents(), Some(125));
    let mut baseline = approved.lines[0].clone();
    baseline.price_approval = None;
    assert_eq!(baseline, before.lines[0]);
    let ledger = store.price_overrides().unwrap();
    assert_eq!(ledger.events.len(), 1);
    let json = serde_json::to_value(&ledger).unwrap();
    assert_eq!(json["events"][0]["actor"]["id"], supervisor.actor().id);
    assert!(
        serde_json::to_string(&ledger)
            .unwrap()
            .contains("Synthetic waiver")
    );
    for session in [&cashier, &supervisor, &administrator] {
        let mut forged = approved.clone();
        forged.revision += 1;
        forged.lines[0].price_approval = None;
        assert!(store.save_ticket_as(session, &forged).is_err());
    }
    assert!(
        store
            .clear_line_price_as(
                &cashier,
                approved.revision,
                1,
                line_id,
                "Remove waiver",
                "clear-denied"
            )
            .is_err()
    );
    assert_eq!(store.list().unwrap()[0], approved);
}

#[test]
fn price_basis_changes_require_clear_and_deleted_line_keeps_approval_history() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let ticket = draft(&store, &administrator, 1);
    let line_id = ticket.lines[0].id;
    let approved = store
        .approve_line_price_as(
            &administrator,
            ticket.revision,
            1,
            line_id,
            125,
            "Approved training price",
            "approve",
        )
        .unwrap();
    let mut changed = approved.clone();
    let approval = changed.lines[0].price_approval.take();
    changed.edit_weight_line(0, "20", "0").unwrap();
    changed.lines[0].price_approval = approval;
    changed.revision += 1;
    assert!(store.save_ticket_as(&administrator, &changed).is_err());
    let cleared = store
        .clear_line_price_as(
            &administrator,
            approved.revision,
            1,
            line_id,
            "Restore baseline before correcting weight",
            "clear",
        )
        .unwrap();
    assert_eq!(cleared.total(), Some(500));
    assert!(cleared.lines[0].price_approval.is_none());
    assert_eq!(store.price_overrides().unwrap().events.len(), 2);
    let mut corrected = cleared.clone();
    corrected.edit_weight_line(0, "20", "0").unwrap();
    let corrected = save_change(&store, &administrator, corrected);
    assert_eq!(corrected.total(), Some(800));
    let approved = store
        .approve_line_price_as(
            &administrator,
            corrected.revision,
            1,
            line_id,
            700,
            "Corrected-load price approval",
            "approve-new",
        )
        .unwrap();
    let ledger = store.price_overrides().unwrap();
    let mut removed = approved;
    removed.remove_line(0).unwrap();
    let removed = save_change(&store, &administrator, removed);
    assert!(removed.lines.is_empty());
    assert_eq!(store.price_overrides().unwrap(), ledger);
    let mut added = removed;
    added.add_weight_line("803", "4", "0").unwrap();
    let added = save_change(&store, &administrator, added);
    assert!(added.lines[0].id > line_id);
    assert!(added.lines[0].price_approval.is_none());
    assert_eq!(added.total(), Some(500));
    assert_eq!(store.price_overrides().unwrap(), ledger);
}

#[test]
fn exact_approval_retry_after_completion_is_read_only_and_still_requires_live_permission() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let supervisor = operator(&store, &administrator, "supervisor", Role::Supervisor);
    let ticket = draft(&store, &administrator, 1);
    let line_id = ticket.lines[0].id;
    let approved = store
        .approve_line_price_as(
            &supervisor,
            ticket.revision,
            1,
            line_id,
            200,
            "Synthetic approval",
            "stable-key",
        )
        .unwrap();
    let ledger = store.price_overrides().unwrap();
    let audit = store.security_audit().unwrap();
    let repeated = store
        .approve_line_price_as(
            &supervisor,
            ticket.revision,
            1,
            line_id,
            200,
            "Synthetic approval",
            "stable-key",
        )
        .unwrap();
    assert_eq!(repeated, approved);
    assert_eq!(store.price_overrides().unwrap(), ledger);
    assert_eq!(store.security_audit().unwrap(), audit);
    assert!(
        store
            .approve_line_price_as(
                &supervisor,
                ticket.revision,
                1,
                line_id,
                201,
                "Synthetic approval",
                "stable-key"
            )
            .is_err()
    );
    assert!(
        store
            .approve_line_price_as(
                &supervisor,
                ticket.revision,
                1,
                line_id,
                200,
                "Synthetic approval",
                "new-stale-key"
            )
            .is_err()
    );
    let mut paid = approved;
    paid.confirm_cash("5").unwrap();
    let paid = save_change(&store, &administrator, paid);
    let mut complete = paid;
    complete.complete(Payment::Cash).unwrap();
    let complete = save_change(&store, &administrator, complete);
    let receipt = complete.receipt();
    let audit = store.security_audit().unwrap();
    assert_eq!(
        store
            .approve_line_price_as(
                &supervisor,
                ticket.revision,
                1,
                line_id,
                200,
                "Synthetic approval",
                "stable-key"
            )
            .unwrap(),
        complete
    );
    assert_eq!(store.security_audit().unwrap(), audit);
    assert_eq!(store.price_overrides().unwrap(), ledger);
    assert_eq!(store.list().unwrap()[0].receipt(), receipt);
    assert!(
        store
            .clear_line_price_as(
                &supervisor,
                complete.revision,
                1,
                line_id,
                "Too late",
                "late-clear"
            )
            .is_err()
    );
    supervisor.revoke();
    assert!(
        store
            .approve_line_price_as(
                &supervisor,
                ticket.revision,
                1,
                line_id,
                200,
                "Synthetic approval",
                "stable-key"
            )
            .is_err()
    );
}

#[test]
fn any_recorded_tender_history_blocks_new_approval_even_after_decline_or_cancellation() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    for (index, outcome) in [
        None,
        Some(CardOutcome::Pending),
        Some(CardOutcome::Declined),
        Some(CardOutcome::Cancelled),
    ]
    .into_iter()
    .enumerate()
    {
        let ticket = draft(&store, &administrator, index as u64 + 1);
        let mut tendered = ticket.clone();
        if let Some(outcome) = outcome {
            tendered
                .start_card("5", &format!("TEST-PRICE-TENDER-{index}"))
                .unwrap();
            if outcome != CardOutcome::Pending {
                tendered.card_result(outcome, false, false).unwrap();
            }
        } else {
            tendered.confirm_cash("5").unwrap();
        }
        let tendered = save_change(&store, &administrator, tendered);
        let before = store.price_overrides().unwrap();
        assert!(
            store
                .approve_line_price_as(
                    &administrator,
                    tendered.revision,
                    tendered.id,
                    tendered.lines[0].id,
                    100,
                    "No repricing after tender",
                    &format!("denied-{index}")
                )
                .is_err()
        );
        assert_eq!(store.price_overrides().unwrap(), before);
        assert_eq!(
            store
                .list()
                .unwrap()
                .into_iter()
                .find(|t| t.id == tendered.id)
                .unwrap(),
            tendered
        );
    }
}

#[test]
fn invalid_state_identity_reason_and_pending_price_are_rejected_without_partial_changes() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let ticket = draft(&store, &administrator, 1);
    let line_id = ticket.lines[0].id;
    let before = store.price_overrides().unwrap();
    for (id, line, amount, reason, key) in [
        (1, line_id, -1, "Reason", "negative"),
        (1, line_id, 100, "", "blank-reason"),
        (1, line_id, 100, "bad\nreason", "control-reason"),
        (1, 999, 100, "Reason", "wrong-line"),
        (999, line_id, 100, "Reason", "wrong-ticket"),
    ] {
        assert!(
            store
                .approve_line_price_as(
                    &administrator,
                    ticket.revision,
                    id,
                    line,
                    amount,
                    reason,
                    key
                )
                .is_err()
        );
    }
    let mut held = ticket.clone();
    held.hold().unwrap();
    let held = save_change(&store, &administrator, held);
    assert!(
        store
            .approve_line_price_as(
                &administrator,
                held.revision,
                1,
                line_id,
                100,
                "Held ticket",
                "held"
            )
            .is_err()
    );
    let mut pending = Ticket::new(2);
    pending.add_line("901", "", "").unwrap();
    let pending = store.save_ticket_as(&administrator, &pending).unwrap();
    assert!(
        store
            .approve_line_price_as(
                &administrator,
                pending.revision,
                2,
                pending.lines[0].id,
                100,
                "No baseline",
                "pending-price"
            )
            .is_err()
    );
    let mut legacy = draft(&store, &administrator, 3);
    legacy.lines[0].id = 0;
    legacy.next_line_id = 1;
    let mut statement = store
        .statement("UPDATE tickets SET payload=?1 WHERE id=?2")
        .unwrap();
    statement
        .text(1, &serde_json::to_string(&legacy).unwrap())
        .unwrap();
    statement.text(2, &format!("{:020}", legacy.id)).unwrap();
    statement.finish().unwrap();
    drop(statement);
    assert!(
        store
            .approve_line_price_as(
                &administrator,
                legacy.revision,
                3,
                0,
                100,
                "Legacy line not identified",
                "legacy"
            )
            .is_err()
    );
    assert_eq!(store.price_overrides().unwrap(), before);
}

#[test]
fn configured_manual_baseline_can_be_approved_and_zero_does_not_bypass_intake_policy() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let mut manual = Ticket::new(1);
    manual.add_line("901", "", "2.75").unwrap();
    let manual = store.save_ticket_as(&administrator, &manual).unwrap();
    let approved = store
        .approve_line_price_as(
            &administrator,
            manual.revision,
            1,
            manual.lines[0].id,
            175,
            "Approved manual-price fixture",
            "manual-approved",
        )
        .unwrap();
    assert_eq!(approved.lines[0].total_cents, Some(275));
    assert_eq!(approved.total(), Some(175));
    let mut tv = Ticket::new(2);
    tv.add_line("801", "1", "").unwrap();
    let tv = store.save_ticket_as(&administrator, &tv).unwrap();
    let mut zero = store
        .approve_line_price_as(
            &administrator,
            tv.revision,
            2,
            tv.lines[0].id,
            0,
            "Synthetic no-charge policy",
            "zero-approved",
        )
        .unwrap();
    assert_eq!(zero.total(), Some(0));
    assert!(zero.confirm_cash("0").is_err());
    let directory = store.directory().unwrap();
    let (next, customer) = directory
        .save_customer(&crate::customers::Customer {
            name: "Synthetic customer".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .save_directory_as(&administrator, &directory, &next)
        .unwrap();
    zero.select_customer(&customer).unwrap();
    assert!(zero.confirm_cash("0").is_err());
    zero.line_intake(0, "Residential", "1", "").unwrap();
    let mut zero = save_change(&store, &administrator, zero);
    zero.confirm_cash("0").unwrap();
    let mut zero = save_change(&store, &administrator, zero);
    zero.complete(Payment::Cash).unwrap();
    let zero = save_change(&store, &administrator, zero);
    assert_eq!(zero.state, State::Completed);
    assert_eq!(zero.total(), Some(0));
    assert_eq!(zero.lines[0].total_cents, Some(500));
}

#[test]
fn audit_write_failure_rolls_back_price_ticket_and_ledger_for_approval_and_clear() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let ticket = draft(&store, &administrator, 1);
    let line_id = ticket.lines[0].id;
    let trigger = "CREATE TRIGGER reject_price_audit BEFORE UPDATE ON app_state WHEN NEW.key='security_audit' BEGIN SELECT RAISE(FAIL,'synthetic audit failure'); END";
    let before = store.price_overrides().unwrap();
    let audit = store.security_audit().unwrap();
    store.execute(trigger).unwrap();
    assert!(
        store
            .approve_line_price_as(
                &administrator,
                ticket.revision,
                1,
                line_id,
                200,
                "Approved fixture",
                "rollback-approve"
            )
            .is_err()
    );
    assert_eq!(store.list().unwrap()[0], ticket);
    assert_eq!(store.price_overrides().unwrap(), before);
    assert_eq!(store.security_audit().unwrap(), audit);
    store.execute("DROP TRIGGER reject_price_audit").unwrap();
    let approved = store
        .approve_line_price_as(
            &administrator,
            ticket.revision,
            1,
            line_id,
            200,
            "Approved fixture",
            "rollback-approve",
        )
        .unwrap();
    let before = store.price_overrides().unwrap();
    let audit = store.security_audit().unwrap();
    store.execute(trigger).unwrap();
    assert!(
        store
            .clear_line_price_as(
                &administrator,
                approved.revision,
                1,
                line_id,
                "Clear fixture",
                "rollback-clear"
            )
            .is_err()
    );
    assert_eq!(store.list().unwrap()[0], approved);
    assert_eq!(store.price_overrides().unwrap(), before);
    assert_eq!(store.security_audit().unwrap(), audit);
}

#[test]
fn schema_four_upgrade_preserves_raw_ticket_and_security_financial_state() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let mut sale = draft(&store, &administrator, 1);
    sale.confirm_cash("5").unwrap();
    let mut sale = save_change(&store, &administrator, sale);
    sale.complete(Payment::Cash).unwrap();
    let sale = save_change(&store, &administrator, sale);
    let (ledger, attempt) = store
        .start_adjustment_as(
            &administrator,
            &store.adjustments().unwrap(),
            1,
            AdjustmentKind::Refund,
            200,
            "Unknown refund migration fixture",
            "migration-intent",
            "",
        )
        .unwrap();
    let (ledger, _) = store
        .record_adjustment_result_as(
            &administrator,
            &ledger,
            1,
            attempt.id,
            AdjustmentOutcome::Unknown,
            false,
            "",
            "migration-unknown",
            false,
        )
        .unwrap();
    let auth = store.auth_directory().unwrap();
    let audit = store.security_audit().unwrap();
    let statement = store.statement("SELECT payload FROM tickets").unwrap();
    assert_eq!(statement.step().unwrap(), ROW);
    let payload = statement.read_text(0).unwrap();
    drop(statement);
    store
        .execute("DELETE FROM app_state WHERE key='price_overrides'")
        .unwrap();
    store.execute("PRAGMA user_version=4").unwrap();
    drop(store);
    let store = db.open();
    assert_eq!(store.auth_directory().unwrap(), auth);
    assert_eq!(store.security_audit().unwrap(), audit);
    assert_eq!(store.adjustments().unwrap(), ledger);
    assert_eq!(store.list().unwrap(), vec![sale]);
    let statement = store.statement("SELECT payload FROM tickets").unwrap();
    assert_eq!(statement.step().unwrap(), ROW);
    assert_eq!(statement.read_text(0).unwrap(), payload);
    drop(statement);
    assert!(store.price_overrides().unwrap().events.is_empty());
    let statement = store.statement("PRAGMA user_version").unwrap();
    assert_eq!(statement.step().unwrap(), ROW);
    let version = unsafe { (store.api.column_int)(statement.stmt.as_ptr(), 0) };
    assert_eq!(version, 7);
    drop(statement);
    store.sign_in("admin", PASSWORD).unwrap();
    assert_eq!(store.adjustments().unwrap(), ledger);
}

#[test]
fn restart_and_backup_restore_preserve_approval_and_missing_schema_five_ledger_fails_closed() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let ticket = draft(&store, &administrator, 1);
    let approved = store
        .approve_line_price_as(
            &administrator,
            ticket.revision,
            1,
            ticket.lines[0].id,
            125,
            "Restart approval fixture",
            "restart-approval",
        )
        .unwrap();
    let ledger = store.price_overrides().unwrap();
    let auth = store.auth_directory().unwrap();
    let backup = db.0.with_extension("backup.sqlite3");
    let restored = db.0.with_extension("restored.sqlite3");
    store.backup_as(&administrator, &backup).unwrap();
    restore_copy(&backup, &restored).unwrap();
    drop(store);
    let store = db.open();
    store.sign_in("admin", PASSWORD).unwrap();
    assert_eq!(store.list().unwrap()[0], approved);
    assert_eq!(store.price_overrides().unwrap(), ledger);
    let copy = Store::open(&restored).unwrap();
    assert_eq!(copy.auth_directory().unwrap(), auth);
    assert_eq!(copy.price_overrides().unwrap(), ledger);
    assert_eq!(copy.list().unwrap(), vec![approved]);
    drop(copy);
    store
        .execute("DELETE FROM app_state WHERE key='price_overrides'")
        .unwrap();
    drop(store);
    assert!(Store::open(&db.0).is_err());
    std::fs::remove_file(backup).unwrap();
    std::fs::remove_file(restored).unwrap();
}

#[path = "store_price_override_workflow_tests.rs"]
mod workflow;
