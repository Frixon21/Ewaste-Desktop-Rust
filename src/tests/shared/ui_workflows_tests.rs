use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

struct TestDatabase(PathBuf);
impl TestDatabase {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "ewaste-native-flow-{}-{stamp}-{sequence}.sqlite3",
            std::process::id()
        )))
    }
    fn open(&self) -> RegisterApp {
        #[cfg(feature = "extended-workflows")]
        {
            let mut app = RegisterApp::new(Store::open(&self.0).unwrap()).unwrap();
            app.test_sign_in();
            app
        }
        #[cfg(not(feature = "extended-workflows"))]
        RegisterApp::new(Store::open(&self.0).unwrap()).unwrap()
    }
}
impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}-journal", self.0.display()));
    }
}

fn add_weight(app: &mut RegisterApp, gross: &str, tare: &str) {
    app.select("803");
    app.quantity = gross.into();
    app.tare = tare.into();
    app.run(Act::Add).unwrap();
}

fn blank_load(app: &mut RegisterApp) {
    add_weight(app, "4", "0");
    app.run(Act::Workflow(WorkflowAct::Blank)).unwrap();
}

fn save_person(app: &mut RegisterApp, name: &str) -> Customer {
    app.wf.customer_edit = Some(Customer {
        name: name.into(),
        phone: "555-0100".into(),
        address: "1 Example Lane".into(),
        ..Customer::default()
    });
    app.run_workflow(WorkflowAct::SaveCustomer).unwrap();
    app.active().customer_record.clone().unwrap()
}

fn save_account(app: &mut RegisterApp, name: &str, active: bool) -> Result<(), String> {
    app.wf.account_edit = Some(BusinessAccount {
        name: name.into(),
        active,
        ..BusinessAccount::default()
    });
    app.run_workflow(WorkflowAct::SaveAccount)
}

fn cash(app: &mut RegisterApp) {
    app.tender = Tender::Cash;
    app.wf.cash = "10.00".into();
    app.run_workflow(WorkflowAct::Cash).unwrap();
}

fn render(app: &mut RegisterApp) {
    let ctx = egui::Context::default();
    for width in [820.0, 1250.0] {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 780.0),
                )),
                ..Default::default()
            },
            |ctx| {
                let mut action = None;
                app.top(ctx, &mut action);
                app.workflow_status(ctx, &mut action);
                app.catalog(ctx, app.active().state == State::Draft);
                app.checkout(ctx, &mut action);
                app.work(ctx, &mut action);
                app.dialogs(ctx, &mut action);
                app.workflow_dialogs(ctx, &mut action);
                #[cfg(feature = "extended-workflows")]
                {
                    assert!(app.catalog_editor.show(ctx, &app.wf.catalog).is_none());
                    assert!(
                        app.closeout_editor
                            .show(ctx, &app.tickets, &app.closeouts)
                            .is_none()
                    );
                }
                assert!(action.is_none());
            },
        );
        assert!(!output.shapes.is_empty());
    }
}

#[test]
fn cash_native_flow_saves_received_change_and_reopens_same_receipt() {
    let db = TestDatabase::new();
    let mut app = db.open();
    blank_load(&mut app);
    assert!(app.run(Act::Complete).is_err());
    cash(&mut app);
    app.run(Act::Complete).unwrap();
    let saved = app.active().clone();
    assert_eq!(saved.state, State::Completed);
    assert_eq!(
        saved.tender_evidence.cash.as_ref().unwrap().change_cents,
        500
    );
    assert_eq!(app.wf.prints.jobs.len(), 1);
    app.run(Act::Complete).unwrap();
    assert_eq!(app.active(), &saved);
    assert_eq!(app.wf.prints.jobs.len(), 1);
    app.show_history = true;
    for filter in [
        History::All,
        History::Draft,
        History::Held,
        History::Completed,
    ] {
        app.history = filter;
        render(&mut app);
    }
    render(&mut app);
    drop(app);
    let mut reopened = db.open();
    assert_eq!(reopened.active(), &saved);
    reopened.run(Act::Receipt).unwrap();
    assert_eq!(reopened.receipt.as_ref().unwrap().2, saved.receipt());
    assert_eq!(reopened.wf.prints.jobs.len(), 1);
}

