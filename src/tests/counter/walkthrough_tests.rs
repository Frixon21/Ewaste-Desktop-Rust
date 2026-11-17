use super::*;
use crate::{
    customers::{BusinessAccount, Customer},
    intake::{IntakeReason, SourceCategory},
    tender::CardOutcome,
};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);
struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "ewaste-walkthrough-{}-{stamp}-{}.sqlite3",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn app(&self) -> RegisterApp {
        RegisterApp::new(Store::open(&self.0).unwrap()).unwrap()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn person(app: &mut RegisterApp) -> Customer {
    app.wf.customer_edit = Some(Customer {
        name: "Synthetic TV customer".into(),
        phone: "555-0100".into(),
        address: "One synthetic customer street".into(),
        ..Default::default()
    });
    app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
    app.active().customer_record.clone().unwrap()
}
fn load_803(app: &mut RegisterApp) {
    app.select("803");
    app.quantity = "4".into();
    app.tare = "0".into();
    app.run(Act::Add).unwrap();
    app.run(Act::Workflow(WorkflowAct::Blank)).unwrap();
}
fn rendered(app: &mut RegisterApp) -> String {
    let ctx = egui::Context::default();
    let mut text = String::new();
    for _ in 0..2 {
        let frame = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(820., 570.),
                )),
                ..Default::default()
            },
            |ctx| app.frame(ctx),
        );
        for shape in frame.shapes {
            if let egui::epaint::Shape::Text(shape) = shape.shape {
                text.push_str(shape.galley.text());
                text.push('\n');
            }
        }
    }
    text
}
fn assert_basic_surface(text: &str) {
    let lower = text.to_ascii_lowercase();
    for excluded in [
        "inventory custody",
        "training tariffs",
        "intake review queue",
        "supervisor line price approval",
        "refund / void",
        "shift closeout",
        "operators",
        "audit history",
        "reporting",
        "claim eligibility",
        "documentary",
        "simulate failure",
        "scan fixture",
        "synthetic scan",
    ] {
        assert!(
            !lower.contains(excluded),
            "Basic counter exposed an extended control or fixture: {excluded}"
        );
    }
}

