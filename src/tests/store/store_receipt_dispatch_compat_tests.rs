use super::*;
use crate::{domain::Payment, printing::CopyKind, receipt_dispatch::DispatchOutcome};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);
#[cfg(feature = "extended-workflows")]
const PASSWORD: &str = "Synthetic historical receipt operator password";
#[cfg(feature = "extended-workflows")]
pub(super) type PrinterTestAccess = crate::auth::Session;
#[cfg(not(feature = "extended-workflows"))]
pub(super) struct PrinterTestAccess;

pub(super) fn printer_test_access(store: &Store) -> PrinterTestAccess {
    #[cfg(feature = "extended-workflows")]
    {
        store
            .bootstrap_administrator("admin", "Receipt test operator", PASSWORD)
            .unwrap()
    }
    #[cfg(not(feature = "extended-workflows"))]
    {
        let _ = store;
        PrinterTestAccess
    }
}
pub(super) fn printer_test_reopen_access(store: &Store) -> PrinterTestAccess {
    #[cfg(feature = "extended-workflows")]
    {
        store.sign_in("admin", PASSWORD).unwrap()
    }
    #[cfg(not(feature = "extended-workflows"))]
    {
        let _ = store;
        PrinterTestAccess
    }
}
pub(super) fn printer_test_actor(
    access: &PrinterTestAccess,
) -> crate::operator_snapshot::OperatorSnapshot {
    #[cfg(feature = "extended-workflows")]
    {
        access.actor().clone()
    }
    #[cfg(not(feature = "extended-workflows"))]
    {
        let _ = access;
        crate::operator_snapshot::OperatorSnapshot::local_counter()
    }
}
impl Store {
    pub(super) fn test_printer_queue(
        &self,
        access: &PrinterTestAccess,
        previous: Option<&str>,
        next: Option<&str>,
    ) -> Result<(), String> {
        #[cfg(feature = "extended-workflows")]
        {
            self.save_receipt_printer_queue_as(access, previous, next)
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            let _ = access;
            self.save_receipt_printer_queue(previous, next)
        }
    }
    pub(super) fn test_prepare_dispatch(
        &self,
        access: &PrinterTestAccess,
        ticket: u64,
        copy: CopyKind,
        key: &str,
        queue: &str,
    ) -> Result<PrepareDispatch, String> {
        #[cfg(feature = "extended-workflows")]
        {
            self.prepare_receipt_dispatch_as(access, ticket, copy, key, queue)
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            let _ = access;
            self.prepare_receipt_dispatch(ticket, copy, key, queue)
        }
    }
    pub(super) fn test_retry_dispatch(
        &self,
        access: &PrinterTestAccess,
        job: u64,
        key: &str,
    ) -> Result<PrepareDispatch, String> {
        #[cfg(feature = "extended-workflows")]
        {
            self.retry_receipt_dispatch_as(access, job, key)
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            let _ = access;
            self.retry_receipt_dispatch(job, key)
        }
    }
    pub(super) fn test_checked_copy(
        &self,
        access: &PrinterTestAccess,
        prior: u64,
        attempt: u32,
        key: &str,
        queue: &str,
        checked: bool,
    ) -> Result<PrepareDispatch, String> {
        #[cfg(feature = "extended-workflows")]
        {
            self.prepare_checked_receipt_copy_as(access, prior, attempt, key, queue, checked)
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            let _ = access;
            self.prepare_checked_receipt_copy(prior, attempt, key, queue, checked)
        }
    }
    pub(super) fn test_printer_ticket(
        &self,
        access: &PrinterTestAccess,
        ticket: &Ticket,
    ) -> Result<Ticket, String> {
        #[cfg(feature = "extended-workflows")]
        {
            self.save_ticket_as(access, ticket)
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            let _ = access;
            self.save_counter_ticket(ticket)
        }
    }
    pub(super) fn test_printer_preview(
        &self,
        access: &PrinterTestAccess,
        previous: &crate::printing::PrintLog,
        next: &crate::printing::PrintLog,
    ) -> Result<(), String> {
        #[cfg(feature = "extended-workflows")]
        {
            self.save_print_log_as(access, previous, next)
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            let _ = access;
            self.save_counter_print_log(previous, next)
        }
    }
}
struct Database(std::path::PathBuf);
impl Database {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "ewaste-print-history-{}-{}-{}.sqlite3",
            std::process::id(),
            crate::domain::now(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn raw_ticket(store: &Store) -> String {
    let mut statement = store
        .statement("SELECT payload FROM tickets WHERE id=?1")
        .unwrap();
    statement.text(1, "00000000000000000001").unwrap();
    assert_eq!(statement.step().unwrap(), ROW);
    statement.read_text(0).unwrap()
}

#[test]
fn saved_legacy_payment_labels_print_without_backfilling_evidence_dates_or_ticket_fields() {
    for payment in [
        Some(Payment::Cash),
        Some(Payment::ExternalCard {
            reference: "LEGACY-REFERENCE".into(),
        }),
        Some(Payment::ChargeTo {
            account: "OLD-ACCOUNT".into(),
        }),
        None,
    ] {
        let db = Database::new();
        let store = Store::open(&db.0).unwrap();
        let access = printer_test_access(&store);
        let mut historical = Ticket::new(1);
        historical.created_at = 10;
        historical.add_weight_line("803", "4", "0").unwrap();
        historical.state = State::Completed;
        historical.payment = payment;
        historical.lines[0].id = 0;
        historical.next_line_id = 1;
        let mut payload = serde_json::to_value(&historical).unwrap();
        for name in [
            "source_intake",
            "receipt_snapshot",
            "completed_at",
            "created_by",
            "completed_by",
            "tender_evidence",
        ] {
            payload.as_object_mut().unwrap().remove(name);
        }
        let payload = serde_json::to_string(&payload).unwrap();
        let mut insert = store
            .statement("INSERT INTO tickets(id,created_at,payload) VALUES(?1,?2,?3)")
            .unwrap();
        insert.text(1, "00000000000000000001").unwrap();
        insert.text(2, "00000000000000000010").unwrap();
        insert.text(3, &payload).unwrap();
        insert.finish().unwrap();
        drop(insert);
        store
            .test_printer_queue(&access, None, Some("Receipt printer"))
            .unwrap();
        let prepared = match store
            .test_prepare_dispatch(
                &access,
                1,
                CopyKind::Internal,
                "historical",
                "Receipt printer",
            )
            .unwrap()
        {
            PrepareDispatch::Submit(prepared) => prepared,
            _ => panic!("new saved historical receipt intent expected"),
        };
        assert_eq!(
            prepared.receipt(),
            format!("INTERNAL COPY\n{}", historical.receipt())
        );
        store
            .record_receipt_dispatch_result(
                prepared,
                DispatchOutcome::Accepted { windows_job_id: 8 },
            )
            .unwrap();
        assert_eq!(raw_ticket(&store), payload);
        let backup = Database::new();
        store.backup(&backup.0).unwrap();
        let copy = Store::open(&backup.0).unwrap();
        assert_eq!(
            copy.receipt_dispatch_log().unwrap(),
            store.receipt_dispatch_log().unwrap()
        );
        assert_eq!(raw_ticket(&copy), payload);
        let loaded = copy.list().unwrap().remove(0);
        assert_eq!(loaded.tender_evidence, Default::default());
        assert!(loaded.account_record.is_none());
        assert!(loaded.completed_at.is_none());
        assert!(loaded.receipt_snapshot.is_none());
    }
}

/// Explicit verification fixture only. This never enumerates or contacts printers.
#[test]
#[ignore = "creates a retained synthetic database for explicit cross-build verification"]
fn export_cross_build_printer_fixture() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".verification/printer-compat");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!(
        "fixture-{}-{}-{}.sqlite3",
        std::process::id(),
        crate::domain::now(),
        NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
    ));
    assert!(!path.exists());
    let store = Store::open(&path).unwrap();
    let access = printer_test_access(&store);
    store
        .test_printer_queue(&access, None, Some("Synthetic verification printer"))
        .unwrap();
    for (index, payment) in [
        Payment::Cash,
        Payment::ExternalCard {
            reference: "LEGACY-REFERENCE".into(),
        },
        Payment::ChargeTo {
            account: "OLD-ACCOUNT".into(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let id = index as u64 + 1;
        let mut ticket = Ticket::new(id);
        ticket.created_at = 10;
        ticket.add_line("801", "1", "").unwrap();
        ticket.lines[0].classification = "Legacy saved classification".into();
        ticket.lines[0].id = 0;
        ticket.next_line_id = 1;
        ticket.state = State::Completed;
        ticket.payment = Some(payment);
        ticket.customer = Some("Synthetic legacy customer".into());
        let mut raw = serde_json::to_value(&ticket).unwrap();
        for name in [
            "source_intake",
            "receipt_snapshot",
            "completed_at",
            "created_by",
            "completed_by",
            "tender_evidence",
        ] {
            raw.as_object_mut().unwrap().remove(name);
        }
        let payload = serde_json::to_string(&raw).unwrap();
        let mut insert = store
            .statement("INSERT INTO tickets(id,created_at,payload) VALUES(?1,?2,?3)")
            .unwrap();
        insert.text(1, &format!("{id:020}")).unwrap();
        insert.text(2, "00000000000000000010").unwrap();
        insert.text(3, &payload).unwrap();
        insert.finish().unwrap();
        drop(insert);
        let prepared = match store
            .test_prepare_dispatch(
                &access,
                id,
                CopyKind::Internal,
                &format!("historical:{id}"),
                "Synthetic verification printer",
            )
            .unwrap()
        {
            PrepareDispatch::Submit(prepared) => prepared,
            _ => panic!("new synthetic dispatch expected"),
        };
        store
            .record_receipt_dispatch_result(
                prepared,
                DispatchOutcome::Accepted {
                    windows_job_id: id as u32,
                },
            )
            .unwrap();
        let mut saved = store
            .statement("SELECT payload FROM tickets WHERE id=?1")
            .unwrap();
        saved.text(1, &format!("{id:020}")).unwrap();
        assert_eq!(saved.step().unwrap(), ROW);
        assert_eq!(saved.read_text(0).unwrap(), payload);
    }
    assert_eq!(store.receipt_dispatch_log().unwrap().jobs.len(), 3);
    drop(store);
    println!("{}", path.display());
}
