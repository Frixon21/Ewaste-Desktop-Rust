use super::*;

#[test]
fn unverified_failed_results_cannot_release_a_reserved_payout() {
    for payment in [
        Payment::Cash,
        Payment::ExternalCard {
            reference: "SALE-VERIFIED".into(),
        },
        Payment::ChargeTo {
            account: "A-VERIFIED".into(),
        },
    ] {
        let ticket = paid_ticket(payment);
        let card = matches!(ticket.payment, Some(Payment::ExternalCard { .. }));
        let (pending, attempt) = start(
            &AdjustmentLedger::default(),
            &ticket,
            AdjustmentKind::Refund,
            100,
            "REFUND-RESERVED",
            110,
        )
        .unwrap();
        for outcome in [AdjustmentOutcome::Cancelled, AdjustmentOutcome::Declined] {
            assert!(
                pending
                    .record_result(
                        &ticket,
                        attempt.id,
                        outcome,
                        false,
                        &attempt.external_reference,
                        &actor(),
                        "unverified-release",
                        111,
                        false
                    )
                    .is_err()
            );
            assert_eq!(pending.unresolved_ticket_ids().unwrap(), vec![ticket.id]);
            assert!(
                pending
                    .start(
                        &ticket,
                        AdjustmentKind::Refund,
                        100,
                        &actor(),
                        "Another payout",
                        "ANOTHER-REQUEST",
                        112,
                        if card { "ANOTHER-REFUND" } else { "" }
                    )
                    .is_err()
            );
        }
        let (mut cancelled, _) = pending
            .record_result(
                &ticket,
                attempt.id,
                AdjustmentOutcome::Cancelled,
                true,
                &attempt.external_reference,
                &actor(),
                "verified-cancellation",
                111,
                false,
            )
            .unwrap();
        if let AdjustmentAction::Result { verified, .. } =
            &mut cancelled.events.last_mut().unwrap().action
        {
            *verified = false;
        }
        assert!(
            cancelled
                .validate_against(std::slice::from_ref(&ticket))
                .is_err()
        );
    }
}

#[test]
fn request_audit_fields_and_result_transitions_fail_without_adding_events() {
    let ticket = paid_ticket(Payment::Cash);
    let empty = AdjustmentLedger::default();
    for bad in ["", "bad key", "bad\nkey", &"A".repeat(129)] {
        assert!(
            empty
                .start(
                    &ticket,
                    AdjustmentKind::Refund,
                    100,
                    &actor(),
                    "Correction",
                    bad,
                    110,
                    ""
                )
                .is_err()
        );
    }
    for bad in ["", "bad\nreason", &"A".repeat(501)] {
        assert!(
            empty
                .start(
                    &ticket,
                    AdjustmentKind::Refund,
                    100,
                    &actor(),
                    bad,
                    "key",
                    110,
                    ""
                )
                .is_err()
        );
    }
    assert!(
        empty
            .start(
                &ticket,
                AdjustmentKind::Refund,
                100,
                &OperatorSnapshot { id: 0, ..actor() },
                "Reason",
                "key",
                110,
                ""
            )
            .is_err()
    );
    assert!(
        empty
            .record_result(
                &ticket,
                1,
                AdjustmentOutcome::Completed,
                true,
                "",
                &actor(),
                "no-intent",
                110,
                false
            )
            .is_err()
    );
    let (pending, attempt) =
        start(&empty, &ticket, AdjustmentKind::Refund, 100, "intent", 110).unwrap();
    for (outcome, verified, reference, at) in [
        (AdjustmentOutcome::Pending, true, "", 111),
        (AdjustmentOutcome::Declined, false, "", 111),
        (AdjustmentOutcome::Completed, true, "wrong-reference", 111),
        (AdjustmentOutcome::Unknown, false, "", 109),
    ] {
        assert!(
            pending
                .record_result(
                    &ticket,
                    attempt.id,
                    outcome,
                    verified,
                    reference,
                    &actor(),
                    "bad-result",
                    at,
                    false
                )
                .is_err()
        );
    }
    assert_eq!(pending.events.len(), 1);
    assert!(empty.events.is_empty());
}

#[test]
fn result_idempotency_cannot_reuse_a_prior_cancellation_for_a_new_cash_intent() {
    let ticket = paid_ticket(Payment::Cash);
    let (pending, first) = start(
        &AdjustmentLedger::default(),
        &ticket,
        AdjustmentKind::Refund,
        100,
        "intent-1",
        110,
    )
    .unwrap();
    let (cancelled, _) = pending
        .record_result(
            &ticket,
            first.id,
            AdjustmentOutcome::Cancelled,
            true,
            "",
            &actor(),
            "cancel-1",
            111,
            false,
        )
        .unwrap();
    let (next, second) = start(
        &cancelled,
        &ticket,
        AdjustmentKind::Refund,
        100,
        "intent-2",
        112,
    )
    .unwrap();
    assert_ne!(first.id, second.id);
    assert!(
        next.record_result(
            &ticket,
            second.id,
            AdjustmentOutcome::Cancelled,
            true,
            "",
            &actor(),
            "cancel-1",
            113,
            false
        )
        .is_err()
    );
    assert_eq!(
        next.attempts(ticket.id).unwrap()[1].outcome,
        AdjustmentOutcome::Pending
    );
}

