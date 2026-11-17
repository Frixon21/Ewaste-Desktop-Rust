use super::*;
use crate::{auth::Role, customers::BusinessAccount, domain::State};

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "supervisor".into(),
        display_name: "Synthetic Supervisor".into(),
        role: Role::Supervisor,
        revision: 1,
    }
}

fn paid_ticket(payment: Payment) -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    match &payment {
        Payment::Cash => ticket.confirm_cash("10").unwrap(),
        Payment::ExternalCard { reference } => {
            ticket.start_card("5", reference).unwrap();
            ticket
                .card_result(crate::tender::CardOutcome::Approved, true, false)
                .unwrap();
        }
        Payment::ChargeTo { account } => ticket
            .select_account(&BusinessAccount {
                id: 1,
                number: account.clone(),
                name: "Example Business".into(),
                active: true,
            })
            .unwrap(),
    }
    ticket.complete(payment).unwrap();
    ticket.created_at = 90;
    ticket.completed_at = Some(100);
    ticket
}

fn start(
    ledger: &AdjustmentLedger,
    ticket: &Ticket,
    kind: AdjustmentKind,
    cents: i64,
    key: &str,
    at: u64,
) -> Result<(AdjustmentLedger, AdjustmentAttempt), String> {
    let card = matches!(ticket.payment, Some(Payment::ExternalCard { .. }));
    ledger.start(
        ticket,
        kind,
        cents,
        &actor(),
        "Synthetic customer correction",
        key,
        at,
        if card { key } else { "" },
    )
}

fn settle(
    ledger: &AdjustmentLedger,
    ticket: &Ticket,
    attempt: &AdjustmentAttempt,
    key: &str,
    at: u64,
) -> (AdjustmentLedger, AdjustmentAttempt) {
    ledger
        .record_result(
            ticket,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            &attempt.external_reference,
            &actor(),
            key,
            at,
            false,
        )
        .unwrap()
}

#[test]
fn cash_partial_refund_and_remaining_void_preserve_the_original_ticket() {
    let ticket = paid_ticket(Payment::Cash);
    let original = ticket.clone();
    let empty = AdjustmentLedger::default();
    let (pending, refund) = start(
        &empty,
        &ticket,
        AdjustmentKind::Refund,
        200,
        "cash-refund-1",
        110,
    )
    .unwrap();
    assert_eq!(refund.outcome, AdjustmentOutcome::Pending);
    let (partial, refund) = settle(&pending, &ticket, &refund, "cash-confirm-1", 111);
    assert_eq!(refund.outcome, AdjustmentOutcome::Completed);
    assert_eq!(refund.method, AdjustmentMethod::CashRefund);
    assert_eq!(partial.total_adjusted(ticket.id).unwrap(), 200);
    assert!(
        start(
            &partial,
            &ticket,
            AdjustmentKind::Refund,
            301,
            "too-much",
            112
        )
        .is_err()
    );
    assert!(
        start(
            &partial,
            &ticket,
            AdjustmentKind::Void,
            200,
            "partial-void",
            112
        )
        .is_err()
    );
    let (pending, attempt) = start(
        &partial,
        &ticket,
        AdjustmentKind::Void,
        300,
        "cash-void",
        112,
    )
    .unwrap();
    let (voided, attempt) = settle(&pending, &ticket, &attempt, "void-confirm", 113);
    assert_eq!(attempt.kind, AdjustmentKind::Void);
    assert_eq!(voided.total_adjusted(ticket.id).unwrap(), 500);
    assert!(
        start(
            &voided,
            &ticket,
            AdjustmentKind::Refund,
            1,
            "empty-balance",
            114
        )
        .is_err()
    );
    assert!(empty.events.is_empty());
    assert_eq!(ticket, original);
    assert!(voided.receipt(&attempt, &ticket).unwrap().contains("$3.00"));
    voided.validate_against(&[ticket]).unwrap();
}

#[test]
fn cash_and_billing_require_explicit_confirmation_and_keep_separate_effects() {
    let cash = paid_ticket(Payment::Cash);
    let (cash_intent, cash_attempt) = start(
        &AdjustmentLedger::default(),
        &cash,
        AdjustmentKind::Refund,
        100,
        "cash-intent",
        110,
    )
    .unwrap();
    assert_eq!(cash_attempt.outcome, AdjustmentOutcome::Pending);
    assert_eq!(cash_intent.total_adjusted(cash.id).unwrap(), 0);
    assert!(
        cash_intent
            .record_result(
                &cash,
                cash_attempt.id,
                AdjustmentOutcome::Completed,
                false,
                "",
                &actor(),
                "no-confirm",
                111,
                false
            )
            .is_err()
    );
    let billed = Ticket {
        id: 2,
        ..paid_ticket(Payment::ChargeTo {
            account: "A000001".into(),
        })
    };
    let (pending, credit) = start(
        &AdjustmentLedger::default(),
        &billed,
        AdjustmentKind::Refund,
        100,
        "billing-credit",
        111,
    )
    .unwrap();
    let (ledger, credit) = settle(&pending, &billed, &credit, "credit-confirm", 112);
    assert_eq!(credit.method, AdjustmentMethod::BillingCredit);
    let voucher = ledger.receipt(&credit, &billed).unwrap();
    assert!(voucher.contains("Pending billing handoff"));
    let effects = ledger.financial_effects(100, 113).unwrap();
    assert_eq!(effects.billing_credits_cents, 100);
    assert_eq!(effects.cash_refunds_cents, 0);
    assert_eq!(effects.card_refunds_cents, 0);
    ledger.validate_against(&[cash, billed]).unwrap();
}

