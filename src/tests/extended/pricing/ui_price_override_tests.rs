use super::*;
struct Db(std::path::PathBuf);
impl Db {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "ewaste-price-native-{}-{}.sqlite3",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
    }
    fn app(&self) -> RegisterApp {
        RegisterApp::new(Store::open(&self.0).unwrap()).unwrap()
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn action(ticket: &Ticket) -> PriceOverrideUiAction {
    PriceOverrideUiAction::Approve {
        ticket_id: ticket.id,
        revision: ticket.revision,
        line_id: ticket.lines[0].id,
        amount_cents: 300,
        reason: "Training courtesy with a retained reason".into(),
        request_key: "native-approval".into(),
    }
}
fn prepared(db: &Db) -> RegisterApp {
    let mut app = db.app();
    app.test_sign_in();
    app.save_action(|ticket| {
        ticket.add_weight_line("803", "20", "0")?;
        ticket.choose_blank()
    })
    .unwrap();
    app
}
#[test]
fn native_approval_flows_into_tender_and_canonical_receipt_after_reopening() {
    let db = Db::new();
    let mut app = prepared(&db);
    let approve = action(app.active());
    app.run_price_override(approve.clone()).unwrap();
    assert_eq!(app.active().total(), Some(300));
    assert_eq!(app.active().lines[0].total_cents, Some(800));
    app.save_action(|ticket| ticket.confirm_cash("5")).unwrap();
    app.run(Act::Complete).unwrap();
    assert_eq!(
        app.active()
            .tender_evidence
            .cash
            .as_ref()
            .unwrap()
            .change_cents,
        200
    );
    let original = app.active().clone();
    let prints = app.wf.prints.clone();
    assert!(original.receipt().contains("Authorized line total $3.00"));
    assert!(!original.receipt().contains("retained reason"));
    app.run_price_override(approve).unwrap();
    assert_eq!(app.active(), &original);
    assert_eq!(app.store.price_overrides().unwrap().events.len(), 1);
    assert_eq!(app.wf.prints, prints);
    drop(app);
    let mut app = db.app();
    assert!(app.security.session.is_none());
    app.test_sign_in();
    assert_eq!(app.active(), &original);
    app.run(Act::Receipt).unwrap();
    assert_eq!(app.receipt.as_ref().unwrap().2, original.receipt());
}
#[test]
fn unsigned_price_handler_cannot_mutate_and_refresh_loads_actual_saved_revision() {
    let db = Db::new();
    let mut app = prepared(&db);
    let requested = action(app.active());
    let original = app.active().clone();
    app.lock_register();
    assert!(app.run_price_override(requested.clone()).is_err());
    assert_eq!(app.store.list().unwrap()[0], original);
    app.test_sign_in();
    let mut changed = original.clone();
    changed.source_intake.notes = "Concurrent synthetic source edit".into();
    changed.revision += 1;
    app.store
        .save_ticket_as(app.operator_session().unwrap(), &changed)
        .unwrap();
    assert!(app.run_price_override(requested).is_err());
    assert_eq!(app.active(), &original);
    app.run_price_override(PriceOverrideUiAction::Refresh)
        .unwrap();
    assert_eq!(app.active(), &changed);
    assert!(app.price_editor.open);
    assert!(app.store.price_overrides().unwrap().events.is_empty());
}
#[test]
fn authenticated_audit_renders_original_previous_new_amount_and_full_reason() {
    let db = Db::new();
    let mut app = prepared(&db);
    app.run_price_override(action(app.active())).unwrap();
    app.security.show_audit = true;
    let context = egui::Context::default();
    let mut text = String::new();
    for _ in 0..2 {
        let frame = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(820., 570.),
                )),
                ..Default::default()
            },
            |ctx| app.operator_panels(ctx),
        );
        for shape in frame.shapes {
            if let egui::epaint::Shape::Text(text_shape) = shape.shape {
                text.push_str(text_shape.galley.text());
                text.push('\n');
            }
        }
    }
    assert!(text.contains("Original tariff $8.00 / Previous $8.00 / New $3.00"));
    assert!(text.contains("Training courtesy with a retained reason"));
}

#[test]
fn price_refresh_keeps_new_ticket_numbers_unique_across_multiple_saved_tickets() {
    let db = Db::new();
    let mut app = prepared(&db);
    app.run(Act::New).unwrap();
    assert_eq!(app.active, 2);
    app.run(Act::Open(1)).unwrap();
    app.run_price_override(PriceOverrideUiAction::Refresh)
        .unwrap();
    app.run(Act::New).unwrap();
    assert_eq!(app.active, 3);
    assert_eq!(
        app.tickets
            .iter()
            .map(|ticket| ticket.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        app.tickets.len()
    );
    assert_eq!(app.store.list().unwrap().len(), 3);
}
