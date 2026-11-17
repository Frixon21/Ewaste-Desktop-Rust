use super::*;
use crate::adjustments::{AdjustmentKind, AdjustmentLedger, AdjustmentOutcome};
use crate::{
    customers::{BusinessAccount, Customer},
    domain::{Payment, State, Ticket},
    tender::CardOutcome,
};

fn draft(id: u64, created: u64) -> Ticket {
    let mut ticket = Ticket::new(id);
    ticket.created_at = created;
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket
}

fn cash(id: u64, completed: u64) -> Ticket {
    let mut ticket = draft(id, completed - 1);
    ticket.confirm_cash("20").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.completed_at = Some(completed);
    ticket
}

#[test]
fn separates_verified_cash_card_and_pending_billing() {
    let cash = cash(1, 110);
    let mut card = draft(2, 111);
    card.start_card("5", "TEST-SHIFT-CARD").unwrap();
    card.card_result(CardOutcome::Approved, true, false)
        .unwrap();
    card.complete(Payment::ExternalCard {
        reference: "TEST-SHIFT-CARD".into(),
    })
    .unwrap();
    card.completed_at = Some(112);
    let mut charge = draft(3, 113);
    charge
        .select_customer(&Customer {
            id: 1,
            number: "C1".into(),
            name: "Synthetic Person".into(),
            ..Default::default()
        })
        .unwrap();
    charge
        .select_account(&BusinessAccount {
            id: 1,
            number: "A1".into(),
            name: "Synthetic Business".into(),
            active: true,
        })
        .unwrap();
    charge
        .complete(Payment::ChargeTo {
            account: "A1".into(),
        })
        .unwrap();
    charge.completed_at = Some(114);
    let report = ShiftReport::from_tickets(&[cash, card, charge], 100, 200).unwrap();
    assert_eq!(report.completed_tickets, 3);
    assert_eq!(report.cash_sales_cents, 500);
    assert_eq!(report.cash_received_cents, 2000);
    assert_eq!(report.cash_change_cents, 1500);
    assert_eq!(report.verified_card_cents, 500);
    assert_eq!(report.pending_billing_cents, 500);
    let closed = report
        .reconcile(10_000, 10_500, 500, "Synthetic operator", "")
        .unwrap();
    assert_eq!(closed.expected_cash_cents, 10_500);
    assert_eq!(closed.cash_variance_cents, 0);
    assert_eq!(closed.card_variance_cents, 0);
    assert_eq!(closed.id, "shift-100-200");
    assert_eq!(closed.report, report);
}

#[test]
fn uses_half_open_completion_window_and_rejects_duplicate_ids() {
    let before = cash(1, 99);
    let start = cash(2, 100);
    let last = cash(3, 199);
    let after = cash(4, 200);
    let report =
        ShiftReport::from_tickets(&[before, start.clone(), last, after], 100, 200).unwrap();
    assert_eq!(report.completed_tickets, 2);
    assert_eq!(report.cash_sales_cents, 1000);
    assert!(ShiftReport::from_tickets(&[start.clone(), start], 100, 200).is_err());
    assert!(ShiftReport::from_tickets(&[], 100, 100).is_err());
    assert!(ShiftReport::from_tickets(&[], 200, 100).is_err());
}

#[test]
fn unresolved_card_and_unpriced_held_work_are_visible_and_preserved() {
    let mut unresolved = draft(1, 110);
    unresolved.start_card("5", "TEST-UNKNOWN").unwrap();
    unresolved
        .card_result(CardOutcome::Unknown, false, false)
        .unwrap();
    let mut held = Ticket::new(2);
    held.created_at = 111;
    held.add_line("901", "", "").unwrap();
    held.hold().unwrap();
    let open = draft(3, 112);
    let originals = vec![unresolved, held, open];
    let report = ShiftReport::from_tickets(&originals, 100, 200).unwrap();
    assert_eq!(report.open_drafts, 2);
    assert_eq!(report.held_tickets, 1);
    assert_eq!(report.unpriced_tickets, 1);
    assert_eq!(report.unresolved_card_ticket_ids, vec![1]);
    assert!(report.reconcile(0, 0, 0, "Operator", "Reviewed").is_err());
    assert_eq!(originals[0].state, State::Draft);
    assert_eq!(originals[1].state, State::Held);
}

