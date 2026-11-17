use super::*;
use crate::catalog::Catalog;
#[cfg(feature = "extended-workflows")]
use crate::closeout::ShiftReport;
use crate::domain::{Payment, Ticket};

struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "ewaste-metadata-{}-{nonce}.sqlite3",
            std::process::id()
        )))
    }
    fn open(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
#[cfg(feature = "extended-workflows")]
fn cash_ticket(id: u64) -> Ticket {
    let mut t = Ticket::new(id);
    t.created_at = 1;
    t.add_weight_line("803", "4", "0").unwrap();
    t.choose_blank().unwrap();
    t.confirm_cash("10").unwrap();
    t.complete(Payment::Cash).unwrap();
    t.completed_at = Some(10);
    t
}
#[test]
fn tariff_change_is_persisted_but_never_reprices_saved_receipts_or_existing_lines() {
    let db = Database::new();
    let store = db.open();
    let catalog = store.catalog().unwrap();
    let mut t = Ticket::new(1);
    t.created_at = 1;
    t.add_catalog_line(&catalog, "803", "4", "0", "", 1)
        .unwrap();
    t.choose_blank().unwrap();
    let mut rule = catalog.rule_for("803", 1).unwrap().clone();
    rule.minimum_cents = Some(700);
    let next = catalog.save_rule(&rule).unwrap();
    store.save_catalog(&catalog, &next).unwrap();
    store.save_catalog(&catalog, &next).unwrap();
    let mut stale = Catalog::default();
    stale.rules[0].rate_cents = Some(600);
    assert!(store.save_catalog(&catalog, &stale).is_err());
    t.edit_weight_line(0, "5", "1").unwrap();
    assert_eq!(t.total(), Some(500));
    t.confirm_cash("5").unwrap();
    t.complete(Payment::Cash).unwrap();
    let receipt = t.receipt();
    store.save(&t).unwrap();
    let mut another = Ticket::new(2);
    another
        .add_catalog_line(&next, "803", "4", "0", "", 1)
        .unwrap();
    assert_eq!(another.total(), Some(700));
    drop(store);
    let store = db.open();
    assert_eq!(store.catalog().unwrap(), next);
    assert_eq!(store.list().unwrap()[0].receipt(), receipt);
}
#[test]
#[cfg(feature = "extended-workflows")]
fn closeout_is_append_only_idempotent_and_rejects_overlap_or_stale_report() {
    let db = Database::new();
    let store = db.open();
    let t = cash_ticket(1);
    store.save(&t).unwrap();
    let report = ShiftReport::from_tickets(&store.list().unwrap(), 0, 20).unwrap();
    let record = report
        .reconcile(10000, 10500, 0, "Training cashier", "")
        .unwrap();
    store.save_closeout(&record).unwrap();
    store.save_closeout(&record).unwrap();
    let changed = report
        .reconcile(10000, 10400, 0, "Training cashier", "Cash short")
        .unwrap();
    assert!(store.save_closeout(&changed).is_err());
    let overlap = ShiftReport::from_tickets(&store.list().unwrap(), 10, 30)
        .unwrap()
        .reconcile(0, 500, 0, "Training cashier", "")
        .unwrap();
    assert!(store.save_closeout(&overlap).is_err());
    let stale = ShiftReport::from_tickets(&store.list().unwrap(), 20, 40)
        .unwrap()
        .reconcile(0, 0, 0, "Training cashier", "")
        .unwrap();
    let mut t2 = cash_ticket(2);
    t2.completed_at = Some(25);
    store.save(&t2).unwrap();
    assert!(store.save_closeout(&stale).is_err());
    drop(store);
    let store = db.open();
    assert_eq!(store.closeouts().unwrap(), vec![record]);
}
#[test]
fn corrupted_metadata_is_reported_instead_of_reset() {
    let db = Database::new();
    let store = db.open();
    store
        .write_state("catalog", &Catalog { rules: vec![] })
        .unwrap();
    // Empty catalog is a valid deliberate disabled profile.
    assert!(store.catalog().unwrap().rules.is_empty());
    let mut bad = Catalog::default();
    bad.rules[0].rate_cents = Some(-1);
    store.write_state("catalog", &bad).unwrap();
    assert!(store.catalog().is_err());
    #[cfg(feature = "extended-workflows")]
    {
        let mut record = ShiftReport::from_tickets(&[], 0, 20)
            .unwrap()
            .reconcile(0, 0, 0, "Cashier", "")
            .unwrap();
        record.cash_variance_cents = 123;
        store.write_state("closeouts", &vec![record]).unwrap();
        assert!(store.closeouts().is_err());
    }
}
