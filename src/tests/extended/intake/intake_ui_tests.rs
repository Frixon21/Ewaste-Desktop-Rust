use super::*;

fn draft() -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.add_weight_line("803", "5", "1").unwrap();
    ticket.choose_blank().unwrap();
    ticket
}

fn editor(ticket: &Ticket) -> IntakeUi {
    let mut editor = IntakeUi {
        open: true,
        ..Default::default()
    };
    editor.reload(ticket);
    editor
}

#[test]
fn opening_and_cancelling_preserves_ticket_and_unknown_fields() {
    let ticket = draft();
    let original = ticket.clone();
    let context = egui::Context::default();
    let mut editor = IntakeUi {
        open: true,
        ..Default::default()
    };
    let output = context.run(Default::default(), |ctx| {
        assert!(editor.show(ctx, &ticket).is_none());
    });
    assert!(!output.shapes.is_empty());
    assert_eq!(ticket, original);
    assert_eq!(editor.form.as_ref().unwrap().source, ticket.source_intake);
    editor.form.as_mut().unwrap().source.source_name = "Unsaved source name".into();
    editor.cancel();
    assert!(!editor.open);
    assert!(editor.form.is_none());
    assert_eq!(ticket, original);
}

#[test]
fn intake_billing_displays_approved_total_separately_from_original_tariff() {
    let mut ticket = draft();
    ticket.lines[0].price_approval = Some(crate::price_overrides::PriceApproval {
        event_id: 1,
        approved_total_cents: 300,
    });
    let mut form = IntakeForm::new(&ticket);
    let context = egui::Context::default();
    let mut text = String::new();
    for _ in 0..2 {
        let frame = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                edit_material(ui, &mut form, &ticket, 0);
            });
        });
        for shape in frame.shapes {
            if let egui::epaint::Shape::Text(text_shape) = shape.shape {
                text.push_str(text_shape.galley.text());
                text.push('\n');
            }
        }
    }
    assert!(text.contains("Billing: 4 lb / $3.00"));
    assert!(text.contains("Original tariff line total: $5.00"));
    assert_eq!(ticket.lines[0].total_cents, Some(500));
}

#[test]
fn stale_ticket_requires_explicit_reload_and_failed_save_preserves_form() {
    let ticket = draft();
    let mut editor = editor(&ticket);
    editor.form.as_mut().unwrap().source.source_name = "Synthetic source".into();
    let newer = Ticket {
        revision: ticket.revision + 1,
        ..ticket.clone()
    };
    assert!(editor.candidate(&newer).is_err());
    assert_eq!(
        editor.form.as_ref().unwrap().source.source_name,
        "Synthetic source"
    );
    assert!(
        editor
            .candidate(&Ticket {
                id: 2,
                ..ticket.clone()
            })
            .is_err()
    );
    editor.error("Synthetic persistence failure".into());
    assert!(editor.open);
    assert!(editor.message.is_some());
    assert_eq!(
        editor.form.as_ref().unwrap().source.source_name,
        "Synthetic source"
    );
    let context = egui::Context::default();
    let _ = context.run(Default::default(), |ctx| {
        assert!(editor.show(ctx, &newer).is_none());
    });
    assert_eq!(
        editor.form.as_ref().unwrap().source.source_name,
        "Synthetic source"
    );
    editor.reload(&newer);
    assert!(editor.message.is_none());
    assert_eq!(editor.original.as_ref().unwrap().revision, newer.revision);
}

#[test]
fn held_completed_and_taken_tender_block_saves_without_changing_evidence() {
    let ticket = draft();
    for state in [crate::domain::State::Held, crate::domain::State::Completed] {
        let locked = Ticket {
            state,
            ..ticket.clone()
        };
        assert!(editor(&locked).candidate(&locked).is_err());
        let context = egui::Context::default();
        let mut editor = editor(&locked);
        let _ = context.run(Default::default(), |ctx| {
            assert!(editor.show(ctx, &locked).is_none());
        });
    }
    let mut taken = ticket.clone();
    taken.confirm_cash("5").unwrap();
    assert!(editor(&taken).candidate(&taken).is_err());
    for outcome in [
        crate::tender::CardOutcome::Pending,
        crate::tender::CardOutcome::Unknown,
        crate::tender::CardOutcome::Approved,
    ] {
        let mut card = ticket.clone();
        card.start_card("5", "EXAMPLE-LOCKED").unwrap();
        if outcome != crate::tender::CardOutcome::Pending {
            card.card_result(
                outcome,
                outcome == crate::tender::CardOutcome::Approved,
                false,
            )
            .unwrap();
        }
        assert!(editor(&card).candidate(&card).is_err());
    }
}

#[test]
fn customer_and_weight_copy_are_explicit_and_do_not_change_billing_or_receipts() {
    let mut ticket = draft();
    let customer = crate::customers::Customer {
        id: 7,
        number: "C000007".into(),
        name: "Example Source".into(),
        address: "7 Example Lane".into(),
        phone: "555-0107".into(),
        notice: "Private notice".into(),
    };
    ticket.select_customer(&customer).unwrap();
    let original = ticket.clone();
    let mut editor = editor(&ticket);
    let form = editor.form.as_mut().unwrap();
    assert!(form.source.source_name.is_empty());
    assert!(form.materials[0].actual_weight_thousandths.is_none());
    form.source.category = SourceCategory::Business;
    form.copy_customer(&ticket).unwrap();
    form.copy_weight(&ticket, 0).unwrap();
    assert_eq!(form.source.category, SourceCategory::Business);
    assert_eq!(form.source.linked_customer_id, Some(7));
    assert!(form.source.notes.is_empty());
    let next = editor.candidate(&ticket).unwrap();
    assert_eq!(next.source_intake.source_name, customer.name);
    assert_eq!(
        next.lines[0].material_intake.actual_weight_thousandths,
        Some(4000)
    );
    assert!(next.lines[0].material_intake.reporting_units.is_none());
    assert_eq!(next.total(), ticket.total());
    assert_eq!(ticket, original);
    assert!(
        next.customer_record
            .as_ref()
            .unwrap()
            .notice
            .contains("Private")
    );
    editor.saved();
    assert!(!editor.open);
    assert!(editor.form.is_none());
}

