use super::*;
use crate::{domain::Payment, intake::*};
struct Db(std::path::PathBuf);
impl Db {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "ewaste-intake-ui-{}-{}.sqlite3",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
    }
    fn app(&self) -> RegisterApp {
        {
            let mut app = RegisterApp::new(Store::open(&self.0).unwrap()).unwrap();
            app.test_sign_in();
            app
        }
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn paid(app: &mut RegisterApp) {
    app.save_action(|t| {
        t.add_weight_line("803", "4", "0")?;
        t.choose_blank()?;
        t.confirm_cash("5")?;
        t.complete(Payment::Cash)
    })
    .unwrap();
}
fn document(dialog: &mut ReviewDialog) {
    let mut preview = dialog.original.clone();
    preview.source_intake = SourceIntake {
        category: SourceCategory::Residential,
        source_name: "Synthetic source".into(),
        source_address: "1 Example Lane".into(),
        reason: IntakeReason::Faulty,
        delivery: DeliveryMode::DropOff,
        ..Default::default()
    };
    preview.lines[0].material_intake = MaterialIntake {
        class: MaterialClass::NonCrtDisplay,
        condition: MaterialCondition::Intact,
        reporting_units: Some(1),
        ..Default::default()
    };
    dialog.form = IntakeForm::new(&preview);
    dialog.reason = "Checked synthetic paperwork".into();
    dialog.disposition = ReviewDisposition::Reviewed;
}
fn render(app: &mut RegisterApp) {
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        let mut action = None;
        app.intake_panels(ctx, &mut action);
    });
}
#[test]
fn same_paid_ticket_moves_through_pending_review_and_not_for_claim_without_repricing() {
    let db = Db::new();
    let mut app = db.app();
    paid(&mut app);
    let original = app.active().clone();
    app.begin_documentary_review(original.id).unwrap();
    render(&mut app);
    assert!(app.record_documentary_review().is_err());
    document(app.intake.review.as_mut().unwrap());
    app.record_documentary_review().unwrap();
    assert_eq!(app.intake.ledger.events.len(), 1);
    assert_eq!(app.active(), &original);
    assert_eq!(
        app.intake
            .ledger
            .latest(original.id)
            .unwrap()
            .snapshot
            .disposition,
        ReviewDisposition::Reviewed
    );
    app.begin_documentary_review(original.id).unwrap();
    let dialog = app.intake.review.as_mut().unwrap();
    dialog.disposition = ReviewDisposition::NotForClaim;
    dialog.reason = "Separate training program disposition".into();
    app.record_documentary_review().unwrap();
    assert_eq!(app.intake.ledger.events.len(), 2);
    assert_eq!(app.active().receipt(), original.receipt());
    app.begin_documentary_review(original.id).unwrap();
    let dialog = app.intake.review.as_mut().unwrap();
    dialog.disposition = ReviewDisposition::PendingReview;
    dialog.reason = "Waiting for additional paperwork".into();
    app.record_documentary_review().unwrap();
    assert_eq!(app.intake.ledger.events.len(), 3);
    drop(app);
    let mut app = db.app();
    assert_eq!(app.intake.ledger.events.len(), 3);
    assert_eq!(app.active(), &original);
    app.intake.queue_open = true;
    render(&mut app);
}
#[test]
fn review_stale_history_is_rejected_and_draft_cannot_be_reviewed() {
    let db = Db::new();
    let mut app = db.app();
    assert!(app.begin_documentary_review(app.active).is_err());
    paid(&mut app);
    let original = app.active().clone();
    app.begin_documentary_review(original.id).unwrap();
    document(app.intake.review.as_mut().unwrap());
    let mut other = db.app();
    other.begin_documentary_review(original.id).unwrap();
    document(other.intake.review.as_mut().unwrap());
    other.record_documentary_review().unwrap();
    assert!(app.record_documentary_review().is_err());
    assert!(app.intake.review.is_some());
    assert_eq!(app.active(), &original);
    app.reload_documentary_review().unwrap();
    assert_eq!(app.intake.ledger.events.len(), 1);
    render(&mut app);
}
#[test]
fn default_export_creates_new_local_file_without_contact_fields() {
    let db = Db::new();
    let mut app = db.app();
    paid(&mut app);
    app.begin_documentary_review(app.active).unwrap();
    document(app.intake.review.as_mut().unwrap());
    app.record_documentary_review().unwrap();
    let path = app.export_intake_review().unwrap();
    let csv = std::fs::read_to_string(&path).unwrap();
    assert!(!csv.contains("1 Example Lane"));
    assert!(!csv.contains("Synthetic source"));
    assert!(csv.contains("Reviewed"));
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(path.parent().unwrap()).unwrap();
}
#[test]
fn documentary_and_queue_actions_remain_visible_at_minimum_window_size() {
    let db = Db::new();
    let mut app = db.app();
    paid(&mut app);
    app.begin_documentary_review(app.active).unwrap();
    let ctx = egui::Context::default();
    for queue in [false, true] {
        if queue {
            app.intake.review = None;
            app.intake.queue_open = true;
        }
        let mut found = false;
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(820., 570.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    let mut action = None;
                    app.intake_panels(ctx, &mut action);
                },
            );
            let label = if queue {
                "Export training review CSV"
            } else {
                "Save new documentary review"
            };
            for shape in &output.shapes {
                if let egui::epaint::Shape::Text(text) = &shape.shape
                    && text.galley.text() == label
                {
                    assert!(text.pos.y + text.galley.size().y <= 570.);
                    assert!(shape.clip_rect.contains(text.pos));
                    found = true;
                }
            }
            if found {
                break;
            }
        }
        assert!(
            found,
            "Intake workflow action was outside the usable window"
        );
    }
}