#[test]
fn older_open_work_remains_in_backlog_after_a_shift_boundary() {
    let mut earlier = draft(1, 50);
    earlier.start_card("5", "TEST-CARRY-FORWARD").unwrap();
    earlier
        .card_result(CardOutcome::Unknown, false, false)
        .unwrap();
    let mut held = Ticket::new(2);
    held.created_at = 60;
    held.add_line("901", "", "").unwrap();
    held.hold().unwrap();
    let future = draft(3, 200);
    let report =
        ShiftReport::from_tickets(&[earlier, held, future, cash(4, 99)], 100, 200).unwrap();
    assert_eq!(report.completed_tickets, 0);
    assert_eq!(report.open_drafts, 1);
    assert_eq!(report.held_tickets, 1);
    assert_eq!(report.unpriced_tickets, 1);
    assert_eq!(report.unresolved_card_ticket_ids, vec![1]);
    assert!(report.reconcile(0, 0, 0, "Operator", "").is_err());
}

#[test]
fn taken_tender_on_older_open_or_held_tickets_blocks_closeout_until_completed() {
    let mut cash_draft = draft(1, 50);
    cash_draft.confirm_cash("5").unwrap();
    let mut card_held = draft(2, 60);
    card_held.start_card("5", "TEST-PAID-HELD").unwrap();
    card_held
        .card_result(CardOutcome::Approved, true, false)
        .unwrap();
    card_held.hold().unwrap();
    let report =
        ShiftReport::from_tickets(&[cash_draft.clone(), card_held.clone()], 100, 200).unwrap();
    assert_eq!(report.paid_draft_ticket_ids, vec![1, 2]);
    assert_eq!(report.cash_sales_cents, 0);
    assert_eq!(report.verified_card_cents, 0);
    assert!(report.reconcile(0, 0, 0, "Operator", "").is_err());
    cash_draft.complete(Payment::Cash).unwrap();
    cash_draft.completed_at = Some(110);
    card_held.resume().unwrap();
    card_held
        .complete(Payment::ExternalCard {
            reference: "TEST-PAID-HELD".into(),
        })
        .unwrap();
    card_held.completed_at = Some(111);
    let report = ShiftReport::from_tickets(&[cash_draft, card_held], 100, 200).unwrap();
    assert!(report.paid_draft_ticket_ids.is_empty());
    assert_eq!(report.cash_sales_cents, 500);
    assert_eq!(report.verified_card_cents, 500);
    assert!(report.reconcile(0, 500, 500, "Operator", "").is_ok());
}

#[test]
fn legacy_and_counterfeit_completed_tenders_require_review() {
    let mut legacy = cash(1, 110);
    legacy.tender_evidence = Default::default();
    legacy.completed_at = None;
    legacy.created_at = 109;
    let mut forged = cash(2, 111);
    forged.payment = Some(Payment::ExternalCard {
        reference: "TEST-NO-EVIDENCE".into(),
    });
    let mut no_payment = cash(3, 112);
    no_payment.payment = None;
    let report = ShiftReport::from_tickets(&[legacy, forged, no_payment], 100, 200).unwrap();
    assert_eq!(report.review_ticket_ids, vec![1, 2, 3]);
    assert_eq!(report.cash_sales_cents, 0);
    assert_eq!(report.verified_card_cents, 0);
    assert_eq!(report.unverified_completed_cents, 1500);
    assert!(
        report
            .reconcile(0, 0, 0, "Operator", "Legacy review")
            .is_err()
    );
}

#[test]
fn discrepancy_requires_reason_and_signed_variance_is_retained() {
    let report = ShiftReport::from_tickets(&[cash(1, 110)], 100, 200).unwrap();
    assert!(report.reconcile(1000, 1400, 100, "Operator", "").is_err());
    let closed = report
        .reconcile(1000, 1400, 100, "Operator", "Counted twice; investigate")
        .unwrap();
    assert_eq!(closed.cash_variance_cents, -100);
    assert_eq!(closed.card_variance_cents, 100);
    assert_eq!(closed.counted_cash_cents, 1400);
    assert_eq!(closed.external_card_cents, 100);
    assert_eq!(closed.report.pending_billing_cents, 0);
    let encoded = serde_json::to_string(&closed).unwrap();
    assert_eq!(serde_json::from_str::<Closeout>(&encoded).unwrap(), closed);
    closed.validate().unwrap();
    let changed = Closeout {
        expected_cash_cents: 0,
        ..closed.clone()
    };
    assert!(changed.validate().is_err());
    let changed = Closeout {
        id: "another-shift".into(),
        ..closed
    };
    assert!(changed.validate().is_err());
}

