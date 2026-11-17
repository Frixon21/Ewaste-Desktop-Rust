use super::*;
use crate::{auth::Role, domain::Payment};

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "supervisor".into(),
        display_name: "Synthetic Supervisor".into(),
        role: Role::Supervisor,
        revision: 1,
    }
}
fn ticket(card: bool) -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    if card {
        ticket.start_card("5", "SALE-1").unwrap();
        ticket
            .card_result(crate::tender::CardOutcome::Approved, true, false)
            .unwrap();
        ticket
            .complete(Payment::ExternalCard {
                reference: "SALE-1".into(),
            })
            .unwrap();
    } else {
        ticket.confirm_cash("5").unwrap();
        ticket.complete(Payment::Cash).unwrap();
    }
    ticket.created_at = 90;
    ticket.completed_at = Some(100);
    ticket
}
fn editor(ticket: &Ticket) -> AdjustmentUi {
    let mut ui = AdjustmentUi {
        open: true,
        ..Default::default()
    };
    ui.reload(ticket);
    ui
}
fn intent(ticket: &Ticket, reference: &str) -> (AdjustmentLedger, AdjustmentAttempt) {
    AdjustmentLedger::default()
        .start(
            ticket,
            AdjustmentKind::Refund,
            200,
            &actor(),
            "Synthetic correction",
            "initial-request",
            110,
            reference,
        )
        .unwrap()
}

#[test]
fn start_is_a_pending_action_and_error_preserves_the_exact_request_until_acknowledged() {
    let ticket = ticket(false);
    let mut ui = editor(&ticket);
    ui.request.amount = "2.00".into();
    ui.request.reason = "Synthetic correction".into();
    let empty = AdjustmentLedger::default();
    let action = ui.start_action(&ticket, &empty, &actor()).unwrap();
    assert!(empty.events.is_empty());
    ui.error("Synthetic save failure".into());
    assert_eq!(ui.pending_action.as_ref(), Some(&action));
    assert!(ui.start_action(&ticket, &empty, &actor()).is_err());
    assert_eq!(ui.request.amount, "2.00");
    ui.saved();
    assert!(ui.open);
    assert!(ui.pending_action.is_none());
    assert!(ui.request.amount.is_empty());
}

#[test]
fn stale_ticket_does_not_receive_the_previous_ticket_adjustment() {
    let ticket = ticket(false);
    let mut ui = editor(&ticket);
    ui.request.amount = "2".into();
    ui.request.reason = "Correction".into();
    let switched = Ticket {
        id: 2,
        ..ticket.clone()
    };
    assert!(
        ui.start_action(&switched, &AdjustmentLedger::default(), &actor())
            .is_err()
    );
    assert_eq!(ui.request.amount, "2");
    let revised = Ticket {
        revision: ticket.revision + 1,
        ..ticket.clone()
    };
    assert!(
        ui.start_action(&revised, &AdjustmentLedger::default(), &actor())
            .is_err()
    );
    ui.reload(&switched);
    assert!(ui.request.amount.is_empty());
    assert_eq!(ui.original.as_ref().unwrap().id, 2);
    ui.locked();
    assert!(!ui.open);
    assert!(ui.original.is_none());
    assert!(ui.pending_action.is_none());
}

#[test]
fn refund_amounts_are_strict_cents_and_void_uses_full_remaining_balance() {
    let ticket = ticket(false);
    for bad in ["", "0", "-1", "NaN", "1.001", "6"] {
        let mut ui = editor(&ticket);
        ui.request.reason = "Correction".into();
        ui.request.amount = bad.into();
        assert!(
            ui.start_action(&ticket, &AdjustmentLedger::default(), &actor())
                .is_err(),
            "{bad}"
        );
    }
    let mut ui = editor(&ticket);
    ui.request.reason = "Void sale".into();
    ui.request.kind = AdjustmentKind::Void;
    ui.request.amount = "wrong input".into();
    match ui
        .start_action(&ticket, &AdjustmentLedger::default(), &actor())
        .unwrap()
    {
        AdjustmentUiAction::Start {
            amount_cents, kind, ..
        } => {
            assert_eq!(amount_cents, 500);
            assert_eq!(kind, AdjustmentKind::Void);
        }
        _ => panic!("Expected reserved void intent"),
    }
}

