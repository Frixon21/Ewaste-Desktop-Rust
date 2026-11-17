use super::*;

#[test]
fn cash_records_received_and_change_and_validates_due() {
    let original = TenderEvidence::default();
    assert!(original.record_cash(500, 499).is_err());
    let paid = original.record_cash(500, 2000).unwrap();
    let cash = paid.cash_for_total(500).unwrap();
    assert_eq!(cash.received_cents, 2000);
    assert_eq!(cash.change_cents, 1500);
    assert!(paid.cash_for_total(501).is_err());
    assert!(original.cash.is_none());
    assert_eq!(paid.record_cash(500, 2000).unwrap(), paid);
    assert!(paid.record_cash(500, 1000).is_err());
    assert!(paid.begin_card(500, "TEST-CARD").is_err());
}

#[test]
fn reference_is_not_approval_and_success_needs_verification_and_matching_amount() {
    let pending = TenderEvidence::default().begin_card(500, "TEST-1").unwrap();
    assert!(pending.approved_card_for_total(500).is_err());
    assert!(pending.begin_card(500, "TEST-2").is_err());
    assert!(pending.record_cash(500, 500).is_err());
    assert!(
        pending
            .record_card_result(CardOutcome::Approved, false)
            .is_err()
    );
    let approved = pending
        .record_card_result(CardOutcome::Approved, true)
        .unwrap();
    assert_eq!(
        approved.approved_card_for_total(500).unwrap().reference,
        "TEST-1"
    );
    assert!(approved.approved_card_for_total(600).is_err());
    assert!(approved.begin_card(500, "TEST-2").is_err());
    assert!(approved.record_cash(500, 500).is_err());
    assert_eq!(
        approved
            .record_card_result(CardOutcome::Approved, true)
            .unwrap(),
        approved
    );
    assert!(
        approved
            .record_card_result(CardOutcome::Declined, false)
            .is_err()
    );
}

#[test]
fn unsuccessful_results_remain_unpaid_and_allow_distinct_retry() {
    for outcome in [
        CardOutcome::Declined,
        CardOutcome::Cancelled,
        CardOutcome::ZipMismatch,
    ] {
        let failed = TenderEvidence::default()
            .begin_card(500, "TEST-FAIL")
            .unwrap()
            .record_card_result(outcome, false)
            .unwrap();
        assert!(failed.approved_card_for_total(500).is_err());
        assert!(failed.begin_card(500, "TEST-FAIL").is_err());
        let retry = failed.begin_card(500, "TEST-RETRY").unwrap();
        assert_eq!(retry.card_attempts.len(), 2);
        assert_eq!(retry.card_attempts[1].id, 2);
        assert!(retry.cash_for_total(500).is_err());
        assert!(failed.record_cash(500, 500).is_ok());
    }
}

#[test]
fn unknown_requires_explicit_reconciliation_without_new_charge() {
    let unknown = TenderEvidence::default()
        .begin_card(500, "TEST-UNKNOWN")
        .unwrap()
        .record_card_result(CardOutcome::Unknown, false)
        .unwrap();
    assert!(unknown.begin_card(500, "TEST-RETRY").is_err());
    assert!(unknown.record_cash(500, 500).is_err());
    assert!(
        unknown
            .record_card_result(CardOutcome::Approved, true)
            .is_err()
    );
    assert!(
        unknown
            .reconcile_card(CardOutcome::Approved, false)
            .is_err()
    );
    assert!(unknown.reconcile_card(CardOutcome::Unknown, true).is_err());
    let resolved = unknown.reconcile_card(CardOutcome::Approved, true).unwrap();
    assert_eq!(resolved.card_attempts.len(), 1);
    assert!(resolved.approved_card_for_total(500).unwrap().reconciled);
    let failed = unknown.reconcile_card(CardOutcome::Declined, true).unwrap();
    assert!(failed.begin_card(500, "TEST-RETRY").is_ok());
}

#[test]
fn invalid_input_and_card_data_are_rejected() {
    let evidence = TenderEvidence::default();
    for reference in [
        "",
        "4111111111111111",
        "4111-1111-1111-1111",
        "bad\nref",
        "TEST\n",
        "bad ref",
        "bad'query",
    ] {
        assert!(
            evidence.begin_card(500, reference).is_err(),
            "{reference:?}"
        );
    }
    assert!(evidence.begin_card(500, &"A".repeat(65)).is_err());
    assert!(evidence.begin_card(-1, "TEST").is_err());
    assert!(evidence.record_cash(-1, 500).is_err());
    assert!(evidence.record_cash(500, i64::MAX).is_err());
    assert!(
        evidence
            .record_card_result(CardOutcome::Approved, true)
            .is_err()
    );
    assert!(
        evidence
            .reconcile_card(CardOutcome::Declined, true)
            .is_err()
    );
    let pending = evidence.begin_card(0, "TEST-ZERO").unwrap();
    assert!(
        pending
            .reconcile_card(CardOutcome::Declined, false)
            .is_err()
    );
    assert!(pending.reconcile_card(CardOutcome::Declined, true).is_ok());
}

#[test]
fn serialization_preserves_evidence_without_raw_payment_fields() {
    let evidence = TenderEvidence::default().record_cash(500, 1000).unwrap();
    let json = serde_json::to_string(&evidence).unwrap();
    assert_eq!(
        serde_json::from_str::<TenderEvidence>(&json).unwrap(),
        evidence
    );
    assert_eq!(
        serde_json::from_str::<TenderEvidence>("{}").unwrap(),
        TenderEvidence::default()
    );
    let card = TenderEvidence::default()
        .begin_card(500, "TEST-1")
        .unwrap()
        .record_card_result(CardOutcome::Approved, true)
        .unwrap();
    let json = serde_json::to_string(&card).unwrap();
    assert_eq!(serde_json::from_str::<TenderEvidence>(&json).unwrap(), card);
}
