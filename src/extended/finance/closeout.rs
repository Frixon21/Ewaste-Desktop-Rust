//! Synthetic shift reconciliation. UTC windows are explicit; no accounting is sent.

use crate::adjustments::{AdjustmentAction, AdjustmentLedger, AdjustmentOutcome};
use crate::domain::{Payment, State, Ticket};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_CENTS: i64 = 99_999_999_999;

fn first_calculation_version() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftReport {
    #[serde(default = "first_calculation_version")]
    pub calculation_version: u32,
    pub start: u64,
    pub end: u64,
    pub completed_tickets: u64,
    pub cash_sales_cents: i64,
    pub cash_received_cents: i64,
    pub cash_change_cents: i64,
    pub verified_card_cents: i64,
    pub pending_billing_cents: i64,
    pub unverified_completed_cents: i64,
    pub open_drafts: u64,
    pub held_tickets: u64,
    pub unpriced_tickets: u64,
    pub unresolved_card_ticket_ids: Vec<u64>,
    #[serde(default)]
    pub paid_draft_ticket_ids: Vec<u64>,
    pub review_ticket_ids: Vec<u64>,
    #[serde(default)]
    pub cash_refunds_cents: i64,
    #[serde(default)]
    pub card_refunds_cents: i64,
    #[serde(default)]
    pub billing_credits_cents: i64,
    #[serde(default)]
    pub adjustment_event_ids: Vec<u64>,
    #[serde(default)]
    pub unresolved_adjustment_ticket_ids: Vec<u64>,
}

impl Default for ShiftReport {
    fn default() -> Self {
        Self {
            calculation_version: 1,
            start: 0,
            end: 0,
            completed_tickets: 0,
            cash_sales_cents: 0,
            cash_received_cents: 0,
            cash_change_cents: 0,
            verified_card_cents: 0,
            pending_billing_cents: 0,
            unverified_completed_cents: 0,
            open_drafts: 0,
            held_tickets: 0,
            unpriced_tickets: 0,
            unresolved_card_ticket_ids: vec![],
            paid_draft_ticket_ids: vec![],
            review_ticket_ids: vec![],
            cash_refunds_cents: 0,
            card_refunds_cents: 0,
            billing_credits_cents: 0,
            adjustment_event_ids: vec![],
            unresolved_adjustment_ticket_ids: vec![],
        }
    }
}

/// Finalized local snapshot. Persist append-only by its stable window ID; a
/// later discrepancy correction must not overwrite the original declaration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closeout {
    #[serde(default)]
    pub actor: Option<crate::auth::OperatorSnapshot>,
    pub id: String,
    pub operator: String,
    pub reason: String,
    pub opening_cash_cents: i64,
    pub counted_cash_cents: i64,
    pub external_card_cents: i64,
    #[serde(default)]
    pub external_card_refunds_cents: i64,
    pub expected_cash_cents: i64,
    pub expected_card_cents: i64,
    /// Counted minus expected: negative means cash short.
    pub cash_variance_cents: i64,
    /// External reported card total minus verified ticket card total.
    pub card_variance_cents: i64,
    pub report: ShiftReport,
}

fn money_value(cents: i64) -> Result<i64, String> {
    if !(0..=MAX_CENTS).contains(&cents) {
        return Err("Use a nonnegative amount within the supported closeout limit.".into());
    }
    Ok(cents)
}

fn add_money(total: &mut i64, value: i64) -> Result<(), String> {
    money_value(value)?;
    *total = money_value(
        total
            .checked_add(value)
            .ok_or("Shift amount limit reached.")?,
    )?;
    Ok(())
}

fn text(value: &str, label: &str, max: usize, required: bool) -> Result<String, String> {
    if value.chars().any(char::is_control) || value.chars().count() > max {
        return Err(format!(
            "{label} must have at most {max} characters and no control characters."
        ));
    }
    let result = value.trim();
    if required && result.is_empty() {
        return Err(format!("Enter {label}."));
    }
    Ok(result.to_owned())
}

impl ShiftReport {
    /// Version two keeps gross sales and separately dated adjustment movements.
    /// Pending backlog is reconstructed as of the cutoff, so later resolution
    /// cannot silently change an earlier report or move a refund to its sale date.
    pub fn from_tickets_and_adjustments(
        tickets: &[Ticket],
        ledger: &AdjustmentLedger,
        start: u64,
        end: u64,
    ) -> Result<Self, String> {
        ledger.validate_against(tickets)?;
        let gross = Self::from_tickets(tickets, start, end)?;
        let effects = ledger.financial_effects(start, end)?;
        let mut settled = ledger
            .events
            .iter()
            .filter(|event| {
                event.at >= start
                    && event.at < end
                    && matches!(
                        &event.action,
                        AdjustmentAction::Result {
                            outcome: AdjustmentOutcome::Completed,
                            ..
                        }
                    )
            })
            .map(|event| event.id)
            .collect::<Vec<_>>();
        settled.sort_unstable();
        let report = Self {
            calculation_version: 2,
            cash_refunds_cents: effects.cash_refunds_cents,
            card_refunds_cents: effects.card_refunds_cents,
            billing_credits_cents: effects.billing_credits_cents,
            adjustment_event_ids: settled,
            unresolved_adjustment_ticket_ids: effects.unresolved_ticket_ids,
            ..gross
        };
        report.validate()?;
        Ok(report)
    }

