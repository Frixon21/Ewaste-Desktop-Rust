use super::*;
use crate::{
    auth::Role,
    domain::{Payment, State},
    price_overrides::PriceApproval,
};
fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 7,
        login: "supervisor".into(),
        display_name: "Synthetic Supervisor".into(),
        role: Role::Supervisor,
        revision: 1,
    }
}
fn draft() -> Ticket {
    let mut ticket = Ticket::new(9);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket
}
fn opened(ticket: &Ticket) -> PriceOverrideUi {
    let mut editor = PriceOverrideUi::default();
    editor.reload(ticket, &actor());
    editor
}
#[test]
fn strict_cents_and_required_reason_reject_bad_or_ambiguous_approvals() {
    for input in [
        "", "-1", "+1", "1.001", "1,000", "1e2", "NaN", ".001", "1000000",
    ] {
        assert!(parse_amount(input).is_err(), "Reject {input}");
    }
    for (input, cents) in [
        ("0", 0),
        (".50", 50),
        ("2.75", 275),
        ("$12.5", 1250),
        ("999999.99", 99_999_999),
    ] {
        assert_eq!(parse_amount(input).unwrap(), cents);
    }
    let ticket = draft();
    let mut editor = opened(&ticket);
    editor.amount = "3.00".into();
    assert!(editor.approve_action(&ticket, &actor()).is_err());
    editor.reason = "bad\nreason".into();
    assert!(editor.approve_action(&ticket, &actor()).is_err());
    editor.reason = "x".repeat(501);
    assert!(editor.approve_action(&ticket, &actor()).is_err());
    editor.reason = " Synthetic approved exception ".into();
    match editor.approve_action(&ticket, &actor()).unwrap() {
        PriceOverrideUiAction::Approve {
            ticket_id,
            revision,
            line_id,
            amount_cents,
            reason,
            request_key,
        } => {
            assert_eq!(ticket_id, ticket.id);
            assert_eq!(revision, ticket.revision);
            assert_eq!(line_id, ticket.lines[0].id);
            assert_eq!(amount_cents, 300);
            assert_eq!(reason, "Synthetic approved exception");
            assert!(!request_key.is_empty());
        }
        _ => panic!("Expected approval"),
    }
}
#[test]
fn saved_error_preserves_exact_action_until_ack_or_explicit_refresh() {
    let ticket = draft();
    let mut editor = opened(&ticket);
    editor.amount = "0".into();
    editor.reason = "Synthetic no-fee approval".into();
    let action = editor.approve_action(&ticket, &actor()).unwrap();
    editor.error("Synthetic storage failure".into());
    editor.amount = "99.99".into();
    editor.reason = "Different unsent reason".into();
    assert_eq!(editor.retry_action(&ticket, &actor()).unwrap(), action);
    assert!(editor.approve_action(&ticket, &actor()).is_err());
    assert!(editor.clear_action(&ticket, &actor()).is_err());
    editor.open = false;
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(editor.show(ctx, &ticket, &actor()).is_none());
    });
    assert_eq!(editor.pending_action.as_ref(), Some(&action));
    editor.open = true;
    editor.saved();
    assert!(!editor.open);
    assert!(editor.pending_action.is_none() && editor.original.is_none());
    editor.reload(&ticket, &actor());
    editor.amount = "3".into();
    editor.reason = "Try again".into();
    editor.approve_action(&ticket, &actor()).unwrap();
    editor.refresh_action();
    assert!(editor.pending_action.is_none());
}
#[test]
fn stale_ticket_or_actor_never_receives_a_previous_line_approval() {
    let ticket = draft();
    let mut editor = opened(&ticket);
    editor.amount = "3".into();
    editor.reason = "Synthetic exception".into();
    editor.approve_action(&ticket, &actor()).unwrap();
    let mut changed = ticket.clone();
    changed.revision += 1;
    assert!(editor.retry_action(&changed, &actor()).is_err());
    let mut another = ticket.clone();
    another.id += 1;
    assert!(editor.retry_action(&another, &actor()).is_err());
    let different = OperatorSnapshot {
        revision: 2,
        ..actor()
    };
    assert!(editor.retry_action(&ticket, &different).is_err());
    editor.refresh_action();
    editor.reload(&changed, &actor());
    editor.amount = "4".into();
    editor.reason = "Checked refreshed ticket".into();
    assert!(editor.approve_action(&changed, &actor()).is_ok());
}
#[test]
fn price_actions_require_supervisor_and_editable_stable_fully_priced_line() {
    let ticket = draft();
    let cashier = OperatorSnapshot {
        role: Role::Cashier,
        ..actor()
    };
    let mut editor = opened(&ticket);
    editor.amount = "3".into();
    editor.reason = "Synthetic exception".into();
    assert!(editor.approve_action(&ticket, &cashier).is_err());
    let mut held = ticket.clone();
    held.hold().unwrap();
    let mut form = opened(&held);
    form.reason = "Held".into();
    assert!(form.approve_action(&held, &actor()).is_err());
    let mut paid = ticket.clone();
    paid.choose_blank().unwrap();
    paid.confirm_cash("5").unwrap();
    let mut form = opened(&paid);
    form.reason = "After cash".into();
    assert!(form.approve_action(&paid, &actor()).is_err());
    paid.complete(Payment::Cash).unwrap();
    let mut form = opened(&paid);
    form.reason = "Finalized".into();
    assert!(form.approve_action(&paid, &actor()).is_err());
    let mut legacy = ticket.clone();
    legacy.lines[0].id = 0;
    let mut form = opened(&legacy);
    form.reason = "Legacy".into();
    assert!(form.approve_action(&legacy, &actor()).is_err());
    let mut manual = Ticket::new(2);
    manual.add_line("901", "", "").unwrap();
    let mut form = opened(&manual);
    form.reason = "Unpriced".into();
    assert!(form.approve_action(&manual, &actor()).is_err());
    manual.line_edit(0, "", "2.75").unwrap();
    form.reload(&manual, &actor());
    form.amount = "1.00".into();
    form.reason = "Manual exception".into();
    assert!(form.approve_action(&manual, &actor()).is_ok());
    let mut free = ticket.clone();
    free.lines[0].base_cents = None;
    let mut form = opened(&free);
    form.reason = "Missing baseline".into();
    assert!(form.approve_action(&free, &actor()).is_err());
}
#[test]
fn existing_approval_can_be_replaced_or_cleared_without_rewriting_baseline() {
    let mut ticket = draft();
    ticket.lines[0].price_approval = Some(PriceApproval {
        event_id: 14,
        approved_total_cents: 300,
    });
    let original = ticket.clone();
    let mut editor = opened(&ticket);
    assert_eq!(editor.amount, "3.00");
    editor.amount = "4.00".into();
    editor.reason = "Replacement supervisor decision".into();
    assert!(matches!(
        editor.approve_action(&ticket, &actor()).unwrap(),
        PriceOverrideUiAction::Approve {
            amount_cents: 400,
            ..
        }
    ));
    assert_eq!(ticket, original);
    editor.saved();
    editor.reload(&ticket, &actor());
    editor.reason = "Restore configured tariff".into();
    assert!(matches!(
        editor.clear_action(&ticket, &actor()).unwrap(),
        PriceOverrideUiAction::Clear { .. }
    ));
    assert_eq!(ticket.lines[0].total_cents, Some(500));
    assert_eq!(ticket.lines[0].effective_total_cents(), Some(300));
    let unapproved = draft();
    let mut form = opened(&unapproved);
    form.reason = "Nothing to clear".into();
    assert!(form.clear_action(&unapproved, &actor()).is_err());
}
fn frame(
    ctx: &egui::Context,
    editor: &mut PriceOverrideUi,
    ticket: &Ticket,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<PriceOverrideUiAction>) {
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
            action = editor.show(ctx, ticket, &actor());
        },
    );
    (output, action)
}
fn button(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
    output.shapes.iter().find_map(|shape| {
        if let egui::epaint::Shape::Text(text) = &shape.shape
            && text.galley.text() == label
        {
            let pos = text.pos + text.galley.size() * 0.5;
            assert!(pos.y < 570.);
            assert!(shape.clip_rect.contains(pos));
            Some(pos)
        } else {
            None
        }
    })
}
#[test]
fn minimum_window_pointer_approval_is_reviewable_and_retry_uses_identical_key() {
    let ticket = draft();
    let mut editor = opened(&ticket);
    editor.amount = "3".into();
    editor.reason = "Synthetic pointer approval".into();
    let ctx = egui::Context::default();
    let mut pos = None;
    for _ in 0..3 {
        let (output, action) = frame(&ctx, &mut editor, &ticket, vec![]);
        assert!(action.is_none());
        pos = button(&output, "Approve line price").or(pos);
    }
    let pos = pos.expect("Approval action remains visible");
    let _ = frame(
        &ctx,
        &mut editor,
        &ticket,
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
        &ctx,
        &mut editor,
        &ticket,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    );
    let action = action.expect("Pointer emitted approval");
    assert!(matches!(
        action,
        PriceOverrideUiAction::Approve {
            amount_cents: 300,
            ..
        }
    ));
    editor.error("Synthetic save failure".into());
    let mut retry = None;
    for _ in 0..3 {
        let (output, next) = frame(&ctx, &mut editor, &ticket, vec![]);
        assert!(next.is_none());
        retry = button(&output, "Retry exact price approval").or(retry);
    }
    let pos = retry.expect("Retry visible");
    let _ = frame(
        &ctx,
        &mut editor,
        &ticket,
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
    let (_, again) = frame(
        &ctx,
        &mut editor,
        &ticket,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    );
    assert_eq!(again, Some(action));
    assert_eq!(ticket.state, State::Draft);
    assert_eq!(ticket.total(), Some(500));
    assert!(ticket.lines[0].price_approval.is_none());
}

#[test]
fn reopening_pending_error_with_same_revision_changed_content_blocks_pointer_retry() {
    let ticket = draft();
    let mut editor = opened(&ticket);
    editor.amount = "3".into();
    editor.reason = "Reviewed before concurrent metadata change".into();
    let action = editor.approve_action(&ticket, &actor()).unwrap();
    editor.error("Synthetic saved-data conflict".into());
    editor.open = false;
    let mut changed = ticket.clone();
    changed.lines[0].notes = "Concurrent edit without a revision bump".into();
    assert_eq!(changed.revision, ticket.revision);
    editor.open = true;
    let ctx = egui::Context::default();
    let mut retry = None;
    let mut stale_warning = false;
    for _ in 0..3 {
        let (output, next) = frame(&ctx, &mut editor, &changed, vec![]);
        assert!(next.is_none());
        retry = button(&output, "Retry exact price approval").or(retry);
        stale_warning |= output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Text(text)
                if text.galley.text().contains("operator changed. Refresh"))
        });
    }
    assert!(stale_warning);
    let pos = retry.expect("Disabled retry remains reviewable");
    for pressed in [true, false] {
        let (_, next) = frame(
            &ctx,
            &mut editor,
            &changed,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
        assert!(next.is_none());
    }
    assert_eq!(editor.pending_action, Some(action));
    assert_eq!(
        editor.message.as_deref(),
        Some("Synthetic saved-data conflict")
    );
    assert!(editor.retry_action(&changed, &actor()).is_err());
}
