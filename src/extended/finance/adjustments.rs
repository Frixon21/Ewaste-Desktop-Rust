//! Append-only adjustment evidence. Persist a Pending intent before returning
//! money or recording a credit. This module never initiates an external refund.
use crate::{
    auth::{OperatorSnapshot, Role},
    domain::{Payment, State, Ticket, money},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_CENTS: i64 = 99_999_999_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdjustmentKind {
    Refund,
    Void,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdjustmentMethod {
    CashRefund,
    ExternalCardRefund,
    BillingCredit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdjustmentOutcome {
    Pending,
    Completed,
    Declined,
    Cancelled,
    Unknown,
}
impl AdjustmentOutcome {
    pub fn unresolved(self) -> bool {
        matches!(self, Self::Pending | Self::Unknown)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdjustmentAction {
    Requested {
        external_reference: String,
    },
    Result {
        outcome: AdjustmentOutcome,
        verified: bool,
        external_reference: String,
        reconcile: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjustmentEvent {
    pub id: u64,
    pub ticket_id: u64,
    pub attempt_id: u64,
    pub at: u64,
    pub actor: OperatorSnapshot,
    pub reason: String,
    pub request_key: String,
    pub kind: AdjustmentKind,
    pub amount_cents: i64,
    pub method: AdjustmentMethod,
    pub original_payment: Payment,
    pub original_account_id: Option<u64>,
    pub original_total_cents: i64,
    pub original_completed_at: u64,
    pub action: AdjustmentAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjustmentAttempt {
    pub id: u64,
    pub ticket_id: u64,
    pub kind: AdjustmentKind,
    pub amount_cents: i64,
    pub method: AdjustmentMethod,
    pub requested_at: u64,
    pub updated_at: u64,
    pub actor: OperatorSnapshot,
    pub reason: String,
    pub outcome: AdjustmentOutcome,
    pub verified: bool,
    pub external_reference: String,
    pub original_payment: Payment,
    pub original_account_id: Option<u64>,
    pub original_total_cents: i64,
    pub original_completed_at: u64,
    pub completed_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdjustmentLedger {
    pub events: Vec<AdjustmentEvent>,
    pub next_id: u64,
}
impl Default for AdjustmentLedger {
    fn default() -> Self {
        Self {
            events: vec![],
            next_id: 1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjustmentEffects {
    pub cash_refunds_cents: i64,
    pub card_refunds_cents: i64,
    pub billing_credits_cents: i64,
    pub unresolved_ticket_ids: Vec<u64>,
}

fn method(payment: &Payment) -> AdjustmentMethod {
    match payment {
        Payment::Cash => AdjustmentMethod::CashRefund,
        Payment::ExternalCard { .. } => AdjustmentMethod::ExternalCardRefund,
        Payment::ChargeTo { .. } => AdjustmentMethod::BillingCredit,
    }
}

fn audit_text(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) || value.chars().count() > 500 || value.trim().is_empty()
    {
        return Err(
            "Enter an adjustment reason of 1–500 characters without control characters.".into(),
        );
    }
    Ok(value.trim().to_owned())
}

fn key(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(
            "Use an adjustment request key of 1–128 letters, digits, '-', '_', '.' or ':'.".into(),
        );
    }
    Ok(())
}

fn reference(value: &str, external: bool) -> Result<String, String> {
    if !external {
        return if value.is_empty() {
            Ok(String::new())
        } else {
            Err("Cash refunds and billing credits do not use external references.".into())
        };
    }
    let trimmed = value.trim();
    if value.chars().any(char::is_control)
        || trimmed.is_empty()
        || trimmed.len() > 64
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(
            "Use a bounded synthetic external refund reference. Do not enter card data.".into(),
        );
    }
    let digits = trimmed.bytes().filter(u8::is_ascii_digit).count();
    if (12..=19).contains(&digits)
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_digit() || b"-_.".contains(&b))
    {
        return Err("Card numbers cannot be used as adjustment references.".into());
    }
    Ok(trimmed.to_owned())
}

fn original(ticket: &Ticket) -> Result<(Payment, i64, u64), String> {
    if ticket.id == 0 || ticket.state != State::Completed {
        return Err("Find the original saved completed ticket first.".into());
    }
    let total = ticket
        .total()
        .filter(|v| (1..=MAX_CENTS).contains(v))
        .ok_or("The original ticket has no verified positive amount.")?;
    let completed = ticket
        .completed_at
        .filter(|at| *at >= ticket.created_at)
        .ok_or("The original completion time needs review.")?;
    let payment = ticket
        .payment
        .clone()
        .ok_or("The original tender needs verification before adjustment.")?;
    match &payment {
        Payment::Cash => {
            ticket.tender_evidence.cash_for_total(total)?;
        }
        Payment::ExternalCard { reference } => {
            if ticket
                .tender_evidence
                .approved_card_for_total(total)?
                .reference
                != *reference
            {
                return Err(
                    "The saved sale reference does not match verified payment evidence.".into(),
                );
            }
        }
        Payment::ChargeTo { account } => {
            ticket.tender_evidence.guard_for_noncard()?;
            if ticket.account_record.as_ref().is_none_or(|a| {
                a.id == 0 || !a.active || a.number != *account || a.name.trim().is_empty()
            }) {
                return Err("The original business account snapshot needs verification.".into());
            }
        }
    }
    Ok((payment, total, completed))
}

fn total_for(attempts: &[AdjustmentAttempt], ticket_id: u64) -> Result<i64, String> {
    attempts
        .iter()
        .filter(|a| a.ticket_id == ticket_id && a.outcome == AdjustmentOutcome::Completed)
        .try_fold(0i64, |sum, a| {
            sum.checked_add(a.amount_cents)
                .ok_or_else(|| "Adjustment totals exceed the supported range.".into())
        })
}

fn unresolved(attempts: &[AdjustmentAttempt]) -> Vec<u64> {
    let mut ids: Vec<_> = attempts
        .iter()
        .filter(|a| a.outcome.unresolved())
        .map(|a| a.ticket_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn same_original(event: &AdjustmentEvent, attempt: &AdjustmentAttempt) -> bool {
    event.original_payment == attempt.original_payment
        && event.original_total_cents == attempt.original_total_cents
        && event.original_completed_at == attempt.original_completed_at
        && event.original_account_id == attempt.original_account_id
}

fn account_id(ticket: &Ticket) -> Option<u64> {
    if matches!(ticket.payment, Some(Payment::ChargeTo { .. })) {
        ticket.account_record.as_ref().map(|a| a.id)
    } else {
        None
    }
}

fn apply_requested(
    event: &AdjustmentEvent,
    external_reference: &str,
    attempts: &mut Vec<AdjustmentAttempt>,
) -> Result<(), String> {
    if event.attempt_id != event.id {
        return Err("An adjustment intent must own its original event ID.".into());
    }
    if attempts.iter().any(|a| {
        a.ticket_id == event.ticket_id && (!same_original(event, a) || a.outcome.unresolved())
    }) {
        return Err(
            "Resolve the existing adjustment or original tender evidence before another attempt."
                .into(),
        );
    }
    let remaining = event
        .original_total_cents
        .checked_sub(total_for(attempts, event.ticket_id)?)
        .ok_or("Invalid adjustment balance.")?;
    if event.amount_cents > remaining
        || (event.kind == AdjustmentKind::Void && event.amount_cents != remaining)
    {
        return Err("Refunds cannot exceed the remaining sale balance. A void must cover the entire remaining balance.".into());
    }
    let external = event.method == AdjustmentMethod::ExternalCardRefund;
    let reference = reference(external_reference, external)?;
    if reference != external_reference
        || (external
            && attempts
                .iter()
                .any(|a| a.external_reference.eq_ignore_ascii_case(&reference)))
    {
        return Err("That external adjustment reference is already reserved. Reconcile the original attempt.".into());
    }
    attempts.push(AdjustmentAttempt {
        id: event.id,
        ticket_id: event.ticket_id,
        kind: event.kind,
        amount_cents: event.amount_cents,
        method: event.method,
        requested_at: event.at,
        updated_at: event.at,
        actor: event.actor.clone(),
        reason: event.reason.clone(),
        outcome: AdjustmentOutcome::Pending,
        verified: false,
        external_reference: reference,
        original_payment: event.original_payment.clone(),
        original_total_cents: event.original_total_cents,
        original_account_id: event.original_account_id,
        original_completed_at: event.original_completed_at,
        completed_at: None,
    });
    Ok(())
}

fn apply_result(
    event: &AdjustmentEvent,
    outcome: AdjustmentOutcome,
    verified: bool,
    external_reference: &str,
    reconcile: bool,
    attempts: &mut [AdjustmentAttempt],
) -> Result<(), String> {
    let attempt = attempts
        .iter_mut()
        .find(|a| a.id == event.attempt_id && a.ticket_id == event.ticket_id)
        .ok_or("Adjustment intent not found.")?;
    if !same_original(event, attempt)
        || event.kind != attempt.kind
        || event.amount_cents != attempt.amount_cents
        || event.method != attempt.method
        || event.reason != attempt.reason
    {
        return Err("The adjustment result does not match its saved intent.".into());
    }
    if !attempt.outcome.unresolved() {
        return Err("This adjustment has a final result. Review its saved evidence instead of retrying a payout.".into());
    }
    if attempt.outcome == AdjustmentOutcome::Unknown && !reconcile {
        return Err("Reconcile the existing unknown result. Do not return money again.".into());
    }
    if reconcile && (!verified || outcome.unresolved()) {
        return Err(
            "Reconciliation requires a verified final result for the existing attempt.".into(),
        );
    }
    if !outcome.unresolved() && !verified {
        return Err("Verify every final adjustment result, including a decline or cancellation, before releasing the saved payout reservation.".into());
    }
    if outcome.unresolved() && verified {
        return Err("Pending or unknown cannot be recorded as a verified final result.".into());
    }
    if event.method != AdjustmentMethod::ExternalCardRefund
        && outcome == AdjustmentOutcome::Declined
    {
        return Err("Use Cancelled for an unperformed cash refund or billing credit.".into());
    }
    let checked = reference(
        external_reference,
        event.method == AdjustmentMethod::ExternalCardRefund,
    )?;
    if checked != external_reference || checked != attempt.external_reference {
        return Err(
            "Reconcile using the saved adjustment reference, not a different transaction.".into(),
        );
    }
    *attempt = AdjustmentAttempt {
        outcome,
        verified,
        updated_at: event.at,
        actor: event.actor.clone(),
        completed_at: (outcome == AdjustmentOutcome::Completed).then_some(event.at),
        ..attempt.clone()
    };
    Ok(())
}

fn apply(event: &AdjustmentEvent, attempts: &mut Vec<AdjustmentAttempt>) -> Result<(), String> {
    match &event.action {
        AdjustmentAction::Requested { external_reference } => {
            apply_requested(event, external_reference, attempts)
        }
        AdjustmentAction::Result {
            outcome,
            verified,
            external_reference,
            reconcile,
        } => apply_result(
            event,
            *outcome,
            *verified,
            external_reference,
            *reconcile,
            attempts,
        ),
    }
}

impl AdjustmentLedger {
    fn project(&self) -> Result<Vec<AdjustmentAttempt>, String> {
        let mut attempts = Vec::new();
        let mut keys = HashSet::new();
        let mut times = HashMap::new();
        let mut next = 1u64;
        for event in &self.events {
            event.actor.validate()?;
            if !matches!(event.actor.role, Role::Supervisor | Role::Administrator) {
                return Err(
                    "A supervisor or administrator must authorize financial adjustments.".into(),
                );
            }
            key(&event.request_key)?;
            if audit_text(&event.reason)? != event.reason
                || event.id != next
                || event.ticket_id == 0
                || !keys.insert(&event.request_key)
                || !(1..=MAX_CENTS).contains(&event.amount_cents)
                || !(1..=MAX_CENTS).contains(&event.original_total_cents)
                || event.at < event.original_completed_at
                || event.method != method(&event.original_payment)
                || match event.method {
                    AdjustmentMethod::BillingCredit => {
                        event.original_account_id.is_none_or(|id| id == 0)
                    }
                    _ => event.original_account_id.is_some(),
                }
            {
                return Err("The adjustment ledger contains inconsistent identity, amount, or audit evidence.".into());
            }
            if times
                .insert(event.ticket_id, event.at)
                .is_some_and(|old| event.at < old)
            {
                return Err("Adjustment history must remain chronological for each ticket.".into());
            }
            apply(event, &mut attempts)?;
            next = next
                .checked_add(1)
                .ok_or("Adjustment event sequence exhausted.")?;
        }
        if self.next_id != next {
            return Err("The adjustment event sequence needs review.".into());
        }
        Ok(attempts)
    }

    fn append_or_retry(&self, event: AdjustmentEvent) -> Result<(Self, AdjustmentAttempt), String> {
        let before = self.project()?;
        if let Some(saved) = self
            .events
            .iter()
            .find(|old| old.request_key == event.request_key)
        {
            let attempt_id = if matches!(event.action, AdjustmentAction::Requested { .. }) {
                saved.attempt_id
            } else {
                event.attempt_id
            };
            let retry = AdjustmentEvent {
                id: saved.id,
                attempt_id,
                at: saved.at,
                ..event
            };
            if &retry != saved {
                return Err("That adjustment request key belongs to a different operation.".into());
            }
            let attempt = before
                .into_iter()
                .find(|a| a.id == saved.attempt_id)
                .ok_or("Saved adjustment intent not found.")?;
            return Ok((self.clone(), attempt));
        }
        let mut events = self.events.clone();
        events.push(event.clone());
        let next = Self {
            events,
            next_id: self
                .next_id
                .checked_add(1)
                .ok_or("Adjustment event sequence exhausted.")?,
        };
        let attempt = next
            .project()?
            .into_iter()
            .find(|a| a.id == event.attempt_id)
            .ok_or("Adjustment intent not found.")?;
        Ok((next, attempt))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        ticket: &Ticket,
        kind: AdjustmentKind,
        amount_cents: i64,
        actor: &OperatorSnapshot,
        reason: &str,
        request_key: &str,
        at: u64,
        external_reference: &str,
    ) -> Result<(Self, AdjustmentAttempt), String> {
        self.project()?;
        let (payment, total, completed) = original(ticket)?;
        let event = AdjustmentEvent {
            id: self.next_id,
            attempt_id: self.next_id,
            ticket_id: ticket.id,
            at,
            actor: actor.clone(),
            reason: audit_text(reason)?,
            request_key: request_key.into(),
            kind,
            amount_cents,
            method: method(&payment),
            original_payment: payment.clone(),
            original_total_cents: total,
            original_completed_at: completed,
            original_account_id: account_id(ticket),
            action: AdjustmentAction::Requested {
                external_reference: reference(
                    external_reference,
                    method(&payment) == AdjustmentMethod::ExternalCardRefund,
                )?,
            },
        };
        self.append_or_retry(event)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_result(
        &self,
        ticket: &Ticket,
        attempt_id: u64,
        outcome: AdjustmentOutcome,
        verified: bool,
        external_reference: &str,
        actor: &OperatorSnapshot,
        request_key: &str,
        at: u64,
        reconcile: bool,
    ) -> Result<(Self, AdjustmentAttempt), String> {
        let attempts = self.project()?;
        let attempt = attempts
            .iter()
            .find(|a| a.id == attempt_id && a.ticket_id == ticket.id)
            .ok_or("Find the saved adjustment intent first.")?;
        let (payment, total, completed) = original(ticket)?;
        let event = AdjustmentEvent {
            id: self.next_id,
            attempt_id,
            ticket_id: ticket.id,
            at,
            actor: actor.clone(),
            reason: attempt.reason.clone(),
            request_key: request_key.into(),
            kind: attempt.kind,
            amount_cents: attempt.amount_cents,
            method: method(&payment),
            original_payment: payment,
            original_total_cents: total,
            original_completed_at: completed,
            original_account_id: account_id(ticket),
            action: AdjustmentAction::Result {
                outcome,
                verified,
                external_reference: reference(
                    external_reference,
                    attempt.method == AdjustmentMethod::ExternalCardRefund,
                )?,
                reconcile,
            },
        };
        self.append_or_retry(event)
    }

    pub fn attempts(&self, ticket_id: u64) -> Result<Vec<AdjustmentAttempt>, String> {
        Ok(self
            .project()?
            .into_iter()
            .filter(|a| a.ticket_id == ticket_id)
            .collect())
    }
    pub fn total_adjusted(&self, ticket_id: u64) -> Result<i64, String> {
        total_for(&self.project()?, ticket_id)
    }
    pub fn unresolved_ticket_ids(&self) -> Result<Vec<u64>, String> {
        Ok(unresolved(&self.project()?))
    }

    pub fn validate_against(&self, tickets: &[Ticket]) -> Result<(), String> {
        self.project()?;
        let mut originals = HashMap::new();
        for ticket in tickets {
            if originals.insert(ticket.id, ticket).is_some() {
                return Err("Duplicate original ticket IDs cannot be reconciled.".into());
            }
        }
        for event in &self.events {
            let ticket = originals
                .get(&event.ticket_id)
                .ok_or("The adjustment ledger refers to a missing original ticket.")?;
            let (payment, total, completed) = original(ticket)?;
            if payment != event.original_payment
                || total != event.original_total_cents
                || completed != event.original_completed_at
                || account_id(ticket) != event.original_account_id
            {
                return Err("The original sale amount, completion time, or payment reference changed. Review its saved evidence.".into());
            }
        }
        Ok(())
    }

    pub fn financial_effects(&self, start: u64, end: u64) -> Result<AdjustmentEffects, String> {
        if start >= end {
            return Err("Choose a nonempty shift interval with an exclusive end.".into());
        }
        self.project()?;
        let mut attempts = Vec::new();
        for event in self.events.iter().filter(|e| e.at < end) {
            apply(event, &mut attempts)?;
        }
        let mut result = AdjustmentEffects {
            unresolved_ticket_ids: unresolved(&attempts),
            ..Default::default()
        };
        for attempt in attempts
            .iter()
            .filter(|a| a.completed_at.is_some_and(|at| at >= start && at < end))
        {
            let bucket = match attempt.method {
                AdjustmentMethod::CashRefund => &mut result.cash_refunds_cents,
                AdjustmentMethod::ExternalCardRefund => &mut result.card_refunds_cents,
                AdjustmentMethod::BillingCredit => &mut result.billing_credits_cents,
            };
            *bucket = bucket
                .checked_add(attempt.amount_cents)
                .ok_or("Shift adjustment totals exceed the supported range.")?;
        }
        Ok(result)
    }

    pub fn receipt(&self, attempt: &AdjustmentAttempt, ticket: &Ticket) -> Result<String, String> {
        self.project()?;
        let (payment, total, completed) = original(ticket)?;
        if attempt.ticket_id != ticket.id
            || payment != attempt.original_payment
            || total != attempt.original_total_cents
            || completed != attempt.original_completed_at
            || account_id(ticket) != attempt.original_account_id
        {
            return Err(
                "The saved adjustment does not match the original financial ticket.".into(),
            );
        }
        if !self.attempts(ticket.id)?.contains(attempt) {
            return Err("Open the saved adjustment before printing its voucher.".into());
        }
        let billing = if attempt.method == AdjustmentMethod::BillingCredit
            && attempt.outcome == AdjustmentOutcome::Completed
        {
            "\nPending billing handoff"
        } else {
            ""
        };
        Ok(format!(
            "E-WASTE ADJUSTMENT\nOriginal ticket #{:06}\nAdjustment #{} / {:?}\nMethod: {:?}\nAmount: {}\nOutcome: {:?}{}\nOperator: {}\nReason: {}\nExternal reference: {}\n",
            ticket.id,
            attempt.id,
            attempt.kind,
            attempt.method,
            money(attempt.amount_cents),
            attempt.outcome,
            billing,
            attempt.actor.display_name,
            attempt.reason,
            attempt.external_reference
        ))
    }
}

#[cfg(test)]
#[path = "../../tests/extended/finance/adjustments_tests.rs"]
mod tests;