    /// Completed tickets use their completion UTC timestamp within [start, end).
    /// Open/held work created before end carries forward regardless of start, so
    /// an older draft's new pending card attempt cannot disappear at shift change.
    /// Legacy completions without a completion time use creation time and require review.
    pub fn from_tickets(tickets: &[Ticket], start: u64, end: u64) -> Result<Self, String> {
        if start >= end {
            return Err("Shift end must be after shift start (UTC seconds).".into());
        }
        let mut ids = HashSet::new();
        let mut report = Self {
            start,
            end,
            ..Self::default()
        };
        for ticket in tickets {
            if ticket.id == 0 || !ids.insert(ticket.id) {
                return Err("Shift input contains an invalid or duplicate ticket number.".into());
            }
            let timestamp = if ticket.state == State::Completed {
                ticket.completed_at.unwrap_or(ticket.created_at)
            } else {
                ticket.created_at
            };
            if timestamp >= end || (ticket.state == State::Completed && timestamp < start) {
                continue;
            }
            if ticket.total().is_none() {
                report.unpriced_tickets += 1;
            }
            if ticket
                .tender_evidence
                .card_attempts
                .iter()
                .any(|a| a.outcome.unresolved())
            {
                report.unresolved_card_ticket_ids.push(ticket.id);
            }
            if ticket.state != State::Completed
                && (ticket.tender_evidence.cash.is_some()
                    || ticket
                        .tender_evidence
                        .card_attempts
                        .iter()
                        .any(|a| a.outcome == crate::tender::CardOutcome::Approved))
            {
                report.paid_draft_ticket_ids.push(ticket.id);
            }
            match ticket.state {
                State::Draft => report.open_drafts += 1,
                State::Held => report.held_tickets += 1,
                State::Completed => report.include_completed(ticket)?,
            }
        }
        report.unresolved_card_ticket_ids.sort_unstable();
        report.paid_draft_ticket_ids.sort_unstable();
        report.review_ticket_ids.sort_unstable();
        Ok(report)
    }

    fn include_completed(&mut self, ticket: &Ticket) -> Result<(), String> {
        self.completed_tickets += 1;
        let total = match ticket.total() {
            Some(total) => money_value(total)?,
            None => {
                self.review_ticket_ids.push(ticket.id);
                return Ok(());
            }
        };
        let verified = ticket.completed_at.is_some()
            && !ticket.lines.is_empty()
            && match &ticket.payment {
                Some(Payment::Cash) => ticket.tender_evidence.cash_for_total(total).is_ok(),
                Some(Payment::ExternalCard { reference }) => ticket
                    .tender_evidence
                    .approved_card_for_total(total)
                    .is_ok_and(|a| a.reference == *reference),
                Some(Payment::ChargeTo { account }) => {
                    ticket.tender_evidence.guard_for_noncard().is_ok()
                        && ticket
                            .account_record
                            .as_ref()
                            .is_some_and(|a| a.id != 0 && a.active && a.number == *account)
                }
                None => false,
            };
        if !verified {
            self.review_ticket_ids.push(ticket.id);
            return add_money(&mut self.unverified_completed_cents, total);
        }
        match &ticket.payment {
            Some(Payment::Cash) => {
                let cash = ticket.tender_evidence.cash_for_total(total)?;
                add_money(&mut self.cash_sales_cents, total)?;
                add_money(&mut self.cash_received_cents, cash.received_cents)?;
                add_money(&mut self.cash_change_cents, cash.change_cents)
            }
            Some(Payment::ExternalCard { .. }) => add_money(&mut self.verified_card_cents, total),
            Some(Payment::ChargeTo { .. }) => add_money(&mut self.pending_billing_cents, total),
            None => Err("Completed ticket has no tender disposition.".into()),
        }
    }

