#![cfg(feature = "extended-workflows")]
use super::*;
use crate::{
    adjustments::{AdjustmentKind, AdjustmentLedger, AdjustmentOutcome},
    auth::{Role, Session},
    domain::Payment,
};
const PASS: &str = "Synthetic financial service password";
struct Db(std::path::PathBuf);
impl Db {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "ewaste-financial-store-{}-{}.sqlite3",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
    }
    fn open(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn original(store: &Store, card: bool) -> Ticket {
    let mut t = Ticket::new(1);
    t.created_at = 90;
    t.add_weight_line("803", "20", "0").unwrap();
    t.choose_blank().unwrap();
    if card {
        t.start_card("8", "TEST-ORIGINAL").unwrap();
        t.card_result(crate::tender::CardOutcome::Approved, true, false)
            .unwrap();
        t.complete(Payment::ExternalCard {
            reference: "TEST-ORIGINAL".into(),
        })
        .unwrap();
    } else {
        t.confirm_cash("10").unwrap();
        t.complete(Payment::Cash).unwrap();
    }
    t.completed_at = Some(100);
    store.save(&t).unwrap();
    t
}
fn administrator(store: &Store) -> Session {
    store
        .bootstrap_administrator("admin", "Synthetic administrator", PASS)
        .unwrap()
}
#[test]
fn refund_intent_confirmation_and_idempotency_preserve_ticket_and_authenticated_audit() {
    let db = Db::new();
    let store = db.open();
    let admin = administrator(&store);
    let t = original(&store, false);
    let empty = store.adjustments().unwrap();
    let (pending, attempt) = store
        .start_adjustment_as(
            &admin,
            &empty,
            t.id,
            AdjustmentKind::Refund,
            300,
            "Synthetic partial refund",
            "intent1",
            "",
        )
        .unwrap();
    assert_eq!(attempt.outcome, AdjustmentOutcome::Pending);
    assert_eq!(pending.total_adjusted(t.id).unwrap(), 0);
    let (result, settled) = store
        .record_adjustment_result_as(
            &admin,
            &pending,
            t.id,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "",
            "result1",
            false,
        )
        .unwrap();
    assert_eq!(settled.amount_cents, 300);
    assert_eq!(result.total_adjusted(t.id).unwrap(), 300);
    let before = store.security_audit().unwrap();
    let (retried, _) = store
        .record_adjustment_result_as(
            &admin,
            &pending,
            t.id,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "",
            "result1",
            false,
        )
        .unwrap();
    assert_eq!(retried, result);
    assert_eq!(store.security_audit().unwrap(), before);
    assert_eq!(store.list().unwrap(), vec![t.clone()]);
    let (void, _) = store
        .start_adjustment_as(
            &admin,
            &result,
            t.id,
            AdjustmentKind::Void,
            500,
            "Return remaining balance",
            "intent2",
            "",
        )
        .unwrap();
    assert_eq!(void.events.last().unwrap().actor.id, admin.actor().id);
    let stale = store.start_adjustment_as(
        &admin,
        &result,
        t.id,
        AdjustmentKind::Refund,
        100,
        "Concurrent request",
        "stale",
        "",
    );
    assert!(stale.is_err());
    drop(store);
    let store = db.open();
    assert_eq!(store.adjustments().unwrap(), void);
    assert_eq!(store.list().unwrap()[0].receipt(), t.receipt());
}
#[test]
fn unknown_card_refund_survives_restart_and_blocks_double_refund_or_cash_fallback() {
    let db = Db::new();
    let store = db.open();
    let admin = administrator(&store);
    let t = original(&store, true);
    let (pending, a) = store
        .start_adjustment_as(
            &admin,
            &AdjustmentLedger::default(),
            t.id,
            AdjustmentKind::Refund,
            500,
            "Verified original reference",
            "card-intent",
            "TEST-REFUND",
        )
        .unwrap();
    let (unknown, _) = store
        .record_adjustment_result_as(
            &admin,
            &pending,
            t.id,
            a.id,
            AdjustmentOutcome::Unknown,
            false,
            "TEST-REFUND",
            "card-unknown",
            false,
        )
        .unwrap();
    drop(store);
    let store = db.open();
    let session = store.sign_in("admin", PASS).unwrap();
    let loaded = store.adjustments().unwrap();
    assert_eq!(loaded, unknown);
    assert!(
        store
            .start_adjustment_as(
                &session,
                &loaded,
                t.id,
                AdjustmentKind::Refund,
                300,
                "Do not retry",
                "new-intent",
                "TEST-SECOND"
            )
            .is_err()
    );
    assert!(
        store
            .record_adjustment_result_as(
                &session,
                &loaded,
                t.id,
                a.id,
                AdjustmentOutcome::Completed,
                true,
                "TEST-REFUND",
                "invalid-result",
                false
            )
            .is_err()
    );
    let (done, _) = store
        .record_adjustment_result_as(
            &session,
            &loaded,
            t.id,
            a.id,
            AdjustmentOutcome::Completed,
            true,
            "TEST-REFUND",
            "resolved",
            true,
        )
        .unwrap();
    assert_eq!(done.total_adjusted(t.id).unwrap(), 500);
    assert_eq!(store.list().unwrap()[0], t);
}
#[test]
fn cashier_and_revoked_session_cannot_adjust_even_on_exact_retry() {
    let db = Db::new();
    let store = db.open();
    let admin = administrator(&store);
    let t = original(&store, false);
    let dir = store.auth_directory().unwrap();
    store
        .create_operator_as(
            &admin,
            &dir,
            "cashier",
            "Synthetic cashier",
            Role::Cashier,
            PASS,
        )
        .unwrap();
    let cashier = store.sign_in("cashier", PASS).unwrap();
    let ledger = store.adjustments().unwrap();
    assert!(
        store
            .start_adjustment_as(
                &cashier,
                &ledger,
                t.id,
                AdjustmentKind::Refund,
                100,
                "Not authorized",
                "cashier",
                ""
            )
            .is_err()
    );
    let (saved, _) = store
        .start_adjustment_as(
            &admin,
            &ledger,
            t.id,
            AdjustmentKind::Refund,
            100,
            "Authorized refund",
            "allowed",
            "",
        )
        .unwrap();
    admin.revoke();
    assert!(
        store
            .start_adjustment_as(
                &admin,
                &ledger,
                t.id,
                AdjustmentKind::Refund,
                100,
                "Authorized refund",
                "allowed",
                ""
            )
            .is_err()
    );
    assert_eq!(store.adjustments().unwrap(), saved);
}
#[test]
fn backup_restore_retains_credentials_audit_and_pending_financial_intent_but_not_session() {
    let db = Db::new();
    let store = db.open();
    let admin = administrator(&store);
    let t = original(&store, false);
    let (ledger, _) = store
        .start_adjustment_as(
            &admin,
            &AdjustmentLedger::default(),
            t.id,
            AdjustmentKind::Refund,
            100,
            "Pending cash handoff",
            "pending",
            "",
        )
        .unwrap();
    let backup = db.0.with_extension("backup.sqlite3");
    let restored = db.0.with_extension("restored.sqlite3");
    store.backup_as(&admin, &backup).unwrap();
    restore_copy(&backup, &restored).unwrap();
    let copy = Store::open(&restored).unwrap();
    assert_eq!(
        copy.auth_directory().unwrap(),
        store.auth_directory().unwrap()
    );
    assert_eq!(copy.adjustments().unwrap(), ledger);
    assert!(!copy.security_audit().unwrap().events.is_empty());
    let login = copy.sign_in("admin", PASS).unwrap();
    assert_eq!(login.actor(), admin.actor());
    drop(copy);
    drop(store);
    std::fs::remove_file(backup).unwrap();
    std::fs::remove_file(restored).unwrap();
}
