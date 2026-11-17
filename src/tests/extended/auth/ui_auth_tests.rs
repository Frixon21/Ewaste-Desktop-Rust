use super::*;
use crate::{auth::Role, customers::Customer, domain::Payment};
use std::path::PathBuf;

const PASSWORD: &str = "Synthetic authentication flow password";
const REPLACEMENT: &str = "Replacement authentication test password";
const PRIVATE_CUSTOMER: &str = "PRIVATE SYNTHETIC CUSTOMER";
const PRIVATE_RECEIPT: &str = "PRIVATE SYNTHETIC RECEIPT";
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "ewaste-auth-ui-{}-{}.sqlite3",
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
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}-journal", self.0.display()));
    }
}
fn bootstrap(app: &mut RegisterApp) {
    app.handle_auth_action(AuthAction::Bootstrap {
        login: "local-admin".into(),
        display_name: "Synthetic Administrator".into(),
        password: PASSWORD.into(),
    })
    .unwrap();
}
fn sign_in(app: &mut RegisterApp, login: &str, password: &str) {
    app.handle_auth_action(AuthAction::SignIn {
        login: login.into(),
        password: password.into(),
    })
    .unwrap();
}
fn authorized(app: &mut RegisterApp) {
    bootstrap(app);
    assert!(app.security.session.is_none());
    sign_in(app, "local-admin", PASSWORD);
}
fn paid_private_ticket(app: &mut RegisterApp) {
    let previous = app.wf.directory.clone();
    let (next, customer) = previous
        .save_customer(&Customer {
            name: PRIVATE_CUSTOMER.into(),
            address: "Private synthetic address".into(),
            phone: "555-0199".into(),
            ..Default::default()
        })
        .unwrap();
    app.store
        .save_directory_as(app.operator_session().unwrap(), &previous, &next)
        .unwrap();
    app.wf.directory = next;
    app.save_action(|ticket| {
        ticket.add_line("803", "4", "")?;
        ticket.select_customer(&customer)?;
        ticket.confirm_cash("5")?;
        ticket.complete(Payment::Cash)
    })
    .unwrap();
}
fn sensitive_forms(app: &mut RegisterApp) {
    app.show_history = true;
    app.history_query = PRIVATE_CUSTOMER.into();
    app.catalog_editor.open = true;
    app.closeout_editor.open = true;
    app.intake.details.open = true;
    app.intake.queue_open = true;
    app.financial.editor.open = true;
    app.price_editor.open = true;
    app.inventory.editor.open = true;
    app.security.ui.show_operators = true;
    app.security.show_audit = true;
    app.receipt = Some((app.active, false, PRIVATE_RECEIPT.into()));
    app.edit = Some(LineEdit::default());
    app.wf.pending_remove = Some(0);
    app.notice = Some("PRIVATE INTERNAL NOTICE".into());
}
fn assert_sensitive_closed(app: &RegisterApp) {
    assert!(app.security.session.is_none());
    assert!(!app.show_history);
    assert!(app.history_query.is_empty());
    assert!(!app.catalog_editor.open && !app.closeout_editor.open);
    assert!(!app.intake.modal_open() && !app.intake.queue_open);
    assert!(!app.financial.editor.open);
    assert!(!app.price_editor.open);
    assert!(!app.inventory.editor.open);
    assert!(!app.security.ui.show_operators && !app.security.show_audit);
    assert!(app.receipt.is_none() && app.edit.is_none());
    assert!(!app.wf.modal_open());
    assert!(app.notice.is_none());
}
fn rendered(app: &mut RegisterApp) -> (bool, String) {
    let context = egui::Context::default();
    let mut permitted = false;
    let mut text = String::new();
    // Mirror the update entry point: a failed authentication guard returns before
    // any register, receipt, source, audit or financial surface is rendered.
    for _ in 0..2 {
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(820., 570.),
                )),
                ..Default::default()
            },
            |ctx| {
                permitted = app.authentication_screen(ctx);
                if permitted {
                    let mut action = None;
                    app.top(ctx, &mut action);
                    app.workflow_status(ctx, &mut action);
                    app.catalog(ctx, app.active().state == State::Draft);
                    app.checkout(ctx, &mut action);
                    app.work(ctx, &mut action);
                    app.dialogs(ctx, &mut action);
                    app.workflow_dialogs(ctx, &mut action);
                    app.intake_panels(ctx, &mut action);
                    app.operator_panels(ctx);
                    app.financial_panels(ctx);
                }
            },
        );
        for shape in output.shapes {
            if let egui::epaint::Shape::Text(value) = shape.shape {
                text.push_str(value.galley.text());
                text.push('\n');
            }
        }
    }
    (permitted, text)
}
fn assert_locked_text(text: &str) {
    assert!(text.contains("Operator sign-in"));
    for hidden in [
        PRIVATE_CUSTOMER,
        PRIVATE_RECEIPT,
        "Private synthetic address",
        "555-0199",
        "PRIVATE INTERNAL NOTICE",
        "Ticket total",
        "Material lines",
        "Refund / void / billing credit",
        "Register audit history",
        "Source and material intake",
    ] {
        assert!(!text.contains(hidden), "A locked frame exposed {hidden}");
    }
}