#[test]
fn invalid_declarations_and_invalid_ticket_values_fail_without_wrapping() {
    let report = ShiftReport::from_tickets(&[], 100, 200).unwrap();
    assert!(report.reconcile(-1, 0, 0, "Operator", "").is_err());
    assert!(report.reconcile(0, -1, 0, "Operator", "").is_err());
    assert!(report.reconcile(0, 0, -1, "Operator", "").is_err());
    assert!(report.reconcile(0, 0, 0, "", "").is_err());
    assert!(report.reconcile(0, 0, 0, "bad\noperator", "").is_err());
    assert!(
        report
            .reconcile(0, 0, 0, "Operator", &"x".repeat(501))
            .is_err()
    );
    assert!(report.reconcile(i64::MAX, 0, 0, "Operator", "").is_err());
    let mut negative = cash(1, 110);
    negative.lines[0].total_cents = Some(-1);
    assert!(ShiftReport::from_tickets(&[negative], 100, 200).is_err());
    let mut zero_id = draft(0, 110);
    zero_id.id = 0;
    assert!(ShiftReport::from_tickets(&[zero_id], 100, 200).is_err());
}

#[test]
fn invalid_saved_reports_and_overflowing_totals_cannot_finalize() {
    let valid = ShiftReport::from_tickets(&[cash(1, 110)], 100, 200).unwrap();
    let altered = ShiftReport {
        cash_sales_cents: 1,
        ..valid.clone()
    };
    assert!(altered.reconcile(0, 500, 0, "Operator", "").is_err());
    let altered = ShiftReport {
        review_ticket_ids: vec![1, 1],
        ..valid.clone()
    };
    assert!(altered.validate().is_err());
    let altered = ShiftReport {
        unresolved_card_ticket_ids: vec![0],
        ..valid.clone()
    };
    assert!(altered.validate().is_err());
    let altered = ShiftReport {
        end: 100,
        ..valid.clone()
    };
    assert!(altered.validate().is_err());
    let altered = ShiftReport {
        verified_card_cents: -1,
        ..valid.clone()
    };
    assert!(altered.validate().is_err());
    assert!(
        valid
            .reconcile(MAX_CENTS, 0, 0, "Operator", "Limit")
            .is_err()
    );
    let mut large = cash(2, 110);
    large.lines[0].total_cents = Some(MAX_CENTS);
    large.tender_evidence = Default::default();
    let mut other = large.clone();
    other.id = 3;
    assert!(ShiftReport::from_tickets(&[large, other], 100, 200).is_err());
}

fn supervisor() -> crate::auth::OperatorSnapshot {
    crate::auth::OperatorSnapshot {
        id: 1,
        login: "supervisor".into(),
        display_name: "Supervisor".into(),
        role: crate::auth::Role::Supervisor,
        revision: 1,
    }
}

fn card(id: u64, completed: u64) -> Ticket {
    let mut ticket = draft(id, completed - 1);
    ticket.start_card("5", &format!("TEST-SALE-{id}")).unwrap();
    ticket
        .card_result(CardOutcome::Approved, true, false)
        .unwrap();
    ticket
        .complete(Payment::ExternalCard {
            reference: format!("TEST-SALE-{id}"),
        })
        .unwrap();
    ticket.completed_at = Some(completed);
    ticket
}