    pub fn reconcile_movements(
        &self,
        opening: i64,
        counted: i64,
        external_card_sales: i64,
        external_card_refunds: i64,
        operator: &str,
        reason: &str,
    ) -> Result<Closeout, String> {
        self.validate()?;
        if !self.review_ticket_ids.is_empty()
            || !self.unresolved_card_ticket_ids.is_empty()
            || !self.paid_draft_ticket_ids.is_empty()
            || !self.unresolved_adjustment_ticket_ids.is_empty()
        {
            return Err("Resolve unknown card or refund outcomes, complete drafts with taken tender, and review unverified completed tickets before finalizing closeout.".into());
        }
        let opening = money_value(opening)?;
        let counted = money_value(counted)?;
        let external_card = money_value(external_card_sales)?;
        let external_card_refunds = money_value(external_card_refunds)?;
        if self.calculation_version == 1 && external_card_refunds != 0 {
            return Err("A version one closeout cannot include refund declarations.".into());
        }
        let operator = text(operator, "operator name", 120, true)?;
        let expected_cash = money_value(
            opening.checked_add(self.cash_sales_cents)
                .and_then(|cash| cash.checked_sub(self.cash_refunds_cents))
                .filter(|cash| *cash >= 0)
                .ok_or("Opening float and cash sales are insufficient for recorded cash refunds, or the cash limit was reached.")?,
        )?;
        let cash_variance = counted - expected_cash;
        let expected_card = self
            .verified_card_cents
            .checked_sub(self.card_refunds_cents)
            .ok_or("Card movement limit reached.")?;
        let declared_card_net = external_card
            .checked_sub(external_card_refunds)
            .ok_or("Declared card movement limit reached.")?;
        let card_variance = declared_card_net
            .checked_sub(expected_card)
            .ok_or("Card variance limit reached.")?;
        let reason = text(
            reason,
            "discrepancy reason",
            500,
            cash_variance != 0 || card_variance != 0,
        )?;
        Ok(Closeout {
            actor: None,
            id: format!("shift-{}-{}", self.start, self.end),
            operator,
            reason,
            opening_cash_cents: opening,
            counted_cash_cents: counted,
            external_card_cents: external_card,
            external_card_refunds_cents: external_card_refunds,
            expected_cash_cents: expected_cash,
            expected_card_cents: expected_card,
            cash_variance_cents: cash_variance,
            card_variance_cents: card_variance,
            report: self.clone(),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.calculation_version, 1 | 2) {
            return Err("Unsupported shift calculation version.".into());
        }
        if self.calculation_version == 1
            && (self.cash_refunds_cents != 0
                || self.card_refunds_cents != 0
                || self.billing_credits_cents != 0
                || !self.adjustment_event_ids.is_empty()
                || !self.unresolved_adjustment_ticket_ids.is_empty())
        {
            return Err("Version one shift snapshots cannot contain adjustment movements.".into());
        }
        if self.start >= self.end {
            return Err("Invalid saved shift window.".into());
        }
        for amount in [
            self.cash_sales_cents,
            self.cash_received_cents,
            self.cash_change_cents,
            self.verified_card_cents,
            self.pending_billing_cents,
            self.unverified_completed_cents,
            self.cash_refunds_cents,
            self.card_refunds_cents,
            self.billing_credits_cents,
        ] {
            money_value(amount)?;
        }
        if self.cash_received_cents.checked_sub(self.cash_change_cents)
            != Some(self.cash_sales_cents)
        {
            return Err("Saved cash sales, received and change do not reconcile.".into());
        }
        for queue in [
            &self.review_ticket_ids,
            &self.unresolved_card_ticket_ids,
            &self.paid_draft_ticket_ids,
            &self.adjustment_event_ids,
            &self.unresolved_adjustment_ticket_ids,
        ] {
            let unique: HashSet<_> = queue.iter().collect();
            if queue.contains(&0) || unique.len() != queue.len() {
                return Err(
                    "Saved shift review queue contains invalid or duplicate ticket numbers.".into(),
                );
            }
        }
        Ok(())
    }
}

impl Closeout {
    /// Recheck declared totals and discrepancy evidence after deserialization.
    pub fn validate(&self) -> Result<(), String> {
        let mut expected = self.report.reconcile_movements(
            self.opening_cash_cents,
            self.counted_cash_cents,
            self.external_card_cents,
            self.external_card_refunds_cents,
            &self.operator,
            &self.reason,
        )?;
        if let Some(actor) = &self.actor {
            actor.validate()?;
            if actor.display_name != self.operator {
                return Err("Closeout attribution differs from its authenticated actor.".into());
            }
        }
        expected.actor = self.actor.clone();
        if expected != *self {
            return Err("Saved closeout snapshot has inconsistent declarations or totals.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
impl ShiftReport {
    pub fn reconcile(
        &self,
        opening: i64,
        counted: i64,
        external_card: i64,
        operator: &str,
        reason: &str,
    ) -> Result<Closeout, String> {
        self.reconcile_movements(opening, counted, external_card, 0, operator, reason)
    }
}

#[cfg(test)]
#[path = "../../tests/extended/finance/closeout_tests.rs"]
mod tests;
