use super::*;

#[cfg(any(test, not(feature = "extended-workflows")))]
pub(crate) struct CheckedCopyRequest<'a> {
    pub prior_job_id: u64,
    pub prior_attempt_id: u32,
    pub key: &'a str,
    pub queue: &'a str,
    pub checked: bool,
}

impl ReceiptDispatchLedger {
    #[cfg(any(test, not(feature = "extended-workflows")))]
    pub fn prepare_checked_copy(
        &self,
        ticket: &Ticket,
        request: &CheckedCopyRequest<'_>,
        actor: &OperatorSnapshot,
        at: u64,
    ) -> Result<(Self, DispatchJob, bool), String> {
        let CheckedCopyRequest {
            prior_job_id,
            prior_attempt_id,
            key,
            queue,
            checked,
        } = *request;
        self.validate()?;
        validate_ticket(ticket)?;
        validate_key(key)?;
        validate_queue(queue)?;
        actor.validate()?;
        if !checked {
            return Err("Check the printer queue and paper, then explicitly confirm another copy may be sent.".into());
        }
        let prior = self
            .job(prior_job_id)
            .ok_or("Original physical receipt request not found.")?;
        if prior.ticket_id != ticket.id
            || prior.source_digest != binding::source_digest(ticket, prior)?
        {
            return Err("The checked request does not match this saved ticket receipt.".into());
        }
        if let Some(existing) = self.jobs.iter().find(|job| {
            job.attempts
                .iter()
                .any(|attempt| attempt.request_key == key)
        }) {
            if existing.request_key == key
                && existing.ticket_id == ticket.id
                && existing.copy == prior.copy
                && existing.queue == queue
                && existing.receipt == prior.receipt
                && existing.checked_copy.as_ref().is_some_and(|link| {
                    link.prior_job_id == prior_job_id && link.prior_attempt_id == prior_attempt_id
                })
            {
                return Ok((self.clone(), existing.clone(), false));
            }
            return Err("This checked-copy key belongs to another receipt request.".into());
        }
        let latest = self
            .jobs
            .iter()
            .rev()
            .find(|job| job.ticket_id == ticket.id && job.copy == prior.copy)
            .ok_or("Original physical receipt request not found.")?;
        let attempt = prior
            .attempts
            .last()
            .ok_or("Original receipt request has no saved attempt.")?;
        if latest.id != prior_job_id || attempt.id != prior_attempt_id {
            return Err("This request was superseded or its attempt changed. Review the latest receipt request.".into());
        }
        if matches!(attempt.state, DispatchState::Accepted { .. }) {
            return Err("This request was accepted by the spooler. Use an ordinary deliberate copy request if another copy is needed.".into());
        }
        if at < attempt.requested_at || attempt.recorded_at.is_some_and(|recorded| at < recorded) {
            return Err("The checked-copy time precedes its original request or result.".into());
        }
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Receipt dispatch limit reached.")?;
        let mut job = DispatchJob {
            id: self.next_id,
            ticket_id: ticket.id,
            copy: prior.copy,
            receipt: prior.receipt.clone(),
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
            checked_copy: Some(Box::new(CheckedCopyLink {
                prior_job_id,
                prior_attempt_id,
                checked_by: actor.clone(),
                checked_at: at,
                queue_and_paper_checked: true,
            })),
            source_digest: String::new(),
        };
        job.source_digest = binding::source_digest(ticket, &job)?;
        let mut next = self.clone();
        next.jobs.push(job.clone());
        next.next_id = next_id;
        next.validate()?;
        Ok((next, job, true))
    }
}

pub(super) fn validate_predecessor(
    job: &DispatchJob,
    prior: Option<&DispatchJob>,
) -> Result<(), String> {
    let first = job
        .attempts
        .first()
        .ok_or("Receipt request has no saved intent.")?;
    if let Some(link) = &job.checked_copy {
        link.checked_by.validate()?;
        let prior =
            prior.ok_or("A checked copy has no preceding request for the same ticket and copy.")?;
        let attempt = prior
            .attempts
            .last()
            .ok_or("The checked original request has no saved attempt.")?;
        if !link.queue_and_paper_checked
            || link.prior_job_id != prior.id
            || link.prior_attempt_id != attempt.id
            || link.checked_by != first.actor
            || link.checked_at != first.requested_at
            || link.checked_at < attempt.requested_at
        {
            return Err("Saved checked-copy evidence does not match its operator, time or original request identity.".into());
        }
        // The prior worker may report its original result after this check. Its
        // identity stays fixed; requiring its current state would lose that result.
    } else if let Some(prior) = prior
        && (!matches!(prior.state(), DispatchState::Accepted { .. })
            || prior
                .attempts
                .last()
                .and_then(|attempt| attempt.recorded_at)
                .is_none_or(|at| first.requested_at < at))
    {
        return Err("Saved receipt requests bypass an unresolved or unsubmitted copy.".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/shared/receipt_dispatch_recovery_tests.rs"]
mod tests;