#[test]
fn serialized_event_tampering_cannot_change_method_amount_reason_or_result_identity() {
    let ticket = paid_ticket(Payment::Cash);
    let (pending, attempt) = start(
        &AdjustmentLedger::default(),
        &ticket,
        AdjustmentKind::Refund,
        100,
        "intent",
        110,
    )
    .unwrap();
    let (done, attempt) = settle(&pending, &ticket, &attempt, "cash-returned", 111);
    let mut bad = done.clone();
    bad.events[1].amount_cents = 200;
    assert!(bad.validate_against(std::slice::from_ref(&ticket)).is_err());
    bad = done.clone();
    bad.events[1].reason = "Different reason".into();
    assert!(bad.validate_against(std::slice::from_ref(&ticket)).is_err());
    bad = done.clone();
    bad.events[1].attempt_id = 999;
    assert!(bad.validate_against(std::slice::from_ref(&ticket)).is_err());
    bad = done.clone();
    bad.events[0].method = AdjustmentMethod::ExternalCardRefund;
    assert!(bad.validate_against(std::slice::from_ref(&ticket)).is_err());
    bad = done.clone();
    bad.events[1].request_key = bad.events[0].request_key.clone();
    assert!(bad.validate_against(std::slice::from_ref(&ticket)).is_err());
    assert!(
        done.receipt(
            &AdjustmentAttempt {
                amount_cents: 200,
                ..attempt
            },
            &ticket
        )
        .is_err()
    );
}

#[test]
fn cashier_snapshots_cannot_start_or_justify_saved_financial_adjustments() {
    let ticket = paid_ticket(Payment::Cash);
    let cashier = OperatorSnapshot {
        role: Role::Cashier,
        ..actor()
    };
    assert!(
        AdjustmentLedger::default()
            .start(
                &ticket,
                AdjustmentKind::Refund,
                100,
                &cashier,
                "Correction",
                "cashier-request",
                110,
                ""
            )
            .is_err()
    );
    let (mut ledger, _) = start(
        &AdjustmentLedger::default(),
        &ticket,
        AdjustmentKind::Refund,
        100,
        "supervisor-request",
        110,
    )
    .unwrap();
    ledger.events[0].actor.role = Role::Cashier;
    assert!(ledger.validate_against(&[ticket]).is_err());
}

#[test]
fn effects_use_result_time_and_half_open_windows_without_changing_prior_closeout() {
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
    let prior = pending.financial_effects(100, 120).unwrap();
    assert_eq!(prior.card_refunds_cents, 0);
    assert_eq!(prior.unresolved_ticket_ids, vec![ticket.id]);
    let (done, _) = pending
        .record_result(
            &ticket,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "REFUND-1",
            &actor(),
            "done",
            120,
            false,
        )
        .unwrap();
    assert_eq!(done.financial_effects(100, 120).unwrap(), prior);
    assert_eq!(
        done.financial_effects(120, 121).unwrap().card_refunds_cents,
        200
    );
    assert!(
        done.financial_effects(120, 121)
            .unwrap()
            .unresolved_ticket_ids
            .is_empty()
    );
    assert!(done.financial_effects(120, 120).is_err());
}

#[test]
fn invalid_and_legacy_unverified_original_tenders_are_blocked() {
    let paid = paid_ticket(Payment::Cash);
    let mut unverified = paid.clone();
    unverified.tender_evidence = Default::default();
    assert!(
        start(
            &AdjustmentLedger::default(),
            &unverified,
            AdjustmentKind::Refund,
            100,
            "legacy",
            110
        )
        .is_err()
    );
    assert!(
        start(
            &AdjustmentLedger::default(),
            &Ticket {
                state: State::Draft,
                ..paid.clone()
            },
            AdjustmentKind::Refund,
            100,
            "draft",
            110
        )
        .is_err()
    );
    assert!(
        start(
            &AdjustmentLedger::default(),
            &paid,
            AdjustmentKind::Refund,
            0,
            "zero",
            110
        )
        .is_err()
    );
    assert!(
        start(
            &AdjustmentLedger::default(),
            &paid,
            AdjustmentKind::Refund,
            -1,
            "negative",
            110
        )
        .is_err()
    );
    assert!(
        start(
            &AdjustmentLedger::default(),
            &paid,
            AdjustmentKind::Refund,
            1,
            "too-early",
            99
        )
        .is_err()
    );
    let mut billed = paid_ticket(Payment::ChargeTo {
        account: "A1".into(),
    });
    let (credit, _) = start(
        &AdjustmentLedger::default(),
        &billed,
        AdjustmentKind::Refund,
        100,
        "credit",
        110,
    )
    .unwrap();
    let mut swapped = billed.clone();
    swapped.account_record.as_mut().unwrap().id = 2;
    assert!(credit.validate_against(&[swapped]).is_err());
    billed.account_record.as_mut().unwrap().active = false;
    assert!(
        start(
            &AdjustmentLedger::default(),
            &billed,
            AdjustmentKind::Refund,
            100,
            "inactive",
            110
        )
        .is_err()
    );
}

