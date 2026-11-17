use super::*;
use crate::{
    printing::{CopyKind, PrintState},
    receipt_dispatch::{DispatchOutcome, DispatchState, PrepareDispatch},
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const QUEUE_A: &str = "Synthetic receipt queue A";
const QUEUE_B: &str = "Synthetic receipt queue B";
type FakeSubmit = Arc<dyn Fn(&str, &str, &str) -> DispatchOutcome + Send + Sync>;
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);
struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "ewaste-physical-integration-{}-{stamp}-{}.sqlite3",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn app(&self, fake: &FakePrinter) -> RegisterApp {
        let mut app = RegisterApp::new(Store::open(&self.0).unwrap()).unwrap();
        install_fake(&mut app, fake);
        app
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct SubmittedCopy {
    queue: String,
    title: String,
    body: String,
}
#[derive(Clone, Default)]
struct FakePrinter {
    calls: Arc<Mutex<Vec<SubmittedCopy>>>,
    outcomes: Arc<Mutex<VecDeque<DispatchOutcome>>>,
}
impl FakePrinter {
    fn new(outcomes: Vec<DispatchOutcome>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into())),
            ..Default::default()
        }
    }
    fn submit(&self) -> FakeSubmit {
        let fake = self.clone();
        Arc::new(move |queue, title, body| {
            let mut calls = fake.calls.lock().unwrap();
            calls.push(SubmittedCopy {
                queue: queue.into(),
                title: title.into(),
                body: body.into(),
            });
            let id = calls.len() as u32;
            drop(calls);
            fake.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(DispatchOutcome::Accepted { windows_job_id: id })
        })
    }
    fn calls(&self) -> Vec<SubmittedCopy> {
        self.calls.lock().unwrap().clone()
    }
}
fn install_fake(app: &mut RegisterApp, fake: &FakePrinter) {
    app.printer = PrinterUi::with_adapter(
        &app.store,
        Arc::new(|| Ok(vec![QUEUE_A.into(), QUEUE_B.into()])),
        fake.submit(),
    )
    .unwrap();
}
fn configure(app: &mut RegisterApp, fake: &FakePrinter, queue: &str) {
    let previous = app.store.receipt_printer_queue().unwrap();
    app.store
        .save_receipt_printer_queue(previous.as_deref(), Some(queue))
        .unwrap();
    install_fake(app, fake);
}
fn cash_load(app: &mut RegisterApp) {
    app.select("803");
    app.quantity = "4".into();
    app.tare = "0".into();
    app.run(Act::Add).unwrap();
    app.run(Act::Workflow(WorkflowAct::Blank)).unwrap();
    app.wf.cash = "5".into();
    app.run(Act::Workflow(WorkflowAct::Cash)).unwrap();
}
fn settle(app: &mut RegisterApp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = app.poll_printer_results();
        let ledger = app.store.receipt_dispatch_log().unwrap();
        if !ledger.jobs.is_empty()
            && ledger
                .jobs
                .iter()
                .all(|job| !matches!(job.state(), DispatchState::Dispatching))
        {
            return;
        }
        result.unwrap();
        assert!(
            Instant::now() < deadline,
            "Fake printer result never became durable"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn screen_text(
    app: &mut RegisterApp,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> Vec<(String, egui::Rect)> {
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
                let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                if shape.clip_rect.contains_rect(rect) {
                    return Some((text.galley.text().into(), rect));
                }
            }
            None
        })
        .collect()
}

