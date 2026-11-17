use super::*;
use sha2::{Digest, Sha256};

/// Consistency binding, not a signature or external audit. Retain the body that
/// was actually prepared, even when another build renders historical text differently.
pub(super) fn source_digest(ticket: &Ticket, job: &DispatchJob) -> Result<String, String> {
    let mut snapshot = serde_json::to_value(ticket)
        .map_err(|_| "Could not bind the saved ticket to its receipt.")?;
    let fields = snapshot
        .as_object_mut()
        .ok_or("The saved ticket snapshot is invalid.")?;
    fields.remove("revision");
    fields.remove("reprints");
    let bytes = serde_json::to_vec(&(
        snapshot,
        job.copy,
        &job.receipt,
        &job.queue,
        &job.request_key,
        &job.actor,
        &job.checked_copy,
        job.attempts.first().map(|attempt| attempt.requested_at),
    ))
    .map_err(|_| "Could not bind the frozen receipt submission fields.")?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