#[test]
fn replay_rejects_corrupt_identity_chronology_and_stale_sale_references() {
    let ticket = paid_ticket(Payment::ExternalCard {
        reference: "SALE-1".into(),
    });
    let (ledger, _) = start(
        &AdjustmentLedger::default(),
        &ticket,
        AdjustmentKind::Refund,
        200,
        "REFUND-1",
        110,
    )
    .unwrap();
    let mut corrupt = ledger.clone();
    corrupt.events[0].id = 2;
    assert!(
        corrupt
            .validate_against(std::slice::from_ref(&ticket))
            .is_err()
    );
    corrupt = ledger.clone();
    corrupt.next_id = 1;
    assert!(
        corrupt
            .validate_against(std::slice::from_ref(&ticket))
            .is_err()
    );
    corrupt = ledger.clone();
    corrupt.events[0].actor.id = 0;
    assert!(
        corrupt
            .validate_against(std::slice::from_ref(&ticket))
            .is_err()
    );
    corrupt = ledger.clone();
    corrupt.events[0].original_payment = Payment::ExternalCard {
        reference: "SALE-OTHER".into(),
    };
    assert!(
        corrupt
            .validate_against(std::slice::from_ref(&ticket))
            .is_err()
    );
    assert!(ledger.validate_against(&[]).is_err());
    assert!(
        ledger
            .validate_against(&[ticket.clone(), ticket.clone()])
            .is_err()
    );
    assert!(
        start(
            &ledger,
            &ticket,
            AdjustmentKind::Refund,
            100,
            "late-key",
            109
        )
        .is_err()
    );
}

#[test]
fn references_and_audit_fields_are_bounded_and_voucher_excludes_source_pii() {
    let ticket = paid_ticket(Payment::ExternalCard {
        reference: "SALE-1".into(),
    });
    for reference in [
        "",
        "4111111111111111",
        "4111-1111-1111-1111",
        "bad\nreference",
        "bad reference",
    ] {
        assert!(
            AdjustmentLedger::default()
                .start(
                    &ticket,
                    AdjustmentKind::Refund,
                    100,
                    &actor(),
                    "Reason",
                    "request",
                    110,
                    reference
                )
                .is_err()
        );
    }
    let cash = Ticket {
        source_intake: crate::intake::SourceIntake {
            source_name: "Private Source".into(),
            phone: "555-0199".into(),
            notes: "Private source note".into(),
            ..Default::default()
        },
        ..paid_ticket(Payment::Cash)
    };
    let (ledger, attempt) = start(
        &AdjustmentLedger::default(),
        &cash,
        AdjustmentKind::Refund,
        100,
        "safe",
        110,
    )
    .unwrap();
    let receipt = ledger.receipt(&attempt, &cash).unwrap();
    assert!(!receipt.contains("Private Source"));
    assert!(!receipt.contains("555-0199"));
    assert!(!receipt.contains("Private source note"));
    assert_eq!(
        serde_json::from_str::<AdjustmentLedger>(&serde_json::to_string(&ledger).unwrap()).unwrap(),
        ledger
    );
    ledger.validate_against(&[cash]).unwrap();
}

#[test]
fn cash_unknown_intent_blocks_another_payout_until_verified_reconciliation() {
    let ticket = paid_ticket(Payment::Cash);
    let (pending, attempt) = start(
        &AdjustmentLedger::default(),
        &ticket,
        AdjustmentKind::Refund,
        200,
        "cash-intent",
        110,
    )
    .unwrap();
    assert!(
        start(
            &pending,
            &ticket,
            AdjustmentKind::Refund,
            200,
            "cash-again",
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
            "",
            &actor(),
            "cash-unknown",
            111,
            false,
        )
        .unwrap();
    assert!(
        unknown
            .record_result(
                &ticket,
                attempt.id,
                AdjustmentOutcome::Cancelled,
                false,
                "",
                &actor(),
                "cash-cancel",
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
                "",
                &actor(),
                "cash-confirm",
                112,
                false
            )
            .is_err()
    );
    let (done, attempt) = unknown
        .record_result(
            &ticket,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "",
            &actor(),
            "cash-reconcile",
            112,
            true,
        )
        .unwrap();
    assert_eq!(done.total_adjusted(ticket.id).unwrap(), 200);
    assert_eq!(
        done.financial_effects(110, 112).unwrap().cash_refunds_cents,
        0
    );
    assert_eq!(
        done.financial_effects(112, 113).unwrap().cash_refunds_cents,
        200
    );
    assert!(
        done.record_result(
            &ticket,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "",
            &actor(),
            "cash-return-again",
            113,
            false
        )
        .is_err()
    );
    done.validate_against(&[ticket]).unwrap();
}
