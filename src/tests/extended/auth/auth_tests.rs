use super::*;
use std::sync::OnceLock;

const PASSWORD: &str = "Synthetic fixture password 2026";
const RESET: &str = "Replacement fixture password 2026";

fn admin() -> (AuthDirectory, Session) {
    static FIXTURE: OnceLock<AuthDirectory> = OnceLock::new();
    let fixture = FIXTURE.get_or_init(|| {
        AuthDirectory::default()
            .bootstrap("admin", "Synthetic Administrator", PASSWORD)
            .unwrap()
            .0
    });
    (
        fixture.clone(),
        fixture.authenticate("admin", PASSWORD).unwrap(),
    )
}

#[test]
fn bootstrap_requires_empty_directory_and_stores_only_salted_argon2id() {
    let (directory, session) = admin();
    directory.validate().unwrap();
    assert_eq!(session.actor().role, Role::Administrator);
    assert_eq!(session.actor().revision, 1);
    assert_eq!(directory.register_id.len(), 32);
    assert!(directory.bootstrap("other", "Other", PASSWORD).is_err());
    let json = serde_json::to_string(&directory).unwrap();
    assert!(json.contains("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(!json.contains(PASSWORD));
    let restored: AuthDirectory = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, directory);
    assert!(restored.authenticate(" ADMIN ", PASSWORD).is_ok());
}

#[test]
fn initialized_or_advanced_empty_directories_never_reopen_administrator_setup() {
    let (directory, _) = admin();
    let missing_operators = AuthDirectory {
        operators: vec![],
        ..directory.clone()
    };
    assert!(missing_operators.validate().is_err());
    assert!(
        missing_operators
            .bootstrap("replacement", "Replacement Administrator", PASSWORD)
            .is_err()
    );
    let advanced_uninitialized = AuthDirectory {
        next_id: 2,
        ..Default::default()
    };
    assert!(advanced_uninitialized.validate().is_err());
    assert!(
        advanced_uninitialized
            .bootstrap("replacement", "Replacement Administrator", PASSWORD)
            .is_err()
    );
    let reset_counter_only = AuthDirectory {
        operators: vec![],
        next_id: 1,
        ..directory
    };
    assert!(reset_counter_only.validate().is_err());
    AuthDirectory::default().validate().unwrap();
}

#[test]
fn role_permissions_are_checked_against_current_directory() {
    let (directory, administrator) = admin();
    let (directory, _) = directory
        .create_operator(
            &administrator,
            "cashier",
            "Synthetic Cashier",
            Role::Cashier,
            PASSWORD,
        )
        .unwrap();
    let (directory, _) = directory
        .create_operator(
            &administrator,
            "supervisor",
            "Synthetic Supervisor",
            Role::Supervisor,
            PASSWORD,
        )
        .unwrap();
    let cashier = directory.authenticate("cashier", PASSWORD).unwrap();
    let supervisor = directory.authenticate("supervisor", PASSWORD).unwrap();
    for permission in [
        Permission::Counter,
        Permission::ManageBusinessAccounts,
        Permission::ManageTariffs,
        Permission::ReviewIntake,
        Permission::ExportRecords,
        Permission::ExportContacts,
        Permission::Closeout,
        Permission::AdjustFinancials,
        Permission::Backup,
        Permission::ManageOperators,
    ] {
        assert_eq!(
            directory.can(&cashier, permission),
            permission == Permission::Counter
        );
        assert_eq!(
            directory.can(&supervisor, permission),
            permission != Permission::ManageOperators
        );
        assert!(directory.can(&administrator, permission));
    }

    assert!(
        directory
            .create_operator(
                &supervisor,
                "forbidden",
                "Forbidden",
                Role::Administrator,
                PASSWORD
            )
            .is_err()
    );
}

#[test]
fn changing_metadata_revokes_old_session_without_mutating_original_directory() {
    let (directory, administrator) = admin();
    let (directory, created) = directory
        .create_operator(
            &administrator,
            "cashier",
            "Synthetic Cashier",
            Role::Cashier,
            PASSWORD,
        )
        .unwrap();
    let cashier = directory.authenticate("cashier", PASSWORD).unwrap();
    for (name, role, active) in [
        ("Corrected name", Role::Cashier, true),
        ("Synthetic Cashier", Role::Supervisor, true),
        ("Synthetic Cashier", Role::Cashier, false),
    ] {
        let (changed, snapshot) = directory
            .update_operator(&administrator, created.id, name, role, active)
            .unwrap();
        assert_eq!(snapshot.revision, created.revision + 1);
        assert!(changed.require(&cashier, Permission::Counter).is_err());
        assert!(directory.require(&cashier, Permission::Counter).is_ok());
    }
}

#[test]
fn last_active_administrator_cannot_be_disabled_or_demoted() {
    let (directory, administrator) = admin();
    let id = administrator.actor().id;
    assert!(
        directory
            .update_operator(&administrator, id, "Admin", Role::Cashier, true)
            .is_err()
    );
    assert!(
        directory
            .update_operator(&administrator, id, "Admin", Role::Administrator, false)
            .is_err()
    );
    let (directory, _) = directory
        .create_operator(
            &administrator,
            "second-admin",
            "Second Administrator",
            Role::Administrator,
            PASSWORD,
        )
        .unwrap();
    let (changed, _) = directory
        .update_operator(
            &administrator,
            id,
            "Former Administrator",
            Role::Cashier,
            true,
        )
        .unwrap();
    assert!(
        changed
            .require(&administrator, Permission::Counter)
            .is_err()
    );
    assert!(
        directory
            .require(&administrator, Permission::ManageOperators)
            .is_ok()
    );
}

#[test]
fn password_reset_revokes_sessions_and_never_trims_passwords() {
    let (directory, administrator) = admin();
    let raw_password = "  Synthetic password with spaces  ";
    let (directory, operator) = directory
        .create_operator(
            &administrator,
            "spaced",
            "Spaced Password",
            Role::Cashier,
            raw_password,
        )
        .unwrap();
    let previous = directory.authenticate("spaced", raw_password).unwrap();
    assert!(
        directory
            .authenticate("spaced", raw_password.trim())
            .is_err()
    );
    let (changed, _) = directory
        .reset_password(&administrator, operator.id, RESET)
        .unwrap();
    assert!(changed.require(&previous, Permission::Counter).is_err());
    assert!(changed.authenticate("spaced", raw_password).is_err());
    assert!(changed.authenticate("spaced", RESET).is_ok());
}

#[test]
fn authentication_failures_are_generic_for_unknown_wrong_and_inactive_accounts() {
    let (directory, administrator) = admin();
    let wrong = directory.authenticate("admin", RESET).unwrap_err();
    assert_eq!(
        directory.authenticate("missing", PASSWORD).unwrap_err(),
        wrong
    );
    assert_eq!(
        directory.authenticate("bad\nlogin", PASSWORD).unwrap_err(),
        wrong
    );
    let (directory, operator) = directory
        .create_operator(
            &administrator,
            "inactive",
            "Inactive",
            Role::Cashier,
            PASSWORD,
        )
        .unwrap();
    let (directory, _) = directory
        .update_operator(
            &administrator,
            operator.id,
            "Inactive",
            Role::Cashier,
            false,
        )
        .unwrap();
    assert_eq!(
        directory.authenticate("inactive", PASSWORD).unwrap_err(),
        wrong
    );
}

#[test]
fn bounded_inputs_and_duplicate_normalized_logins_fail_before_creation() {
    let (directory, administrator) = admin();
    assert!(
        directory
            .create_operator(
                &administrator,
                " ADMIN ",
                "Duplicate",
                Role::Cashier,
                PASSWORD
            )
            .is_err()
    );
    for login in ["", "bad login", "bad\nlogin", &"x".repeat(49)] {
        assert!(
            directory
                .create_operator(&administrator, login, "Example", Role::Cashier, PASSWORD)
                .is_err()
        );
    }
    for password in ["short", &"x".repeat(129)] {
        assert!(
            directory
                .create_operator(&administrator, "new", "Example", Role::Cashier, password)
                .is_err()
        );
    }
    assert!(
        directory
            .create_operator(&administrator, "new", "bad\nname", Role::Cashier, PASSWORD)
            .is_err()
    );
    assert_eq!(directory.operators.len(), 1);
}

#[test]
fn persisted_hash_resource_parameters_are_rejected_before_verification() {
    let (directory, _) = admin();
    for invalid in [
        directory.operators[0]
            .password_hash
            .replace("m=19456", "m=4294967295"),
        directory.operators[0]
            .password_hash
            .replace("t=2", "t=9999999"),
        directory.operators[0].password_hash.replace("p=1", "p=99"),
        directory.operators[0]
            .password_hash
            .replace("argon2id", "argon2i"),
        directory.operators[0].password_hash.replace("v=19", "v=16"),
        "plaintext is not a password hash".into(),
        "x".repeat(513),
    ] {
        let mut changed = directory.clone();
        changed.operators[0].password_hash = invalid;
        assert!(changed.validate().is_err());
        assert!(changed.authenticate("admin", PASSWORD).is_err());
    }
}

#[test]
fn session_revocation_is_shared_and_register_binding_prevents_cross_register_use() {
    let (directory, _) = admin();
    let session = directory.authenticate("admin", PASSWORD).unwrap();
    let clone = session.clone();
    clone.revoke();
    assert!(session.is_revoked());
    assert!(directory.require(&session, Permission::Counter).is_err());
    let (other, other_session) = AuthDirectory::default()
        .bootstrap("admin", "Synthetic Administrator", PASSWORD)
        .unwrap();
    assert!(directory.register_id != other.register_id);
    assert!(
        directory
            .require(&other_session, Permission::Counter)
            .is_err()
    );
    assert!(
        other
            .require(
                &directory.authenticate("admin", PASSWORD).unwrap(),
                Permission::Counter
            )
            .is_err()
    );
}

#[test]
fn public_snapshots_and_debug_output_redact_password_material() {
    let (directory, session) = admin();
    let snapshot = directory.operators[0].snapshot();
    snapshot.validate().unwrap();
    let snapshot_json = serde_json::to_string(&snapshot).unwrap();
    for output in [
        snapshot_json,
        format!("{directory:?}"),
        format!("{session:?}"),
    ] {
        assert!(!output.contains(PASSWORD));
        assert!(!output.contains("$argon2"));
    }
    let mut corrupt = directory.clone();
    corrupt.operators.push(directory.operators[0].clone());
    assert!(corrupt.validate().is_err());
    let mut corrupt = directory.clone();
    corrupt.operators[0].active = false;
    assert!(corrupt.validate().is_err());
    assert!(serde_json::from_str::<AuthDirectory>("{}").is_err());
}

#[test]
fn unicode_password_and_identity_bounds_are_checked_without_trimming_secrets() {
    assert!(validate_password(&"x".repeat(14)).is_err());
    assert!(validate_password(&"x".repeat(15)).is_ok());
    assert!(validate_password(&"🧪".repeat(128)).is_ok());
    assert!(validate_password(&"🧪".repeat(129)).is_err());
    assert!(normalize_name(&"🧪".repeat(120)).is_ok());
    assert!(normalize_name(&"🧪".repeat(121)).is_err());
    assert!(normalize_name(&" ".repeat(10_000)).is_err());
    assert!(normalize_login(&" ".repeat(10_000)).is_err());
    assert_eq!(
        normalize_login(" Admin.Login@Site ").unwrap(),
        "admin.login@site"
    );
}
