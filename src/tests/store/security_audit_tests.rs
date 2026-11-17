use super::*;
use crate::operator_snapshot::Role;

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "audit-reviewer".into(),
        display_name: "Synthetic audit reviewer".into(),
        role: Role::Administrator,
        revision: 1,
    }
}

#[test]
fn append_preserves_original_and_records_attributed_semantic_action() {
    let actor = actor();
    let original = AuditLedger::default();
    let next = original
        .append(
            Some(&actor),
            AuditAction::TicketSaved,
            " ticket:42 ",
            " Draft line added ",
            100,
        )
        .unwrap();
    assert!(original.events.is_empty());
    assert_eq!(original.next_id, 1);
    assert_eq!(next.events[0].id, 1);
    assert_eq!(next.events[0].actor, Some(actor));
    assert_eq!(next.events[0].subject, "ticket:42");
    assert_eq!(next.events[0].detail, "Draft line added");
    assert_eq!(next.next_id, 2);
    next.validate().unwrap();
}

#[test]
fn only_failed_signin_may_omit_actor_and_bootstrap_requires_attribution() {
    let empty = AuditLedger::default();
    assert!(
        empty
            .append(
                None,
                AuditAction::SignInFailed,
                "register",
                "Authentication rejected",
                100
            )
            .is_ok()
    );
    for action in [
        AuditAction::BootstrapAdministrator,
        AuditAction::SignIn,
        AuditAction::SignOut,
        AuditAction::OperatorCreated,
        AuditAction::OperatorUpdated,
        AuditAction::PasswordReset,
        AuditAction::TicketSaved,
        AuditAction::CustomerDirectorySaved,
        AuditAction::BusinessDirectorySaved,
        AuditAction::TariffsSaved,
        AuditAction::IntakeReviewSaved,
        AuditAction::CloseoutSaved,
        AuditAction::PrintJobSaved,
        AuditAction::RecordsExported,
        AuditAction::BackupCreated,
        AuditAction::AdjustmentRecorded,
    ] {
        assert!(
            empty.append(None, action, "register", "", 100).is_err(),
            "{action:?}"
        );
        assert!(
            empty
                .append(Some(&actor()), action, "register", "", 100)
                .is_ok(),
            "{action:?}"
        );
    }
}

#[test]
fn timestamps_are_nondecreasing_and_backward_clock_preserves_history() {
    let first = AuditLedger::default()
        .append(Some(&actor()), AuditAction::SignIn, "operator:1", "", 100)
        .unwrap();
    let second = first
        .append(
            Some(&actor()),
            AuditAction::TicketSaved,
            "ticket:1",
            "",
            100,
        )
        .unwrap();
    assert!(
        second
            .append(Some(&actor()), AuditAction::SignOut, "operator:1", "", 99)
            .is_err()
    );
    assert_eq!(second.events.len(), 2);
    assert_eq!(second.next_id, 3);
    let later = second
        .append(Some(&actor()), AuditAction::SignOut, "operator:1", "", 101)
        .unwrap();
    assert_eq!(later.events[2].at, 101);
}

#[test]
fn field_limits_controls_and_actor_validation_reject_invalid_append() {
    let ledger = AuditLedger::default();
    let actor = actor();
    for (subject, detail) in [
        ("", ""),
        ("bad\nsubject", ""),
        ("ticket:1", "bad\tdetail"),
        (&"s".repeat(121), ""),
        ("ticket:1", &"d".repeat(501)),
    ] {
        assert!(
            ledger
                .append(Some(&actor), AuditAction::TicketSaved, subject, detail, 1)
                .is_err()
        );
    }
    assert!(
        ledger
            .append(
                Some(&actor),
                AuditAction::TicketSaved,
                &"s".repeat(120),
                &"d".repeat(500),
                1
            )
            .is_ok()
    );
    let mut bad = actor;
    bad.id = 0;
    assert!(
        ledger
            .append(Some(&bad), AuditAction::TicketSaved, "ticket:1", "", 1)
            .is_err()
    );
}

#[test]
fn repeated_append_is_explicitly_a_new_event_and_serde_preserves_history() {
    let one = AuditLedger::default()
        .append(Some(&actor()), AuditAction::SignOut, "operator:1", "", 100)
        .unwrap();
    let two = one
        .append(Some(&actor()), AuditAction::SignOut, "operator:1", "", 100)
        .unwrap();
    assert_eq!(two.events.len(), 2);
    assert_ne!(two.events[0].id, two.events[1].id);
    let json = serde_json::to_string(&two).unwrap();
    assert_eq!(serde_json::from_str::<AuditLedger>(&json).unwrap(), two);
    assert!(!json.contains("Synthetic audit password 2026"));
    assert!(!json.contains("$argon2"));
    assert_eq!(
        serde_json::from_str::<AuditLedger>("{}").unwrap(),
        AuditLedger::default()
    );
}

#[test]
fn restored_corrupt_sequence_timestamp_attribution_and_text_are_rejected() {
    let valid = AuditLedger::default()
        .append(Some(&actor()), AuditAction::SignIn, "operator:1", "", 100)
        .unwrap()
        .append(None, AuditAction::SignInFailed, "register", "Rejected", 101)
        .unwrap();
    for next_id in [0, 2, 4] {
        assert!(
            AuditLedger {
                next_id,
                ..valid.clone()
            }
            .validate()
            .is_err()
        );
    }
    let mut corrupt = valid.clone();
    corrupt.events[1].id = 1;
    assert!(corrupt.validate().is_err());
    let mut corrupt = valid.clone();
    corrupt.events[1].id = 3;
    assert!(corrupt.validate().is_err());
    let mut corrupt = valid.clone();
    corrupt.events[1].at = 99;
    assert!(corrupt.validate().is_err());
    let mut corrupt = valid.clone();
    corrupt.events[0].actor = None;
    assert!(corrupt.validate().is_err());
    let mut corrupt = valid.clone();
    corrupt.events[0].subject = " unnormalized ".into();
    assert!(corrupt.validate().is_err());
    let mut corrupt = valid;
    corrupt.events[0].detail = "bad\rtext".into();
    assert!(corrupt.validate().is_err());
}

#[test]
fn ordinary_names_containing_hash_or_password_words_are_not_blanket_rejected() {
    let ledger = AuditLedger::default()
        .append(
            Some(&actor()),
            AuditAction::OperatorCreated,
            "hashbrown-user",
            "Password reset requested through authorized workflow",
            100,
        )
        .unwrap();
    ledger.validate().unwrap();
}
