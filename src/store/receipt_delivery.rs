use super::*;
#[cfg(all(test, feature = "extended-workflows"))]
use crate::auth::{Permission, Session};
use crate::receipt_dispatch::{ReceiptDispatchLedger, validate_queue};
#[cfg(any(test, not(feature = "extended-workflows")))]
use crate::{
    operator_snapshot::OperatorSnapshot,
    printing::CopyKind,
    receipt_dispatch::{CheckedCopyRequest, DispatchJob, DispatchOutcome, DispatchState},
    security_audit::AuditAction,
};
#[cfg(any(test, not(feature = "extended-workflows")))]
use std::sync::Arc;

#[derive(Clone)]
#[cfg(any(test, not(feature = "extended-workflows")))]
pub struct PreparedDispatch {
    binding: Arc<()>,
    job: Arc<DispatchJob>,
    attempt_id: u32,
}
#[cfg(any(test, not(feature = "extended-workflows")))]
impl PreparedDispatch {
    pub fn job_id(&self) -> u64 {
        self.job.id
    }
    pub fn attempt_id(&self) -> u32 {
        self.attempt_id
    }
    pub fn ticket_id(&self) -> u64 {
        self.job.ticket_id
    }
    pub fn copy(&self) -> CopyKind {
        self.job.copy
    }
    pub fn receipt(&self) -> &str {
        &self.job.receipt
    }
    pub fn queue(&self) -> &str {
        &self.job.queue
    }
}
#[cfg(any(test, not(feature = "extended-workflows")))]
pub enum PrepareDispatch {
    Submit(PreparedDispatch),
    Existing(DispatchJob),
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrinterSettings {
    queue: Option<String>,
}
impl Store {
    pub fn receipt_dispatch_log(&self) -> Result<ReceiptDispatchLedger, String> {
        let log: ReceiptDispatchLedger = self.read_required("receipt_dispatches")?;
        log.validate_against(&self.list()?)?;
        Ok(log)
    }
    pub fn receipt_printer_queue(&self) -> Result<Option<String>, String> {
        let settings: PrinterSettings = self.read_required("printer_settings")?;
        if let Some(queue) = &settings.queue {
            validate_queue(queue)?;
        }
        Ok(settings.queue)
    }
    #[cfg(not(feature = "extended-workflows"))]
    fn local_receipt_operation<T>(
        &self,
        operation: impl FnOnce(&OperatorSnapshot) -> Result<T, String>,
    ) -> Result<T, String> {
        self.transaction(|| operation(&OperatorSnapshot::local_counter()))
    }
    #[cfg(not(feature = "extended-workflows"))]
    pub fn save_receipt_printer_queue(
        &self,
        previous: Option<&str>,
        next: Option<&str>,
    ) -> Result<(), String> {
        self.local_receipt_operation(|actor| {
            self.save_receipt_printer_queue_for(actor, previous, next)
        })
    }
    #[cfg(all(test, feature = "extended-workflows"))]
    pub fn save_receipt_printer_queue_as(
        &self,
        session: &Session,
        previous: Option<&str>,
        next: Option<&str>,
    ) -> Result<(), String> {
        self.authorized(session, Permission::Counter, |actor| {
            self.save_receipt_printer_queue_for(&actor, previous, next)
        })
    }
    #[cfg(any(test, not(feature = "extended-workflows")))]
    fn save_receipt_printer_queue_for(
        &self,
        actor: &OperatorSnapshot,
        previous: Option<&str>,
        next: Option<&str>,
    ) -> Result<(), String> {
        if let Some(queue) = next {
            validate_queue(queue)?;
        }
        let current = self.receipt_printer_queue()?;
        if current.as_deref() != previous {
            return Err(
                "The saved printer queue changed. Reload before saving a selection.".into(),
            );
        }
        if current.as_deref() != next {
            self.write_state(
                "printer_settings",
                &PrinterSettings {
                    queue: next.map(str::to_owned),
                },
            )?;
            self.append_audit(
                Some(actor),
                AuditAction::PrintJobSaved,
                "physical-printer-selection",
                "Saved an explicit receipt printer queue selection",
            )?;
        }
        Ok(())
    }
    #[cfg(not(feature = "extended-workflows"))]
    pub fn prepare_receipt_dispatch(
        &self,
        ticket_id: u64,
        copy: CopyKind,
        key: &str,
        queue: &str,
    ) -> Result<PrepareDispatch, String> {
        self.local_receipt_operation(|actor| {
            self.prepare_receipt_dispatch_for(actor, ticket_id, copy, key, queue)
        })
    }
    #[cfg(all(test, feature = "extended-workflows"))]
    pub fn prepare_receipt_dispatch_as(
        &self,
        session: &Session,
        ticket_id: u64,
        copy: CopyKind,
        key: &str,
        queue: &str,
    ) -> Result<PrepareDispatch, String> {
        self.authorized(session, Permission::Counter, |actor| {
            self.prepare_receipt_dispatch_for(&actor, ticket_id, copy, key, queue)
        })
    }
    #[cfg(any(test, not(feature = "extended-workflows")))]
    fn prepare_receipt_dispatch_for(
        &self,
        actor: &OperatorSnapshot,
        ticket_id: u64,
        copy: CopyKind,
        key: &str,
        queue: &str,
    ) -> Result<PrepareDispatch, String> {
        let ticket = self
            .list()?
            .into_iter()
            .find(|ticket| ticket.id == ticket_id)
            .ok_or("Saved ticket not found.")?;
        let current = self.receipt_dispatch_log()?;
        let (next, job, submit) =
            current.prepare(&ticket, copy, key, queue, actor, crate::domain::now())?;
        if submit && self.receipt_printer_queue()?.as_deref() != Some(queue) {
            return Err(
                "Explicitly select and save this printer queue before sending a receipt.".into(),
            );
        }
        self.save_dispatch_change(
            &current,
            &next,
            &job,
            actor,
            "Saved physical receipt intent before contacting the printer",
        )?;
        self.prepared_or_existing(job, submit)
    }
    #[cfg(not(feature = "extended-workflows"))]
    pub fn retry_receipt_dispatch(
        &self,
        job_id: u64,
        key: &str,
    ) -> Result<PrepareDispatch, String> {
        self.local_receipt_operation(|actor| self.retry_receipt_dispatch_for(actor, job_id, key))
    }
    #[cfg(all(test, feature = "extended-workflows"))]
    pub fn retry_receipt_dispatch_as(
        &self,
        session: &Session,
        job_id: u64,
        key: &str,
    ) -> Result<PrepareDispatch, String> {
        self.authorized(session, Permission::Counter, |actor| {
            self.retry_receipt_dispatch_for(&actor, job_id, key)
        })
    }
    #[cfg(any(test, not(feature = "extended-workflows")))]
    fn retry_receipt_dispatch_for(
        &self,
        actor: &OperatorSnapshot,
        job_id: u64,
        key: &str,
    ) -> Result<PrepareDispatch, String> {
        let current = self.receipt_dispatch_log()?;
        let (next, job, submit) = current.retry(job_id, key, actor, crate::domain::now())?;
        self.save_dispatch_change(
            &current,
            &next,
            &job,
            actor,
            "Saved explicit retry of a proven unsubmitted physical receipt",
        )?;
        self.prepared_or_existing(job, submit)
    }
    #[cfg(not(feature = "extended-workflows"))]
    pub fn prepare_checked_receipt_copy(
        &self,
        prior_job_id: u64,
        prior_attempt_id: u32,
        key: &str,
        queue: &str,
        checked: bool,
    ) -> Result<PrepareDispatch, String> {
        self.local_receipt_operation(|actor| {
            self.prepare_checked_receipt_copy_for(
                actor,
                &CheckedCopyRequest {
                    prior_job_id,
                    prior_attempt_id,
                    key,
                    queue,
                    checked,
                },
            )
        })
    }
    #[cfg(all(test, feature = "extended-workflows"))]
    pub fn prepare_checked_receipt_copy_as(
        &self,
        session: &Session,
        prior_job_id: u64,
        prior_attempt_id: u32,
        key: &str,
        queue: &str,
        checked: bool,
    ) -> Result<PrepareDispatch, String> {
        self.authorized(session, Permission::Counter, |actor| {
            self.prepare_checked_receipt_copy_for(
                &actor,
                &CheckedCopyRequest {
                    prior_job_id,
                    prior_attempt_id,
                    key,
                    queue,
                    checked,
                },
            )
        })
    }
    #[cfg(any(test, not(feature = "extended-workflows")))]
    fn prepare_checked_receipt_copy_for(
        &self,
        actor: &OperatorSnapshot,
        request: &CheckedCopyRequest<'_>,
    ) -> Result<PrepareDispatch, String> {
        let current = self.receipt_dispatch_log()?;
        let prior = current
            .job(request.prior_job_id)
            .ok_or("Original physical receipt request not found.")?;
        let ticket = self
            .list()?
            .into_iter()
            .find(|ticket| ticket.id == prior.ticket_id)
            .ok_or("Original saved ticket not found.")?;
        let (next, job, submit) =
            current.prepare_checked_copy(&ticket, request, actor, crate::domain::now())?;
        if submit && self.receipt_printer_queue()?.as_deref() != Some(request.queue) {
            return Err(
                "Explicitly select and save this printer queue before sending a checked new copy."
                    .into(),
            );
        }
        self.save_dispatch_change(&current,&next,&job,actor,"Operator checked the printer queue and paper and explicitly authorized a linked new receipt copy")?;
        self.prepared_or_existing(job, submit)
    }
    #[cfg(any(test, not(feature = "extended-workflows")))]
    pub fn record_receipt_dispatch_result(
        &self,
        prepared: PreparedDispatch,
        outcome: DispatchOutcome,
    ) -> Result<DispatchJob, String> {
        if !Arc::ptr_eq(&self.dispatch_binding, &prepared.binding) {
            return Err("This printer result belongs to another register connection. Keep its saved request unresolved for review.".into());
        }
        self.transaction(|| {
            let current = self.receipt_dispatch_log()?;
            let job = current
                .job(prepared.job_id())
                .ok_or("Saved physical receipt request not found.")?;
            let mut expected = job.clone();
            let attempt = expected
                .attempts
                .last_mut()
                .ok_or("Saved receipt request has no submission intent.")?;
            if attempt.id != prepared.attempt_id {
                return Err("This printer result belongs to an earlier submission attempt.".into());
            }
            attempt.state = DispatchState::Dispatching;
            attempt.recorded_at = None;
            if expected != *prepared.job {
                return Err(
                    "This printer result does not match the saved receipt submission intent."
                        .into(),
                );
            }
            let actor = job
                .attempts
                .last()
                .ok_or("Saved receipt request has no operator attribution.")?
                .actor
                .clone();
            let (next, updated) =
                current.record(job.id, prepared.attempt_id, &outcome, crate::domain::now())?;
            self.save_dispatch_change(
                &current,
                &next,
                &updated,
                &actor,
                "Recorded physical receipt submission outcome without resending",
            )?;
            Ok(updated)
        })
    }
    #[cfg(any(test, not(feature = "extended-workflows")))]
    fn prepared_or_existing(
        &self,
        job: DispatchJob,
        submit: bool,
    ) -> Result<PrepareDispatch, String> {
        if !submit {
            return Ok(PrepareDispatch::Existing(job));
        }
        let attempt_id = job
            .attempts
            .last()
            .ok_or("Saved receipt request has no submission intent.")?
            .id;
        Ok(PrepareDispatch::Submit(PreparedDispatch {
            binding: Arc::clone(&self.dispatch_binding),
            job: Arc::new(job),
            attempt_id,
        }))
    }
    #[cfg(any(test, not(feature = "extended-workflows")))]
    fn save_dispatch_change(
        &self,
        current: &ReceiptDispatchLedger,
        next: &ReceiptDispatchLedger,
        job: &DispatchJob,
        actor: &OperatorSnapshot,
        detail: &str,
    ) -> Result<(), String> {
        if current != next {
            next.validate_against(&self.list()?)?;
            self.write_state("receipt_dispatches", next)?;
            self.append_audit(
                Some(actor),
                AuditAction::PrintJobSaved,
                &format!("physical-receipt:{}", job.id),
                detail,
            )?;
        }
        Ok(())
    }
}
