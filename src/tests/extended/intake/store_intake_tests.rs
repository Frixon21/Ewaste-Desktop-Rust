#![cfg(feature = "extended-workflows")]
use super::*;
use crate::{domain::Payment, intake_review::*};
struct Db(std::path::PathBuf);
impl Db {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "ewaste-intake-{}-{}.sqlite3",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
    }
    fn open(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn completed() -> Ticket {
    let mut t = Ticket::new(1);
    t.created_at = 1;
    t.add_weight_line("803", "4", "0").unwrap();
    t.choose_blank().unwrap();
    t.confirm_cash("5").unwrap();
    t.complete(Payment::Cash).unwrap();
    t.completed_at = Some(100);
    t
}
#[test]
fn documentary_reviews_are_append_only_persistent_and_separate_from_receipts() {
    let db = Db::new();
    let store = db.open();
    let ticket = completed();
    store.save(&ticket).unwrap();
    let receipt = ticket.receipt();
    let ledger = store.intake_reviews().unwrap();
    let mut snapshot = ReviewSnapshot::from_ticket(&ticket).unwrap();
    snapshot.disposition = ReviewDisposition::NotForClaim;
    let (next, _) = ledger
        .record(
            &ticket,
            &snapshot,
            "Training reviewer",
            "Outside training claim workflow",
            "review1",
            110,
        )
        .unwrap();
    store.save_intake_reviews(&ledger, &next).unwrap();
    store.save_intake_reviews(&ledger, &next).unwrap();
    let (stale, _) = ledger
        .record(
            &ticket,
            &snapshot,
            "Other reviewer",
            "Concurrent review",
            "review-other",
            111,
        )
        .unwrap();
    assert!(store.save_intake_reviews(&ledger, &stale).is_err());
    let mut rewritten = next.clone();
    rewritten.events[0].reason = "Changed past reason".into();
    assert!(store.save_intake_reviews(&next, &rewritten).is_err());
    let (later, _) = next
        .record(
            &ticket,
            &ReviewSnapshot::from_ticket(&ticket).unwrap(),
            "Training reviewer",
            "Needs source documents",
            "review2",
            120,
        )
        .unwrap();
    store.save_intake_reviews(&next, &later).unwrap();
    assert_eq!(store.list().unwrap()[0], ticket);
    drop(store);
    let store = db.open();
    assert_eq!(store.intake_reviews().unwrap(), later);
    assert_eq!(store.list().unwrap()[0].receipt(), receipt);
}
#[test]
fn corrupt_or_orphan_review_and_reused_line_id_are_rejected() {
    let db = Db::new();
    let store = db.open();
    let mut t = Ticket::new(1);
    t.add_line("801", "1", "").unwrap();
    store.save(&t).unwrap();
    let mut invalid = t.clone();
    invalid.source_intake.source_name = "bad\nname".into();
    invalid.revision += 1;
    assert!(store.save(&invalid).is_err());
    let mut moved = t.clone();
    moved.next_line_id = 1;
    moved.revision += 1;
    assert!(store.save(&moved).is_err());
    let other = completed();
    let snapshot = ReviewSnapshot {
        disposition: ReviewDisposition::NotForClaim,
        ..ReviewSnapshot::from_ticket(&other).unwrap()
    };
    let (orphan, _) = ReviewLedger::default()
        .record(
            &other,
            &snapshot,
            "Reviewer",
            "Synthetic reason",
            "orphan",
            110,
        )
        .unwrap();
    assert!(
        store
            .save_intake_reviews(&ReviewLedger::default(), &orphan)
            .is_err()
    );
    store.write_state("intake_reviews", &orphan).unwrap();
    assert!(store.intake_reviews().is_err());
}
#[test]
fn review_ledger_survives_backup_restore_with_original_financial_ticket() {
    let db = Db::new();
    let store = db.open();
    let ticket = completed();
    store.save(&ticket).unwrap();
    let snapshot = ReviewSnapshot {
        disposition: ReviewDisposition::NotForClaim,
        ..ReviewSnapshot::from_ticket(&ticket).unwrap()
    };
    let (ledger, _) = ReviewLedger::default()
        .record(
            &ticket,
            &snapshot,
            "Reviewer",
            "Training disposition",
            "backupreview",
            110,
        )
        .unwrap();
    store
        .save_intake_reviews(&ReviewLedger::default(), &ledger)
        .unwrap();
    let backup = db.0.with_extension("backup.sqlite3");
    let restored = db.0.with_extension("restored.sqlite3");
    store.backup(&backup).unwrap();
    restore_copy(&backup, &restored).unwrap();
    let copy = Store::open(&restored).unwrap();
    assert_eq!(copy.intake_reviews().unwrap(), ledger);
    assert_eq!(copy.list().unwrap(), vec![ticket]);
    drop(copy);
    drop(store);
    std::fs::remove_file(backup).unwrap();
    std::fs::remove_file(restored).unwrap();
}
#[test]
fn schema_two_upgrade_preserves_payload_and_promotes_version_for_intake_safety() {
    let db = Db::new();
    let store = db.open();
    let t = completed();
    store.save(&t).unwrap();
    let stored_before = store.statement("SELECT payload FROM tickets").unwrap();
    assert_eq!(stored_before.step().unwrap(), ROW);
    let payload = stored_before.read_text(0).unwrap();
    drop(stored_before);
    store.execute("PRAGMA user_version=2").unwrap();
    drop(store);
    let store = db.open();
    let version = store.statement("PRAGMA user_version").unwrap();
    assert_eq!(version.step().unwrap(), ROW);
    let number = unsafe { (store.api.column_int)(version.stmt.as_ptr(), 0) };
    assert_eq!(number, 7);
    drop(version);
    let stored_after = store.statement("SELECT payload FROM tickets").unwrap();
    assert_eq!(stored_after.step().unwrap(), ROW);
    assert_eq!(stored_after.read_text(0).unwrap(), payload);
    drop(stored_after);
    assert_eq!(store.list().unwrap(), vec![t]);
}
#[test]
fn persistence_rejects_reintroduced_deleted_and_new_legacy_line_ids() {
    let db = Db::new();
    let store = db.open();
    let mut t = Ticket::new(1);
    t.add_line("801", "1", "").unwrap();
    let deleted = t.lines[0].clone();
    store.save(&t).unwrap();
    t.remove_line(0).unwrap();
    t.revision += 1;
    store.save(&t).unwrap();
    let mut forged = t.clone();
    forged.lines.push(deleted);
    forged.revision += 1;
    assert!(store.save(&forged).is_err());
    let mut zero = t.clone();
    zero.add_line("801", "1", "").unwrap();
    zero.lines[0].id = 0;
    zero.revision += 1;
    assert!(store.save(&zero).is_err());
    let mut fresh = Ticket::new(2);
    fresh.add_line("801", "1", "").unwrap();
    fresh.lines[0].id = 0;
    assert!(store.save(&fresh).is_err());
}
#[test]
fn legacy_zero_id_lines_remain_readable_reprintable_and_reviewable_without_backfill() {
    let db = Db::new();
    let store = db.open();
    let ticket = completed();
    store.save(&ticket).unwrap();
    let mut json = serde_json::to_value(&ticket).unwrap();
    json.as_object_mut().unwrap().remove("next_line_id");
    json.as_object_mut().unwrap().remove("source_intake");
    for line in json["lines"].as_array_mut().unwrap() {
        line.as_object_mut().unwrap().remove("id");
        line.as_object_mut().unwrap().remove("material_intake");
    }
    let mut update = store.statement("UPDATE tickets SET payload=?1").unwrap();
    update
        .text(1, &serde_json::to_string(&json).unwrap())
        .unwrap();
    update.finish().unwrap();
    drop(update);
    drop(store);
    let store = db.open();
    let mut old = store.list().unwrap().remove(0);
    assert_eq!(old.lines[0].id, 0);
    let receipt = old.receipt();
    old.reprint().unwrap();
    old.revision += 1;
    store.save(&old).unwrap();
    assert_eq!(old.receipt(), receipt);
    let snapshot = ReviewSnapshot {
        disposition: ReviewDisposition::NotForClaim,
        ..ReviewSnapshot::from_ticket(&old).unwrap()
    };
    assert_eq!(
        snapshot.materials[0].identity,
        LineIdentity::LegacyOrdinal(0)
    );
    let (ledger, _) = ReviewLedger::default()
        .record(
            &old,
            &snapshot,
            "Reviewer",
            "Unmapped legacy source",
            "legacy-source-review",
            110,
        )
        .unwrap();
    store
        .save_intake_reviews(&ReviewLedger::default(), &ledger)
        .unwrap();
    assert_eq!(store.list().unwrap()[0].lines[0].id, 0);
}
