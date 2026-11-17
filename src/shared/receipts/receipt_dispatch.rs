//! Durable intent and outcomes for physical receipt submission. Acceptance means
//! the Windows spooler accepted a job; it does not prove paper was produced.
#[cfg(not(feature = "extended-workflows"))]
pub use crate::store::{PrepareDispatch, PreparedDispatch};
use crate::{
    domain::{State, Ticket},
    operator_snapshot::OperatorSnapshot,
    printing::CopyKind,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchState {
    Dispatching,
    Accepted { windows_job_id: u32 },
    NotSubmitted { detail: String },
    Unknown { detail: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(test, not(feature = "extended-workflows")))]
pub enum DispatchOutcome {
    Accepted { windows_job_id: u32 },
    NotSubmitted { detail: String },
    Unknown { detail: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchAttempt {
    pub id: u32,
    pub request_key: String,
    pub actor: OperatorSnapshot,
    pub requested_at: u64,
    pub recorded_at: Option<u64>,
    pub state: DispatchState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchJob {
    pub id: u64,
    pub ticket_id: u64,
    pub copy: CopyKind,
    pub receipt: String,
    pub queue: String,
    pub actor: OperatorSnapshot,
    pub request_key: String,
    pub attempts: Vec<DispatchAttempt>,
    pub source_digest: String,
    #[serde(default)]
    pub checked_copy: Option<Box<CheckedCopyLink>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedCopyLink {
    pub prior_job_id: u64,
    pub prior_attempt_id: u32,
    pub checked_by: OperatorSnapshot,
    pub checked_at: u64,
    pub queue_and_paper_checked: bool,
}
impl DispatchJob {
    pub fn state(&self) -> &DispatchState {
        &self
            .attempts
            .last()
            .expect("validated dispatch job has an attempt")
            .state
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptDispatchLedger {
    pub jobs: Vec<DispatchJob>,
    pub next_id: u64,
}
impl Default for ReceiptDispatchLedger {
    fn default() -> Self {
        Self {
            jobs: vec![],
            next_id: 1,
        }
    }
}
impl ReceiptDispatchLedger {
    #[cfg(any(test, not(feature = "extended-workflows")))]
    pub fn job(&self, id: u64) -> Option<&DispatchJob> {
        self.jobs.iter().find(|job| job.id == id)
    }
    fn validate(&self) -> Result<(), String> {
        let mut keys = std::collections::HashSet::new();
        if self.next_id
            != u64::try_from(self.jobs.len())
                .map_err(|_| "Receipt dispatch limit reached.")?
                .checked_add(1)
                .ok_or("Receipt dispatch limit reached.")?
        {
            return Err("Saved receipt dispatch identifiers are inconsistent.".into());
        }
        for (index, job) in self.jobs.iter().enumerate() {
            if job.id != index as u64 + 1 || job.ticket_id == 0 || job.attempts.is_empty() {
                return Err("A saved receipt dispatch is missing its identity or intent.".into());
            }
            validate_queue(&job.queue)?;
            validate_receipt(&job.receipt)?;
            let heading = match job.copy {
                CopyKind::Customer => "CUSTOMER RECEIPT\n",
                CopyKind::Internal => "INTERNAL COPY\n",
            };
            if job
                .receipt
                .strip_prefix(heading)
                .is_none_or(|body| body.trim().is_empty())
                || job.source_digest.len() != 64
                || !job
                    .source_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err("Saved physical receipt header or source binding is invalid.".into());
            }
            job.actor.validate()?;
            if job.request_key != job.attempts[0].request_key || job.actor != job.attempts[0].actor
            {
                return Err(
                    "Saved receipt dispatch attribution does not match its initial attempt.".into(),
                );
            }
            for (ordinal, attempt) in job.attempts.iter().enumerate() {
                validate_key(&attempt.request_key)?;
                attempt.actor.validate()?;
                validate_state(&attempt.state)?;
                if attempt.id as usize != ordinal + 1
                    || !keys.insert(attempt.request_key.as_str())
                    || (attempt.state == DispatchState::Dispatching)
                        != attempt.recorded_at.is_none()
                    || attempt
                        .recorded_at
                        .is_some_and(|at| at < attempt.requested_at)
                {
                    return Err("Saved receipt submission attempt history is inconsistent.".into());
                }
                if ordinal > 0 {
                    let prior = &job.attempts[ordinal - 1];
                    if !matches!(prior.state, DispatchState::NotSubmitted { .. })
                        || prior.recorded_at.is_none_or(|at| attempt.requested_at < at)
                    {
                        return Err(
                            "Only proven unsubmitted receipts can have a later attempt.".into()
                        );
                    }
                }
            }
            let prior = self.jobs[..index]
                .iter()
                .rev()
                .find(|prior| prior.ticket_id == job.ticket_id && prior.copy == job.copy);
            recovery::validate_predecessor(job, prior)?;
        }
        Ok(())
    }

    pub fn validate_against(&self, tickets: &[Ticket]) -> Result<(), String> {
        self.validate()?;
        for job in &self.jobs {
            let ticket = tickets
                .iter()
                .find(|ticket| ticket.id == job.ticket_id)
                .ok_or("A physical receipt refers to a missing saved ticket.")?;
            validate_ticket(ticket)?;
            if job.source_digest != binding::source_digest(ticket, job)?
                || job.attempts[0].requested_at < ticket.completed_at.unwrap_or(0)
            {
                return Err(
                    "A saved physical receipt differs from its completed ticket snapshot.".into(),
                );
            }
        }
        Ok(())
    }

    #[cfg(any(test, not(feature = "extended-workflows")))]
    pub fn prepare(
        &self,
        ticket: &Ticket,
        copy: CopyKind,
        key: &str,
        queue: &str,
        actor: &OperatorSnapshot,
        at: u64,
    ) -> Result<(Self, DispatchJob, bool), String> {
        self.validate()?;
        validate_ticket(ticket)?;
        validate_key(key)?;
        validate_queue(queue)?;
        actor.validate()?;
        if let Some(job) = self.jobs.iter().find(|job| {
            job.attempts
                .iter()
                .any(|attempt| attempt.request_key == key)
        }) {
            if job.request_key != key
                || job.ticket_id != ticket.id
                || job.copy != copy
                || job.queue != queue
                || job.source_digest != binding::source_digest(ticket, job)?
            {
                return Err(
                    "This request key belongs to another receipt or submission attempt.".into(),
                );
            }
            return Ok((self.clone(), job.clone(), false));
        }
        let receipt = receipt_body(ticket, copy);
        validate_receipt(&receipt)?;
        if self
            .jobs
            .iter()
            .rev()
            .find(|job| job.ticket_id == ticket.id && job.copy == copy)
            .is_some_and(|job| !matches!(job.state(), DispatchState::Accepted { .. }))
        {
            return Err("This copy has an unresolved or unsubmitted request. Check the existing request; do not send another copy.".into());
        }
        if at < ticket.completed_at.unwrap_or(0) {
            return Err("Receipt dispatch cannot precede ticket completion.".into());
        }
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Receipt dispatch limit reached.")?;
        let mut job = DispatchJob {
            id: self.next_id,
            ticket_id: ticket.id,
            copy,
            receipt,
            queue: queue.into(),
            actor: actor.clone(),
            request_key: key.into(),
            attempts: vec![DispatchAttempt {
                id: 1,
                request_key: key.into(),
                actor: actor.clone(),
                requested_at: at,
                recorded_at: None,
                state: DispatchState::Dispatching,
            }],
            checked_copy: None,
            source_digest: String::new(),
        };
        job.source_digest = binding::source_digest(ticket, &job)?;
        let mut next = self.clone();
        next.jobs.push(job.clone());
        next.next_id = next_id;
        next.validate()?;
        Ok((next, job, true))
    }

    #[cfg(any(test, not(feature = "extended-workflows")))]
    pub fn retry(
        &self,
        id: u64,
        key: &str,
        actor: &OperatorSnapshot,
        at: u64,
    ) -> Result<(Self, DispatchJob, bool), String> {
        self.validate()?;
        validate_key(key)?;
        actor.validate()?;
        let job = self.job(id).ok_or("Physical receipt request not found.")?;
        if let Some(existing) = self.jobs.iter().find(|job| {
            job.attempts
                .iter()
                .any(|attempt| attempt.request_key == key)
        }) {
            return if existing.id == id {
                Ok((self.clone(), existing.clone(), false))
            } else {
                Err("This retry key belongs to another receipt request.".into())
            };
        }
        if self.jobs.iter().any(|later| {
            later.id > job.id && later.ticket_id == job.ticket_id && later.copy == job.copy
        }) {
            return Err("This receipt request was superseded by a checked new copy. Review the latest request; do not retry the old one.".into());
        }
        let latest = job
            .attempts
            .last()
            .ok_or("Receipt request has no saved intent.")?;
        if !matches!(latest.state, DispatchState::NotSubmitted { .. }) {
            return Err("Retry is allowed only when the previous attempt was proven not submitted. Dispatching and unknown results require a printer check.".into());
        }
        if latest.recorded_at.is_none_or(|recorded| at < recorded) {
            return Err("The retry time precedes the saved result.".into());
        }
        let mut updated = job.clone();
        updated.attempts.push(DispatchAttempt {
            id: latest
                .id
                .checked_add(1)
                .ok_or("Receipt attempt limit reached.")?,
            request_key: key.into(),
            actor: actor.clone(),
            requested_at: at,
            recorded_at: None,
            state: DispatchState::Dispatching,
        });
        let next = self.replace(updated.clone());
        next.validate()?;
        Ok((next, updated, true))
    }

    #[cfg(any(test, not(feature = "extended-workflows")))]
    pub fn record(
        &self,
        id: u64,
        attempt: u32,
        outcome: &DispatchOutcome,
        at: u64,
    ) -> Result<(Self, DispatchJob), String> {
        self.validate()?;
        let state = outcome.state();
        validate_state(&state)?;
        let job = self.job(id).ok_or("Physical receipt request not found.")?;
        let saved = job
            .attempts
            .last()
            .ok_or("Receipt request has no saved intent.")?;
        if saved.id != attempt {
            return Err("This result belongs to an earlier receipt attempt.".into());
        }
        if saved.state != DispatchState::Dispatching {
            return if saved.state == state {
                Ok((self.clone(), job.clone()))
            } else {
                Err("The saved receipt outcome cannot be changed or resubmitted.".into())
            };
        }
        if at < saved.requested_at {
            return Err("Receipt result time precedes its saved intent.".into());
        }
        let mut updated = job.clone();
        let result = updated
            .attempts
            .last_mut()
            .ok_or("Receipt request has no saved intent.")?;
        result.state = state;
        result.recorded_at = Some(at);
        let next = self.replace(updated.clone());
        next.validate()?;
        Ok((next, updated))
    }

    #[cfg(any(test, not(feature = "extended-workflows")))]
    fn replace(&self, updated: DispatchJob) -> Self {
        Self {
            jobs: self
                .jobs
                .iter()
                .map(|job| {
                    if job.id == updated.id {
                        updated.clone()
                    } else {
                        job.clone()
                    }
                })
                .collect(),
            next_id: self.next_id,
        }
    }
}

#[cfg(any(test, not(feature = "extended-workflows")))]
impl DispatchOutcome {
    fn state(&self) -> DispatchState {
        match self {
            Self::Accepted { windows_job_id } => DispatchState::Accepted {
                windows_job_id: *windows_job_id,
            },
            Self::NotSubmitted { detail } => DispatchState::NotSubmitted {
                detail: detail.clone(),
            },
            Self::Unknown { detail } => DispatchState::Unknown {
                detail: detail.clone(),
            },
        }
    }
}
pub(crate) fn validate_queue(queue: &str) -> Result<(), String> {
    if queue.trim().is_empty()
        || queue.trim() != queue
        || queue.chars().count() > 240
        || queue.chars().any(char::is_control)
    {
        return Err(
            "Select a printer queue with at most 240 characters and no control characters.".into(),
        );
    }
    Ok(())
}
fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(
            "Receipt request keys require 1 to 128 letters, digits, '-', '_', '.' or ':'.".into(),
        );
    }
    Ok(())
}
fn validate_receipt(receipt: &str) -> Result<(), String> {
    if receipt.trim().is_empty()
        || receipt.len() > 65_536
        || receipt
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("The saved receipt body is invalid or too long.".into());
    }
    Ok(())
}
fn validate_state(state: &DispatchState) -> Result<(), String> {
    match state {
        DispatchState::Accepted { windows_job_id: 0 } => {
            return Err("Spooler acceptance requires a positive Windows job identifier.".into());
        }
        DispatchState::Unknown { detail } | DispatchState::NotSubmitted { detail }
            if detail.trim().is_empty()
                || detail.trim() != detail
                || detail.chars().count() > 500
                || detail.chars().any(char::is_control) =>
        {
            return Err(
                "Receipt result details require 1 to 500 characters without control characters."
                    .into(),
            );
        }
        _ => (),
    }
    Ok(())
}
fn validate_ticket(ticket: &Ticket) -> Result<(), String> {
    if ticket.id == 0
        || ticket.state != State::Completed
        || ticket.completed_at.is_some_and(|at| at < ticket.created_at)
    {
        return Err("Physical receipts require an already completed saved ticket.".into());
    }
    // Printing saved history neither establishes payment nor changes it. New
    // financial completion remains guarded by the authenticated counter service.
    Ok(())
}
#[cfg(any(test, not(feature = "extended-workflows")))]
fn receipt_body(ticket: &Ticket, copy: CopyKind) -> String {
    format!(
        "{}\n{}",
        match copy {
            CopyKind::Customer => "CUSTOMER RECEIPT",
            CopyKind::Internal => "INTERNAL COPY",
        },
        ticket.receipt()
    )
}
#[cfg(test)]
#[path = "../../tests/shared/receipt_dispatch_tests.rs"]
mod tests;

#[path = "receipt_dispatch_binding.rs"]
mod binding;
#[path = "receipt_dispatch_recovery.rs"]
mod recovery;
#[cfg(any(test, not(feature = "extended-workflows")))]
pub(crate) use recovery::CheckedCopyRequest;

#[cfg(test)]
#[path = "../../tests/shared/receipt_dispatch_compat_tests.rs"]
mod compatibility_tests;