#[test]
fn card_unknown_needs_reconciliation_and_native_completion_uses_verified_attempt() {
    let db = TestDatabase::new();
    let mut app = db.open();
    blank_load(&mut app);
    app.tender = Tender::Card;
    app.reference = "EXAMPLE-NATIVE-1".into();
    app.wf.card_amount = "0".into();
    assert!(app.run_workflow(WorkflowAct::BeginCard).is_err());
    app.wf.card_amount = "5.00".into();
    app.run_workflow(WorkflowAct::BeginCard).unwrap();
    assert!(app.run(Act::Complete).is_err());
    app.wf.outcome = CardOutcome::Unknown;
    app.run_workflow(WorkflowAct::CardResult(false)).unwrap();
    assert!(app.run_workflow(WorkflowAct::BeginCard).is_err());
    assert!(app.run_workflow(WorkflowAct::Cash).is_err());
    let before = app.active().clone();
    assert!(app.run_workflow(WorkflowAct::Blank).is_err());
    assert_eq!(app.active(), &before);
    app.wf.outcome = CardOutcome::Approved;
    assert!(app.run_workflow(WorkflowAct::CardResult(true)).is_err());
    app.wf.verified = true;
    app.run_workflow(WorkflowAct::CardResult(true)).unwrap();
    assert!(!app.wf.verified);
    render(&mut app);
    app.run(Act::Complete).unwrap();
    assert_eq!(app.active().tender_evidence.card_attempts.len(), 1);
    assert!(app.active().receipt().contains("EXAMPLE-NATIVE-1"));
    let saved = app.active().clone();
    drop(app);
    assert_eq!(db.open().active(), &saved);
}

#[test]
fn unsuccessful_card_results_allow_cash_but_never_complete_as_paid_card() {
    for outcome in [
        CardOutcome::Declined,
        CardOutcome::Cancelled,
        CardOutcome::ZipMismatch,
    ] {
        let db = TestDatabase::new();
        let mut app = db.open();
        blank_load(&mut app);
        app.tender = Tender::Card;
        app.reference = "EXAMPLE-FAILED".into();
        app.wf.card_amount = "5".into();
        app.run_workflow(WorkflowAct::BeginCard).unwrap();
        app.wf.outcome = outcome;
        app.run_workflow(WorkflowAct::CardResult(false)).unwrap();
        assert!(app.run(Act::Complete).is_err());
        assert_eq!(app.active().state, State::Draft);
        render(&mut app);
        cash(&mut app);
        app.run(Act::Complete).unwrap();
        assert_eq!(app.active().payment, Some(Payment::Cash));
        assert_eq!(
            app.active().tender_evidence.card_attempts[0].outcome,
            outcome
        );
    }
}

#[test]
fn customer_create_edit_select_preserves_material_and_completed_snapshot() {
    let db = TestDatabase::new();
    let mut app = db.open();
    add_weight(&mut app, "4", "0");
    let lines = app.active().lines.clone();
    let first = save_person(&mut app, "Example Customer");
    app.wf.customer_edit = Some(Customer {
        phone: "555-0101".into(),
        ..first.clone()
    });
    render(&mut app);
    app.run_workflow(WorkflowAct::SaveCustomer).unwrap();
    assert_eq!(app.active().lines, lines);
    assert_eq!(app.wf.directory.customers.len(), 1);
    assert_eq!(app.active().customer_record.as_ref().unwrap().id, first.id);
    cash(&mut app);
    app.run(Act::Complete).unwrap();
    let completed = app.active().clone();
    app.run(Act::New).unwrap();
    app.run_workflow(WorkflowAct::SelectCustomer(first.id))
        .unwrap();
    let current = app.active().customer_record.clone().unwrap();
    app.wf.customer_edit = Some(Customer {
        name: "Renamed Example".into(),
        ..current
    });
    app.run_workflow(WorkflowAct::SaveCustomer).unwrap();
    assert_eq!(app.wf.directory.customers.len(), 1);
    app.run(Act::Open(completed.id)).unwrap();
    assert_eq!(app.active(), &completed);
    assert_eq!(
        app.active().customer_record.as_ref().unwrap().name,
        "Example Customer"
    );
}

