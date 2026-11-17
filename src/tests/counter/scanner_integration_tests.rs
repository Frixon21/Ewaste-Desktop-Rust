use super::*;
use crate::{
    customers::Customer,
    scanner::{CaptureSession, MAX_CAPTURE_BYTES, ScanCandidate},
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[path = "scanner_tail_tests.rs"]
mod tail;

const ID_MARKER: &str = "SYNTHETIC-SECRET-ID";
const DOB_MARKER: &str = "01011990";
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);
struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "ewaste-scanner-integration-{}-{stamp}-{}.sqlite3",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn app(&self) -> (RegisterApp, Arc<AtomicU64>) {
        let mut app = RegisterApp::new(Store::open(&self.0).unwrap()).unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let submitted = calls.clone();
        app.printer = PrinterUi::with_adapter(
            &app.store,
            Arc::new(|| Ok(vec![])),
            Arc::new(move |_, _, _| {
                submitted.fetch_add(1, Ordering::SeqCst);
                crate::receipt_dispatch::DispatchOutcome::Accepted { windows_job_id: 17 }
            }),
        )
        .unwrap();
        app.select("803");
        app.quantity = "4".into();
        app.tare = "0".into();
        app.run(Act::Add).unwrap();
        (app, calls)
    }
    fn backup(&self) -> std::path::PathBuf {
        self.0.with_extension("backup.sqlite3")
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.backup());
    }
}
fn barcode() -> String {
    let body = format!(
        "DLDAQ{ID_MARKER}\nDBB{DOB_MARKER}\nDCSSAMPLE\nDACJANE\nDDEN\nDDFN\nDDGN\nDAG100 SYNTHETIC WAY\nDAITESTVILLE\nDAJCA\nDAK000000000\nDCGUSA\r"
    );
    format!(
        "@\n\u{1e}\rANSI 636000120001DL0031{:04}{body}",
        body.chars().count()
    )
}
fn candidate() -> ScanCandidate {
    let mut capture = CaptureSession::new();
    capture.append_text(&barcode()).unwrap();
    capture.review().unwrap()
}
fn frame(app: &mut RegisterApp, ctx: &egui::Context, events: Vec<egui::Event>) -> String {
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(820., 570.),
            )),
            events,
            ..Default::default()
        },
        |ctx| app.frame(ctx),
    );
    output
        .shapes
        .into_iter()
        .filter_map(|shape| {
            if let egui::epaint::Shape::Text(text) = shape.shape {
                Some(text.galley.text().to_owned())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
fn review(app: &mut RegisterApp, ctx: &egui::Context) {
    app.run(Act::ScannerOpen).unwrap();
    frame(app, ctx, vec![egui::Event::Paste(barcode())]);
    if app.scanner.capture.is_some() {
        app.run(Act::ScannerReview).unwrap();
    }
    assert!(app.scanner.capture.is_none());
}
fn seed_customers(app: &mut RegisterApp, phones: &[&str]) -> Vec<Customer> {
    let scanned = candidate();
    let mut customers = vec![];
    for phone in phones {
        let previous = app.wf.directory.clone();
        let (next, customer) = previous
            .save_customer(&Customer {
                name: scanned.name.clone(),
                address: scanned.address.clone(),
                phone: (*phone).into(),
                ..Default::default()
            })
            .unwrap();
        app.store.save_counter_directory(&previous, &next).unwrap();
        app.wf.directory = next;
        customers.push(customer);
    }
    customers
}
fn assert_no_markers(text: &str) {
    for marker in [ID_MARKER, DOB_MARKER, "DBB01011990"] {
        assert!(
            !text.contains(marker),
            "Raw scanner identity data escaped its capture boundary"
        );
    }
}
fn assert_private_storage(app: &RegisterApp, db: &Database) {
    let state = serde_json::json!({"directory":app.store.directory().unwrap(),"tickets":app.store.list().unwrap(),"audit":app.store.security_audit().unwrap(),"preview":app.store.print_log().unwrap(),"dispatches":app.store.receipt_dispatch_log().unwrap()});
    assert_no_markers(&state.to_string());
    assert_no_markers(&format!(
        "{:?}{:?}{:?}{:?}",
        app.error, app.notice, app.wf.customer_edit, app.scanner.capture
    ));
    assert_no_markers(&String::from_utf8_lossy(&std::fs::read(&db.0).unwrap()));
    app.store.backup(&db.backup()).unwrap();
    assert_no_markers(&String::from_utf8_lossy(
        &std::fs::read(db.backup()).unwrap(),
    ));
}

fn assert_keyboard_events_scrubbed(ctx: &egui::Context) {
    let cleared = ctx.input(|input| {
        input.events.iter().chain(&input.raw.events).all(|event| {
            !matches!(
                event,
                egui::Event::Key { .. }
                    | egui::Event::Text(_)
                    | egui::Event::Paste(_)
                    | egui::Event::Ime(_)
                    | egui::Event::Copy
                    | egui::Event::Cut
            )
        })
    });
    assert!(
        cleared,
        "Scanner keyboard data remained in an input event array"
    );
}

#[test]
fn capture_modal_blocks_pointer_clicks_at_underlying_new_and_complete_positions() {
    let db = Database::new();
    let (mut app, calls) = db.app();
    let ctx = egui::Context::default();
    let customer = seed_customers(&mut app, &["555-0444"]).remove(0);
    app.run(Act::Workflow(WorkflowAct::SelectCustomer(customer.id)))
        .unwrap();
    app.wf.account_edit = Some(crate::customers::BusinessAccount {
        number: "SCANNER-POINTER-01".into(),
        name: "Synthetic pointer account".into(),
        active: true,
        ..Default::default()
    });
    app.run(Act::Workflow(WorkflowAct::SaveAccount)).unwrap();
    app.tender = Tender::Charge;
    let before = app.active().clone();
    let mut eligible = before.clone();
    eligible
        .complete(Payment::ChargeTo {
            account: "SCANNER-POINTER-01".into(),
        })
        .unwrap();
    let directory = app.store.directory().unwrap();
    let mut positions = vec![];
    for _ in 0..2 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(820., 570.),
                )),
                ..Default::default()
            },
            |ctx| app.frame(ctx),
        );
        positions = output
            .shapes
            .into_iter()
            .filter_map(|shape| {
                if let egui::epaint::Shape::Text(text) = shape.shape {
                    let label = text.galley.text();
                    if matches!(label, "F2  New ticket" | "F12  Complete") {
                        return Some((
                            label.to_owned(),
                            egui::Rect::from_min_size(text.pos, text.galley.size()).center(),
                        ));
                    }
                }
                None
            })
            .collect();
    }
    assert_eq!(
        positions.len(),
        2,
        "Normal register action positions were not rendered"
    );
    for (_, pos) in positions {
        app.run(Act::ScannerOpen).unwrap();
        frame(&mut app, &ctx, vec![egui::Event::Paste(barcode())]);
        for pressed in [true, false] {
            let text = frame(
                &mut app,
                &ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert_no_markers(&text);
        }
        assert_eq!(app.active(), &before);
        assert_eq!(app.store.list().unwrap(), vec![before.clone()]);
        assert_eq!(app.store.directory().unwrap(), directory);
        assert!(app.store.print_log().unwrap().jobs.is_empty());
        assert!(app.store.receipt_dispatch_log().unwrap().jobs.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        app.run(Act::ScannerCancel).unwrap();
        assert!(app.scanner.capture.is_none());
        assert!(app.scanner.ticket_id.is_none());
    }
    assert_private_storage(&app, &db);
}

#[test]
fn focus_loss_clears_ime_capture_and_blocks_raw_events_until_manual_acknowledgement() {
    let db = Database::new();
    let (mut app, calls) = db.app();
    let ctx = egui::Context::default();
    let before = app.active().clone();
    app.run(Act::ScannerOpen).unwrap();
    let text = frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit(ID_MARKER.into())),
            egui::Event::Ime(egui::ImeEvent::Commit(barcode())),
        ],
    );
    assert_no_markers(&text);
    assert_eq!(app.scanner.capture.as_ref().unwrap().len(), barcode().len());
    assert_no_markers(&format!("{:?}", app.scanner.capture));
    assert_keyboard_events_scrubbed(&ctx);
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(820., 570.),
            )),
            focused: false,
            events: vec![
                egui::Event::Ime(egui::ImeEvent::Preedit(DOB_MARKER.into())),
                egui::Event::Ime(egui::ImeEvent::Commit(ID_MARKER.into())),
                egui::Event::Paste(barcode()),
            ],
            ..Default::default()
        },
        |ctx| app.frame(ctx),
    );
    for shape in output.shapes {
        if let egui::epaint::Shape::Text(text) = shape.shape {
            assert_no_markers(text.galley.text());
        }
    }
    assert!(app.scanner.capture.is_none());
    assert!(app.scanner.ticket_id.is_none());
    assert_keyboard_events_scrubbed(&ctx);
    assert_eq!(app.active(), &before);
    assert_eq!(app.store.list().unwrap(), vec![before.clone()]);
    for _ in 0..2 {
        let text = frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit(ID_MARKER.into())),
                egui::Event::Ime(egui::ImeEvent::Commit(ID_MARKER.into())),
                egui::Event::Text(DOB_MARKER.into()),
                egui::Event::Paste(barcode()),
                key(egui::Key::Tab),
                key(egui::Key::Enter),
                key(egui::Key::F2),
                key(egui::Key::F8),
                key(egui::Key::F12),
            ],
        );
        assert_no_markers(&text);
        assert_keyboard_events_scrubbed(&ctx);
        assert!(app.scanner.capture.is_none());
        assert_eq!(app.active(), &before);
        assert_eq!(app.store.list().unwrap(), vec![before.clone()]);
        assert_no_markers(&serde_json::to_string(&app.store.security_audit().unwrap()).unwrap());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    app.run(Act::ScannerCancel).unwrap();
    app.run(Act::ScannerOpen).unwrap();
    frame(&mut app, &ctx, vec![egui::Event::Paste(barcode())]);
    app.run(Act::ScannerReview).unwrap();
    assert_eq!(
        app.wf.customer_edit.as_ref().unwrap().name,
        candidate().name
    );
    assert_eq!(app.active(), &before);
    assert_private_storage(&app, &db);
}

