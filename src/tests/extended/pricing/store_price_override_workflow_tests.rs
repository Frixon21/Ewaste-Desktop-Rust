use super::*;
use crate::closeout::ShiftReport;

#[test]
fn export_rejects_unbound_price_authority_without_recording_a_successful_export() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let baseline = draft(&store, &administrator, 1);
    let mut approved = store
        .approve_line_price_as(
            &administrator,
            baseline.revision,
            baseline.id,
            baseline.lines[0].id,
            125,
            "Synthetic valid approval before corruption",
            "export-authority-fixture",
        )
        .unwrap();
    let audit = store.security_audit().unwrap();
    approved.lines[0].price_approval.as_mut().unwrap().event_id = 999;
    let mut statement = store
        .statement("UPDATE tickets SET payload=?1 WHERE id=?2")
        .unwrap();
    statement
        .text(1, &serde_json::to_string(&approved).unwrap())
        .unwrap();
    statement.text(2, &format!("{:020}", approved.id)).unwrap();
    statement.finish().unwrap();
    drop(statement);
    assert!(store.price_overrides().is_err());
    assert!(
        store
            .prepare_intake_export_as(&administrator, false)
            .is_err()
    );
    assert_eq!(store.security_audit().unwrap(), audit);
}

#[test]
fn approved_sale_uses_authorized_total_for_cash_receipt_refund_cap_and_closeout() {
    let db = Db::new();
    let store = db.open();
    let administrator = admin(&store);
    let cashier = operator(&store, &administrator, "cashier", Role::Cashier);
    let baseline = draft(&store, &cashier, 1);
    let approved = store
        .approve_line_price_as(
            &administrator,
            baseline.revision,
            baseline.id,
            baseline.lines[0].id,
            125,
            "Synthetic authorized reduction",
            "downstream-price-approval",
        )
        .unwrap();
    assert_eq!(approved.lines[0].total_cents, Some(500));
    assert_eq!(approved.total(), Some(125));
    let approvals = store.price_overrides().unwrap();

    let mut received = approved;
    received.confirm_cash("5").unwrap();
    let received = save_change(&store, &cashier, received);
    let cash = received.tender_evidence.cash.as_ref().unwrap();
    assert_eq!(cash.amount_due_cents, 125);
    assert_eq!(cash.received_cents, 500);
    assert_eq!(cash.change_cents, 375);
    let mut completed = received;
    completed.complete(Payment::Cash).unwrap();
    let completed = save_change(&store, &cashier, completed);
    assert_eq!(
        completed.completed_by.as_ref().unwrap().id,
        cashier.actor().id
    );
    let receipt = completed.receipt();
    assert!(receipt.contains("Total  $1.25"));
    assert_eq!(
        completed.receipt_snapshot.as_deref(),
        Some(receipt.as_str())
    );

    let original_adjustments = store.adjustments().unwrap();
    let audit = store.security_audit().unwrap();
    assert!(
        store
            .start_adjustment_as(
                &administrator,
                &original_adjustments,
                completed.id,
                AdjustmentKind::Refund,
                126,
                "Synthetic excessive refund",
                "refund-over-approved-price",
                "",
            )
            .is_err()
    );
    assert_eq!(store.adjustments().unwrap(), original_adjustments);
    assert_eq!(store.security_audit().unwrap(), audit);

    let (pending, attempt) = store
        .start_adjustment_as(
            &administrator,
            &original_adjustments,
            completed.id,
            AdjustmentKind::Refund,
            125,
            "Return the approved amount actually collected",
            "refund-approved-price",
            "",
        )
        .unwrap();
    let (refunded, result) = store
        .record_adjustment_result_as(
            &administrator,
            &pending,
            completed.id,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "",
            "refund-approved-price-result",
            false,
        )
        .unwrap();
    assert_eq!(result.amount_cents, 125);
    assert_eq!(result.original_total_cents, 125);
    assert_eq!(store.list().unwrap(), vec![completed.clone()]);
    assert_eq!(store.list().unwrap()[0].receipt(), receipt);
    assert_eq!(store.price_overrides().unwrap(), approvals);

    let report = ShiftReport::from_tickets_and_adjustments(
        &store.list().unwrap(),
        &refunded,
        0,
        crate::domain::now().checked_add(1).unwrap(),
    )
    .unwrap();
    assert_eq!(report.calculation_version, 2);
    assert_eq!(report.cash_sales_cents, 125);
    assert_eq!(report.cash_received_cents, 500);
    assert_eq!(report.cash_change_cents, 375);
    assert_eq!(report.cash_refunds_cents, 125);
    assert_eq!(report.verified_card_cents, 0);
    assert_eq!(report.billing_credits_cents, 0);
    assert_eq!(
        report.adjustment_event_ids,
        vec![refunded.events.last().unwrap().id]
    );
    let closeout = report
        .reconcile_movements(0, 0, 0, 0, "Synthetic administrator", "")
        .unwrap();
    assert_eq!(closeout.expected_cash_cents, 0);
    assert_eq!(closeout.cash_variance_cents, 0);
}