#[test]
fn customer_id_survives_directory_save_when_ticket_attachment_is_stale() {
    let db = TestDatabase::new();
    let mut app = db.open();
    add_weight(&mut app, "4", "0");
    let other = Store::open(&db.0).unwrap();
    let mut changed = app.active().clone();
    changed.line_intake(0, "Other correction", "2", "").unwrap();
    changed.revision += 1;
    other.save(&changed).unwrap();
    app.wf.customer_edit = Some(Customer {
        name: "Saved Once".into(),
        ..Default::default()
    });
    assert!(app.run_workflow(WorkflowAct::SaveCustomer).is_err());
    let saved_id = app.wf.customer_edit.as_ref().unwrap().id;
    assert_ne!(saved_id, 0);
    assert_eq!(app.wf.directory.customers.len(), 1);
    assert!(app.run_workflow(WorkflowAct::SaveCustomer).is_err());
    assert_eq!(app.wf.directory.customers.len(), 1);
    app.tickets = app.store.list().unwrap();
    app.run_workflow(WorkflowAct::SaveCustomer).unwrap();
    assert_eq!(app.active().customer_record.as_ref().unwrap().id, saved_id);
    assert_eq!(app.active().lines[0].classification, "Other correction");
    assert_eq!(other.directory().unwrap().customers.len(), 1);
}

#[test]
#[cfg(feature = "extended-workflows")]
fn scanner_review_failures_and_phone_matches_preserve_draft_until_selection() {
    let db = TestDatabase::new();
    let mut app = db.open();
    add_weight(&mut app, "4", "0");
    let existing = save_person(&mut app, "Different Name With Same Phone");
    let before = app.active().clone();
    for fixture in [ScanFixture::Unreadable, ScanFixture::Disconnected] {
        assert!(app.run_workflow(WorkflowAct::Scan(fixture)).is_err());
        assert_eq!(app.active(), &before);
        assert_eq!(app.wf.directory.customers.len(), 1);
    }
    app.run_workflow(WorkflowAct::Scan(ScanFixture::Success))
        .unwrap();
    assert_eq!(app.active(), &before);
    assert!(app.wf.scan_matches.contains(&existing.id));
    assert_eq!(app.wf.customer_edit.as_ref().unwrap().id, 0);
    render(&mut app);
    app.run_workflow(WorkflowAct::SelectCustomer(existing.id))
        .unwrap();
    assert!(app.wf.customer_edit.is_none());
    app.run_workflow(WorkflowAct::Scan(ScanFixture::Duplicate))
        .unwrap();
    assert!(app.wf.scan_matches.contains(&existing.id));
    assert_eq!(app.wf.directory.customers.len(), 1);
    assert_eq!(app.active().lines, before.lines);
}

