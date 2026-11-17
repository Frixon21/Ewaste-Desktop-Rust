use super::*;
use crate::{
    adjustments::{AdjustmentKind, AdjustmentLedger, AdjustmentOutcome},
    auth::{OperatorSnapshot, Permission, Role, Session},
    closeout::ShiftReport,
    domain::Payment,
    security_audit::AuditAction,
};

const PASSWORD: &str = "Synthetic closeout security password";

struct TestDatabase(std::path::PathBuf);

impl TestDatabase {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "ewaste-closeout-security-{}-{nonce}.sqlite3",
            std::process::id()
        )))
    }

    fn open(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn administrator(store: &Store) -> Session {
    store
        .bootstrap_administrator("admin", "Authenticated Administrator", PASSWORD)
        .unwrap()
}

fn cash_sale(store: &Store) -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.created_at = 90;
    ticket.add_line("803", "4", "").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.completed_at = Some(100);
    store.save(&ticket).unwrap();
    ticket
}

fn pending_refund(ticket: &Ticket, actor: &OperatorSnapshot) -> AdjustmentLedger {
    AdjustmentLedger::default()
        .start(
            ticket,
            AdjustmentKind::Refund,
            200,
            actor,
            "Synthetic partial refund",
            "intent",
            120,
            "",
        )
        .unwrap()
        .0
}

fn completed_refund(ticket: &Ticket, actor: &OperatorSnapshot) -> AdjustmentLedger {
    pending_refund(ticket, actor)
        .record_result(
            ticket,
            1,
            AdjustmentOutcome::Completed,
            true,
            "",
            actor,
            "result",
            121,
            false,
        )
        .unwrap()
        .0
}

fn current_report(store: &Store, end: u64) -> ShiftReport {
    ShiftReport::from_tickets_and_adjustments(
        &store.list().unwrap(),
        &store.adjustments().unwrap(),
        0,
        end,
    )
    .unwrap()
}

#[test]
fn cashier_cannot_finalize_even_a_valid_refund_aware_closeout() {
    let database = TestDatabase::new();
    let store = database.open();
    let administrator = administrator(&store);
    let auth = store.auth_directory().unwrap();
    store
        .create_operator_as(
            &administrator,
            &auth,
            "cashier",
            "Cashier",
            Role::Cashier,
            PASSWORD,
        )
        .unwrap();
    let cashier = store.sign_in("cashier", PASSWORD).unwrap();
    cash_sale(&store);
    let record = current_report(&store, 200)
        .reconcile_movements(0, 500, 0, 0, "Cashier", "")
        .unwrap();
    record.validate().unwrap();
    let audit = store.security_audit().unwrap();
    assert!(store.save_closeout_as(&cashier, &record).is_err());
    assert!(store.closeouts().unwrap().is_empty());
    assert_eq!(store.security_audit().unwrap(), audit);
}

#[test]
fn new_v1_closeouts_cannot_bypass_refund_aware_service() {
    let database = TestDatabase::new();
    let store = database.open();
    let administrator = administrator(&store);
    cash_sale(&store);
    let legacy = ShiftReport::from_tickets(&store.list().unwrap(), 0, 200)
        .unwrap()
        .reconcile(0, 500, 0, "Administrator", "")
        .unwrap();
    legacy.validate().unwrap();
    assert_eq!(legacy.report.calculation_version, 1);
    assert!(store.save_closeout_as(&administrator, &legacy).is_err());
    assert!(store.closeouts().unwrap().is_empty());
}

#[test]
fn v2_caller_cannot_omit_a_completed_refund_or_its_settled_event_identity() {
    let database = TestDatabase::new();
    let store = database.open();
    let administrator = administrator(&store);
    let ticket = cash_sale(&store);
    let ledger = completed_refund(&ticket, administrator.actor());
    store.write_state("adjustments", &ledger).unwrap();
    let actual = current_report(&store, 200);
    assert_eq!(actual.cash_refunds_cents, 200);
    assert_eq!(actual.adjustment_event_ids, vec![2]);

    let omitted = ShiftReport {
        calculation_version: 2,
        ..ShiftReport::from_tickets(std::slice::from_ref(&ticket), 0, 200).unwrap()
    };
    let omitted = omitted
        .reconcile_movements(0, 500, 0, 0, "Administrator", "")
        .unwrap();
    omitted.validate().unwrap();
    let audit = store.security_audit().unwrap();
    assert!(store.save_closeout_as(&administrator, &omitted).is_err());
    let missing_identity = ShiftReport {
        adjustment_event_ids: vec![],
        ..actual
    };
    let missing_identity = missing_identity
        .reconcile_movements(0, 300, 0, 0, "Administrator", "")
        .unwrap();
    missing_identity.validate().unwrap();
    assert!(
        store
            .save_closeout_as(&administrator, &missing_identity)
            .is_err()
    );
    assert!(store.closeouts().unwrap().is_empty());
    assert_eq!(store.adjustments().unwrap(), ledger);
    assert_eq!(store.list().unwrap(), vec![ticket]);
    assert_eq!(store.security_audit().unwrap(), audit);
}

#[test]
fn unresolved_refund_queue_cannot_be_removed_by_a_whole_report_candidate() {
    let database = TestDatabase::new();
    let store = database.open();
    let administrator = administrator(&store);
    let ticket = cash_sale(&store);
    let ledger = pending_refund(&ticket, administrator.actor());
    store.write_state("adjustments", &ledger).unwrap();
    let actual = current_report(&store, 200);
    assert_eq!(actual.unresolved_adjustment_ticket_ids, vec![ticket.id]);
    assert!(
        actual
            .reconcile_movements(0, 500, 0, 0, "Administrator", "")
            .is_err()
    );
    let forged = ShiftReport {
        unresolved_adjustment_ticket_ids: vec![],
        ..actual
    };
    let forged = forged
        .reconcile_movements(0, 500, 0, 0, "Administrator", "")
        .unwrap();
    forged.validate().unwrap();
    assert!(store.save_closeout_as(&administrator, &forged).is_err());
    assert_eq!(store.adjustments().unwrap(), ledger);
    assert!(store.closeouts().unwrap().is_empty());
}

