use super::*;

fn charge_ready(app: &mut RegisterApp) {
    let customer = seed_customers(app, &["555-0555"]).remove(0);
    app.run(Act::Workflow(WorkflowAct::SelectCustomer(customer.id)))
        .unwrap();
    app.wf.account_edit = Some(crate::customers::BusinessAccount {
        number: "SCANNER-TAIL-01".into(),
        name: "Synthetic tail account".into(),
        active: true,
        ..Default::default()
    });
    app.run(Act::Workflow(WorkflowAct::SaveAccount)).unwrap();
    app.tender = Tender::Charge;
    let mut eligible = app.active().clone();
    eligible
        .complete(Payment::ChargeTo {
            account: "SCANNER-TAIL-01".into(),
        })
        .unwrap();
}

#[test]
fn automatic_clear_escape_and_focus_loss_keep_next_frame_keyboard_tail_blocked_until_acknowledged()
{
    for cause in ["oversize", "escape", "focus loss"] {
        let db = Database::new();
        let (mut app, calls) = db.app();
        let ctx = egui::Context::default();
        charge_ready(&mut app);
        let before = app.active().clone();
        let directory = app.store.directory().unwrap();
        app.run(Act::ScannerOpen).unwrap();
        frame(&mut app, &ctx, vec![egui::Event::Text(ID_MARKER.into())]);
        match cause {
            "oversize" => {
                frame(
                    &mut app,
                    &ctx,
                    vec![egui::Event::Paste("X".repeat(MAX_CAPTURE_BYTES + 1))],
                );
            }
            "escape" => {
                frame(&mut app, &ctx, vec![key(egui::Key::Escape)]);
            }
            _ => {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(820., 570.),
                        )),
                        focused: false,
                        ..Default::default()
                    },
                    |ctx| app.frame(ctx),
                );
            }
        }
        assert!(
            app.scanner.capture.is_none(),
            "Raw capture was retained after {cause}"
        );
        let text = frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::Text(ID_MARKER.into()),
                key(egui::Key::F2),
                key(egui::Key::F8),
                key(egui::Key::F12),
            ],
        );
        assert_eq!(
            app.active(),
            &before,
            "Scanner tail reached register commands after {cause}"
        );
        assert_eq!(app.store.list().unwrap(), vec![before.clone()]);
        assert_eq!(app.store.directory().unwrap(), directory);
        assert!(app.store.print_log().unwrap().jobs.is_empty());
        assert!(app.store.receipt_dispatch_log().unwrap().jobs.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(app.scanner.capture.is_none());
        assert_no_markers(&text);
        assert_keyboard_events_scrubbed(&ctx);
        app.run(Act::ScannerCancel).unwrap();
        frame(&mut app, &ctx, vec![key(egui::Key::F12)]);
        assert_eq!(
            app.active().state,
            State::Completed,
            "Explicit manual acknowledgement did not restore the register"
        );
        assert_eq!(app.store.list().unwrap().len(), 1);
    }
}

#[test]
fn ticket_change_keeps_same_and_next_frame_ime_text_out_of_counter_until_explicit_acknowledgement()
{
    let db = Database::new();
    let (mut app, calls) = db.app();
    let ctx = egui::Context::default();
    let before = app.active().clone();
    app.run(Act::New).unwrap();
    app.select("803");
    app.quantity = "7".into();
    app.tare = "0".into();
    app.run(Act::Add).unwrap();
    let next = app.active().clone();
    app.run(Act::Open(before.id)).unwrap();
    app.run(Act::ScannerOpen).unwrap();
    frame(&mut app, &ctx, vec![egui::Event::Paste(barcode())]);
    app.run(Act::Open(next.id)).unwrap();
    let tickets = app.store.list().unwrap();
    let directory = app.store.directory().unwrap();
    for _ in 0..2 {
        let text = frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit(ID_MARKER.into())),
                egui::Event::Ime(egui::ImeEvent::Commit(barcode())),
                egui::Event::Text(DOB_MARKER.into()),
                egui::Event::Paste(ID_MARKER.into()),
                key(egui::Key::Tab),
                key(egui::Key::Enter),
                key(egui::Key::F2),
                key(egui::Key::F8),
                key(egui::Key::F12),
            ],
        );
        assert!(app.scanner.capture.is_none());
        assert_no_markers(&text);
        assert_keyboard_events_scrubbed(&ctx);
        assert_eq!(app.active(), &next);
        assert_eq!(app.store.list().unwrap(), tickets);
        assert_eq!(app.store.directory().unwrap(), directory);
        assert!(app.wf.customer_edit.is_none());
        assert!(app.receipt.is_none());
        assert!(app.store.print_log().unwrap().jobs.is_empty());
        assert!(app.store.receipt_dispatch_log().unwrap().jobs.is_empty());
        assert_no_markers(&serde_json::to_string(&app.store.security_audit().unwrap()).unwrap());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    app.run(Act::ScannerCancel).unwrap();
    frame(&mut app, &ctx, vec![key(egui::Key::F2)]);
    assert_ne!(app.active().id, next.id);
    assert_eq!(app.store.list().unwrap().len(), tickets.len() + 1);
    assert_eq!(
        app.store
            .list()
            .unwrap()
            .iter()
            .find(|ticket| ticket.id == before.id),
        Some(&before)
    );
    assert_eq!(app.store.directory().unwrap(), directory);
    assert_private_storage(&app, &db);
}
