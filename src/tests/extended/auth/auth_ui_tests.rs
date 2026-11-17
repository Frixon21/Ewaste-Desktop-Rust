use super::*;

fn setup() -> AuthUi {
    AuthUi {
        login: "synthetic-admin".into(),
        display_name: "Synthetic Administrator".into(),
        password: "Synthetic long test password".into(),
        confirmation: "Synthetic long test password".into(),
        ..Default::default()
    }
}
#[cfg(feature = "extended-workflows")]
fn administrator() -> (AuthDirectory, Session) {
    AuthDirectory::default()
        .bootstrap(
            "synthetic-admin",
            "Synthetic Administrator",
            "Synthetic long test password",
        )
        .unwrap()
}
#[test]
fn first_setup_requires_matching_credentials_and_moves_password_only_into_action() {
    let mut ui = setup();
    ui.confirmation = "Different synthetic test password".into();
    assert!(ui.sign_in_action(true).is_err());
    ui.clear_passwords();
    assert!(ui.password.is_empty() && ui.confirmation.is_empty());
    ui = setup();
    let action = ui.sign_in_action(true).unwrap();
    match action {
        AuthAction::Bootstrap {
            login,
            display_name,
            password,
        } => {
            assert_eq!(login, "synthetic-admin");
            assert_eq!(display_name, "Synthetic Administrator");
            assert_eq!(password, "Synthetic long test password");
        }
        _ => panic!("Expected bootstrap"),
    }
    assert!(ui.password.is_empty() && ui.confirmation.is_empty());
    assert!(ui.sign_in_action(true).is_err());
}
#[test]
fn feedback_keeps_metadata_but_clears_every_secret_on_failure_and_lock() {
    let mut ui = setup();
    #[cfg(feature = "extended-workflows")]
    {
        ui.show_operators = true;
        ui.form = Some(OperatorForm::new());
        ui.form.as_mut().unwrap().login = "synthetic-cashier".into();
        ui.form.as_mut().unwrap().password = "Another synthetic password".into();
        ui.form.as_mut().unwrap().confirmation = "Another synthetic password".into();
    }
    ui.feedback(Some("Sign-in failed. Try again.".into()));
    assert_eq!(ui.login, "synthetic-admin");
    #[cfg(feature = "extended-workflows")]
    assert_eq!(ui.form.as_ref().unwrap().login, "synthetic-cashier");
    assert!(ui.password.is_empty() && ui.confirmation.is_empty());
    #[cfg(feature = "extended-workflows")]
    {
        assert!(ui.form.as_ref().unwrap().password.is_empty());
        assert!(ui.form.as_ref().unwrap().confirmation.is_empty());
    }
    ui.locked();
    #[cfg(feature = "extended-workflows")]
    {
        assert!(!ui.show_operators);
        assert!(ui.form.is_none());
    }
    assert!(ui.login.is_empty() && ui.display_name.is_empty());
}
#[test]
#[cfg(feature = "extended-workflows")]
fn operator_forms_reject_stale_edits_and_require_admin_permission() {
    let (directory, session) = administrator();
    let (next, cashier) = directory
        .create_operator(
            &session,
            "synthetic-cashier",
            "Synthetic Cashier",
            Role::Cashier,
            "Another synthetic password",
        )
        .unwrap();
    let cashier_session = next
        .authenticate("synthetic-cashier", "Another synthetic password")
        .unwrap();
    let mut ui = AuthUi {
        show_operators: true,
        ..Default::default()
    };
    let operator = next
        .operators
        .iter()
        .find(|operator| operator.id == cashier.id)
        .unwrap();
    ui.form = Some(OperatorForm::existing(operator));
    assert!(ui.operator_action(&next, &cashier_session).is_err());
    let (changed, _) = next
        .update_operator(&session, cashier.id, "Changed Cashier", Role::Cashier, true)
        .unwrap();
    assert!(ui.operator_action(&changed, &session).is_err());
    ui.form = Some(OperatorForm::existing(
        changed
            .operators
            .iter()
            .find(|operator| operator.id == cashier.id)
            .unwrap(),
    ));
    ui.form.as_mut().unwrap().display_name = "Updated Cashier".into();
    match ui.operator_action(&changed, &session).unwrap() {
        AuthAction::UpdateOperator {
            id,
            display_name,
            role,
            active,
        } => {
            assert_eq!(id, cashier.id);
            assert_eq!(display_name, "Updated Cashier");
            assert_eq!(role, Role::Cashier);
            assert!(active);
        }
        _ => panic!("Expected metadata update"),
    }
}
#[test]
#[cfg(feature = "extended-workflows")]
fn create_and_reset_require_confirmation_and_never_emit_role_change_with_password_reset() {
    let (directory, session) = administrator();
    let mut ui = AuthUi::default();
    let mut form = OperatorForm::new();
    form.login = "synthetic-supervisor".into();
    form.display_name = "Synthetic Supervisor".into();
    form.role = Role::Supervisor;
    form.password = "Another synthetic password".into();
    form.confirmation = "Different synthetic password".into();
    ui.form = Some(form);
    assert!(ui.operator_action(&directory, &session).is_err());
    ui.form.as_mut().unwrap().confirmation = "Another synthetic password".into();
    match ui.operator_action(&directory, &session).unwrap() {
        AuthAction::CreateOperator { role, password, .. } => {
            assert_eq!(role, Role::Supervisor);
            assert_eq!(password, "Another synthetic password");
        }
        _ => panic!("Expected create"),
    }
    let admin = directory.operators.first().unwrap();
    let mut form = OperatorForm::existing(admin);
    form.reset_mode = true;
    form.role = Role::Cashier;
    form.password = "New synthetic password phrase".into();
    form.confirmation = "New synthetic password phrase".into();
    ui.form = Some(form);
    match ui.operator_action(&directory, &session).unwrap() {
        AuthAction::ResetPassword { id, password } => {
            assert_eq!(id, admin.id);
            assert_eq!(password, "New synthetic password phrase");
        }
        _ => panic!("Expected credentials-only reset"),
    }
    assert!(
        ui.form.as_ref().unwrap().password.is_empty()
            && ui.form.as_ref().unwrap().confirmation.is_empty()
    );
}
#[test]
fn sign_in_and_setup_render_masked_credentials_with_visible_actions_at_minimum_size() {
    for bootstrap in [false, true] {
        let mut ui = setup();
        let ctx = egui::Context::default();
        let label = if bootstrap {
            "Create first administrator"
        } else {
            "Sign in"
        };
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
                    assert!(ui.sign_in(ctx, bootstrap).is_none());
                },
            );
            for shape in output.shapes {
                if let egui::epaint::Shape::Text(text) = shape.shape {
                    assert!(!text.galley.text().contains("Synthetic long test password"));
                    if text.galley.text() == label {
                        assert!(text.pos.y + text.galley.size().y <= 570.);
                        assert!(shape.clip_rect.contains(text.pos));
                        found = true;
                    }
                }
            }
            if found {
                break;
            }
        }
        assert!(found, "Sign-in action must remain visible");
    }
}
#[test]
#[cfg(feature = "extended-workflows")]
fn admin_window_checks_permission_before_exposing_operator_list_or_action() {
    let (directory, session) = administrator();
    let (directory, _) = directory
        .create_operator(
            &session,
            "synthetic-cashier",
            "Synthetic Cashier",
            Role::Cashier,
            "Another synthetic password",
        )
        .unwrap();
    let cashier = directory
        .authenticate("synthetic-cashier", "Another synthetic password")
        .unwrap();
    let mut ui = AuthUi {
        show_operators: true,
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(ui.operators(ctx, &directory, &cashier).is_none());
    });
    assert!(!ui.show_operators);
    assert!(ui.form.is_none());
    ui.show_operators = true;
    ui.form = Some(OperatorForm::new());
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
                assert!(ui.operators(ctx, &directory, &session).is_none());
            },
        );
        for shape in output.shapes {
            if let egui::epaint::Shape::Text(text) = shape.shape
                && text.galley.text() == "Create operator"
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
    assert!(found, "Operator action must remain visible");
    ui.signed_in();
    assert!(ui.form.is_none());
}