fn refund(
    ledger: &AdjustmentLedger,
    ticket: &Ticket,
    kind: AdjustmentKind,
    amount: i64,
    at: u64,
) -> AdjustmentLedger {
    let reference = if matches!(ticket.payment, Some(Payment::ExternalCard { .. })) {
        format!("TEST-REFUND-{at}")
    } else {
        String::new()
    };
    let (pending, attempt) = ledger
        .start(
            ticket,
            kind,
            amount,
            &supervisor(),
            "Synthetic adjustment",
            &format!("request-{at}"),
            at,
            &reference,
        )
        .unwrap();
    pending
        .record_result(
            ticket,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            &reference,
            &supervisor(),
            &format!("result-{at}"),
            at + 1,
            false,
        )
        .unwrap()
        .0
}

#[test]
fn legacy_json_defaults_to_v1_and_old_snapshot_remains_unchanged() {
    let mut closed = ShiftReport::from_tickets(&[cash(1, 110)], 100, 200)
        .unwrap()
        .reconcile(0, 500, 0, "Supervisor", "")
        .unwrap();
    closed.actor = Some(supervisor());
    let mut encoded = serde_json::to_value(&closed).unwrap();
    encoded
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
        encoded["report"].as_object_mut().unwrap().remove(field);
    }
    let restored: Closeout = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored.report.calculation_version, 1);
    assert_eq!(restored.report.cash_refunds_cents, 0);
    assert_eq!(restored, closed);
    restored.validate().unwrap();
}

#[test]
fn refunds_of_prior_shift_sales_and_remaining_void_are_dated_once() {
    let old = cash(1, 90);
    let new = cash(2, 110);
    let original = old.clone();
    let ledger = refund(
        &AdjustmentLedger::default(),
        &old,
        AdjustmentKind::Refund,
        200,
        120,
    );
    let ledger = refund(&ledger, &old, AdjustmentKind::Void, 300, 130);
    let report =
        ShiftReport::from_tickets_and_adjustments(&[old.clone(), new], &ledger, 100, 200).unwrap();
    assert_eq!(report.calculation_version, 2);
    assert_eq!(report.cash_sales_cents, 500);
    assert_eq!(report.cash_refunds_cents, 500);
    assert_eq!(report.adjustment_event_ids, vec![2, 4]);
    let closed = report
        .reconcile_movements(1_000, 1_000, 0, 0, "Supervisor", "")
        .unwrap();
    assert_eq!(closed.expected_cash_cents, 1_000);
    closed.validate().unwrap();
    assert_eq!(old, original);
    assert_eq!(old.receipt(), original.receipt());
}

#[test]
fn card_refund_only_shift_has_signed_expected_net_and_separate_declarations() {
    let old = card(1, 90);
    let ledger = refund(
        &AdjustmentLedger::default(),
        &old,
        AdjustmentKind::Refund,
        500,
        120,
    );
    let report = ShiftReport::from_tickets_and_adjustments(&[old], &ledger, 100, 200).unwrap();
    assert_eq!(report.verified_card_cents, 0);
    assert_eq!(report.card_refunds_cents, 500);
    let closed = report
        .reconcile_movements(0, 0, 0, 500, "Supervisor", "")
        .unwrap();
    assert_eq!(closed.external_card_cents, 0);
    assert_eq!(closed.external_card_refunds_cents, 500);
    assert_eq!(closed.expected_card_cents, -500);
    assert_eq!(closed.card_variance_cents, 0);
    closed.validate().unwrap();
    assert!(
        report
            .reconcile_movements(0, 0, -500, 0, "Supervisor", "")
            .is_err()
    );
    assert!(
        report
            .reconcile_movements(0, 0, 0, -1, "Supervisor", "")
            .is_err()
    );
    assert!(report.reconcile(0, 0, 0, "Supervisor", "").is_err());
}

#[test]
fn cash_refund_requires_enough_float_and_a_reason_for_signed_discrepancy() {
    let old = cash(1, 90);
    let ledger = refund(
        &AdjustmentLedger::default(),
        &old,
        AdjustmentKind::Refund,
        500,
        120,
    );
    let report = ShiftReport::from_tickets_and_adjustments(&[old], &ledger, 100, 200).unwrap();
    assert!(
        report
            .reconcile_movements(499, 0, 0, 0, "Supervisor", "Insufficient float")
            .is_err()
    );
    assert_eq!(
        report
            .reconcile_movements(500, 0, 0, 0, "Supervisor", "")
            .unwrap()
            .expected_cash_cents,
        0
    );
    assert!(
        report
            .reconcile_movements(500, 1, 0, 0, "Supervisor", "")
            .is_err()
    );
    assert_eq!(
        report
            .reconcile_movements(500, 1, 0, 0, "Supervisor", "Recounted")
            .unwrap()
            .cash_variance_cents,
        1
    );
}

