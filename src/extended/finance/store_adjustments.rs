use super::*;
use crate::{
    adjustments::{AdjustmentAttempt, AdjustmentKind, AdjustmentLedger, AdjustmentOutcome},
    auth::{Permission, Session},
    security_audit::AuditAction,
};
impl Store {
    pub fn adjustments(&self) -> Result<AdjustmentLedger, String> {
        let ledger: AdjustmentLedger = self.read_required("adjustments")?;
        ledger.validate_against(&self.list()?)?;
        Ok(ledger)
    }
    fn adjustment_context(
        &self,
        expected: &AdjustmentLedger,
        key: &str,
        ticket_id: u64,
    ) -> Result<(AdjustmentLedger, Ticket), String> {
        let current = self.adjustments()?;
        if &current != expected && !current.events.iter().any(|event| event.request_key == key) {
            return Err("Financial adjustment history changed. Refresh before reserving or confirming an adjustment.".into());
        }
        let ticket = self
            .list()?
            .into_iter()
            .find(|t| t.id == ticket_id)
            .ok_or("Original ticket not found.")?;
        Ok((current, ticket))
    }
    fn commit_adjustment(
        &self,
        current: &AdjustmentLedger,
        next: &AdjustmentLedger,
        attempt: &AdjustmentAttempt,
        actor: &crate::auth::OperatorSnapshot,
    ) -> Result<(), String> {
        if current == next {
            return Ok(());
        }
        if next.events.len() != current.events.len() + 1
            || !next.events.starts_with(&current.events)
        {
            return Err(
                "Financial adjustments are append-only, one intent or result at a time.".into(),
            );
        }
        let event = next.events.last().ok_or("Adjustment event missing.")?;
        if self
            .closeouts()?
            .iter()
            .any(|closeout| closeout.report.start <= event.at && event.at < closeout.report.end)
        {
            return Err("This adjustment time falls inside a closed shift. Refresh the register clock before proceeding.".into());
        }
        next.validate_against(&self.list()?)?;
        self.write_state("adjustments", next)?;
        self.append_audit(
            Some(actor),
            AuditAction::AdjustmentRecorded,
            &format!("adjustment:{}", attempt.id),
            &format!(
                "Ticket {}; {:?}; {:?}; amount {} cents",
                attempt.ticket_id, attempt.kind, attempt.outcome, attempt.amount_cents
            ),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn start_adjustment_as(
        &self,
        session: &Session,
        expected: &AdjustmentLedger,
        ticket_id: u64,
        kind: AdjustmentKind,
        amount_cents: i64,
        reason: &str,
        key: &str,
        reference: &str,
    ) -> Result<(AdjustmentLedger, AdjustmentAttempt), String> {
        self.authorized(session, Permission::AdjustFinancials, |actor| {
            let (current, ticket) = self.adjustment_context(expected, key, ticket_id)?;
            let (next, attempt) = current.start(
                &ticket,
                kind,
                amount_cents,
                &actor,
                reason,
                key,
                crate::domain::now(),
                reference,
            )?;
            self.commit_adjustment(&current, &next, &attempt, &actor)?;
            Ok((next, attempt))
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn record_adjustment_result_as(
        &self,
        session: &Session,
        expected: &AdjustmentLedger,
        ticket_id: u64,
        attempt_id: u64,
        outcome: AdjustmentOutcome,
        verified: bool,
        reference: &str,
        key: &str,
        reconcile: bool,
    ) -> Result<(AdjustmentLedger, AdjustmentAttempt), String> {
        self.authorized(session, Permission::AdjustFinancials, |actor| {
            let (current, ticket) = self.adjustment_context(expected, key, ticket_id)?;
            let (next, attempt) = current.record_result(
                &ticket,
                attempt_id,
                outcome,
                verified,
                reference,
                &actor,
                key,
                crate::domain::now(),
                reconcile,
            )?;
            self.commit_adjustment(&current, &next, &attempt, &actor)?;
            Ok((next, attempt))
        })
    }
}
