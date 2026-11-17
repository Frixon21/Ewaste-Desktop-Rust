//! Reasoned pre-tender line totals. Catalog arithmetic remains frozen; only the
//! separately authorized effective amount changes. This ledger moves no money.
use crate::{
    auth::{OperatorSnapshot, Role},
    catalog::PricingRule,
    domain::{Line, State, Ticket, rule_line},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub use crate::record_compat::PriceApproval;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceBasis {
    pub code: String,
    pub name: String,
    pub quantity: String,
    pub unit: Option<String>,
    pub gross_thousandths: Option<i64>,
    pub tare_thousandths: Option<i64>,
    pub rule_snapshot: PricingRule,
    pub rate_cents: Option<i64>,
    pub base_cents: i64,
    pub minimum_cents: i64,
    pub total_cents: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceSnapshot {
    pub basis: PriceBasis,
    pub approval: Option<PriceApproval>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PriceOverrideAction {
    Apply { approved_total_cents: i64 },
    Clear,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceOverrideEvent {
    pub id: u64,
    pub request_key: String,
    pub ticket_id: u64,
    pub line_id: u64,
    pub ticket_revision: u64,
    pub at: u64,
    pub actor: OperatorSnapshot,
    pub reason: String,
    pub before: PriceSnapshot,
    pub after: PriceSnapshot,
    pub action: PriceOverrideAction,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PriceOverrideLedger {
    pub events: Vec<PriceOverrideEvent>,
    pub next_id: u64,
}
impl Default for PriceOverrideLedger {
    fn default() -> Self {
        Self {
            events: vec![],
            next_id: 1,
        }
    }
}

impl PriceBasis {
    pub fn from_line(line: &Line) -> Result<Self, String> {
        let basis = Self {
            code: line.code.clone(),
            name: line.name.clone(),
            quantity: line.quantity.clone(),
            unit: line.unit.clone(),
            gross_thousandths: line.gross_thousandths,
            tare_thousandths: line.tare_thousandths,
            rule_snapshot: line
                .rule_snapshot
                .clone()
                .ok_or("A saved pricing rule is required before price approval.")?,
            rate_cents: line.rate_cents,
            base_cents: line.base_cents.ok_or("Resolve the baseline price first.")?,
            minimum_cents: line
                .minimum_cents
                .ok_or("Resolve the baseline price first.")?,
            total_cents: line
                .total_cents
                .ok_or("Resolve the baseline price first.")?,
        };
        basis.validate()?;
        Ok(basis)
    }
    pub fn validate(&self) -> Result<(), String> {
        if [self.base_cents, self.minimum_cents, self.total_cents]
            .iter()
            .any(|value| *value < 0)
            || self.base_cents.checked_add(self.minimum_cents) != Some(self.total_cents)
        {
            return Err("The original base, minimum and line total do not reconcile.".into());
        }
        let manual = if self.rule_snapshot.rate_cents.is_none() {
            format!("{}.{:02}", self.total_cents / 100, self.total_cents % 100)
        } else {
            String::new()
        };
        let expected = rule_line(&self.rule_snapshot, &self.quantity, &manual)?;
        if self.code != expected.code
            || self.name != expected.name
            || self.quantity != expected.quantity
            || self.unit != expected.unit
            || self.rate_cents != expected.rate_cents
            || Some(self.base_cents) != expected.base_cents
            || Some(self.minimum_cents) != expected.minimum_cents
            || Some(self.total_cents) != expected.total_cents
        {
            return Err("The original line does not match its frozen pricing rule.".into());
        }
        match (self.gross_thousandths, self.tare_thousandths) {
            (None, None) => (),
            (Some(gross), Some(tare))
                if self.unit.as_deref() == Some("lb")
                    && gross > tare
                    && tare >= 0
                    && gross <= 999_999_999 =>
            {
                if crate::domain::decimal(&self.quantity, 3, "Review the original net pounds.")?
                    != gross - tare
                {
                    return Err("The original gross, tare and net pounds do not agree.".into());
                }
            }
            _ => return Err("Record both original gross and tare, or leave both unknown.".into()),
        }
        Ok(())
    }
}
impl PriceSnapshot {
    pub fn from_line(line: &Line) -> Result<Self, String> {
        if let Some(approval) = &line.price_approval {
            approval.validate()?;
        }
        Ok(Self {
            basis: PriceBasis::from_line(line)?,
            approval: line.price_approval.clone(),
        })
    }
    pub fn effective_total_cents(&self) -> i64 {
        self.approval
            .as_ref()
            .map_or(self.basis.total_cents, |a| a.approved_total_cents)
    }
}

fn reason(value: &str) -> Result<String, String> {
    if value.trim().is_empty() || value.chars().count() > 500 || value.chars().any(char::is_control)
    {
        return Err(
            "Enter a price-approval reason of 1–500 characters without control characters.".into(),
        );
    }
    Ok(value.trim().to_owned())
}
fn actor_valid(actor: &OperatorSnapshot) -> Result<(), String> {
    actor.validate()?;
    if !matches!(actor.role, Role::Supervisor | Role::Administrator) {
        return Err("A supervisor or administrator must approve line prices.".into());
    }
    Ok(())
}
fn key_valid(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(
            "Use a unique price-approval key of 1–128 letters, digits, '-', '_', '.' or ':'."
                .into(),
        );
    }
    Ok(())
}

type Latest = HashMap<(u64, u64), PriceOverrideEvent>;

impl PriceOverrideLedger {
    fn project(&self) -> Result<Latest, String> {
        let mut latest: Latest = HashMap::new();
        let mut keys = HashSet::new();
        let mut ticket_clocks: HashMap<u64, (u64, u64)> = HashMap::new();
        let mut next = 1u64;
        let mut last_at = 0u64;
        for event in &self.events {
            actor_valid(&event.actor)?;
            key_valid(&event.request_key)?;
            event.before.basis.validate()?;
            event.after.basis.validate()?;
            if event.id != next
                || event.ticket_id == 0
                || event.line_id == 0
                || event.ticket_revision == u64::MAX
                || !keys.insert(&event.request_key)
                || reason(&event.reason)? != event.reason
                || event.before.basis != event.after.basis
            {
                return Err("Price-approval history contains inconsistent identity, sequence or baseline data.".into());
            }
            if event.at < last_at {
                return Err(
                    "Price-approval timestamps cannot move backwards across the register.".into(),
                );
            }
            last_at = event.at;
            if ticket_clocks
                .insert(event.ticket_id, (event.at, event.ticket_revision))
                .is_some_and(|(at, revision)| event.at < at || event.ticket_revision <= revision)
            {
                return Err("Price-approval history must advance ticket revisions and retain chronological timestamps.".into());
            }
            let old = latest.get(&(event.ticket_id, event.line_id));
            let expected_before = old.and_then(|old| old.after.approval.as_ref());
            if event.before.approval.as_ref() != expected_before
                || old.is_some_and(|old| {
                    old.after.approval.is_some() && old.after.basis != event.before.basis
                })
            {
                return Err("Price-approval history replaced an active authority or its original measurement.".into());
            }
            let expected_after = match event.action {
                PriceOverrideAction::Apply {
                    approved_total_cents,
                } => Some(PriceApproval {
                    event_id: event.id,
                    approved_total_cents,
                }),
                PriceOverrideAction::Clear => {
                    if expected_before.is_none() {
                        return Err("Only an active saved price approval can be cleared.".into());
                    }
                    None
                }
            };
            if let Some(approval) = &expected_after {
                approval.validate()?;
            }
            if event.after.approval != expected_after {
                return Err(
                    "A price-approval snapshot does not match its authorized action.".into(),
                );
            }
            latest.insert((event.ticket_id, event.line_id), event.clone());
            next = next
                .checked_add(1)
                .ok_or("Price-approval event sequence exhausted.")?;
        }
        if self.next_id != next {
            return Err("The price-approval event sequence needs review.".into());
        }
        Ok(latest)
    }

    fn validate_ticket(&self, ticket: &Ticket, latest: &Latest) -> Result<(), String> {
        ticket.validate_intake_records()?;
        for event in self
            .events
            .iter()
            .filter(|event| event.ticket_id == ticket.id)
        {
            if event.at < ticket.created_at
                || event.line_id >= ticket.next_line_id
                || ticket.revision <= event.ticket_revision
                || (ticket.state == State::Completed
                    && ticket.completed_at.is_none_or(|at| event.at > at))
            {
                return Err("Saved price approval does not belong to this ticket's revision, line sequence or chronology.".into());
            }
        }
        for line in &ticket.lines {
            let event = latest.get(&(ticket.id, line.id));
            let expected = event.and_then(|event| event.after.approval.as_ref());
            if line.price_approval.as_ref() != expected {
                return Err("The line price approval is missing, stripped, superseded or attached to another line.".into());
            }
            if expected.is_some()
                && (line.id == 0
                    || Some(PriceBasis::from_line(line)?)
                        != event.map(|event| event.after.basis.clone()))
            {
                return Err("The approved line's original price or measurement changed. Explicitly clear approval before correction.".into());
            }
        }
        Ok(())
    }

    pub fn validate_against(&self, tickets: &[Ticket]) -> Result<(), String> {
        let latest = self.project()?;
        let mut ids = HashSet::new();
        for ticket in tickets {
            if ticket.id == 0 || !ids.insert(ticket.id) {
                return Err("Price-approval input has invalid or duplicate ticket IDs.".into());
            }
            self.validate_ticket(ticket, &latest)?;
        }
        if self
            .events
            .iter()
            .any(|event| !ids.contains(&event.ticket_id))
        {
            return Err("Price-approval history refers to a missing original ticket.".into());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn approve(
        &self,
        ticket: &Ticket,
        line_id: u64,
        amount: i64,
        actor: &OperatorSnapshot,
        reason: &str,
        key: &str,
        at: u64,
    ) -> Result<(Self, Ticket), String> {
        self.change(
            ticket,
            line_id,
            PriceOverrideAction::Apply {
                approved_total_cents: amount,
            },
            actor,
            reason,
            key,
            at,
        )
    }
    pub fn clear(
        &self,
        ticket: &Ticket,
        line_id: u64,
        actor: &OperatorSnapshot,
        reason: &str,
        key: &str,
        at: u64,
    ) -> Result<(Self, Ticket), String> {
        self.change(
            ticket,
            line_id,
            PriceOverrideAction::Clear,
            actor,
            reason,
            key,
            at,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn change(
        &self,
        ticket: &Ticket,
        line_id: u64,
        action: PriceOverrideAction,
        actor: &OperatorSnapshot,
        explanation: &str,
        key: &str,
        at: u64,
    ) -> Result<(Self, Ticket), String> {
        let latest = self.project()?;
        actor_valid(actor)?;
        key_valid(key)?;
        let explanation = reason(explanation)?;
        self.validate_ticket(ticket, &latest)?;
        if let Some(event) = self.events.iter().find(|event| event.request_key == key) {
            if event.ticket_id != ticket.id
                || event.line_id != line_id
                || event.action != action
                || event.actor != *actor
                || event.reason != explanation
            {
                return Err(
                    "That price-approval request key belongs to a different operation.".into(),
                );
            }
            return Ok((self.clone(), ticket.clone()));
        }
        ticket.editable_intake()?;
        if ticket.tender_evidence.cash.is_some() || !ticket.tender_evidence.card_attempts.is_empty()
        {
            return Err("Price approval is available only before any tender attempt. Preserve the payment history and resolve it first.".into());
        }
        let position = ticket.lines.iter().position(|line| line.id == line_id && line_id > 0).ok_or("Select a saved material line with a stable ID. Re-add legacy zero-ID lines before price approval.")?;
        let before = PriceSnapshot::from_line(&ticket.lines[position])?;
        let approval = match action {
            PriceOverrideAction::Apply {
                approved_total_cents,
            } => Some(PriceApproval {
                event_id: self.next_id,
                approved_total_cents,
            }),
            PriceOverrideAction::Clear => {
                if before.approval.is_none() {
                    return Err("This line has no active price approval to clear.".into());
                }
                None
            }
        };
        if let Some(approval) = &approval {
            approval.validate()?;
        }
        let after = PriceSnapshot {
            basis: before.basis.clone(),
            approval: approval.clone(),
        };
        let event = PriceOverrideEvent {
            id: self.next_id,
            request_key: key.into(),
            ticket_id: ticket.id,
            line_id,
            ticket_revision: ticket.revision,
            at,
            actor: actor.clone(),
            reason: explanation,
            before,
            after,
            action,
        };
        let mut events = self.events.clone();
        events.push(event);
        let next = Self {
            events,
            next_id: self
                .next_id
                .checked_add(1)
                .ok_or("Price-approval event sequence exhausted.")?,
        };
        let mut candidate = ticket.clone();
        candidate.lines[position].price_approval = approval;
        candidate.revision = ticket
            .revision
            .checked_add(1)
            .ok_or("Ticket revision sequence exhausted.")?;
        next.validate_ticket(&candidate, &next.project()?)?;
        Ok((next, candidate))
    }
}

/// Retained lines cannot gain, lose or change price authority through an ordinary
/// counter save. Deleting a whole line is allowed; the separate ledger survives.
pub fn validate_ordinary_candidate(
    previous: Option<&Ticket>,
    candidate: &Ticket,
) -> Result<(), String> {
    for line in &candidate.lines {
        if let Some(approval) = &line.price_approval {
            approval.validate()?;
        }
        let old = previous.and_then(|ticket| {
            ticket
                .lines
                .iter()
                .find(|old| old.id == line.id && line.id > 0)
        });
        match old {
            Some(old) => {
                if old.price_approval != line.price_approval {
                    return Err(
                        "Use the dedicated supervisor command to apply or clear price approval."
                            .into(),
                    );
                }
                if old.price_approval.is_some()
                    && PriceBasis::from_line(old)? != PriceBasis::from_line(line)?
                {
                    return Err("Explicitly clear price approval before changing billed quantity, measurement, unit or pricing.".into());
                }
            }
            None if line.price_approval.is_some() => {
                return Err(
                    "A new or legacy material line cannot carry price authority from another line."
                        .into(),
                );
            }
            None if line.id > 0 && previous.is_some_and(|ticket| line.id < ticket.next_line_id) => {
                return Err("A removed stable line ID cannot be reused.".into());
            }
            None => (),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/extended/pricing/price_overrides_tests.rs"]
mod tests;
