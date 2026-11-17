use super::*;

fn prior(start: u64, end: u64) -> Closeout {
    ShiftReport::from_tickets(&[], start, end)
        .unwrap()
        .reconcile(0, 0, 0, "Synthetic operator", "")
        .unwrap()
}

#[test]
fn presets_use_saved_window_end_and_capture_a_stable_end() {
    let mut panel = CloseoutUi::default();
    assert_eq!(panel.interval(&[], 200).unwrap(), (0, 200));
    assert_eq!(panel.interval(&[], 250).unwrap(), (0, 200));
    panel.window_end = None;
    let saved = vec![prior(100, 150), prior(0, 100)];
    assert_eq!(panel.interval(&saved, 200).unwrap(), (150, 200));
    panel.mode = IntervalMode::AllHistory;
    assert_eq!(panel.interval(&saved, 200).unwrap(), (0, 200));
    assert!(overlaps_saved(&saved, 0, 200));
    assert!(!overlaps_saved(&saved, 150, 200));
    panel.window_end = None;
    panel.mode = IntervalMode::SinceLast;
    assert!(panel.interval(&saved, 149).is_err());
}

#[test]
fn current_second_completion_remains_available_for_next_interval() {
    let mut panel = CloseoutUi::default();
    let (start, end) = panel.interval(&[], 200).unwrap();
    let closed = ShiftReport::from_tickets(&[], start, end)
        .unwrap()
        .reconcile(0, 0, 0, "Operator", "")
        .unwrap();
    let mut ticket = Ticket::new(1);
    ticket.created_at = 100;
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(crate::domain::Payment::Cash).unwrap();
    ticket.completed_at = Some(200);
    panel.window_end = None;
    let (next_start, next_end) = panel.interval(&[closed], 201).unwrap();
    let next = ShiftReport::from_tickets(&[ticket], next_start, next_end).unwrap();
    assert_eq!(next.completed_tickets, 1);
    assert_eq!(next.cash_sales_cents, 500);
}

#[test]
fn calendar_labels_are_utc_and_do_not_expose_raw_epoch_inputs() {
    assert_eq!(utc_label(0), "1970-01-01 00:00:00 UTC");
    assert_eq!(utc_label(86_400), "1970-01-02 00:00:00 UTC");
    assert_eq!(utc_label(1_709_164_800), "2024-02-29 00:00:00 UTC");
    assert_eq!(utc_label(u64::MAX), "Outside supported calendar range");
}

#[test]
fn declarations_require_discrepancy_reason_and_preserve_errors() {
    let report = ShiftReport::from_tickets(&[], 100, 200).unwrap();
    let mut panel = CloseoutUi {
        operator: "Operator".into(),
        counted: "1.00".into(),
        ..Default::default()
    };
    assert!(panel.candidate(&report).is_err());
    panel.reason = "Recounted discrepancy".into();
    assert_eq!(panel.candidate(&report).unwrap().cash_variance_cents, 100);
    panel.opening = "-1".into();
    assert!(panel.candidate(&report).is_err());
    panel.error("Disk unavailable".into());
    assert_eq!(panel.error.as_deref(), Some("Disk unavailable"));
    panel.open = true;
    panel.saved();
    assert!(!panel.open);
    assert!(panel.error.is_none());
    assert_eq!(panel.operator, "Operator");
}

#[test]
fn panel_renders_headlessly_with_saved_history_without_saving() {
    let mut panel = CloseoutUi {
        open: true,
        ..Default::default()
    };
    let context = egui::Context::default();
    let saved = vec![prior(100, 200)];
    let output = context.run(egui::RawInput::default(), |ctx| {
        assert!(panel.show(ctx, &[], &saved).is_none());
    });
    assert!(!output.shapes.is_empty());
    panel.selected_history = Some(saved[0].id.clone());
    let _ = context.run(egui::RawInput::default(), |ctx| {
        assert!(panel.show(ctx, &[], &saved).is_none());
    });
    assert_eq!(saved[0].report.start, 100);
}

#[test]
fn authenticated_operator_and_refund_declarations_render_and_reconcile_separately() {
    use crate::{
        adjustments::{AdjustmentKind, AdjustmentOutcome},
        auth::{OperatorSnapshot, Role},
        domain::Payment,
        tender::CardOutcome,
    };
    let actor = OperatorSnapshot {
        id: 1,
        login: "supervisor".into(),
        display_name: "Supervisor".into(),
        role: Role::Supervisor,
        revision: 1,
    };
    let mut ticket = Ticket::new(1);
    ticket.created_at = 89;
    ticket.add_line("803", "4", "").unwrap();
    ticket.choose_blank().unwrap();
    ticket.start_card("5", "TEST-CLOSEOUT-SALE").unwrap();
    ticket
        .card_result(CardOutcome::Approved, true, false)
        .unwrap();
    ticket
        .complete(Payment::ExternalCard {
            reference: "TEST-CLOSEOUT-SALE".into(),
        })
        .unwrap();
    ticket.completed_at = Some(90);
    let (pending, attempt) = AdjustmentLedger::default()
        .start(
            &ticket,
            AdjustmentKind::Refund,
            500,
            &actor,
            "Synthetic refund",
            "request",
            120,
            "TEST-CLOSEOUT-REFUND",
        )
        .unwrap();
    let (ledger, _) = pending
        .record_result(
            &ticket,
            attempt.id,
            AdjustmentOutcome::Completed,
            true,
            "TEST-CLOSEOUT-REFUND",
            &actor,
            "result",
            121,
            false,
        )
        .unwrap();
    let report =
        ShiftReport::from_tickets_and_adjustments(&[ticket.clone()], &ledger, 100, 200).unwrap();
    let mut panel = CloseoutUi {
        open: true,
        window_end: Some(200),
        external_card_refunds: "5.00".into(),
        ..Default::default()
    };
    panel.set_operator(&actor.display_name);
    let candidate = panel.candidate(&report).unwrap();
    assert_eq!(candidate.operator, "Supervisor");
    assert_eq!(candidate.external_card_cents, 0);
    assert_eq!(candidate.external_card_refunds_cents, 500);
    assert_eq!(candidate.expected_card_cents, -500);
    let previous = ShiftReport::from_tickets(&[ticket.clone()], 0, 100)
        .unwrap()
        .reconcile(0, 0, 500, "Supervisor", "")
        .unwrap();
    let context = egui::Context::default();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(820., 570.),
            )),
            ..Default::default()
        },
        |context| {
            assert!(
                panel
                    .show_with_adjustments(
                        context,
                        &[ticket.clone()],
                        std::slice::from_ref(&previous),
                        &ledger
                    )
                    .is_none()
            );
        },
    );
    assert!(!output.shapes.is_empty());
    panel.saved();
    assert_eq!(panel.external_card_refunds, "0.00");
    assert_eq!(panel.operator, "Supervisor");
}