#[test]
fn charge_to_requires_saved_active_selection_and_keeps_person_separate() {
    let db = TestDatabase::new();
    let mut app = db.open();
    add_weight(&mut app, "4", "0");
    let person = save_person(&mut app, "Example Delivery Person");
    app.tender = Tender::Charge;
    app.account = "Example Business typed only".into();
    assert!(app.run(Act::Complete).is_err());
    assert!(save_account(&mut app, "Example Business", false).is_err());
    let inactive = app.wf.account_edit.as_ref().unwrap().id;
    assert_ne!(inactive, 0);
    assert!(
        app.run_workflow(WorkflowAct::SelectAccount(inactive))
            .is_err()
    );
    save_account(&mut app, "Example Business East", true).unwrap();
    let wrong = app.active().account_record.clone().unwrap();
    save_account(&mut app, "Example Business West", true).unwrap();
    let correct = app.active().account_record.clone().unwrap();
    app.run_workflow(WorkflowAct::SelectAccount(wrong.id))
        .unwrap();
    assert_eq!(app.active().account_record.as_ref().unwrap().id, wrong.id);
    app.run_workflow(WorkflowAct::SelectAccount(correct.id))
        .unwrap();
    render(&mut app);
    app.run(Act::Complete).unwrap();
    assert_eq!(app.active().customer_record.as_ref().unwrap().id, person.id);
    assert_eq!(
        app.active().payment,
        Some(Payment::ChargeTo {
            account: correct.number
        })
    );
    assert!(app.active().receipt().contains("Pending billing"));
    assert!(app.active().receipt().contains("Signature:"));
    assert!(app.active().tender_evidence.cash.is_none());
    let saved = app.active().clone();
    drop(app);
    assert_eq!(db.open().active(), &saved);
}

#[test]
fn line_removal_needs_confirmation_and_weight_edits_reset_entry_state() {
    let db = TestDatabase::new();
    let mut app = db.open();
    add_weight(&mut app, "21", "1");
    assert_eq!(app.active().lines[0].quantity, "20");
    assert!(app.tare.is_empty());
    add_weight(&mut app, "4", "0");
    app.run(Act::Remove(1)).unwrap();
    assert_eq!(app.active().lines.len(), 2);
    assert_eq!(app.wf.pending_remove, Some(1));
    render(&mut app);
    app.wf.pending_remove = None;
    assert_eq!(app.active().lines.len(), 2);
    app.run(Act::Remove(1)).unwrap();
    app.run_workflow(WorkflowAct::Delete(1)).unwrap();
    assert_eq!(app.active().lines.len(), 1);
    app.run(Act::Edit(0)).unwrap();
    assert_eq!(app.edit.as_ref().unwrap().quantity, "21");
    assert_eq!(app.edit.as_ref().unwrap().tare, "1");
    app.edit.as_mut().unwrap().quantity = "5".into();
    app.edit.as_mut().unwrap().tare = "6".into();
    let original = app.active().clone();
    assert!(app.run(Act::SaveEdit(0)).is_err());
    assert_eq!(app.active(), &original);
    app.edit.as_mut().unwrap().tare = "1".into();
    render(&mut app);
    app.run(Act::SaveEdit(0)).unwrap();
    assert_eq!(app.active().lines[0].quantity, "4");
    assert_eq!(app.active().total(), Some(500));
    app.tare = "99".into();
    app.run(Act::New).unwrap();
    assert!(app.tare.is_empty());
    assert!(app.wf.pending_remove.is_none());
    assert!(app.edit.is_none());
}

#[test]
#[cfg(feature = "extended-workflows")]
fn printer_failure_after_completion_retries_same_saved_job_and_view_has_no_side_effect() {
    let db = TestDatabase::new();
    let mut app = db.open();
    blank_load(&mut app);
    cash(&mut app);
    app.wf.fail_print = true;
    assert!(app.run(Act::Complete).is_err());
    let saved = app.active().clone();
    assert_eq!(saved.state, State::Completed);
    let failed = app.wf.prints.jobs[0].clone();
    assert_eq!(failed.state, PrintState::Failed);
    let prints = app.wf.prints.clone();
    app.run(Act::Receipt).unwrap();
    render(&mut app);
    assert_eq!(app.wf.prints, prints);
    drop(app);
    let mut app = db.open();
    assert_eq!(app.active(), &saved);
    assert_eq!(app.wf.prints, prints);
    app.run_workflow(WorkflowAct::RetryPrint(failed.id))
        .unwrap();
    assert_eq!(app.wf.prints.jobs.len(), 1);
    assert_eq!(app.wf.prints.jobs[0].attempts, 2);
    assert_eq!(app.wf.prints.jobs[0].receipt, saved.receipt());
    app.run_workflow(WorkflowAct::RetryPrint(failed.id))
        .unwrap();
    app.run(Act::CustomerCopy).unwrap();
    assert_eq!(app.wf.prints.jobs.len(), 1);
    assert_eq!(app.wf.prints.jobs[0].attempts, 2);
    app.run(Act::Reprint).unwrap();
    assert_eq!(app.wf.prints.successful_internal_copies(saved.id), 1);
    assert_eq!(app.active(), &saved);
    app.run(Act::Receipt).unwrap();
    assert_eq!(app.wf.prints.jobs.len(), 2);
}