#[test]
fn armed_capture_consumes_shortcuts_and_control_keys_without_changing_the_load() {
    let db = Database::new();
    let (mut app, calls) = db.app();
    let ctx = egui::Context::default();
    app.select("803");
    app.quantity = "9".into();
    app.tare = "1".into();
    app.wf.cash = "5".into();
    let before = app.active().clone();
    let directory = app.store.directory().unwrap();
    let count = app.store.list().unwrap().len();
    app.run(Act::ScannerOpen).unwrap();
    let events = [
        egui::Key::F2,
        egui::Key::F8,
        egui::Key::F12,
        egui::Key::F3,
        egui::Key::Tab,
        egui::Key::Delete,
        egui::Key::Enter,
        egui::Key::F2,
    ]
    .into_iter()
    .map(key)
    .collect();
    frame(&mut app, &ctx, events);
    assert_eq!(app.active(), &before);
    assert_eq!(app.store.list().unwrap().len(), count);
    assert_eq!(app.store.directory().unwrap(), directory);
    assert_eq!(app.material, Some("803"));
    assert_eq!(app.quantity, "9");
    assert_eq!(app.tare, "1");
    assert_eq!(app.wf.cash, "5");
    assert!(!app.show_history);
    assert!(app.receipt.is_none());
    assert!(app.wf.pending_remove.is_none());
    assert!(app.edit.is_none());
    assert!(app.store.print_log().unwrap().jobs.is_empty());
    assert!(app.store.receipt_dispatch_log().unwrap().jobs.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn unique_scan_requires_explicit_selection_and_keeps_saved_phone() {
    let db = Database::new();
    let (mut app, _) = db.app();
    let ctx = egui::Context::default();
    let existing = seed_customers(&mut app, &["555-0123"]).remove(0);
    let before = app.active().clone();
    let directory = app.store.directory().unwrap();
    let audit = app.store.security_audit().unwrap();
    review(&mut app, &ctx);
    assert_eq!(app.active(), &before);
    assert_eq!(app.store.directory().unwrap(), directory);
    assert_eq!(app.store.security_audit().unwrap(), audit);
    assert_eq!(app.wf.scan_matches, vec![existing.id]);
    let suggested = app.wf.customer_edit.as_ref().unwrap();
    assert_eq!(suggested.id, 0);
    assert!(suggested.number.is_empty());
    assert!(suggested.phone.is_empty());
    assert!(suggested.notice.is_empty());
    assert_eq!(suggested.name, candidate().name);
    assert_eq!(suggested.address, candidate().address);
    app.run(Act::Workflow(WorkflowAct::SelectCustomer(existing.id)))
        .unwrap();
    assert_eq!(app.active().customer_record.as_ref(), Some(&existing));
    assert_eq!(app.active().lines, before.lines);
    assert_eq!(app.store.directory().unwrap(), directory);
    assert_private_storage(&app, &db);
}

#[test]
fn duplicate_review_never_merges_phones_and_new_customer_starts_without_phone() {
    let db = Database::new();
    let (mut app, _) = db.app();
    let ctx = egui::Context::default();
    let existing = seed_customers(&mut app, &["555-0111", "555-0222"]);
    let before = app.active().clone();
    let directory = app.store.directory().unwrap();
    review(&mut app, &ctx);
    assert_eq!(
        app.wf.scan_matches,
        existing.iter().map(|c| c.id).collect::<Vec<_>>()
    );
    assert_eq!(app.active(), &before);
    assert_eq!(app.store.directory().unwrap(), directory);
    app.run(Act::Workflow(WorkflowAct::SelectCustomer(existing[1].id)))
        .unwrap();
    assert_eq!(
        app.active().customer_record.as_ref().unwrap().phone,
        "555-0222"
    );
    assert_eq!(app.store.directory().unwrap(), directory);
    app.run(Act::New).unwrap();
    app.select("803");
    app.quantity = "4".into();
    app.run(Act::Add).unwrap();
    review(&mut app, &ctx);
    assert!(app.wf.customer_edit.as_ref().unwrap().phone.is_empty());
    app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
    let saved = app.active().customer_record.as_ref().unwrap();
    assert!(saved.phone.is_empty());
    assert!(!existing.iter().any(|customer| customer.id == saved.id));
    assert_eq!(
        &app.store.directory().unwrap().customers[..2],
        existing.as_slice()
    );
    assert_private_storage(&app, &db);
}

#[test]
fn capture_debug_and_review_discard_raw_identity_fields_without_database_writes() {
    let db = Database::new();
    let (mut app, _) = db.app();
    let ctx = egui::Context::default();
    let before = app.active().clone();
    let directory = app.store.directory().unwrap();
    app.run(Act::ScannerOpen).unwrap();
    let text = frame(&mut app, &ctx, vec![egui::Event::Paste(barcode())]);
    assert_no_markers(&text);
    assert_no_markers(&format!("{:?}", app.scanner.capture));
    if app.scanner.capture.is_some() {
        app.run(Act::ScannerReview).unwrap();
    }
    assert_eq!(app.active(), &before);
    assert_eq!(app.store.directory().unwrap(), directory);
    assert_private_storage(&app, &db);
}

#[test]
fn keyboard_wedge_text_and_enter_events_across_frames_reach_explicit_safe_review() {
    let db = Database::new();
    let (mut app, calls) = db.app();
    let ctx = egui::Context::default();
    let before = app.active().clone();
    let directory = app.store.directory().unwrap();
    let audit = app.store.security_audit().unwrap();
    app.run(Act::ScannerOpen).unwrap();
    let events = barcode()
        .chars()
        .filter_map(|character| match character {
            '\u{1e}' => None,
            '\r' | '\n' => Some(key(egui::Key::Enter)),
            character => Some(egui::Event::Text(character.to_string())),
        })
        .collect::<Vec<_>>();
    for batch in events.chunks(17) {
        let text = frame(&mut app, &ctx, batch.to_vec());
        assert_no_markers(&text);
        assert!(app.scanner.capture.is_some());
        assert_eq!(app.active(), &before);
        assert_eq!(app.store.directory().unwrap(), directory);
    }
    assert!(app.wf.customer_edit.is_none());
    assert_eq!(app.store.security_audit().unwrap(), audit);
    app.run(Act::ScannerReview).unwrap();
    assert!(app.scanner.capture.is_none());
    let reviewed = app.wf.customer_edit.as_ref().unwrap();
    let expected = candidate();
    assert_eq!(reviewed.name, expected.name);
    assert_eq!(reviewed.address, expected.address);
    assert!(reviewed.phone.is_empty());
    assert_eq!(reviewed.id, 0);
    assert_eq!(app.active(), &before);
    assert_eq!(app.store.directory().unwrap(), directory);
    assert_eq!(app.store.security_audit().unwrap(), audit);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_private_storage(&app, &db);
}

#[test]
fn malformed_oversized_cancel_and_ticket_change_clear_capture_with_manual_fallback() {
    let db = Database::new();
    let (mut app, _) = db.app();
    let ctx = egui::Context::default();
    let before = app.active().clone();
    for raw in [
        format!("invalid {ID_MARKER} {DOB_MARKER}"),
        "X".repeat(MAX_CAPTURE_BYTES + 1),
    ] {
        app.run(Act::ScannerOpen).unwrap();
        frame(&mut app, &ctx, vec![egui::Event::Paste(raw)]);
        let mut feedback = format!("{:?}{:?}", app.error, app.scanner.message);
        if app.scanner.capture.is_some() {
            let error = app.run(Act::ScannerReview).unwrap_err();
            assert_no_markers(&error);
            feedback.push_str(&error);
        }
        assert!(app.scanner.capture.is_none());
        assert_eq!(app.active(), &before);
        assert_no_markers(&feedback);
        assert!(feedback.to_ascii_lowercase().contains("manual"));
        app.run(Act::ScannerCancel).unwrap();
    }
    app.run(Act::ScannerOpen).unwrap();
    frame(&mut app, &ctx, vec![egui::Event::Text(ID_MARKER.into())]);
    app.run(Act::ScannerClear).unwrap();
    assert!(app.scanner.capture.as_ref().unwrap().is_empty());
    frame(&mut app, &ctx, vec![egui::Event::Text(ID_MARKER.into())]);
    app.run(Act::ScannerCancel).unwrap();
    assert!(app.scanner.capture.is_none());
    assert_eq!(app.active(), &before);
    assert_eq!(app.active(), &before);
    app.run(Act::New).unwrap();
    let next = app.active().id;
    app.run(Act::Open(before.id)).unwrap();
    app.run(Act::ScannerOpen).unwrap();
    frame(&mut app, &ctx, vec![egui::Event::Text(ID_MARKER.into())]);
    app.run(Act::Open(next)).unwrap();
    assert!(app.scanner.capture.is_none());
    assert_eq!(
        app.store
            .list()
            .unwrap()
            .iter()
            .find(|ticket| ticket.id == before.id),
        Some(&before)
    );
    app.run(Act::ScannerCancel).unwrap();
    app.wf.customer_edit = Some(Customer {
        name: "Synthetic manual fallback".into(),
        phone: "555-0333".into(),
        ..Default::default()
    });
    app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
    assert_eq!(
        app.active().customer_record.as_ref().unwrap().phone,
        "555-0333"
    );
    assert_private_storage(&app, &db);
}
