//! A synthetic receipt spool. It records saved receipt snapshots and retry state;
//! it does not submit Windows printer jobs or change tickets/payment evidence.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CopyKind {
    Customer,
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrintState {
    Failed,
    Succeeded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrintJob {
    pub id: u64,
    pub request_key: String,
    pub ticket_id: u64,
    pub copy: CopyKind,
    pub receipt: String,
    pub state: PrintState,
    pub attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintLog {
    pub jobs: Vec<PrintJob>,
    pub next_id: u64,
}

impl Default for PrintLog {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
        }
    }
}

fn validate_request(ticket_id: u64, receipt: &str, request_key: &str) -> Result<(), String> {
    if ticket_id == 0 {
        return Err("A saved ticket number is required to print.".into());
    }
    if receipt.trim().is_empty()
        || receipt.len() > 65_536
        || receipt
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err("A valid saved receipt snapshot is required to print.".into());
    }
    if request_key.is_empty()
        || request_key.len() > 128
        || !request_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(
            "Use a unique print request key of at most 128 letters, digits, '-', '_', '.' or ':'."
                .into(),
        );
    }
    Ok(())
}

impl PrintLog {
    /// Validate persisted spool metadata before accepting or changing it.
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for job in &self.jobs {
            validate_request(job.ticket_id, &job.receipt, &job.request_key)?;
            if job.id == 0
                || job.id >= self.next_id
                || job.attempts == 0
                || !ids.insert(job.id)
                || !keys.insert(job.request_key.as_str())
            {
                return Err(
                    "Print log has inconsistent job identifiers or attempt history.".into(),
                );
            }
        }
        if self.next_id == 0 {
            return Err("Print log has an invalid next job identifier.".into());
        }
        Ok(())
    }

    /// Request a synthetic copy of an already saved ticket. Keep request_key for
    /// retries; a new deliberate paper copy must have a fresh key. Repeated
    /// successful requests are harmless, and failed requests retry the same job.
    pub fn request(
        &self,
        ticket_id: u64,
        copy: CopyKind,
        receipt: &str,
        request_key: &str,
        simulate_failure: bool,
    ) -> Result<(Self, PrintJob), String> {
        self.validate()?;
        validate_request(ticket_id, receipt, request_key)?;
        if let Some(job) = self.jobs.iter().find(|job| job.request_key == request_key) {
            if job.ticket_id != ticket_id || job.copy != copy || job.receipt != receipt {
                return Err("This print request belongs to a different saved receipt.".into());
            }
            return self.retry(job.id, simulate_failure);
        }
        let max_existing = self.jobs.iter().map(|job| job.id).max().unwrap_or(0);
        let id = self.next_id.max(
            max_existing
                .checked_add(1)
                .ok_or("Print job limit reached.")?,
        );
        let next_id = id.checked_add(1).ok_or("Print job limit reached.")?;
        let job = PrintJob {
            id,
            request_key: request_key.to_owned(),
            ticket_id,
            copy,
            receipt: receipt.to_owned(),
            state: if simulate_failure {
                PrintState::Failed
            } else {
                PrintState::Succeeded
            },
            attempts: 1,
        };
        let mut jobs = self.jobs.clone();
        jobs.push(job.clone());
        Ok((Self { jobs, next_id }, job))
    }

    /// Retry a failed job using its original receipt. Retrying a successful job
    /// returns it without another attempt or an additional successful-copy count.
    pub fn retry(&self, job_id: u64, simulate_failure: bool) -> Result<(Self, PrintJob), String> {
        self.validate()?;
        let job = self.job(job_id).ok_or("Print job not found.")?;
        if job.state == PrintState::Succeeded {
            return Ok((self.clone(), job.clone()));
        }
        let attempts = job
            .attempts
            .checked_add(1)
            .ok_or("Print retry limit reached.")?;
        let updated = PrintJob {
            attempts,
            state: if simulate_failure {
                PrintState::Failed
            } else {
                PrintState::Succeeded
            },
            ..job.clone()
        };
        let jobs = self
            .jobs
            .iter()
            .map(|job| {
                if job.id == job_id {
                    updated.clone()
                } else {
                    job.clone()
                }
            })
            .collect();
        Ok((
            Self {
                jobs,
                next_id: self.next_id,
            },
            updated,
        ))
    }

    /// Read-only preview: viewing a job has no printing or audit side effects.
    pub fn job(&self, job_id: u64) -> Option<&PrintJob> {
        self.jobs.iter().find(|job| job.id == job_id)
    }

    /// Only successful internal copies contribute to the reprint tally.
    pub fn successful_internal_copies(&self, ticket_id: u64) -> usize {
        self.jobs
            .iter()
            .filter(|job| {
                job.ticket_id == ticket_id
                    && job.copy == CopyKind::Internal
                    && job.state == PrintState::Succeeded
            })
            .count()
    }
}

#[cfg(test)]
#[path = "../../tests/shared/printing_tests.rs"]
mod tests;