#[test]
fn service_stamps_the_authenticated_actor_and_exact_retry_is_non_destructive() {
    let database = TestDatabase::new();
    let store = database.open();
    let administrator = administrator(&store);
    cash_sale(&store);
    let mut candidate = current_report(&store, 200)
        .reconcile_movements(0, 500, 0, 0, "Forged Operator", "")
        .unwrap();
    candidate.actor = Some(OperatorSnapshot {
        id: 999,
        login: "forged".into(),
        display_name: "Forged Operator".into(),
        role: Role::Administrator,
        revision: 1,
    });
    candidate.validate().unwrap();
    let saved = store.save_closeout_as(&administrator, &candidate).unwrap();
    assert_eq!(saved.actor.as_ref(), Some(administrator.actor()));
    assert_eq!(saved.operator, administrator.actor().display_name);
    assert_eq!(candidate.operator, "Forged Operator");
    let audit = store.security_audit().unwrap();
    let event = audit.events.last().unwrap();
    assert_eq!(event.action, AuditAction::CloseoutSaved);
    assert_eq!(event.actor.as_ref(), Some(administrator.actor()));
    assert_eq!(
        store.save_closeout_as(&administrator, &saved).unwrap(),
        saved
    );
    assert_eq!(store.security_audit().unwrap(), audit);
    assert_eq!(store.closeouts().unwrap(), vec![saved]);
}

#[test]
fn authorized_service_refuses_a_future_cutoff_without_persisting_or_auditing() {
    let database = TestDatabase::new();
    let store = database.open();
    let administrator = administrator(&store);
    let end = crate::domain::now().checked_add(3_600).unwrap();
    let record = current_report(&store, end)
        .reconcile_movements(0, 0, 0, 0, "Administrator", "")
        .unwrap();
    record.validate().unwrap();
    let audit = store.security_audit().unwrap();
    let error = store.save_closeout_as(&administrator, &record).unwrap_err();
    assert!(error.contains("future"));
    assert!(store.closeouts().unwrap().is_empty());
    assert_eq!(store.security_audit().unwrap(), audit);
}

#[test]
fn restored_closed_period_blocks_a_financial_result_if_clock_falls_inside_it() {
    let database = TestDatabase::new();
    let store = database.open();
    let administrator = administrator(&store);
    let ticket = cash_sale(&store);
    let empty = store.adjustments().unwrap();
    let (pending, attempt) = store
        .start_adjustment_as(
            &administrator,
            &empty,
            ticket.id,
            AdjustmentKind::Refund,
            200,
            "Synthetic payout intent",
            "intent-now",
            "",
        )
        .unwrap();
    // A previously saved closeout from a clock that was ahead is a recovery
    // fixture; the authenticated service itself refuses future cutoffs.
    let end = crate::domain::now().checked_add(3_600).unwrap();
    let historical = ShiftReport::from_tickets(&[ticket], 0, end)
        .unwrap()
        .reconcile(0, 500, 0, "Historical Operator", "")
        .unwrap();
    store.save_closeout(&historical).unwrap();
    assert!(
        store
            .require_session(&administrator, Permission::AdjustFinancials)
            .is_ok()
    );
    let audit = store.security_audit().unwrap();
    let error = store
        .record_adjustment_result_as(
            &administrator,
            &pending,
            1,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "",
            "result-now",
            false,
        )
        .unwrap_err();
    assert!(error.contains("closed shift"));
    assert_eq!(store.adjustments().unwrap(), pending);
    assert_eq!(store.security_audit().unwrap(), audit);
    assert_eq!(store.closeouts().unwrap(), vec![historical]);
}

#[test]
fn historic_v1_json_remains_readable_without_rewrite_or_refund_recalculation() {
    let database = TestDatabase::new();
    let store = database.open();
    let administrator = administrator(&store);
    let ticket = cash_sale(&store);
    let historical = ShiftReport::from_tickets(std::slice::from_ref(&ticket), 0, 200)
        .unwrap()
        .reconcile(0, 500, 0, "Historical Operator", "")
        .unwrap();
    let mut old_json = serde_json::to_value(&historical).unwrap();
    old_json.as_object_mut().unwrap().remove("actor");
    old_json
        .as_object_mut()
        .unwrap()
        .remove("external_card_refunds_cents");
    for field in [
        "calculation_version",
        "cash_refunds_cents",
        "card_refunds_cents",
        "billing_credits_cents",
        "adjustment_event_ids",
        "unresolved_adjustment_ticket_ids",
    ] {
        old_json["report"].as_object_mut().unwrap().remove(field);
    }
    let payload = vec![old_json];
    store.write_state("closeouts", &payload).unwrap();
    let ledger = completed_refund(&ticket, administrator.actor());
    // Shift history stays immutable even when adjustment evidence is available.
    store.write_state("adjustments", &ledger).unwrap();
    drop(store);
    let reopened = database.open();
    assert_eq!(reopened.closeouts().unwrap(), vec![historical]);
    assert_eq!(
        reopened
            .read_state::<Vec<serde_json::Value>>("closeouts")
            .unwrap(),
        payload
    );
    assert_eq!(reopened.adjustments().unwrap(), ledger);
}