#[test]
fn printer_settings_discover_lazily_and_require_explicit_choice_without_test_print() {
    let db = Database::new();
    let fake = FakePrinter::default();
    let mut app = db.app(&fake);
    let discovered = Arc::new(AtomicU64::new(0));
    let observed = discovered.clone();
    app.printer = PrinterUi::with_adapter(
        &app.store,
        Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(vec![QUEUE_A.into(), QUEUE_B.into()])
        }),
        fake.submit(),
    )
    .unwrap();
    let ctx = egui::Context::default();
    screen_text(&mut app, &ctx, vec![]);
    assert_eq!(discovered.load(Ordering::SeqCst), 0);
    assert_eq!(app.store.receipt_printer_queue().unwrap(), None);
    app.run(Act::PrinterSettings).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let labels = loop {
        let labels = screen_text(&mut app, &ctx, vec![]);
        if !app.printer.discovering && labels.iter().any(|(label, _)| label == QUEUE_A) {
            break labels;
        }
        assert!(Instant::now() < deadline, "Fake discovery never finished");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(discovered.load(Ordering::SeqCst), 1);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(820., 570.));
    for expected in ["Save printer choice", "Cancel"] {
        let (_, rect) = labels.iter().find(|(label, _)| label == expected).unwrap();
        assert!(screen.contains_rect(*rect), "{expected} is outside 820x570");
    }
    assert_eq!(app.printer.queue, None);
    assert_eq!(app.printer.choice, None);
    assert_eq!(app.store.receipt_printer_queue().unwrap(), None);
    app.run(Act::PrinterSaveChoice).unwrap();
    assert!(!app.printer.open);
    assert_eq!(app.store.receipt_printer_queue().unwrap(), None);
    app.run(Act::PrinterSettings).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.printer.discovering {
        app.poll_printer_results().unwrap();
        assert!(Instant::now() < deadline, "Fake rediscovery never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.printer.choice, None);
    app.printer.choice = Some(QUEUE_B.into());
    assert_eq!(app.store.receipt_printer_queue().unwrap(), None);
    app.run(Act::PrinterSaveChoice).unwrap();
    assert!(!app.printer.open);
    assert_eq!(
        app.store.receipt_printer_queue().unwrap().as_deref(),
        Some(QUEUE_B)
    );
    assert!(fake.calls().is_empty());
    assert!(app.store.receipt_dispatch_log().unwrap().jobs.is_empty());
    drop(app);
    let reopened = db.app(&fake);
    assert_eq!(reopened.printer.queue.as_deref(), Some(QUEUE_B));
    assert!(fake.calls().is_empty());
}

#[test]
fn preview_is_not_dispatch_and_first_paper_customer_copy_survives_old_preview_success() {
    let db = Database::new();
    let fake = FakePrinter::default();
    let mut app = db.app(&fake);
    assert_eq!(app.store.receipt_printer_queue().unwrap(), None);
    assert_eq!(app.printer.queue, None);
    cash_load(&mut app);
    app.run(Act::Complete).unwrap();
    let paid = app.active().clone();
    assert_eq!(app.wf.prints.jobs.len(), 1);
    assert_eq!(app.wf.prints.jobs[0].state, PrintState::Succeeded);
    assert!(app.store.receipt_dispatch_log().unwrap().jobs.is_empty());
    app.run(Act::Receipt).unwrap();
    app.run(Act::Receipt).unwrap();
    assert!(fake.calls().is_empty());
    assert!(app.store.receipt_dispatch_log().unwrap().jobs.is_empty());
    drop(app);
    let mut app = db.app(&fake);
    assert_eq!(app.store.receipt_printer_queue().unwrap(), None);
    configure(&mut app, &fake, QUEUE_A);
    app.run(Act::CustomerCopy).unwrap();
    app.run(Act::Receipt).unwrap();
    settle(&mut app);
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(app.store.list().unwrap()[0], paid);
    drop(app);
    let mut app = db.app(&fake);
    let ledger = app.store.receipt_dispatch_log().unwrap();
    assert_eq!(ledger.jobs.len(), 1);
    let customer = &ledger.jobs[0];
    assert_eq!(customer.copy, CopyKind::Customer);
    assert_eq!(customer.queue, QUEUE_A);
    assert!(customer.receipt.starts_with("CUSTOMER RECEIPT"));
    assert!(customer.receipt.contains(&paid.receipt()));
    assert_eq!(fake.calls()[0].body, customer.receipt);
    assert_eq!(fake.calls()[0].queue, QUEUE_A);
    let before = ledger.clone();
    app.run(Act::CustomerCopy).unwrap();
    app.poll_printer_results().unwrap();
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(app.store.receipt_dispatch_log().unwrap(), before);
    app.run(Act::Receipt).unwrap();
    assert_eq!(fake.calls().len(), 1);
    configure(&mut app, &fake, QUEUE_B);
    app.run(Act::Reprint).unwrap();
    settle(&mut app);
    let copies = app.store.receipt_dispatch_log().unwrap();
    assert_eq!(copies.jobs.len(), 2);
    assert_eq!(copies.jobs[0], before.jobs[0]);
    assert_eq!(copies.jobs[1].copy, CopyKind::Internal);
    assert_eq!(copies.jobs[1].queue, QUEUE_B);
    assert!(copies.jobs[1].receipt.starts_with("INTERNAL COPY"));
    assert_eq!(fake.calls()[1].body, copies.jobs[1].receipt);
    assert_eq!(app.active(), &paid);
    assert_eq!(app.store.list().unwrap().len(), 1);
}

#[test]
fn completion_stays_paid_after_not_submitted_and_only_explicit_retry_sends_again() {
    let db = Database::new();
    let fake = FakePrinter::new(vec![
        DispatchOutcome::NotSubmitted {
            detail: "Synthetic failure before spool submission".into(),
        },
        DispatchOutcome::Accepted { windows_job_id: 42 },
    ]);
    let mut app = db.app(&fake);
    configure(&mut app, &fake, QUEUE_A);
    cash_load(&mut app);
    app.run(Act::Complete).unwrap();
    let paid = app.active().clone();
    settle(&mut app);
    assert_eq!(paid.state, State::Completed);
    assert_eq!(paid.total(), Some(500));
    assert_eq!(app.active(), &paid);
    assert_eq!(app.store.list().unwrap().len(), 1);
    let failed = app.store.receipt_dispatch_log().unwrap();
    assert_eq!(failed.jobs.len(), 1);
    let job = &failed.jobs[0];
    assert!(matches!(job.state(), DispatchState::NotSubmitted { .. }));
    assert_eq!(fake.calls().len(), 1);
    app.run(Act::CustomerCopy).unwrap();
    app.poll_printer_results().unwrap();
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(app.store.receipt_dispatch_log().unwrap(), failed);
    app.retry_physical_receipt(job.id).unwrap();
    settle(&mut app);
    let retried = app.store.receipt_dispatch_log().unwrap();
    assert_eq!(retried.jobs.len(), 1);
    assert_eq!(retried.jobs[0].attempts.len(), 2);
    assert_eq!(retried.jobs[0].receipt, job.receipt);
    assert_eq!(retried.jobs[0].queue, job.queue);
    assert!(matches!(
        retried.jobs[0].state(),
        DispatchState::Accepted { windows_job_id: 42 }
    ));
    assert_eq!(fake.calls().len(), 2);
    assert_eq!(app.active(), &paid);
    assert_eq!(app.store.list().unwrap().len(), 1);
}

#[test]
fn accepted_by_fake_but_unrecorded_crash_reopens_dispatching_without_resubmission() {
    let db = Database::new();
    let fake = FakePrinter::default();
    let mut app = db.app(&fake);
    cash_load(&mut app);
    app.run(Act::Complete).unwrap();
    let paid = app.active().clone();
    configure(&mut app, &fake, QUEUE_A);
    let prepared = app
        .store
        .prepare_receipt_dispatch(
            paid.id,
            CopyKind::Customer,
            &format!("physical-customer:{}", paid.id),
            QUEUE_A,
        )
        .unwrap();
    let PrepareDispatch::Submit(capability) = prepared else {
        panic!("First physical request must reserve a new attempt");
    };
    let outcome = fake.submit()(
        capability.queue(),
        "Synthetic crash boundary",
        capability.receipt(),
    );
    assert!(matches!(outcome, DispatchOutcome::Accepted { .. }));
    let pending = app.store.receipt_dispatch_log().unwrap();
    assert!(matches!(
        pending.jobs[0].state(),
        DispatchState::Dispatching
    ));
    drop(capability);
    drop(app);
    let mut reopened = db.app(&fake);
    assert_eq!(reopened.store.receipt_dispatch_log().unwrap(), pending);
    reopened.poll_printer_results().unwrap();
    reopened.run(Act::Receipt).unwrap();
    reopened.run(Act::CustomerCopy).unwrap();
    assert!(reopened.retry_physical_receipt(pending.jobs[0].id).is_err());
    assert!(
        reopened
            .store
            .prepare_receipt_dispatch(paid.id, CopyKind::Customer, "new-key-after-crash", QUEUE_A)
            .is_err()
    );
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(reopened.store.receipt_dispatch_log().unwrap(), pending);
    assert_eq!(reopened.active(), &paid);
}

#[test]
fn unknown_submission_blocks_same_copy_with_new_keys_and_never_undoes_the_sale() {
    let db = Database::new();
    let fake = FakePrinter::new(vec![DispatchOutcome::Unknown {
        detail: "Synthetic spool outcome unavailable".into(),
    }]);
    let mut app = db.app(&fake);
    configure(&mut app, &fake, QUEUE_A);
    cash_load(&mut app);
    app.run(Act::Complete).unwrap();
    let paid = app.active().clone();
    settle(&mut app);
    let unknown = app.store.receipt_dispatch_log().unwrap();
    assert!(matches!(
        unknown.jobs[0].state(),
        DispatchState::Unknown { .. }
    ));
    app.run(Act::CustomerCopy).unwrap();
    assert!(app.retry_physical_receipt(unknown.jobs[0].id).is_err());
    assert!(
        app.store
            .prepare_receipt_dispatch(
                paid.id,
                CopyKind::Customer,
                "another-key-after-unknown",
                QUEUE_A
            )
            .is_err()
    );
    app.poll_printer_results().unwrap();
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(app.store.receipt_dispatch_log().unwrap(), unknown);
    assert_eq!(app.active(), &paid);
    assert_eq!(app.store.list().unwrap().len(), 1);
}

#[test]
fn failed_result_persistence_keeps_capability_and_save_again_never_resubmits() {
    let db = Database::new();
    let fake = FakePrinter::new(vec![DispatchOutcome::Accepted { windows_job_id: 91 }]);
    let mut app = db.app(&fake);
    configure(&mut app, &fake, QUEUE_A);
    cash_load(&mut app);
    app.run(Act::Complete).unwrap();
    let paid = app.active().clone();
    let pending = app.store.receipt_dispatch_log().unwrap();
    assert!(matches!(
        pending.jobs[0].state(),
        DispatchState::Dispatching
    ));
    app.store.test_receipt_result_audit_failure(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let error = loop {
        if let Err(error) = app.poll_printer_results() {
            break error;
        }
        assert!(
            Instant::now() < deadline,
            "Fake printer outcome never arrived"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(error.contains("Save the result again"));
    assert_eq!(fake.calls().len(), 1);
    assert!(app.run(Act::PrinterRecover(pending.jobs[0].id)).is_err());
    assert_eq!(app.store.receipt_dispatch_log().unwrap(), pending);
    assert_eq!(app.active(), &paid);
    let ctx = egui::Context::default();
    let labels = screen_text(&mut app, &ctx, vec![]);
    assert!(
        labels
            .iter()
            .any(|(label, _)| label == "Save printer result again")
    );
    assert!(app.run(Act::PrinterSaveResults).is_err());
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(app.store.receipt_dispatch_log().unwrap(), pending);
    app.store.test_receipt_result_audit_failure(false).unwrap();
    app.run(Act::PrinterSaveResults).unwrap();
    let saved = app.store.receipt_dispatch_log().unwrap();
    assert_eq!(saved.jobs.len(), 1);
    assert_eq!(saved.jobs[0].attempts.len(), 1);
    assert!(matches!(
        saved.jobs[0].state(),
        DispatchState::Accepted { windows_job_id: 91 }
    ));
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(app.active(), &paid);
    assert_eq!(app.store.list().unwrap().len(), 1);
    drop(app);
    let mut reopened = db.app(&fake);
    assert_eq!(reopened.store.receipt_dispatch_log().unwrap(), saved);
    reopened.run(Act::CustomerCopy).unwrap();
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(reopened.active(), &paid);
}

#[test]
fn checked_unknown_copy_uses_explicit_new_queue_once_and_preserves_the_sale() {
    let db = Database::new();
    let fake = FakePrinter::new(vec![
        DispatchOutcome::Unknown {
            detail: "Synthetic unknown spool outcome".into(),
        },
        DispatchOutcome::Accepted { windows_job_id: 92 },
    ]);
    let mut app = db.app(&fake);
    configure(&mut app, &fake, QUEUE_A);
    cash_load(&mut app);
    app.run(Act::Complete).unwrap();
    let dispatching_id = app.store.receipt_dispatch_log().unwrap().jobs[0].id;
    assert!(app.run(Act::PrinterRecover(dispatching_id)).is_err());
    settle(&mut app);
    let paid = app.active().clone();
    let original = app.store.receipt_dispatch_log().unwrap().jobs[0].clone();
    assert!(matches!(original.state(), DispatchState::Unknown { .. }));
    configure(&mut app, &fake, QUEUE_B);
    app.run(Act::PrinterRecover(original.id)).unwrap();
    let review = app.printer.recovery.as_ref().unwrap();
    assert_eq!(review.prior_job_id, original.id);
    assert_eq!(
        review.prior_attempt_id,
        original.attempts.last().unwrap().id
    );
    assert_eq!(review.queue, QUEUE_B);
    assert!(!review.checked);
    assert!(app.run(Act::PrinterConfirmRecovery).is_err());
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(
        app.store.receipt_dispatch_log().unwrap().jobs,
        vec![original.clone()]
    );
    app.printer.recovery.as_mut().unwrap().checked = true;
    let before_audit = app.store.security_audit().unwrap();
    app.run(Act::PrinterConfirmRecovery).unwrap();
    assert!(app.run(Act::PrinterConfirmRecovery).is_err());
    settle(&mut app);
    let saved = app.store.receipt_dispatch_log().unwrap();
    assert_eq!(saved.jobs.len(), 2);
    assert_eq!(saved.jobs[0], original);
    let successor = &saved.jobs[1];
    assert_eq!(successor.ticket_id, paid.id);
    assert_eq!(successor.copy, original.copy);
    assert_eq!(successor.receipt, original.receipt);
    assert_eq!(successor.queue, QUEUE_B);
    let link = successor
        .checked_copy
        .as_ref()
        .expect("Checked copy lost its prior request link");
    assert_eq!(link.prior_job_id, original.id);
    assert_eq!(link.prior_attempt_id, original.attempts.last().unwrap().id);
    assert!(link.queue_and_paper_checked);
    assert_eq!(link.checked_by, successor.actor);
    assert_eq!(link.checked_at, successor.attempts[0].requested_at);
    assert!(matches!(
        successor.state(),
        DispatchState::Accepted { windows_job_id: 92 }
    ));
    assert_eq!(fake.calls().len(), 2);
    assert_eq!(fake.calls()[1].queue, QUEUE_B);
    assert_eq!(fake.calls()[1].body, original.receipt);
    assert!(app.store.security_audit().unwrap().events.len() > before_audit.events.len());
    let duplicate = app
        .store
        .prepare_checked_receipt_copy(
            original.id,
            original.attempts.last().unwrap().id,
            &successor.request_key,
            QUEUE_B,
            true,
        )
        .unwrap();
    assert!(matches!(duplicate, PrepareDispatch::Existing(job) if job.id == successor.id));
    assert!(app.run(Act::PrinterRecover(original.id)).is_err());
    assert_eq!(fake.calls().len(), 2);
    assert_eq!(app.active(), &paid);
    assert_eq!(app.store.list().unwrap().len(), 1);
    drop(app);
    let mut reopened = db.app(&fake);
    reopened.poll_printer_results().unwrap();
    assert_eq!(reopened.store.receipt_dispatch_log().unwrap(), saved);
    assert!(reopened.run(Act::PrinterRecover(original.id)).is_err());
    assert_eq!(fake.calls().len(), 2);
    assert_eq!(reopened.active(), &paid);
}