#[test]
fn external_unknown_requires_verified_reconciliation_with_same_reference() {
    let ticket = paid_ticket(Payment::ExternalCard {
        reference: "SALE-1".into(),
    });
    let (pending, attempt) = start(
        &AdjustmentLedger::default(),
        &ticket,
        AdjustmentKind::Refund,
        200,
        "REFUND-1",
        110,
    )
    .unwrap();
    assert_eq!(attempt.outcome, AdjustmentOutcome::Pending);
    assert_eq!(pending.total_adjusted(ticket.id).unwrap(), 0);
    assert!(
        start(
            &pending,
            &ticket,
            AdjustmentKind::Refund,
            100,
            "REFUND-2",
            111
        )
        .is_err()
    );
    let (unknown, _) = pending
        .record_result(
            &ticket,
            attempt.id,
            AdjustmentOutcome::Unknown,
            false,
            "REFUND-1",
            &actor(),
            "unknown-1",
            111,
            false,
        )
        .unwrap();
    assert!(
        unknown
            .record_result(
                &ticket,
                attempt.id,
                AdjustmentOutcome::Completed,
                true,
                "REFUND-1",
                &actor(),
                "wrong-path",
                112,
                false
            )
            .is_err()
    );
    assert!(
        unknown
            .record_result(
                &ticket,
                attempt.id,
                AdjustmentOutcome::Completed,
                false,
                "REFUND-1",
                &actor(),
                "unverified",
                112,
                true
            )
            .is_err()
    );
    assert!(
        unknown
            .record_result(
                &ticket,
                attempt.id,
                AdjustmentOutcome::Completed,
                true,
                "REFUND-OTHER",
                &actor(),
                "wrong-reference",
                112,
                true
            )
            .is_err()
    );
    let (done, completed) = unknown
        .record_result(
            &ticket,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "REFUND-1",
            &actor(),
            "reconciled-1",
            112,
            true,
        )
        .unwrap();
    assert_eq!(done.attempts(ticket.id).unwrap().len(), 1);
    assert_eq!(completed.outcome, AdjustmentOutcome::Completed);
    assert_eq!(done.total_adjusted(ticket.id).unwrap(), 200);
    assert!(done.unresolved_ticket_ids().unwrap().is_empty());
    assert_eq!(done.events.len(), 3);
    done.validate_against(&[ticket]).unwrap();
}

#[test]
fn decline_and_cancel_do_not_consume_balance_or_approve_typed_reference() {
    for outcome in [AdjustmentOutcome::Declined, AdjustmentOutcome::Cancelled] {
        let ticket = paid_ticket(Payment::ExternalCard {
            reference: "SALE-1".into(),
        });
        let (pending, attempt) = start(
            &AdjustmentLedger::default(),
            &ticket,
            AdjustmentKind::Refund,
            500,
            "REFUND-1",
            110,
        )
        .unwrap();
        assert!(
            pending
                .record_result(
                    &ticket,
                    attempt.id,
                    AdjustmentOutcome::Completed,
                    false,
                    "REFUND-1",
                    &actor(),
                    "unverified",
                    111,
                    false
                )
                .is_err()
        );
        let (failed, _) = pending
            .record_result(
                &ticket,
                attempt.id,
                outcome,
                true,
                "REFUND-1",
                &actor(),
                "failed",
                111,
                false,
            )
            .unwrap();
        assert_eq!(failed.total_adjusted(ticket.id).unwrap(), 0);
        assert!(failed.unresolved_ticket_ids().unwrap().is_empty());
        let (retry, second) = start(
            &failed,
            &ticket,
            AdjustmentKind::Refund,
            500,
            "REFUND-2",
            112,
        )
        .unwrap();
        assert_eq!(second.method, AdjustmentMethod::ExternalCardRefund);
        assert_eq!(retry.attempts(ticket.id).unwrap().len(), 2);
        assert!(
            failed
                .start(
                    &ticket,
                    AdjustmentKind::Refund,
                    500,
                    &actor(),
                    "Another refund",
                    "NEW-REQUEST",
                    112,
                    "REFUND-1"
                )
                .is_err()
        );
    }
}

#[test]
fn exact_request_retries_are_idempotent_and_conflicting_keys_are_rejected() {
    let ticket = paid_ticket(Payment::Cash);
    let (ledger, first) = start(
        &AdjustmentLedger::default(),
        &ticket,
        AdjustmentKind::Refund,
        200,
        "request-1",
        110,
    )
    .unwrap();
    let (same, attempt) = start(
        &ledger,
        &ticket,
        AdjustmentKind::Refund,
        200,
        "request-1",
        150,
    )
    .unwrap();
    assert_eq!(same, ledger);
    assert_eq!(attempt, first);
    assert!(
        start(
            &ledger,
            &ticket,
            AdjustmentKind::Refund,
            201,
            "request-1",
            150
        )
        .is_err()
    );
    let card = paid_ticket(Payment::ExternalCard {
        reference: "SALE-1".into(),
    });
    let (pending, attempt) = start(
        &AdjustmentLedger::default(),
        &card,
        AdjustmentKind::Refund,
        200,
        "REFUND-1",
        110,
    )
    .unwrap();
    let (done, _) = pending
        .record_result(
            &card,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "REFUND-1",
            &actor(),
            "result-1",
            111,
            false,
        )
        .unwrap();
    let (same, _) = done
        .record_result(
            &card,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "REFUND-1",
            &actor(),
            "result-1",
            150,
            false,
        )
        .unwrap();
    assert_eq!(same, done);
    assert!(
        done.record_result(
            &card,
            attempt.id,
            AdjustmentOutcome::Declined,
            false,
            "REFUND-1",
            &actor(),
            "result-1",
            150,
            false
        )
        .is_err()
    );
}

#[path = "adjustments_replay_tests.rs"]
mod replay;