#[test]
fn unresolved_refund_backlog_is_frozen_as_of_exclusive_end_despite_later_resolution() {
    let old = card(1, 40);
    let (pending, attempt) = AdjustmentLedger::default()
        .start(
            &old,
            AdjustmentKind::Refund,
            500,
            &supervisor(),
            "Synthetic refund",
            "old-intent",
            50,
            "TEST-OLD-REFUND",
        )
        .unwrap();
    let before =
        ShiftReport::from_tickets_and_adjustments(std::slice::from_ref(&old), &pending, 100, 200)
            .unwrap();
    assert_eq!(before.unresolved_adjustment_ticket_ids, vec![1]);
    assert!(
        before
            .reconcile_movements(0, 0, 0, 0, "Supervisor", "")
            .is_err()
    );
    let (done, _) = pending
        .record_result(
            &old,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "TEST-OLD-REFUND",
            &supervisor(),
            "late-result",
            250,
            true,
        )
        .unwrap();
    assert_eq!(
        ShiftReport::from_tickets_and_adjustments(std::slice::from_ref(&old), &done, 100, 200)
            .unwrap(),
        before
    );
    let later = ShiftReport::from_tickets_and_adjustments(&[old], &done, 200, 300).unwrap();
    assert!(later.unresolved_adjustment_ticket_ids.is_empty());
    assert_eq!(later.card_refunds_cents, 500);
    assert_eq!(later.adjustment_event_ids, vec![2]);
}

#[test]
fn current_second_refund_is_carried_into_next_shift_and_versions_are_checked() {
    let old = card(1, 90);
    let ledger = refund(
        &AdjustmentLedger::default(),
        &old,
        AdjustmentKind::Refund,
        500,
        199,
    );
    let before =
        ShiftReport::from_tickets_and_adjustments(std::slice::from_ref(&old), &ledger, 100, 200)
            .unwrap();
    assert_eq!(before.card_refunds_cents, 0);
    assert_eq!(before.unresolved_adjustment_ticket_ids, vec![1]);
    let next = ShiftReport::from_tickets_and_adjustments(&[old], &ledger, 200, 201).unwrap();
    assert_eq!(next.card_refunds_cents, 500);
    let mut corrupt = next.clone();
    corrupt.calculation_version = 99;
    assert!(corrupt.validate().is_err());
    let mut corrupt = next;
    corrupt.calculation_version = 1;
    assert!(corrupt.validate().is_err());
}

#[test]
fn billing_credit_is_a_separate_movement_without_cash_or_card_effects() {
    let mut old = draft(1, 89);
    old.select_customer(&Customer {
        id: 1,
        number: "C1".into(),
        name: "Synthetic Source".into(),
        ..Default::default()
    })
    .unwrap();
    old.select_account(&BusinessAccount {
        id: 1,
        number: "A1".into(),
        name: "Synthetic Business".into(),
        active: true,
    })
    .unwrap();
    old.complete(Payment::ChargeTo {
        account: "A1".into(),
    })
    .unwrap();
    old.completed_at = Some(90);
    let ledger = refund(
        &AdjustmentLedger::default(),
        &old,
        AdjustmentKind::Refund,
        500,
        120,
    );
    let report =
        ShiftReport::from_tickets_and_adjustments(std::slice::from_ref(&old), &ledger, 100, 200)
            .unwrap();
    assert_eq!(report.billing_credits_cents, 500);
    assert_eq!(report.pending_billing_cents, 0);
    assert_eq!(report.cash_refunds_cents, 0);
    assert_eq!(report.card_refunds_cents, 0);
    let closed = report
        .reconcile_movements(0, 0, 0, 0, "Supervisor", "")
        .unwrap();
    assert_eq!(closed.expected_cash_cents, 0);
    assert_eq!(closed.expected_card_cents, 0);
    assert!(old.receipt().contains("Pending billing"));
}
