use super::Store;
use crate::domain::{Payment, Ticket};
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_path(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ewaste-{label}-{}-{nonce}.sqlite3",
        std::process::id()
    ))
}

#[test]
fn directory_roundtrip_and_concurrent_edits_are_transactional() {
    use crate::customers::{Customer, Directory};
    let path = temporary_path("directory");
    let store = Store::open(&path).unwrap();
    let other_register = Store::open(&path).unwrap();
    let original = store.directory().unwrap();
    assert_eq!(original, Directory::default());
    let (next, customer) = original
        .save_customer(&Customer {
            name: "Synthetic Person".into(),
            ..Default::default()
        })
        .unwrap();
    store.save_directory(&original, &next).unwrap();
    store.save_directory(&original, &next).unwrap();
    let (stale, _) = original
        .save_customer(&Customer {
            name: "Other Person".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(other_register.save_directory(&original, &stale).is_err());
    let mut invalid = next.clone();
    invalid.customers[0].name = "bad\nname".into();
    assert!(store.save_directory(&next, &invalid).is_err());
    drop(other_register);
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.directory().unwrap().customers, vec![customer]);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn previous_schema_migrates_without_altering_ticket_payload() {
    let path = temporary_path("migration");
    let store = Store::open(&path).unwrap();
    let ticket = Ticket::new(8);
    store.save(&ticket).unwrap();
    store.execute("DROP TABLE app_state").unwrap();
    store.execute("PRAGMA user_version = 1").unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.list().unwrap(), vec![ticket]);
    assert!(store.directory().unwrap().customers.is_empty());
    assert_eq!(store.print_log().unwrap(), Default::default());
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn draft_revision_prevents_overwriting_another_operator_edit() {
    let path = temporary_path("revision");
    let store = Store::open(&path).unwrap();
    let other_register = Store::open(&path).unwrap();
    let original = Ticket::new(1);
    store.save(&original).unwrap();
    let mut next = original.clone();
    next.add_line("803", "4", "").unwrap();
    assert!(store.save(&next).is_err());
    next.revision = 1;
    store.save(&next).unwrap();
    store.save(&next).unwrap();
    let mut stale = original.clone();
    stale.add_line("803", "20", "").unwrap();
    stale.revision = 1;
    assert!(other_register.save(&stale).is_err());
    let mut missing = Ticket::new(2);
    missing.revision = 9;
    assert!(store.save(&missing).is_err());
    assert_eq!(store.list().unwrap(), vec![next]);
    drop(other_register);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn print_history_survives_restart_and_stale_retry_cannot_overwrite_it() {
    use crate::printing::{CopyKind, PrintLog, PrintState};
    let path = temporary_path("print-log");
    let store = Store::open(&path).unwrap();
    let original = store.print_log().unwrap();
    assert_eq!(original, PrintLog::default());
    let (failed, job) = original
        .request(
            1,
            CopyKind::Customer,
            "Saved synthetic receipt",
            "customer-1",
            true,
        )
        .unwrap();
    store.save_print_log(&original, &failed).unwrap();
    let (succeeded, retried) = failed.retry(job.id, false).unwrap();
    store.save_print_log(&failed, &succeeded).unwrap();
    store.save_print_log(&failed, &succeeded).unwrap();
    let (stale, _) = failed
        .request(
            1,
            CopyKind::Internal,
            "Saved synthetic receipt",
            "internal-1",
            false,
        )
        .unwrap();
    assert!(store.save_print_log(&failed, &stale).is_err());
    let mut invalid = succeeded.clone();
    invalid.next_id = 0;
    assert!(store.save_print_log(&succeeded, &invalid).is_err());
    assert_eq!(store.list().unwrap(), vec![]);
    drop(store);
    let store = Store::open(&path).unwrap();
    let restored = store.print_log().unwrap();
    assert_eq!(restored.jobs.len(), 1);
    assert_eq!(restored.jobs[0], retried);
    assert_eq!(restored.jobs[0].state, PrintState::Succeeded);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn newer_and_corrupt_metadata_are_rejected_without_resetting_records() {
    let path = temporary_path("metadata");
    let store = Store::open(&path).unwrap();
    store.execute("INSERT INTO app_state(key,payload) VALUES ('directory','{\"version\":99,\"data\":{}}')").unwrap();
    assert!(store.directory().is_err());
    assert!(
        store
            .save_directory(&Default::default(), &Default::default())
            .is_err()
    );
    store
        .execute("UPDATE app_state SET payload='not json' WHERE key='directory'")
        .unwrap();
    assert!(store.directory().is_err());
    store.execute("PRAGMA user_version = 99").unwrap();
    drop(store);
    assert!(Store::open(&path).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn card_references_are_reserved_across_tickets_before_approval() {
    use crate::tender::TenderEvidence;
    let path = temporary_path("card-reference");
    let store = Store::open(&path).unwrap();
    let mut first = Ticket::new(1);
    first.tender_evidence = TenderEvidence::default()
        .begin_card(500, "TEST-REF-1")
        .unwrap();
    store.save(&first).unwrap();
    let mut second = Ticket::new(2);
    second.tender_evidence = TenderEvidence::default()
        .begin_card(500, "test-ref-1")
        .unwrap();
    assert!(store.save(&second).is_err());
    assert_eq!(store.list().unwrap(), vec![first]);
    second.tender_evidence = TenderEvidence::default()
        .begin_card(500, "TEST-REF-2")
        .unwrap();
    store.save(&second).unwrap();
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn store_rejects_unverified_completion_even_when_state_is_forged() {
    let path = temporary_path("completion-boundary");
    let store = Store::open(&path).unwrap();
    let mut ticket = Ticket::new(1);
    ticket.add_line("803", "4", "").unwrap();
    ticket.choose_blank().unwrap();
    store.save(&ticket).unwrap();
    ticket.state = crate::domain::State::Completed;
    ticket.payment = Some(Payment::Cash);
    ticket.revision += 1;
    assert!(store.save(&ticket).is_err());
    let mut new = ticket.clone();
    new.id = 2;
    new.revision = 0;
    assert!(store.save(&new).is_err());
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn sqlite_persists_receipts_and_rejects_stale_edits() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "ewaste-sqlite-{}-{nonce}.sqlite3",
        std::process::id()
    ));
    let mut old = Ticket::new(1);
    old.created_at = 1;
    let mut ticket = Ticket::new(u64::MAX);
    ticket.created_at = 2;
    ticket.add_line("801", "", "").unwrap();
    let draft = ticket.clone();

    let store = Store::open(&path).unwrap();
    assert!(store.list().unwrap().is_empty());
    store.save(&ticket).unwrap();
    store.save(&old).unwrap();
    ticket
        .select_customer(&crate::customers::Customer {
            id: 1,
            number: "C1".into(),
            name: "Synthetic Person".into(),
            ..Default::default()
        })
        .unwrap();
    ticket
        .line_intake(0, "Training flat screen", "1", "")
        .unwrap();
    #[cfg(not(feature = "extended-workflows"))]
    {
        ticket.source_intake.category = crate::intake::SourceCategory::Residential;
    }
    ticket.confirm_cash("5.00").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.revision += 1;
    store.save(&ticket).unwrap();
    store.save(&ticket).unwrap();
    assert!(store.save(&draft).is_err());
    ticket.reprint().unwrap();
    ticket.revision += 1;
    store.save(&ticket).unwrap();
    let mut modified = ticket.clone();
    modified.lines[0].total_cents = Some(1);
    assert!(store.save(&modified).is_err());
    drop(store);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.list().unwrap(), vec![ticket, old]);
    drop(store);
    std::fs::remove_file(path).unwrap();
}
