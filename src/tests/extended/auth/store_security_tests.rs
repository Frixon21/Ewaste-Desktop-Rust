#![cfg(feature = "extended-workflows")]
use super::*;
use crate::auth::{Permission, Role};
const PASS: &str = "Synthetic register operator password";
struct Db(std::path::PathBuf);
impl Db {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
                "ewaste-security-{}-{}.sqlite3",
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
#[test]
fn bootstrap_signin_roles_and_revocation_are_checked_at_the_store_boundary() {
    let db = Db::new();
    let store = db.open();
    let admin = store
        .bootstrap_administrator("admin", "Synthetic administrator", PASS)
        .unwrap();
    assert!(
        store
            .bootstrap_administrator("another", "Another", PASS)
            .is_err()
    );
    let original = store.auth_directory().unwrap();
    let next = store
        .create_operator_as(
            &admin,
            &original,
            "cashier",
            "Synthetic cashier",
            Role::Cashier,
            PASS,
        )
        .unwrap();
    let cashier = store.sign_in("cashier", PASS).unwrap();
    assert!(store.require_session(&cashier, Permission::Counter).is_ok());
    let catalog = store.catalog().unwrap();
    assert!(store.save_catalog_as(&cashier, &catalog, &catalog).is_err());
    let cashier_id = cashier.actor().id;
    let _ = store
        .update_operator_as(
            &admin,
            &next,
            cashier_id,
            "Synthetic cashier",
            Role::Cashier,
            false,
        )
        .unwrap();
    assert!(
        store
            .require_session(&cashier, Permission::Counter)
            .is_err()
    );
    let foreign = Db::new();
    let other = foreign.open();
    other
        .bootstrap_administrator("admin", "Synthetic administrator", PASS)
        .unwrap();
    assert!(other.require_session(&admin, Permission::Counter).is_err());
    admin.revoke();
    assert!(store.require_session(&admin, Permission::Counter).is_err());
}
#[test]
fn cashier_cannot_forge_prices_or_modify_business_directory_even_with_valid_ticket() {
    let db = Db::new();
    let store = db.open();
    let admin = store
        .bootstrap_administrator("admin", "Administrator", PASS)
        .unwrap();
    let directory = store.auth_directory().unwrap();
    store
        .create_operator_as(
            &admin,
            &directory,
            "cashier",
            "Cashier",
            Role::Cashier,
            PASS,
        )
        .unwrap();
    let cashier = store.sign_in("cashier", PASS).unwrap();
    let mut t = Ticket::new(1);
    t.add_weight_line("803", "4", "0").unwrap();
    t.choose_blank().unwrap();
    let saved = store.save_ticket_as(&cashier, &t).unwrap();
    assert_eq!(saved.created_by.as_ref().unwrap().id, cashier.actor().id);
    let mut forged = saved.clone();
    forged.revision += 1;
    forged.lines[0].total_cents = Some(1);
    assert!(store.save_ticket_as(&cashier, &forged).is_err());
    let old = store.directory().unwrap();
    let (next, _) = old
        .save_account(&crate::customers::BusinessAccount {
            name: "Synthetic account".into(),
            active: true,
            ..Default::default()
        })
        .unwrap();
    assert!(store.save_directory_as(&cashier, &old, &next).is_err());
    let mut counters = old.clone();
    counters.next_account_id += 1;
    assert!(store.save_directory_as(&cashier, &old, &counters).is_err());
    let customers = old
        .save_customer(&crate::customers::Customer {
            name: "Synthetic customer".into(),
            ..Default::default()
        })
        .unwrap()
        .0;
    store.save_directory_as(&cashier, &old, &customers).unwrap();
    assert_eq!(store.list().unwrap()[0], saved);
    assert!(!store.security_audit().unwrap().events.is_empty());
}
#[test]
fn version_four_missing_auth_fails_closed_and_login_throttle_survives_reopen() {
    let db = Db::new();
    let store = db.open();
    store
        .bootstrap_administrator("admin", "Administrator", PASS)
        .unwrap();
    for _ in 0..5 {
        assert!(store.sign_in("admin", "wrong password").is_err());
    }
    assert!(store.sign_in("admin", PASS).is_err());
    drop(store);
    let store = db.open();
    assert!(store.sign_in("admin", PASS).is_err());
    store
        .execute("DELETE FROM app_state WHERE key='auth'")
        .unwrap();
    drop(store);
    assert!(Store::open(&db.0).is_err());
}
#[test]
fn failed_authorized_transaction_rolls_back_mutation_and_audit_together() {
    let db = Db::new();
    let store = db.open();
    let admin = store
        .bootstrap_administrator("admin", "Administrator", PASS)
        .unwrap();
    let before = store.security_audit().unwrap();
    let result: Result<(), String> = store.authorized(&admin, Permission::Counter, |actor| {
        store.write_state("test-partial", &actor.id)?;
        Err("Synthetic write failure".into())
    });
    assert!(result.is_err());
    assert_eq!(
        store.read_state::<Option<u64>>("test-partial").unwrap(),
        None
    );
    assert_eq!(store.security_audit().unwrap(), before);
}
#[test]
fn initialized_register_cannot_reopen_setup_when_auth_directory_is_replaced_with_default() {
    let db = Db::new();
    let store = db.open();
    store
        .bootstrap_administrator("admin", "Administrator", PASS)
        .unwrap();
    store
        .write_state("auth", &crate::auth::AuthDirectory::default())
        .unwrap();
    drop(store);
    assert!(Store::open(&db.0).is_err());
}

#[test]
fn completion_freezes_canonical_receipt_and_drafts_cannot_store_printable_snapshots() {
    let db = Db::new();
    let store = db.open();
    let admin = store
        .bootstrap_administrator("admin", "Administrator", PASS)
        .unwrap();
    let mut draft = Ticket::new(1);
    draft.add_weight_line("803", "20", "0").unwrap();
    draft.choose_blank().unwrap();
    let mut injected = draft.clone();
    injected.receipt_snapshot = Some("False paid receipt".into());
    assert!(store.save_ticket_as(&admin, &injected).is_err());
    let saved = store.save_ticket_as(&admin, &draft).unwrap();
    let mut complete = saved.clone();
    complete.confirm_cash("10").unwrap();
    complete.complete(crate::domain::Payment::Cash).unwrap();
    complete.revision = saved.revision + 1;
    let expected = complete.receipt();
    complete.receipt_snapshot = Some("Total $0.00 / Paid".into());
    let frozen = store.save_ticket_as(&admin, &complete).unwrap();
    assert_eq!(frozen.receipt(), expected);
    assert!(frozen.receipt().contains("$8.00"));
    let mut reprinted = frozen.clone();
    reprinted.reprint().unwrap();
    reprinted.revision = frozen.revision + 1;
    let reprinted = store.save_ticket_as(&admin, &reprinted).unwrap();
    assert_eq!(reprinted.receipt(), expected);
    let mut forged = reprinted.clone();
    forged.receipt_snapshot = Some("Changed after completion".into());
    assert!(store.save_ticket_as(&admin, &forged).is_err());
    drop(store);
    assert_eq!(db.open().list().unwrap()[0], reprinted);
}
