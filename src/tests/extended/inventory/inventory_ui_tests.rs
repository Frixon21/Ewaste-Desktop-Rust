use super::*;
use crate::{
    domain::Payment,
    intake::SourceCategory,
    intake_review::{ReviewDisposition, ReviewSnapshot},
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
fn completed() -> Ticket {
    let mut ticket = Ticket::new(9);
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.source_intake.category = SourceCategory::Anonymous;
    ticket.lines[0].material_intake.reporting_units = None;
    ticket.lines[0].material_intake.actual_weight_thousandths = None;
    ticket
}
fn opened(
    tickets: &[Ticket],
    reviews: &ReviewLedger,
    directory: &InventoryDirectory,
    ledger: &InventoryLedger,
) -> InventoryUi {
    let mut editor = InventoryUi::default();
    editor.bind(tickets, reviews, directory, ledger, &actor());
    editor
}
#[test]
fn receive_requires_confirmation_and_independent_optional_dates_without_billing_copies() {
    let ticket = completed();
    let tickets = vec![ticket.clone()];
    let reviews = ReviewLedger::default();
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.reason = "Synthetic custody receive".into();
    assert!(editor.receive_action().is_err());
    editor.confirmed = true;
    editor.container = "x".repeat(81);
    assert!(editor.receive_action().is_err());
    editor.container.clear();
    let action = editor.receive_action().unwrap();
    let InventoryUiAction::Receive { input, .. } = action else {
        panic!("Receive expected")
    };
    assert_eq!(input.ticket_id, 9);
    assert_eq!(input.review_event_id, None);
    assert_eq!(input.received_at, None);
    assert_eq!(input.first_accumulation_at, None);
    assert_eq!(input.location_id, 1);
    let preview = editor.selected_origin().unwrap();
    assert_eq!(preview.material.reporting_units, None);
    assert_eq!(preview.material.actual_weight_thousandths, None);
    assert_eq!(ticket.lines[0].gross_thousandths, Some(4000));
    editor.saved();
    editor.bind(&tickets, &reviews, &directory, &ledger, &actor());
    editor.reason = "Synthetic dated receipt".into();
    editor.confirmed = true;
    editor.received_date = "2026-11-16".into();
    let InventoryUiAction::Receive { input, .. } = editor.receive_action().unwrap() else {
        panic!()
    };
    assert!(input.received_at.is_some());
    assert_eq!(input.first_accumulation_at, None);
    assert!(optional_date("2026-02-30").is_err());
    assert_eq!(optional_date(" ").unwrap(), None);
}
#[test]
fn selected_exact_review_snapshot_is_pinned_and_never_silently_latest() {
    let ticket = completed();
    let mut first = ReviewSnapshot::from_ticket(&ticket).unwrap();
    first.source.source_name = "Snapshot one".into();
    first.materials[0].details.reporting_units = Some(3);
    let mut second = first.clone();
    second.source.source_name = "Snapshot two".into();
    second.materials[0].details.reporting_units = Some(8);
    let events = vec![
        crate::intake_review::ReviewEvent {
            actor: Some(actor()),
            id: 1,
            ticket_id: 9,
            at: ticket.completed_at.unwrap(),
            operator: actor().display_name,
            reason: "First documentary correction".into(),
            request_key: "review:one".into(),
            snapshot: first,
        },
        crate::intake_review::ReviewEvent {
            actor: Some(actor()),
            id: 2,
            ticket_id: 9,
            at: ticket.completed_at.unwrap(),
            operator: actor().display_name,
            reason: "Second correction".into(),
            request_key: "review:two".into(),
            snapshot: second,
        },
    ];
    let reviews = ReviewLedger { events, next_id: 3 };
    let tickets = vec![ticket];
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.review_event_id = Some(1);
    let origin = editor.selected_origin().unwrap();
    assert_eq!(origin.source.source_name, "Snapshot one");
    assert_eq!(origin.material.reporting_units, Some(3));
    assert_eq!(origin.review_disposition, ReviewDisposition::PendingReview);
    editor.confirmed = true;
    editor.reason = "Receive exactly first review".into();
    let InventoryUiAction::Receive { input, .. } = editor.receive_action().unwrap() else {
        panic!()
    };
    assert_eq!(input.review_event_id, Some(1));
}
#[test]
fn pending_error_retry_is_exact_and_changed_directory_ledger_ticket_or_actor_blocks() {
    let tickets = vec![completed()];
    let reviews = ReviewLedger::default();
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.confirmed = true;
    editor.reason = "Original receive".into();
    let action = editor.receive_action().unwrap();
    editor.error("Synthetic write failure".into());
    editor.reason = "Unsent change".into();
    assert_eq!(editor.retry_action().unwrap(), action);
    let mut changed = directory.clone();
    changed.locations[0].name = "Changed staging".into();
    assert!(editor.stale(&tickets, &reviews, &changed, &ledger, &actor()));
    let mut changed = ledger.clone();
    changed.next_id += 1;
    assert!(editor.stale(&tickets, &reviews, &directory, &changed, &actor()));
    let mut changed = tickets.clone();
    changed[0].lines[0].notes = "Concurrent metadata".into();
    assert!(editor.stale(&changed, &reviews, &directory, &ledger, &actor()));
    let mut changed = actor();
    changed.revision += 1;
    assert!(editor.stale(&tickets, &reviews, &directory, &ledger, &changed));
    editor.open = false;
    assert_eq!(editor.pending.as_ref(), Some(&action));
    editor.saved();
    assert!(editor.pending.is_none() && editor.base.is_none());
}
#[test]
fn locations_preserve_actual_active_flags_and_validate_bounded_fields() {
    let tickets = vec![completed()];
    let reviews = ReviewLedger::default();
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.location_edit_id = Some(1);
    editor.location_name = "Renamed staging".into();
    editor.location_active = false;
    let InventoryUiAction::SaveLocation { location } = editor.location_action().unwrap() else {
        panic!()
    };
    assert_eq!(location.id, 1);
    assert!(!location.active);
    assert_eq!(location.name, "Renamed staging");
    editor.saved();
    editor.bind(&tickets, &reviews, &directory, &ledger, &actor());
    editor.location_name = "New storage".into();
    editor.location_active = true;
    let InventoryUiAction::SaveLocation { location } = editor.location_action().unwrap() else {
        panic!()
    };
    assert_eq!(location.id, 0);
    assert!(location.active);
    assert!(bounded("bad\nname", "name", 120, true).is_err());
    assert!(shipment_weight("-2").is_err());
    assert!(shipment_weight("2.0001").is_err());
    assert_eq!(shipment_weight("").unwrap(), None);
    assert_eq!(shipment_weight("2.125").unwrap(), Some(2125));
}
fn with_on_hand() -> (
    Vec<Ticket>,
    ReviewLedger,
    InventoryDirectory,
    InventoryLedger,
) {
    let tickets = vec![completed()];
    let reviews = ReviewLedger::default();
    let mut directory = InventoryDirectory::default();
    directory.locations.push(InventoryLocation {
        id: 2,
        name: "Synthetic storage".into(),
        active: true,
    });
    directory.next_id = 3;
    let empty = InventoryLedger::default();
    let editor = opened(&tickets, &reviews, &directory, &empty);
    let at = tickets[0].completed_at.unwrap() + 1;
    let lot = InventoryLot {
        id: 1,
        origin: editor.selected_origin().unwrap(),
        location: directory.locations[0].clone(),
        container: "Original bin".into(),
        received_at: None,
        first_accumulation_at: None,
        recorded_at: at,
        received_by: actor(),
        status: LotStatus::OnHand,
        shipment_id: None,
    };
    let event = InventoryEvent {
        id: 1,
        at,
        actor: actor(),
        request_key: "inventory:received".into(),
        reason: "Synthetic received whole lot".into(),
        action: InventoryAction::Received { lot },
    };
    (
        tickets,
        reviews,
        directory,
        InventoryLedger {
            events: vec![event],
            next_id: 2,
            next_lot_id: 2,
            next_shipment_id: 1,
        },
    )
}
#[test]
fn movement_and_shipment_require_on_hand_whole_lots_and_preserve_unknown_measures() {
    let (tickets, reviews, directory, ledger) = with_on_hand();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.reason = "Cannot receive twice".into();
    editor.confirmed = true;
    assert!(editor.receive_action().is_err());
    editor.confirmed = false;
    editor.move_lot = Some(1);
    editor.location_id = 2;
    editor.container = "New bin".into();
    editor.reason = "Synthetic whole lot movement".into();
    assert!(editor.move_action().is_err());
    editor.confirmed = true;
    let action = editor.move_action().unwrap();
    assert!(matches!(
        action,
        InventoryUiAction::Move {
            lot_id: 1,
            to_location_id: 2,
            ..
        }
    ));
    editor.error("Movement write failed".into());
    assert_eq!(editor.retry_action().unwrap(), action);
    editor.saved();
    editor.bind(&tickets, &reviews, &directory, &ledger, &actor());
    editor.ship_lots = vec![1];
    editor.destination_name = "Synthetic receiving facility".into();
    editor.reason = "Synthetic shipment".into();
    assert!(editor.ship_action().is_err());
    editor.confirmed = true;
    assert!(
        editor.ship_action().is_err(),
        "Evidence is required independently of confirmation"
    );
    editor.evidence = "x".repeat(121);
    assert!(editor.ship_action().is_err());
    editor.evidence = "Synthetic manifest reference".into();
    let InventoryUiAction::Ship { input, .. } = editor.ship_action().unwrap() else {
        panic!()
    };
    assert_eq!(input.lot_ids, vec![1]);
    assert_eq!(input.actual_weight_thousandths, None);
    assert_eq!(input.shipped_at, None);
    assert_eq!(input.carrier, PartySnapshot::default());
    assert_eq!(input.evidence_reference, "Synthetic manifest reference");
    assert_eq!(
        ledger
            .project()
            .unwrap()
            .lot(1)
            .unwrap()
            .origin
            .material
            .actual_weight_thousandths,
        None
    );
    editor.saved();
    editor.bind(&tickets, &reviews, &directory, &ledger, &actor());
    editor.ship_lots = vec![99];
    editor.destination_name = "Synthetic facility".into();
    editor.reason = "Invalid lot".into();
    editor.confirmed = true;
    assert!(editor.ship_action().is_err());
}
fn frame(
    ctx: &egui::Context,
    editor: &mut InventoryUi,
    tickets: &[Ticket],
    reviews: &ReviewLedger,
    directory: &InventoryDirectory,
    ledger: &InventoryLedger,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<InventoryUiAction>) {
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
        |ctx| action = editor.show(ctx, tickets, reviews, directory, ledger, &actor()),
    );
    (output, action)
}
fn text_position(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
    output.shapes.iter().find_map(|s| {
        if let egui::epaint::Shape::Text(text) = &s.shape
            && text.galley.text() == label
        {
            let pos = text.pos + text.galley.size() * 0.5;
            assert!(
                pos.y < 570. && s.clip_rect.contains(pos),
                "Visible control: {label}"
            );
            Some(pos)
        } else {
            None
        }
    })
}
fn click(
    ctx: &egui::Context,
    editor: &mut InventoryUi,
    inputs: (
        &[Ticket],
        &ReviewLedger,
        &InventoryDirectory,
        &InventoryLedger,
    ),
    pos: egui::Pos2,
) -> Option<InventoryUiAction> {
    let mut action = None;
    for pressed in [true, false] {
        let (_, next) = frame(
            ctx,
            editor,
            inputs.0,
            inputs.1,
            inputs.2,
            inputs.3,
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
        action = next.or(action);
    }
    action
}
#[test]
fn minimum_window_pointer_confirmation_then_receive_and_exact_retry_are_visible() {
    let tickets = vec![completed()];
    let reviews = ReviewLedger::default();
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.tab = Tab::Receive;
    editor.reason = "Synthetic pointer receipt".into();
    let ctx = egui::Context::default();
    let mut output = None;
    for _ in 0..3 {
        output = Some(
            frame(
                &ctx,
                &mut editor,
                &tickets,
                &reviews,
                &directory,
                &ledger,
                vec![],
            )
            .0,
        );
    }
    let output = output.unwrap();
    for tab in [
        "On hand",
        "Receive",
        "Move",
        "Ship",
        "Locations",
        "History / issues",
    ] {
        assert!(text_position(&output, tab).is_some());
    }
    let submit =
        text_position(&output, "Receive confirmed whole lot").expect("Receive action visible");
    let inputs = (&tickets[..], &reviews, &directory, &ledger);
    assert!(click(&ctx, &mut editor, inputs, submit).is_none());
    assert!(editor.message.as_deref().unwrap().contains("Confirm"));
    let (mut output, _) = frame(
        &ctx,
        &mut editor,
        &tickets,
        &reviews,
        &directory,
        &ledger,
        vec![],
    );
    for _ in 0..2 {
        output = frame(
            &ctx,
            &mut editor,
            &tickets,
            &reviews,
            &directory,
            &ledger,
            vec![],
        )
        .0;
    }
    let confirm = text_position(
        &output,
        "I confirm this pinned source and material snapshot for this whole lot",
    )
    .expect("Pinned snapshot confirmation visible");
    assert!(click(&ctx, &mut editor, inputs, confirm).is_none());
    assert!(editor.confirmed);
    let output = frame(
        &ctx,
        &mut editor,
        &tickets,
        &reviews,
        &directory,
        &ledger,
        vec![],
    )
    .0;
    let submit = text_position(&output, "Receive confirmed whole lot").unwrap();
    let action = click(&ctx, &mut editor, inputs, submit).expect("Confirmed receive emitted");
    editor.error("Synthetic receive save failure".into());
    let mut retry = None;
    for _ in 0..3 {
        retry = text_position(
            &frame(
                &ctx,
                &mut editor,
                &tickets,
                &reviews,
                &directory,
                &ledger,
                vec![],
            )
            .0,
            "Retry exact inventory action",
        )
        .or(retry)
    }
    assert_eq!(
        click(&ctx, &mut editor, inputs, retry.unwrap()),
        Some(action)
    );
    assert!(ledger.events.is_empty());
}
#[test]
fn unauthorized_actor_hides_inventory_and_discards_sensitive_pending_form() {
    let tickets = vec![completed()];
    let reviews = ReviewLedger::default();
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.confirmed = true;
    editor.reason = "Synthetic private reason".into();
    editor.receive_action().unwrap();
    let cashier = OperatorSnapshot {
        role: Role::Cashier,
        ..actor()
    };
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(
            editor
                .show(ctx, &tickets, &reviews, &directory, &ledger, &cashier)
                .is_none()
        )
    });
    assert!(!editor.open);
    assert!(editor.pending.is_none() && editor.base.is_none());
    assert!(output.shapes.is_empty());
}
#[test]
fn minimum_window_move_ship_locations_history_have_visible_actions_and_unknowns() {
    let (tickets, reviews, directory, ledger) = with_on_hand();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    let ctx = egui::Context::default();
    for (tab, label) in [
        (Tab::Move, "Move confirmed whole lot"),
        (Tab::Ship, "Ship confirmed whole lots"),
        (Tab::Locations, "Save location"),
        (Tab::OnHand, "Export inventory CSV"),
    ] {
        editor.tab = tab;
        let mut output = None;
        for _ in 0..3 {
            output = Some(
                frame(
                    &ctx,
                    &mut editor,
                    &tickets,
                    &reviews,
                    &directory,
                    &ledger,
                    vec![],
                )
                .0,
            );
        }
        let output = output.unwrap();
        assert!(
            text_position(&output, label).is_some(),
            "Action visible for each tab"
        );
        assert!(text_position(&output, "Refresh saved inventory").is_some());
    }
    assert!(!editor.include_contacts);
    editor.tab = Tab::History;
    let output = frame(
        &ctx,
        &mut editor,
        &tickets,
        &reviews,
        &directory,
        &ledger,
        vec![],
    )
    .0;
    assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::epaint::Shape::Text(text)if text.galley.text()=="Append-only custody history")));
}
#[test]
fn stale_saved_directory_disables_pending_pointer_retry_until_explicit_refresh() {
    let tickets = vec![completed()];
    let reviews = ReviewLedger::default();
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.confirmed = true;
    editor.reason = "Synthetic pending custody action".into();
    let action = editor.receive_action().unwrap();
    editor.error("Synthetic storage conflict".into());
    let mut changed = directory.clone();
    changed.locations[0].name = "Changed saved staging".into();
    let ctx = egui::Context::default();
    let mut output = None;
    for _ in 0..3 {
        output = Some(
            frame(
                &ctx,
                &mut editor,
                &tickets,
                &reviews,
                &changed,
                &ledger,
                vec![],
            )
            .0,
        );
    }
    let output = output.unwrap();
    let retry = text_position(&output, "Retry exact inventory action")
        .expect("Pending retry remains visible");
    let inputs = (&tickets[..], &reviews, &changed, &ledger);
    assert!(click(&ctx, &mut editor, inputs, retry).is_none());
    assert_eq!(editor.pending, Some(action));
    assert_eq!(
        editor.message.as_deref(),
        Some("Synthetic storage conflict")
    );
    let output = frame(
        &ctx,
        &mut editor,
        &tickets,
        &reviews,
        &changed,
        &ledger,
        vec![],
    )
    .0;
    let refresh = text_position(&output, "Refresh saved inventory").unwrap();
    assert_eq!(
        click(&ctx, &mut editor, inputs, refresh),
        Some(InventoryUiAction::Refresh)
    );
    assert!(editor.pending.is_none());
    editor.saved();
    editor.open = true;
    let _ = frame(
        &ctx,
        &mut editor,
        &tickets,
        &reviews,
        &changed,
        &ledger,
        vec![],
    );
    assert!(!editor.stale(&tickets, &reviews, &changed, &ledger, &actor()));
    assert_eq!(editor.received_date, "");
    assert_eq!(editor.accumulation_date, "");
}
#[test]
fn fresh_locations_form_action_creates_through_directory_and_existing_edit_keeps_identity() {
    let tickets = vec![completed()];
    let reviews = ReviewLedger::default();
    let directory = InventoryDirectory::default();
    let ledger = InventoryLedger::default();
    let mut editor = opened(&tickets, &reviews, &directory, &ledger);
    editor.tab = Tab::Locations;
    editor.location_name = "Synthetic new storage".into();
    editor.location_active = true;
    let ctx = egui::Context::default();
    let mut output = None;
    for _ in 0..3 {
        output = Some(
            frame(
                &ctx,
                &mut editor,
                &tickets,
                &reviews,
                &directory,
                &ledger,
                vec![],
            )
            .0,
        );
    }
    let pos = text_position(&output.unwrap(), "Save location").expect("New location save visible");
    let InventoryUiAction::SaveLocation { location } = click(
        &ctx,
        &mut editor,
        (&tickets, &reviews, &directory, &ledger),
        pos,
    )
    .expect("Fresh form emits save action") else {
        panic!("Location save expected")
    };
    let (updated, created) = directory
        .save_location(&location)
        .expect("UI creation action must satisfy directory creation contract");
    assert_eq!(created.id, directory.next_id);
    assert_eq!(updated.next_id, directory.next_id + 1);
    assert_eq!(created.name, "Synthetic new storage");
    assert!(created.active);
    assert_eq!(directory.locations.len(), 1);
    editor.saved();
    editor.bind(&tickets, &reviews, &updated, &ledger, &actor());
    editor.location_edit_id = Some(created.id);
    editor.location_name = "Renamed new storage".into();
    editor.location_active = false;
    let InventoryUiAction::SaveLocation { location } = editor.location_action().unwrap() else {
        panic!()
    };
    assert_eq!(location.id, created.id);
    let (renamed, saved) = updated.save_location(&location).unwrap();
    assert_eq!(saved.id, created.id);
    assert!(!saved.active);
    assert_eq!(saved.name, "Renamed new storage");
    assert_eq!(renamed.next_id, updated.next_id);
    assert_eq!(renamed.locations.len(), updated.locations.len());
}