#[test]
fn customer_copy_recovers_missing_spool_without_refinalizing_or_tendering_again() {
    let db = TestDatabase::new();
    let mut app = db.open();
    blank_load(&mut app);
    cash(&mut app);
    app.save_action(|t| t.complete(Payment::Cash)).unwrap();
    let saved = app.active().clone();
    assert!(app.wf.prints.jobs.is_empty());
    drop(app);
    let mut app = db.open();
    app.run(Act::CustomerCopy).unwrap();
    app.run(Act::CustomerCopy).unwrap();
    assert_eq!(app.active(), &saved);
    assert_eq!(app.wf.prints.jobs.len(), 1);
    assert_eq!(app.wf.prints.jobs[0].ticket_id, saved.id);
    assert_eq!(app.wf.prints.jobs[0].copy, CopyKind::Customer);
    assert_eq!(app.wf.prints.jobs[0].receipt, saved.receipt());
    render(&mut app);
}

#[test]
#[cfg(feature = "extended-workflows")]
fn flat_screen_requires_classification_and_hold_resume_preserves_intake() {
    let db = TestDatabase::new();
    let mut app = db.open();
    app.select("801");
    app.run(Act::Add).unwrap();
    save_person(&mut app, "Example TV Customer");
    app.wf.cash = "5".into();
    assert!(app.run_workflow(WorkflowAct::Cash).is_err());
    app.run(Act::Edit(0)).unwrap();
    let edit = app.edit.as_mut().unwrap();
    edit.class = "Flat screen display".into();
    edit.pieces = "2".into();
    edit.note = "Internal handling note".into();
    app.run(Act::SaveEdit(0)).unwrap();
    let lines = app.active().lines.clone();
    assert_eq!(lines[0].quantity, "1");
    assert_eq!(lines[0].piece_count, Some(2));
    app.run(Act::Hold).unwrap();
    assert_eq!(app.active().state, State::Held);
    render(&mut app);
    app.run(Act::Resume).unwrap();
    assert_eq!(app.active().lines, lines);
    cash(&mut app);
    app.run(Act::Complete).unwrap();
    assert_eq!(app.active().total(), Some(500));
    assert!(!app.active().receipt().contains("Internal handling note"));
}