#[test]
fn fresh_counter_opens_without_login_and_saves_and_reopens_803() {
    let db = Database::new();
    let mut app = RegisterApp::new(Store::open(&db.0).unwrap()).unwrap();
    let ctx = egui::Context::default();
    let first_frame = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(820., 570.),
            )),
            ..Default::default()
        },
        |ctx| app.frame(ctx),
    );
    let text = first_frame
        .shapes
        .iter()
        .filter_map(|shape| {
            if let egui::epaint::Shape::Text(text) = &shape.shape {
                Some(text.galley.text())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    for required in ["Materials", "F2  New ticket", "F3  History"] {
        assert!(
            text.contains(required),
            "Fresh counter did not immediately show {required}: {text}"
        );
    }
    let lower = text.to_ascii_lowercase();
    for forbidden in [
        "set up this register",
        "sign in",
        "sign-in",
        "password",
        "lock / sign out",
    ] {
        assert!(
            !lower.contains(forbidden),
            "Fresh counter exposed a login control: {forbidden}"
        );
    }
    load_803(&mut app);
    app.wf.cash = "5".into();
    app.run(Act::Workflow(WorkflowAct::Cash)).unwrap();
    app.run(Act::Complete).unwrap();
    let paid = app.active().clone();
    assert_eq!(paid.state, State::Completed);
    assert_eq!(paid.total(), Some(500));
    assert_eq!(app.store.list().unwrap(), vec![paid.clone()]);
    drop(app);
    let mut reopened = RegisterApp::new(Store::open(&db.0).unwrap()).unwrap();
    assert!(rendered(&mut reopened).contains("Materials"));
    assert_eq!(reopened.active(), &paid);
    reopened.run(Act::Receipt).unwrap();
    assert_eq!(reopened.receipt.as_ref().unwrap().2, paid.receipt());
}

#[test]
fn tv_add_opens_prefilled_source_category_and_free_text_tv_cannot_replace_selection() {
    let db = Database::new();
    let mut app = db.app();
    let mut customer = person(&mut app);
    customer.phone = "555-0199".into();
    app.wf.customer_edit = Some(customer.clone());
    app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
    app.select("801");
    app.run(Act::Add).unwrap();
    let details = app
        .details
        .as_ref()
        .expect("TV entry must open the source form");
    assert_eq!(details.index, 0);
    assert_eq!(details.source.source_name, customer.name);
    assert_eq!(details.source.source_address, customer.address);
    assert_eq!(details.source.phone, "555-0199");
    assert_eq!(details.source.category, SourceCategory::Unspecified);
    let text = rendered(&mut app);
    assert_basic_surface(&text);
    assert!(text.to_lowercase().contains("source category"));
    app.save_action(|ticket| ticket.line_intake(0, "TV", "1", ""))
        .unwrap();
    app.wf.cash = "5".into();
    assert!(app.run(Act::Workflow(WorkflowAct::Cash)).is_err());
    assert!(app.active().tender_evidence.cash.is_none());
    app.run(Act::Details(0)).unwrap();
    app.details.as_mut().unwrap().source.category = SourceCategory::Residential;
    app.details.as_mut().unwrap().pieces = "1".into();
    app.run(Act::SaveDetails).unwrap();
    assert_eq!(
        app.active().source_intake.category,
        SourceCategory::Residential
    );
    assert_eq!(app.active().total(), Some(500));
    app.wf.cash = "10".into();
    app.run(Act::Workflow(WorkflowAct::Cash)).unwrap();
    app.run(Act::Complete).unwrap();
    let paid = app.active().clone();
    assert_eq!(paid.state, State::Completed);
    assert_eq!(paid.customer_record.as_ref().unwrap().phone, "555-0199");
    assert!(paid.receipt().contains("Synthetic TV customer"));
    assert!(paid.receipt().contains("555-0199"));
    assert!(
        paid.receipt()
            .contains(&paid.customer_record.as_ref().unwrap().address)
    );
    app.run(Act::Receipt).unwrap();
    assert_eq!(app.receipt.as_ref().unwrap().2, paid.receipt());
    app.run(Act::Reprint).unwrap();
    assert_eq!(app.active(), &paid);
    assert_eq!(app.wf.prints.successful_internal_copies(paid.id), 1);
    drop(app);
    let mut app = db.app();
    app.run(Act::Open(paid.id)).unwrap();
    app.run(Act::Receipt).unwrap();
    assert_eq!(app.active(), &paid);
    assert_eq!(app.receipt.as_ref().unwrap().2, paid.receipt());
}

#[test]
fn anonymous_four_pound_load_keeps_minimum_and_receipts_without_extended_workflows() {
    let db = Database::new();
    let mut app = db.app();
    load_803(&mut app);
    assert!(app.details.is_none());
    let line = &app.active().lines[0];
    assert_eq!(line.base_cents, Some(160));
    assert_eq!(line.minimum_cents, Some(340));
    assert_eq!(line.total_cents, Some(500));
    assert_eq!(app.active().total(), Some(500));
    assert_basic_surface(&rendered(&mut app));
    app.wf.cash = "5".into();
    app.run(Act::Workflow(WorkflowAct::Cash)).unwrap();
    app.run(Act::Complete).unwrap();
    let original = app.active().clone();
    app.run(Act::Receipt).unwrap();
    app.run(Act::Reprint).unwrap();
    assert_eq!(app.active(), &original);
    assert_eq!(app.store.list().unwrap().len(), 1);
    assert!(original.receipt().contains("$1.60"));
    assert!(original.receipt().contains("$3.40"));
    assert_eq!(app.wf.prints.successful_internal_copies(original.id), 1);
}

#[test]
fn unknown_external_card_is_reconciled_on_the_same_attempt_before_completion() {
    let db = Database::new();
    let mut app = db.app();
    load_803(&mut app);
    app.tender = Tender::Card;
    app.reference = "TEST-BASIC-UNKNOWN".into();
    app.wf.card_amount = "5".into();
    app.run(Act::Workflow(WorkflowAct::BeginCard)).unwrap();
    app.wf.outcome = CardOutcome::Unknown;
    app.wf.verified = false;
    app.run(Act::Workflow(WorkflowAct::CardResult(false)))
        .unwrap();
    assert!(app.run(Act::Workflow(WorkflowAct::BeginCard)).is_err());
    app.wf.cash = "5".into();
    assert!(app.run(Act::Workflow(WorkflowAct::Cash)).is_err());
    assert!(app.run(Act::Complete).is_err());
    assert_eq!(app.active().tender_evidence.card_attempts.len(), 1);
    app.wf.outcome = CardOutcome::Approved;
    app.wf.verified = true;
    app.run(Act::Workflow(WorkflowAct::CardResult(true)))
        .unwrap();
    app.run(Act::Complete).unwrap();
    assert_eq!(app.active().state, State::Completed);
    assert_eq!(app.active().tender_evidence.card_attempts.len(), 1);
    assert_eq!(
        app.active().payment,
        Some(Payment::ExternalCard {
            reference: "TEST-BASIC-UNKNOWN".into()
        })
    );
}

#[test]
fn charge_to_business_stays_separate_from_the_delivering_customer_and_is_pending_billing() {
    let db = Database::new();
    let mut app = db.app();
    load_803(&mut app);
    let customer = person(&mut app);
    app.wf.account_edit = Some(BusinessAccount {
        name: "Synthetic receiving business".into(),
        active: true,
        ..Default::default()
    });
    app.run(Act::Workflow(WorkflowAct::SaveAccount)).unwrap();
    let account = app.active().account_record.clone().unwrap();
    assert_ne!(account.number, customer.number);
    app.tender = Tender::Charge;
    app.run(Act::Complete).unwrap();
    let paid = app.active();
    assert_eq!(paid.customer_record.as_ref().unwrap().id, customer.id);
    assert_eq!(paid.account_record.as_ref().unwrap().id, account.id);
    assert!(paid.tender_evidence.cash.is_none());
    assert!(paid.tender_evidence.card_attempts.is_empty());
    assert!(paid.receipt().contains("Pending billing"));
    assert!(paid.receipt().contains("Signature:"));
}

#[test]
fn linked_tv_source_reopens_with_corrected_customer_phone_and_preserves_category_and_load() {
    let db = Database::new();
    let mut app = db.app();
    let mut customer = person(&mut app);
    app.select("801");
    app.run(Act::Add).unwrap();
    app.details.as_mut().unwrap().source.category = SourceCategory::Residential;
    app.details.as_mut().unwrap().source.reason = IntakeReason::Faulty;
    app.run(Act::SaveDetails).unwrap();
    let lines = app.active().lines.clone();
    let total = app.active().total();
    customer.phone = "555-0199".into();
    customer.address = "Corrected synthetic street".into();
    app.wf.customer_edit = Some(customer.clone());
    app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
    app.run(Act::Details(0)).unwrap();
    let details = app.details.as_ref().unwrap();
    assert_eq!(details.source.phone, "555-0199");
    assert_eq!(details.source.source_address, customer.address);
    assert_eq!(details.source.category, SourceCategory::Residential);
    assert_eq!(details.source.reason, IntakeReason::Faulty);
    app.details.as_mut().unwrap().source.category = SourceCategory::Business;
    app.details.as_mut().unwrap().source.reason = IntakeReason::OfficeClosure;
    customer.phone = "555-0200".into();
    app.wf.customer_edit = Some(customer.clone());
    app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
    app.run(Act::Details(0)).unwrap();
    let details = app.details.as_ref().unwrap();
    assert_eq!(details.source.phone, "555-0200");
    assert_eq!(details.source.category, SourceCategory::Business);
    assert_eq!(details.source.reason, IntakeReason::OfficeClosure);
    app.wf.customer_edit = Some(Customer {
        name: "Different synthetic source customer".into(),
        phone: "555-0222".into(),
        address: "Different synthetic street".into(),
        ..Default::default()
    });
    app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
    let selected = app.active().customer_record.clone().unwrap();
    assert_ne!(selected.id, customer.id);
    app.run(Act::Details(0)).unwrap();
    let details = app.details.as_ref().unwrap();
    assert_eq!(details.source.source_name, selected.name);
    assert_eq!(details.source.source_address, selected.address);
    assert_eq!(details.source.phone, selected.phone);
    assert_eq!(details.source.linked_customer_id, Some(selected.id));
    assert_eq!(details.source.category, SourceCategory::Business);
    assert_eq!(details.source.reason, IntakeReason::OfficeClosure);
    assert_eq!(app.active().lines, lines);
    assert_eq!(app.active().total(), total);
}

#[test]
fn manually_corrected_source_contact_remains_independent_after_customer_changes() {
    let db = Database::new();
    let mut app = db.app();
    let mut customer = person(&mut app);
    app.select("801");
    app.run(Act::Add).unwrap();
    let form = app.details.as_mut().unwrap();
    form.source.category = SourceCategory::Residential;
    form.source.source_name = "Independent material source".into();
    form.source.source_address = "Independent source street".into();
    form.source.phone = "555-0999".into();
    app.run(Act::SaveDetails).unwrap();
    assert_eq!(app.active().source_intake.linked_customer_id, None);
    let lines = app.active().lines.clone();
    customer.phone = "555-0199".into();
    customer.address = "Corrected delivering customer street".into();
    app.wf.customer_edit = Some(customer);
    app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
    app.run(Act::Details(0)).unwrap();
    let source = &app.details.as_ref().unwrap().source;
    assert_eq!(source.source_name, "Independent material source");
    assert_eq!(source.source_address, "Independent source street");
    assert_eq!(source.phone, "555-0999");
    assert_eq!(source.linked_customer_id, None);
    assert_eq!(source.category, SourceCategory::Residential);
    assert_eq!(app.active().lines, lines);
    assert_eq!(app.active().total(), Some(500));
}

#[test]
fn ticket_navigation_clears_transient_editors_and_preserves_the_saved_load() {
    let db = Database::new();
    let mut app = db.app();
    person(&mut app);
    app.select("801");
    app.run(Act::Add).unwrap();
    app.wf.customer_edit = app.active().customer_record.clone();
    app.show_history = true;
    app.receipt = Some((app.active, false, "Unsaved receipt preview".into()));
    let saved = app.active().clone();
    app.run(Act::New).unwrap();
    assert_ne!(app.active, saved.id);
    assert!(app.details.is_none());
    assert!(app.wf.customer_edit.is_none());
    assert!(app.receipt.is_none());
    assert_eq!(
        app.store
            .list()
            .unwrap()
            .iter()
            .find(|ticket| ticket.id == saved.id),
        Some(&saved)
    );
    app.run(Act::Open(saved.id)).unwrap();
    assert_eq!(app.active(), &saved);
    assert!(!app.show_history);
    let text = rendered(&mut app);
    assert!(text.contains("Materials"));
    assert!(!text.contains("Unsaved receipt preview"));
    assert!(!text.to_lowercase().contains("password"));
}

#[test]
fn tv_and_laptop_receipt_uses_actual_shared_source_category_instead_of_stale_line_labels() {
    let db = Database::new();
    let mut app = db.app();
    person(&mut app);
    app.select("801");
    app.run(Act::Add).unwrap();
    app.details.as_mut().unwrap().source.category = SourceCategory::Residential;
    app.run(Act::SaveDetails).unwrap();
    app.select("802");
    app.quantity = "4".into();
    app.tare = "0".into();
    app.run(Act::Add).unwrap();
    app.details.as_mut().unwrap().source.category = SourceCategory::Business;
    app.run(Act::SaveDetails).unwrap();
    assert_eq!(
        app.active().source_intake.category,
        SourceCategory::Business
    );
    assert_eq!(app.active().total(), Some(1000));
    app.wf.cash = "10".into();
    app.run(Act::Workflow(WorkflowAct::Cash)).unwrap();
    app.run(Act::Complete).unwrap();
    let completed = app.active().clone();
    let receipt = completed.receipt();
    assert!(receipt.contains("Business"));
    assert!(
        !receipt.contains("Residential"),
        "Receipt repeated an old line label instead of the ticket's actual source category"
    );
    app.run(Act::Reprint).unwrap();
    assert_eq!(app.active(), &completed);
    assert_eq!(app.active().receipt(), receipt);
    assert_eq!(app.wf.prints.successful_internal_copies(completed.id), 1);
    app.run(Act::New).unwrap();
    person(&mut app);
    app.select("801");
    app.run(Act::Add).unwrap();
    let text = rendered(&mut app);
    assert!(text.contains("Source category for this ticket"));
    app.details.as_mut().unwrap().source.category = SourceCategory::Business;
    app.run(Act::SaveDetails).unwrap();
    app.save_action(|ticket| ticket.line_intake(0, "", "", ""))
        .unwrap();
    assert_eq!(
        app.active().source_intake.category,
        SourceCategory::Business
    );
    app.ensure_source_choice().unwrap();
    app.active().validate_intake().unwrap();
}

#[test]
fn direct_payment_refreshes_linked_customer_source_but_preserves_independent_source_fields() {
    for mode in 0..3 {
        let db = Database::new();
        let mut app = db.app();
        let mut customer_a = person(&mut app);
        app.wf.customer_edit = Some(Customer {
            name: "Second selected customer".into(),
            phone: "555-0222".into(),
            address: "Second selected customer street".into(),
            ..Default::default()
        });
        app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
        let customer_b = app.active().customer_record.clone().unwrap();
        app.run(Act::Workflow(WorkflowAct::SelectCustomer(customer_a.id)))
            .unwrap();
        app.select("801");
        app.run(Act::Add).unwrap();
        let form = app.details.as_mut().unwrap();
        form.source.category = SourceCategory::Residential;
        form.source.reason_detail = "Counter visit".into();
        if mode == 2 {
            form.source.source_name = "Independent source".into();
            form.source.source_address = "Independent source street".into();
            form.source.phone = "555-0999".into();
        }
        app.run(Act::SaveDetails).unwrap();
        let original_source = app.active().source_intake.clone();
        let lines = app.active().lines.clone();
        let selected = if mode == 0 {
            customer_a.phone = "555-0199".into();
            customer_a.address = "Corrected first customer street".into();
            app.wf.customer_edit = Some(customer_a.clone());
            app.run(Act::Workflow(WorkflowAct::SaveCustomer)).unwrap();
            customer_a
        } else {
            app.run(Act::Workflow(WorkflowAct::SelectCustomer(customer_b.id)))
                .unwrap();
            customer_b
        };
        assert!(app.details.is_none());
        app.wf.cash = "5".into();
        app.run(Act::Workflow(WorkflowAct::Cash)).unwrap();
        assert!(app.details.is_none());
        let source = &app.active().source_intake;
        if mode == 2 {
            assert_eq!(source, &original_source);
            assert_eq!(source.linked_customer_id, None);
        } else {
            assert_eq!(source.source_name, selected.name);
            assert_eq!(source.source_address, selected.address);
            assert_eq!(source.phone, selected.phone);
            assert_eq!(source.linked_customer_id, Some(selected.id));
            assert_eq!(source.category, original_source.category);
            assert_eq!(source.reason_detail, original_source.reason_detail);
        }
        assert_eq!(app.active().lines, lines);
        assert_eq!(app.active().total(), Some(500));
        app.run(Act::Complete).unwrap();
        assert_eq!(app.active().state, State::Completed);
        assert_eq!(
            app.active().customer_record.as_ref().unwrap().id,
            selected.id
        );
    }
}
