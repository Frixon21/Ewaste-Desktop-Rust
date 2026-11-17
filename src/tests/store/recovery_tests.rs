use super::*;
use crate::{customers::Customer, printing::CopyKind};

fn path(label: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ewaste-recovery-{label}-{}-{nonce}.sqlite3",
        std::process::id()
    ))
}

#[test]
fn readonly_backup_source_never_creates_or_migrates_missing_or_invalid_files() {
    let missing = path("readonly-missing");
    assert!(Store::open_readonly(&missing).is_err());
    assert!(!missing.exists());
    let empty = path("readonly-empty");
    std::fs::write(&empty, []).unwrap();
    assert!(Store::open_readonly(&empty).is_err());
    assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0);
    std::fs::remove_file(empty).unwrap();
}

#[test]
fn readonly_v1_backup_preserves_source_bytes_and_migrates_only_the_copy() {
    let source = path("readonly-v1");
    let backup = path("readonly-v1-backup");
    let store = Store::open(&source).unwrap();
    let ticket = Ticket::new(1);
    store.save(&ticket).unwrap();
    store.execute("DROP TABLE app_state").unwrap();
    store.execute("PRAGMA user_version = 1").unwrap();
    drop(store);
    let before = std::fs::read(&source).unwrap();
    let readonly = Store::open_readonly(&source).unwrap();
    assert_eq!(readonly.list().unwrap(), vec![ticket.clone()]);
    assert!(readonly.save(&Ticket::new(2)).is_err());
    readonly.backup(&backup).unwrap();
    drop(readonly);
    assert_eq!(std::fs::read(&source).unwrap(), before);
    let restored = Store::open(&backup).unwrap();
    assert_eq!(restored.list().unwrap(), vec![ticket]);
    assert!(restored.directory().unwrap().customers.is_empty());
    drop(restored);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(backup).unwrap();
}

#[test]
fn readonly_v2_source_supports_vacuum_into_without_writing_source() {
    let source = path("readonly-v2");
    let backup = path("readonly-v2-backup");
    let store = Store::open(&source).unwrap();
    store.save(&Ticket::new(4)).unwrap();
    drop(store);
    let before = std::fs::read(&source).unwrap();
    let readonly = Store::open_readonly(&source).unwrap();
    readonly.backup(&backup).unwrap();
    drop(readonly);
    assert_eq!(std::fs::read(&source).unwrap(), before);
    let restored = Store::open(&backup).unwrap();
    assert_eq!(restored.list().unwrap()[0].id, 4);
    drop(restored);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(backup).unwrap();
}

#[test]
fn foreign_sqlite_is_refused_before_copy_migration_and_source_is_untouched() {
    let source = path("foreign");
    let target = path("foreign-copy");
    let store = Store::open(&source).unwrap();
    store.execute("DROP TABLE tickets").unwrap();
    store.execute("DROP TABLE app_state").unwrap();
    store
        .execute("CREATE TABLE unrelated (value TEXT)")
        .unwrap();
    store
        .execute("INSERT INTO unrelated(value) VALUES ('preserve foreign records')")
        .unwrap();
    store.execute("PRAGMA user_version = 0").unwrap();
    drop(store);
    let original = std::fs::read(&source).unwrap();
    assert!(Store::open_readonly(&source).is_err());
    let error = restore_copy(&source, &target).unwrap_err();
    assert!(error.contains("source version 0"));
    assert!(error.contains("retained"));
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert_eq!(std::fs::read(&target).unwrap(), original);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(target).unwrap();
}