#[test]
fn pending_and_unknown_use_saved_method_and_reference_and_require_final_verification() {
    let ticket = ticket(true);
    let (pending, attempt) = intent(&ticket, "REFUND-1");
    let mut ui = editor(&ticket);
    ui.prepare_result(&attempt);
    assert!(ui.result_action(&ticket, &pending, &actor()).is_err());
    ui.result.as_mut().unwrap().verified = true;
    match ui.result_action(&ticket, &pending, &actor()).unwrap() {
        AdjustmentUiAction::Result {
            reference,
            reconcile,
            attempt_id,
            ..
        } => {
            assert_eq!(reference, "REFUND-1");
            assert!(!reconcile);
            assert_eq!(attempt_id, attempt.id);
        }
        _ => panic!("Expected existing refund result"),
    }
    ui.saved();
    let (unknown, attempt) = pending
        .record_result(
            &ticket,
            attempt.id,
            AdjustmentOutcome::Unknown,
            false,
            "REFUND-1",
            &actor(),
            "unknown",
            111,
            false,
        )
        .unwrap();
    ui.prepare_result(&attempt);
    ui.result.as_mut().unwrap().verified = true;
    match ui.result_action(&ticket, &unknown, &actor()).unwrap() {
        AdjustmentUiAction::Result {
            reference,
            reconcile,
            ..
        } => {
            assert_eq!(reference, "REFUND-1");
            assert!(reconcile);
        }
        _ => panic!("Expected reconciliation"),
    }
}

fn frame(
    ctx: &egui::Context,
    ui: &mut AdjustmentUi,
    ticket: &Ticket,
    ledger: &AdjustmentLedger,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<AdjustmentUiAction>) {
    let mut action = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(820., 570.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            action = ui.show(ctx, ticket, ledger, &actor());
        },
    );
    (output, action)
}
fn button_position(output: &egui::FullOutput, caption: &str) -> Option<egui::Pos2> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        egui::epaint::Shape::Text(text) if text.galley.text() == caption => {
            let pos = text.pos + text.galley.size() * 0.5;
            assert!(pos.y < 570.);
            assert!(shape.clip_rect.contains(pos));
            Some(pos)
        }
        _ => None,
    })
}

#[test]
fn minimum_window_renders_reserved_request_and_result_actions_without_financial_effects() {
    let ticket = ticket(false);
    let context = egui::Context::default();
    let mut ui = editor(&ticket);
    ui.request.amount = "2.00".into();
    ui.request.reason = "Synthetic correction".into();
    let empty = AdjustmentLedger::default();
    let mut button = None;
    for _ in 0..3 {
        let (output, action) = frame(&context, &mut ui, &ticket, &empty, vec![]);
        assert!(action.is_none());
        button = button_position(&output, "Reserve adjustment intent").or(button);
    }
    let pos = button.expect("Reservation action visible");
    let _ = frame(
        &context,
        &mut ui,
        &ticket,
        &empty,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
    );
    let (_, action) = frame(
        &context,
        &mut ui,
        &ticket,
        &empty,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    );
    assert!(matches!(action, Some(AdjustmentUiAction::Start { .. })));
    assert!(empty.events.is_empty());
    ui.saved();
    let (pending, _) = intent(&ticket, "");
    for _ in 0..3 {
        let (output, action) = frame(&context, &mut ui, &ticket, &pending, vec![]);
        assert!(action.is_none());
        if button_position(&output, "Confirm cash returned").is_some() {
            assert_eq!(pending.total_adjusted(ticket.id).unwrap(), 0);
            return;
        }
    }
    panic!("Saved cash intent confirmation should remain visible");
}