#[test]
fn reusable_form_normalizes_optional_reporting_fields_and_retains_invalid_input() {
    let ticket = draft();
    let mut form = IntakeForm::new(&ticket);
    assert!(!form.dirty(&ticket));
    form.units[0] = "3".into();
    form.weights[0] = "4.125".into();
    form.materials[0].class = MaterialClass::OtherElectronics;
    let normalized = form.normalized().unwrap();
    assert_eq!(normalized.materials[0].reporting_units, Some(3));
    assert_eq!(
        normalized.materials[0].actual_weight_thousandths,
        Some(4125)
    );
    assert!(form.dirty(&ticket));
    for bad in ["-1", "1.5", "0", "1000000"] {
        form.units[0] = bad.into();
        assert!(form.normalized().is_err());
        assert_eq!(form.units[0], bad);
    }
    form.units[0].clear();
    for bad in ["-1", "NaN", "0", "1.0001"] {
        form.weights[0] = bad.into();
        assert!(form.normalized().is_err());
    }
    form.weights[0].clear();
    assert!(
        form.normalized().unwrap().materials[0]
            .reporting_units
            .is_none()
    );
    assert!(
        form.normalized().unwrap().materials[0]
            .actual_weight_thousandths
            .is_none()
    );
}

#[test]
fn copy_helpers_never_invent_customer_identity_or_unmeasured_weight() {
    let ticket = draft();
    let mut form = IntakeForm::new(&ticket);
    assert!(form.copy_customer(&ticket).is_err());
    let mut unmeasured = ticket.clone();
    unmeasured.lines[0].gross_thousandths = None;
    unmeasured.lines[0].tare_thousandths = None;
    assert!(form.copy_weight(&unmeasured, 0).is_err());
    assert!(form.copy_weight(&ticket, 9).is_err());
    assert!(form.weights[0].is_empty());
    let context = egui::Context::default();
    let output = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| form.fields(ui, &ticket));
    });
    assert!(!output.shapes.is_empty());
    assert_eq!(ticket.source_intake, SourceIntake::default());
}

#[test]
fn independent_source_detach_preserves_fields_and_allows_documentary_review() {
    use crate::intake_review::{ReviewDisposition, ReviewSnapshot};
    let mut ticket = draft();
    ticket
        .select_customer(&crate::customers::Customer {
            id: 7,
            number: "C000007".into(),
            name: "Example Original Customer".into(),
            phone: "555-0107".into(),
            address: "7 Example Lane".into(),
            ..Default::default()
        })
        .unwrap();
    let mut form = IntakeForm::new(&ticket);
    form.copy_customer(&ticket).unwrap();
    assert!(form.customer_link_label(&ticket).contains("C000007"));
    form.source.category = SourceCategory::Residential;
    form.source.reason = IntakeReason::EmptyingGarage;
    form.source.delivery = DeliveryMode::DropOff;
    form.source.source_name = "Independent Actual Source".into();
    form.source.source_address = "8 Separate Lane".into();
    form.source.notes = "Retain this review note".into();
    form.materials[0].class = MaterialClass::OtherElectronics;
    form.materials[0].condition = MaterialCondition::Intact;
    form.units[0] = "1".into();
    ticket.choose_blank().unwrap();
    assert!(
        form.customer_link_label(&ticket)
            .contains("different from current customer")
    );
    ticket.confirm_cash("5").unwrap();
    ticket.complete(crate::domain::Payment::Cash).unwrap();
    let frozen = ticket.clone();
    let mut snapshot = ReviewSnapshot::from_ticket(&ticket).unwrap();
    snapshot.source = form.normalized().unwrap().source;
    snapshot.materials[0].details = form.normalized().unwrap().materials[0].clone();
    snapshot.disposition = ReviewDisposition::Reviewed;
    assert!(snapshot.normalized_for(&ticket).is_err());
    let before = form.source.clone();
    form.detach_customer_link();
    assert!(
        form.customer_link_label(&ticket)
            .contains("Independent source")
    );
    assert_eq!(
        form.source,
        SourceIntake {
            linked_customer_id: None,
            ..before
        }
    );
    snapshot.source = form.normalized().unwrap().source;
    snapshot.normalized_for(&ticket).unwrap();
    assert_eq!(ticket, frozen);
}

#[test]
fn intake_save_and_cancel_stay_visible_in_minimum_register_window() {
    let ticket = draft();
    let context = egui::Context::default();
    let mut editor = editor(&ticket);
    editor.form.as_mut().unwrap().source.source_name = "Unsaved review".into();
    for _ in 0..3 {
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(820., 570.),
                )),
                ..Default::default()
            },
            |ctx| {
                assert!(editor.show(ctx, &ticket).is_none());
            },
        );
        let footer = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if ["Validate and save intake", "Cancel changes"]
                        .contains(&text.galley.text()) =>
                {
                    Some((text, shape.clip_rect))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if footer.len() == 2 {
            for (text, clip) in footer {
                assert!(text.pos.y + text.galley.size().y <= 570.);
                assert!(clip.contains(text.pos));
            }
            return;
        }
    }
    panic!("Save and cancel actions did not render within the intake window");
}