#[test]
fn bootstrap_requires_explicit_sign_in_and_reopening_starts_locked() {
    let db = Database::new();
    let mut app = db.app();
    let (permitted, text) = rendered(&mut app);
    assert!(!permitted);
    assert!(text.contains("Set up this register"));
    assert!(!text.contains("Ticket total"));
    bootstrap(&mut app);
    assert!(app.security.session.is_none());
    let (permitted, text) = rendered(&mut app);
    assert!(!permitted);
    assert_locked_text(&text);
    assert!(
        app.handle_auth_action(AuthAction::SignIn {
            login: "local-admin".into(),
            password: "Wrong synthetic password".into()
        })
        .is_err()
    );
    assert!(app.security.session.is_none());
    sign_in(&mut app, "local-admin", PASSWORD);
    assert_eq!(app.operator_session().unwrap().actor().login, "local-admin");
    let (permitted, text) = rendered(&mut app);
    assert!(permitted);
    assert!(text.contains("Ticket total"));
    app.save_action(|ticket| ticket.add_line("803", "4", ""))
        .unwrap();
    let saved = app.active().clone();
    drop(app);
    let mut reopened = db.app();
    assert!(reopened.security.session.is_none());
    assert_eq!(reopened.active(), &saved);
    let (permitted, text) = rendered(&mut reopened);
    assert!(!permitted);
    assert_locked_text(&text);
    sign_in(&mut reopened, "local-admin", PASSWORD);
    assert_eq!(reopened.active(), &saved);
}
#[test]
fn unsigned_counter_handlers_cannot_change_saved_ticket_or_audit() {
    let db = Database::new();
    let mut app = db.app();
    let ticket = app.active().clone();
    let stored = app.store.list().unwrap();
    let audit = app.store.security_audit().unwrap();
    app.select("803");
    app.quantity = "4".into();
    assert!(app.run(Act::Add).is_err());
    assert!(app.run(Act::New).is_err());
    assert!(
        app.save_action(|ticket| ticket.add_line("801", "1", ""))
            .is_err()
    );
    assert_eq!(app.active(), &ticket);
    assert_eq!(app.store.list().unwrap(), stored);
    assert_eq!(app.store.security_audit().unwrap(), audit);
}
#[test]
fn logout_hides_private_ticket_and_clears_every_sensitive_open_surface() {
    let db = Database::new();
    let mut app = db.app();
    authorized(&mut app);
    paid_private_ticket(&mut app);
    let saved = app.active().clone();
    let session = app.operator_session().unwrap().clone();
    app.receipt = Some((saved.id, false, saved.receipt()));
    let (permitted, text) = rendered(&mut app);
    assert!(permitted);
    assert!(text.contains(PRIVATE_CUSTOMER));
    sensitive_forms(&mut app);
    app.handle_auth_action(AuthAction::Logout).unwrap();
    assert_sensitive_closed(&app);
    assert!(session.is_revoked());
    let (permitted, text) = rendered(&mut app);
    assert!(!permitted);
    assert_locked_text(&text);
    assert_eq!(
        app.store.list().unwrap().iter().find(|t| t.id == saved.id),
        Some(&saved)
    );
    sign_in(&mut app, "local-admin", PASSWORD);
    assert_eq!(app.active(), &saved);
    assert!(!app.show_history && !app.intake.modal_open() && !app.financial.editor.open);
}
#[test]
fn revoked_role_and_reset_password_lock_open_financial_and_source_forms() {
    let db = Database::new();
    let mut app = db.app();
    authorized(&mut app);
    paid_private_ticket(&mut app);
    app.handle_auth_action(AuthAction::CreateOperator {
        login: "supervisor".into(),
        display_name: "Synthetic Supervisor".into(),
        role: Role::Supervisor,
        password: PASSWORD.into(),
    })
    .unwrap();
    let administrator = app.operator_session().unwrap().clone();
    app.handle_auth_action(AuthAction::Logout).unwrap();
    sign_in(&mut app, "supervisor", PASSWORD);
    let supervisor = app.operator_session().unwrap().clone();
    let supervisor_id = supervisor.actor().id;
    sensitive_forms(&mut app);
    let separate = Store::open(&db.0).unwrap();
    let admin = separate.sign_in("local-admin", PASSWORD).unwrap();
    let directory = separate.auth_directory().unwrap();
    separate
        .update_operator_as(
            &admin,
            &directory,
            supervisor_id,
            "Synthetic Supervisor",
            Role::Cashier,
            true,
        )
        .unwrap();
    let (permitted, text) = rendered(&mut app);
    assert!(!permitted);
    assert_sensitive_closed(&app);
    assert_locked_text(&text);
    assert!(supervisor.is_revoked());
    sign_in(&mut app, "supervisor", PASSWORD);
    assert!(!app.can(Permission::AdjustFinancials));
    let cashier = app.operator_session().unwrap().clone();
    sensitive_forms(&mut app);
    let directory = separate.auth_directory().unwrap();
    separate
        .reset_operator_password_as(&admin, &directory, supervisor_id, REPLACEMENT)
        .unwrap();
    let (permitted, text) = rendered(&mut app);
    assert!(!permitted);
    assert_sensitive_closed(&app);
    assert_locked_text(&text);
    assert!(cashier.is_revoked());
    assert!(
        app.handle_auth_action(AuthAction::SignIn {
            login: "supervisor".into(),
            password: PASSWORD.into()
        })
        .is_err()
    );
    sign_in(&mut app, "supervisor", REPLACEMENT);
    assert_eq!(app.operator_session().unwrap().actor().role, Role::Cashier);
    assert!(administrator.is_revoked());
}
#[test]
fn inactivity_lock_preserves_saved_draft_and_hides_open_private_surfaces() {
    let db = Database::new();
    let mut app = db.app();
    authorized(&mut app);
    paid_private_ticket(&mut app);
    let saved = app.active().clone();
    let session = app.operator_session().unwrap().clone();
    sensitive_forms(&mut app);
    app.security.last_activity =
        std::time::Instant::now() - std::time::Duration::from_secs(15 * 60 + 1);
    let (permitted, text) = rendered(&mut app);
    assert!(!permitted);
    assert_sensitive_closed(&app);
    assert_locked_text(&text);
    assert!(session.is_revoked());
    assert_eq!(app.active(), &saved);
}
