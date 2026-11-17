//! Application audit history is append-only through register Store commands.
//! This is not cryptographically tamper-proof; Windows file permissions remain
//! necessary. Callers must never put credentials, hashes or session tokens here.

use crate::operator_snapshot::OperatorSnapshot;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditAction {
    BootstrapAdministrator,
    SignIn,
    SignInFailed,
    SignOut,
    OperatorCreated,
    OperatorUpdated,
    PasswordReset,
    TicketSaved,
    CustomerDirectorySaved,
    BusinessDirectorySaved,
    TariffsSaved,
    IntakeReviewSaved,
    CloseoutSaved,
    PrintJobSaved,
    RecordsExported,
    BackupCreated,
    AdjustmentRecorded,
    PriceOverrideRecorded,
    InventoryDirectorySaved,
    InventoryRecorded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: u64,
    pub at: u64,
    pub actor: Option<OperatorSnapshot>,
    pub action: AuditAction,
    pub subject: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditLedger {
    pub events: Vec<AuditEvent>,
    pub next_id: u64,
}

impl Default for AuditLedger {
    fn default() -> Self {
        Self {
            events: vec![],
            next_id: 1,
        }
    }
}

fn text(input: &str, label: &str, limit: usize, required: bool) -> Result<String, String> {
    if input.chars().count() > limit || input.chars().any(char::is_control) {
        return Err(format!(
            "Audit {label} must have at most {limit} characters and no control characters."
        ));
    }
    let result = input.trim();
    if required && result.is_empty() {
        return Err(format!("An audit {label} is required."));
    }
    Ok(result.to_owned())
}

fn attribution(actor: Option<&OperatorSnapshot>, action: AuditAction) -> Result<(), String> {
    match actor {
        Some(actor) => actor.validate(),
        None if action == AuditAction::SignInFailed => Ok(()),
        None => Err("This audit action requires register attribution.".into()),
    }
}

impl AuditLedger {
    /// Recheck persisted event order and attribution without interpreting a
    /// historical snapshot as authority for any new action.
    pub fn validate(&self) -> Result<(), String> {
        let expected = u64::try_from(self.events.len())
            .ok()
            .and_then(|len| len.checked_add(1))
            .ok_or("Audit event identifier limit reached.")?;
        if self.next_id != expected {
            return Err("Audit history has an inconsistent next event identifier.".into());
        }
        let mut previous_at = None;
        for (index, event) in self.events.iter().enumerate() {
            if event.id != index as u64 + 1 {
                return Err("Audit event identifiers must be contiguous and append-only.".into());
            }
            attribution(event.actor.as_ref(), event.action)?;
            if text(&event.subject, "subject", 120, true)? != event.subject
                || text(&event.detail, "detail", 500, false)? != event.detail
            {
                return Err("Saved audit text must be normalized.".into());
            }
            if previous_at.is_some_and(|previous| event.at < previous) {
                return Err("The system clock moved backwards relative to audit history. Correct the clock before continuing.".into());
            }
            previous_at = Some(event.at);
        }
        Ok(())
    }

    /// Append once after a semantic state change. This helper is intentionally
    /// not idempotent: the Store transaction owns retry detection.
    pub fn append(
        &self,
        actor: Option<&OperatorSnapshot>,
        action: AuditAction,
        subject: &str,
        detail: &str,
        at: u64,
    ) -> Result<Self, String> {
        self.validate()?;
        attribution(actor, action)?;
        if self.events.last().is_some_and(|event| at < event.at) {
            return Err("The system clock moved backwards relative to audit history. Correct the clock before continuing.".into());
        }
        let subject = text(subject, "subject", 120, true)?;
        let detail = text(detail, "detail", 500, false)?;
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Audit event identifier limit reached.")?;
        let event = AuditEvent {
            id: self.next_id,
            at,
            actor: actor.cloned(),
            action,
            subject,
            detail,
        };
        let mut events = self.events.clone();
        events.push(event);
        Ok(Self { events, next_id })
    }
}

#[cfg(test)]
#[path = "../tests/store/security_audit_tests.rs"]
mod tests;