#[test]
#[cfg(feature = "extended-workflows")]
fn catalog_changes_preserve_draft_rule_and_completed_receipt_snapshots() {
    let db = TestDatabase::new();
    let mut app = db.open();
    blank_load(&mut app);
    let old_rule = app.active().lines[0].rule_snapshot.clone().unwrap();
    let revised = crate::catalog::PricingRule {
        name: "Revised training e-waste".into(),
        rate_cents: Some(200),
        minimum_cents: Some(0),
        classification_required: true,
        blank_allowed: false,
        ..old_rule.clone()
    };
    let next = app.wf.catalog.save_rule(&revised).unwrap();
    app.store.save_catalog(&app.wf.catalog, &next).unwrap();
    app.wf.catalog = next;
    app.catalog_editor.open = true;
    render(&mut app);
    app.catalog_editor.saved();
    assert_eq!(
        app.active().lines[0].rule_snapshot.as_ref(),
        Some(&old_rule)
    );
    app.run(Act::Edit(0)).unwrap();
    app.edit.as_mut().unwrap().quantity = "20".into();
    app.run(Act::SaveEdit(0)).unwrap();
    assert_eq!(app.active().total(), Some(800));
    cash(&mut app);
    app.run(Act::Complete).unwrap();
    let saved = app.active().clone();
    let receipt = saved.receipt();
    app.run(Act::New).unwrap();
    add_weight(&mut app, "4", "0");
    assert_eq!(app.active().total(), Some(800));
    assert_eq!(app.active().lines[0].rule_snapshot.as_ref(), Some(&revised));
    assert!(app.run_workflow(WorkflowAct::Blank).is_err());
    save_person(&mut app, "New Tariff Customer");
    app.wf.cash = "10".into();
    assert!(app.run_workflow(WorkflowAct::Cash).is_err());
    app.run(Act::Edit(0)).unwrap();
    app.edit.as_mut().unwrap().class = "Revised requirement".into();
    app.run(Act::SaveEdit(0)).unwrap();
    cash(&mut app);
    app.run(Act::Complete).unwrap();
    app.run(Act::Open(saved.id)).unwrap();
    assert_eq!(app.active().receipt(), receipt);
    assert_eq!(app.active(), &saved);
    drop(app);
    let mut app = db.open();
    assert_eq!(
        app.wf.catalog.rule_for("803", crate::domain::now()),
        Some(&revised)
    );
    app.run(Act::Open(saved.id)).unwrap();
    assert_eq!(app.active().receipt(), receipt);
}

#[test]
#[cfg(feature = "extended-workflows")]
fn settings_previews_preserve_draft_and_catalog_and_display_save_errors() {
    let db = TestDatabase::new();
    let mut app = db.open();
    blank_load(&mut app);
    let ticket = app.active().clone();
    let catalog = app.wf.catalog.clone();
    app.catalog_editor.open = true;
    render(&mut app);
    app.catalog_editor
        .error("Synthetic save failure; keep the reviewable draft".into());
    render(&mut app);
    assert!(app.catalog_editor.open);
    app.catalog_editor.saved();
    app.closeout_editor.open = true;
    render(&mut app);
    assert_eq!(app.active(), &ticket);
    assert_eq!(app.wf.catalog, catalog);
    assert_eq!(app.store.catalog().unwrap(), catalog);
    assert!(app.store.closeouts().unwrap().is_empty());
}

#[test]
#[cfg(feature = "extended-workflows")]
fn new_catalog_material_can_be_corrected_without_static_shortcut_registration() {
    let db = TestDatabase::new();
    let mut app = db.open();
    let rule = crate::catalog::PricingRule {
        id: "training-custom-v1".into(),
        code: "SYNTH".into(),
        name: "Synthetic counted e-waste".into(),
        unit: Some("each".into()),
        rate_cents: Some(250),
        minimum_cents: Some(0),
        effective_from: 0,
        effective_until: None,
        provisional: true,
        classification_required: false,
        blank_allowed: true,
    };
    let next = app.wf.catalog.save_rule(&rule).unwrap();
    app.store.save_catalog(&app.wf.catalog, &next).unwrap();
    app.wf.catalog = next;
    app.material_text = "SYNTH".into();
    app.quantity = "1".into();
    app.run(Act::Add).unwrap();
    app.run(Act::Edit(0)).unwrap();
    app.edit.as_mut().unwrap().quantity = "2".into();
    app.run(Act::SaveEdit(0)).unwrap();
    assert_eq!(app.active().total(), Some(500));
    assert_eq!(app.active().lines[0].rule_snapshot.as_ref(), Some(&rule));
    app.run_workflow(WorkflowAct::Blank).unwrap();
    cash(&mut app);
    app.run(Act::Complete).unwrap();
    assert!(app.active().receipt().contains("SYNTH"));
}
