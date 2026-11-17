//! Append-only documentary reviews of frozen training tickets, never claim approval.

use crate::{
    domain::{State, Ticket},
    intake::{MaterialIntake, SourceIntake},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineIdentity {
    Stable(u64),
    LegacyOrdinal(usize),
}

impl LineIdentity {
    /// A zero-ID legacy line can only be referenced by its immutable completed
    /// ticket position. New lines use never-reused stable IDs.
    pub fn from_ticket(ticket: &Ticket, index: usize) -> Result<Self, String> {
        if ticket.state != State::Completed {
            return Err("Complete and save the ticket before documentary review.".into());
        }
        let line = ticket.lines.get(index).ok_or("Material line not found.")?;
        Ok(if line.id == 0 {
            Self::LegacyOrdinal(index)
        } else {
            Self::Stable(line.id)
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewDisposition {
    #[default]
    PendingReview,
    /// Documentary completeness only; never a legal/program eligibility decision.
    Reviewed,
    NotForClaim,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewedMaterial {
    pub identity: LineIdentity,
    pub details: MaterialIntake,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSnapshot {
    pub source: SourceIntake,
    pub materials: Vec<ReviewedMaterial>,
    pub disposition: ReviewDisposition,
}

impl ReviewSnapshot {
    /// List documentary issues even while an existing Reviewed form is being
    /// corrected. The editable preview never changes its financial ticket.
    pub fn issues(&self, ticket: &Ticket) -> Result<Vec<crate::intake::IntakeIssue>, String> {
        let preview = Self {
            disposition: ReviewDisposition::PendingReview,
            ..self.clone()
        }
        .normalized_for(ticket)?;
        let mut candidate = ticket.clone();
        candidate.source_intake = preview.source;
        for (line, material) in candidate.lines.iter_mut().zip(preview.materials) {
            line.material_intake = material.details;
        }
        Ok(candidate.intake_issues())
    }

    pub fn from_ticket(ticket: &Ticket) -> Result<Self, String> {
        let materials = ticket
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                Ok(ReviewedMaterial {
                    identity: LineIdentity::from_ticket(ticket, index)?,
                    details: line.material_intake.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Self {
            source: ticket.source_intake.clone(),
            materials,
            disposition: ReviewDisposition::PendingReview,
        }
        .normalized_for(ticket)
    }

    fn normalized_fields(&self) -> Result<Self, String> {
        let mut identities = HashSet::new();
        let materials = self
            .materials
            .iter()
            .map(|material| {
                if material.identity == LineIdentity::Stable(0)
                    || !identities.insert(material.identity)
                {
                    return Err(
                        "Documentary review contains an invalid or duplicate material identity."
                            .into(),
                    );
                }
                Ok(ReviewedMaterial {
                    identity: material.identity,
                    details: material.details.normalized()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            source: self.source.normalized()?,
            materials,
            disposition: self.disposition,
        })
    }

    /// Validate identity membership against frozen lines and normalize materials
    /// into their original ticket order. Only documentary fields enter the clone.
    pub fn normalized_for(&self, ticket: &Ticket) -> Result<Self, String> {
        if ticket.state != State::Completed || ticket.id == 0 || ticket.lines.is_empty() {
            return Err(
                "A saved completed ticket with material lines is required for documentary review."
                    .into(),
            );
        }
        ticket.validate_intake_records()?;
        let fields = self.normalized_fields()?;
        if fields.materials.len() != ticket.lines.len() {
            return Err("Review every frozen material line exactly once.".into());
        }
        let mut materials = Vec::with_capacity(ticket.lines.len());
        let mut candidate = ticket.clone();
        candidate.source_intake = fields.source.clone();
        for (index, line) in candidate.lines.iter_mut().enumerate() {
            let identity = LineIdentity::from_ticket(ticket, index)?;
            let reviewed=fields.materials.iter().find(|m|m.identity==identity)
                .ok_or("Documentary review refers to a different material line. Reopen the saved ticket.")?;
            line.material_intake = reviewed.details.clone();
            materials.push(reviewed.clone());
        }
        if fields.disposition == ReviewDisposition::Reviewed
            && !candidate.intake_issues().is_empty()
        {
            return Err("Resolve every documentary issue before marking this snapshot Reviewed. This does not establish claim eligibility.".into());
        }
        Ok(Self {
            materials,
            ..fields
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEvent {
    #[serde(default)]
    pub actor: Option<crate::auth::OperatorSnapshot>,
    pub id: u64,
    pub ticket_id: u64,
    pub at: u64,
    pub operator: String,
    pub reason: String,
    pub request_key: String,
    pub snapshot: ReviewSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewLedger {
    pub events: Vec<ReviewEvent>,
    pub next_id: u64,
}

impl Default for ReviewLedger {
    fn default() -> Self {
        Self {
            events: vec![],
            next_id: 1,
        }
    }
}

fn audit_text(input: &str, label: &str, max: usize) -> Result<String, String> {
    if input.trim().is_empty() || input.chars().any(char::is_control) || input.chars().count() > max
    {
        return Err(format!(
            "Enter {label} using at most {max} characters without control characters."
        ));
    }
    Ok(input.trim().to_owned())
}

fn request_key(input: &str) -> Result<String, String> {
    if input.is_empty()
        || input.len() > 128
        || !input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(
            "Use a unique review request key of at most 128 letters, digits, '-', '_', '.' or ':'."
                .into(),
        );
    }
    Ok(input.to_owned())
}

impl ReviewLedger {
    pub fn latest(&self, ticket_id: u64) -> Option<&ReviewEvent> {
        self.events
            .iter()
            .rev()
            .find(|event| event.ticket_id == ticket_id)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.next_id == 0 {
            return Err("Review ledger has an invalid next event ID.".into());
        }
        let mut keys = HashSet::new();
        let mut times = HashMap::new();
        let mut previous_id = 0;
        for event in &self.events {
            if let Some(actor) = &event.actor {
                actor.validate()?;
                if actor.display_name != event.operator {
                    return Err("Review attribution does not match its authenticated actor.".into());
                }
            }
            if event.id <= previous_id
                || event.id >= self.next_id
                || event.ticket_id == 0
                || !keys.insert(&event.request_key)
            {
                return Err("Review history contains invalid event IDs, ticket numbers or repeated request keys.".into());
            }
            if audit_text(&event.operator, "operator", 120)? != event.operator
                || audit_text(&event.reason, "review reason", 500)? != event.reason
            {
                return Err("Saved review attribution must be normalized.".into());
            }
            request_key(&event.request_key)?;
            if event.snapshot.normalized_fields()? != event.snapshot
                || event.snapshot.materials.is_empty()
            {
                return Err("Saved review snapshot is empty or not normalized.".into());
            }
            if times
                .insert(event.ticket_id, event.at)
                .is_some_and(|previous| previous > event.at)
            {
                return Err("Review timestamps must not move backwards for a ticket.".into());
            }
            previous_id = event.id;
        }
        Ok(())
    }

    /// Use when restoring persisted history: field checks alone cannot establish
    /// that an event references the right frozen ticket or a complete document.
    pub fn validate_against(&self, tickets: &[Ticket]) -> Result<(), String> {
        self.validate()?;
        let mut ids = HashSet::new();
        if tickets.iter().any(|ticket| !ids.insert(ticket.id)) {
            return Err("Duplicate ticket numbers in documentary review input.".into());
        }
        for event in &self.events {
            let ticket = tickets
                .iter()
                .find(|ticket| ticket.id == event.ticket_id)
                .ok_or("A documentary review references a missing ticket.")?;
            if event.snapshot.normalized_for(ticket)? != event.snapshot
                || event.at < ticket.completed_at.unwrap_or(ticket.created_at)
            {
                return Err("A saved documentary review does not match its frozen ticket or completion time.".into());
            }
        }
        Ok(())
    }

    /// Idempotent retries preserve the original event time. A fresh key appends
    /// history; callers must persist append-only rather than replace past events.
    pub fn record(
        &self,
        ticket: &Ticket,
        input: &ReviewSnapshot,
        operator: &str,
        reason: &str,
        key: &str,
        at: u64,
    ) -> Result<(Self, ReviewEvent), String> {
        self.validate()?;
        let snapshot = input.normalized_for(ticket)?;
        let operator = audit_text(operator, "operator", 120)?;
        let reason = audit_text(reason, "review reason", 500)?;
        let key = request_key(key)?;
        if let Some(event) = self.events.iter().find(|event| event.request_key == key) {
            return if event.ticket_id == ticket.id
                && event.snapshot == snapshot
                && event.operator == operator
                && event.reason == reason
            {
                Ok((self.clone(), event.clone()))
            } else {
                Err("This review request key is already attached to different evidence or attribution.".into())
            };
        }
        if at < ticket.completed_at.unwrap_or(ticket.created_at)
            || self.latest(ticket.id).is_some_and(|event| event.at > at)
        {
            return Err(
                "A documentary review cannot predate completion or the ticket's previous review."
                    .into(),
            );
        }
        let id = self.next_id;
        let next_id = id
            .checked_add(1)
            .ok_or("Documentary review event ID limit reached.")?;
        let event = ReviewEvent {
            actor: None,
            id,
            ticket_id: ticket.id,
            at,
            operator,
            reason,
            request_key: key,
            snapshot,
        };
        let mut events = self.events.clone();
        events.push(event.clone());
        Ok((Self { events, next_id }, event))
    }
}

#[cfg(test)]
#[path = "../../tests/extended/intake/intake_review_tests.rs"]
mod tests;