#[test]
fn backup_roundtrip_restores_all_register_records_to_a_new_file() {
    let original_path = path("original");
    let backup_path = path("backup");
    let restored_path = path("restored");
    let store = Store::open(&original_path).unwrap();
    let mut ticket = Ticket::new(1);
    ticket.created_at = 100;
    ticket.add_line("803", "4", "").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(crate::domain::Payment::Cash).unwrap();
    ticket.completed_at = Some(110);
    store.save(&ticket).unwrap();
    let directory = store.directory().unwrap();
    let (directory_next, _) = directory
        .save_customer(&Customer {
            name: "Synthetic Backup Person".into(),
            ..Default::default()
        })
        .unwrap();
    store.save_directory(&directory, &directory_next).unwrap();
    let log = store.print_log().unwrap();
    let (log_next, _) = log
        .request(
            1,
            CopyKind::Customer,
            &ticket.receipt(),
            "backup-copy",
            true,
        )
        .unwrap();
    store.save_print_log(&log, &log_next).unwrap();
    let catalog = store.catalog().unwrap();
    let edited = crate::catalog::PricingRule {
        rate_cents: Some(45),
        ..catalog.rule_for("803", 0).unwrap().clone()
    };
    let catalog_next = catalog.save_rule(&edited).unwrap();
    store.save_catalog(&catalog, &catalog_next).unwrap();
    let closeout = crate::closeout::ShiftReport::from_tickets(&[ticket.clone()], 0, 200)
        .unwrap()
        .reconcile(0, 500, 0, "Synthetic Operator", "")
        .unwrap();
    store.save_closeout(&closeout).unwrap();
    store.backup(&backup_path).unwrap();
    let before = std::fs::read(&backup_path).unwrap();
    restore_copy(&backup_path, &restored_path).unwrap();
    assert_eq!(std::fs::read(&backup_path).unwrap(), before);
    let restored = Store::open(&restored_path).unwrap();
    assert_eq!(restored.list().unwrap(), vec![ticket]);
    assert_eq!(restored.directory().unwrap(), directory_next);
    assert_eq!(restored.print_log().unwrap(), log_next);
    assert_eq!(restored.catalog().unwrap(), catalog_next);
    assert_eq!(restored.closeouts().unwrap(), vec![closeout]);
    drop(restored);
    drop(store);
    for file in [original_path, backup_path, restored_path] {
        std::fs::remove_file(file).unwrap();
    }
}

#[test]
fn backup_and_restore_never_overwrite_existing_files() {
    let source = path("source");
    let existing = path("existing");
    let store = Store::open(&source).unwrap();
    std::fs::write(&existing, b"preserve me").unwrap();
    assert!(store.backup(&existing).is_err());
    assert!(store.backup(&source).is_err());
    assert!(store.backup(Path::new("relative.sqlite3")).is_err());
    assert!(restore_copy(&source, &existing).is_err());
    assert_eq!(std::fs::read(&existing).unwrap(), b"preserve me");
    std::fs::write(&existing, []).unwrap();
    assert!(store.backup(&existing).is_err());
    drop(store);
    assert!(restore_copy(&source, &existing).is_err());
    assert_eq!(std::fs::metadata(&existing).unwrap().len(), 0);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(existing).unwrap();
}

#[test]
fn corrupt_restore_is_rejected_and_original_is_untouched() {
    let source = path("corrupt");
    let target = path("quarantine");
    std::fs::write(&source, b"not a SQLite database").unwrap();
    let before = std::fs::read(&source).unwrap();
    let error = restore_copy(&source, &target).unwrap_err();
    assert!(error.contains("retained"));
    assert_eq!(std::fs::read(&source).unwrap(), before);
    assert_eq!(std::fs::read(&target).unwrap(), before);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(target).unwrap();
}

#[test]
fn active_journals_and_invalid_paths_are_refused_before_creating_output() {
    let source = path("journal-source");
    let target = path("journal-target");
    std::fs::write(&source, b"synthetic").unwrap();
    for suffix in ["-journal", "-wal"] {
        let journal = companion_path(&source, suffix);
        std::fs::write(&journal, b"pending").unwrap();
        assert!(restore_copy(&source, &target).is_err());
        assert!(!target.exists());
        std::fs::remove_file(journal).unwrap();
    }
    assert!(restore_copy(&source, Path::new("relative.sqlite3")).is_err());
    assert!(restore_copy(&source, &source).is_err());
    std::fs::remove_file(source).unwrap();
}

#[test]
fn empty_files_and_live_register_sources_cannot_be_restored() {
    let source = path("empty");
    let target = path("empty-copy");
    std::fs::write(&source, []).unwrap();
    assert!(restore_copy(&source, &target).is_err());
    assert_eq!(std::fs::metadata(&source).unwrap().len(), 0);
    assert_eq!(std::fs::metadata(&target).unwrap().len(), 0);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(target).unwrap();

    let source = path("live");
    let target = path("live-copy");
    let store = Store::open(&source).unwrap();
    assert!(restore_copy(&source, &target).is_err());
    assert!(!target.exists());
    assert!(store.list().unwrap().is_empty());
    drop(store);
    std::fs::remove_file(source).unwrap();
}

#[test]
fn structurally_valid_sqlite_with_invalid_app_records_is_rejected() {
    let source = path("bad-record");
    let target = path("bad-record-copy");
    let store = Store::open(&source).unwrap();
    store
        .execute("INSERT INTO app_state(key,payload) VALUES ('directory','not valid JSON')")
        .unwrap();
    drop(store);
    let original = std::fs::read(&source).unwrap();
    let error = restore_copy(&source, &target).unwrap_err();
    assert!(error.contains("directory"));
    assert!(error.contains("retained"));
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert!(target.exists());
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(target).unwrap();
}
