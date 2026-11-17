use super::*;
use crate::{
    auth::{OperatorSnapshot, Permission, Session},
    price_overrides::{PriceOverrideAction, PriceOverrideLedger},
    security_audit::AuditAction,
};

impl Store {
    pub fn price_overrides(&self) -> Result<PriceOverrideLedger, String> {
        let ledger: PriceOverrideLedger = self.read_required("price_overrides")?;
        ledger.validate_against(&self.list()?)?;
        Ok(ledger)
    }
    fn price_context(
        &self,
        expected_revision: u64,
        ticket_id: u64,
        key: &str,
    ) -> Result<(PriceOverrideLedger, Ticket), String> {
        let ledger = self.price_overrides()?;
        let ticket = self
            .list()?
            .into_iter()
            .find(|ticket| ticket.id == ticket_id)
            .ok_or("Ticket not found. Reload saved records.")?;
        if ticket.revision != expected_revision
            && !ledger.events.iter().any(|event| event.request_key == key)
        {
            return Err("This ticket changed. Reload it before approving another price.".into());
        }
        Ok((ledger, ticket))
    }
    fn commit_price_override(
        &self,
        current: &PriceOverrideLedger,
        next: &PriceOverrideLedger,
        old: &Ticket,
        candidate: &Ticket,
        actor: &OperatorSnapshot,
    ) -> Result<(), String> {
        if current == next {
            if old != candidate {
                return Err("A price retry cannot change the saved ticket.".into());
            }
            return Ok(());
        }
        if next.events.len() != current.events.len() + 1
            || !next.events.starts_with(&current.events)
        {
            return Err("Append one authorized price decision at a time.".into());
        }
        let event = next
            .events
            .last()
            .ok_or("Price decision event is missing.")?;
        if &event.actor != actor {
            return Err("A price decision must use the current authenticated operator.".into());
        }
        crate::counter_validation::validate_counter_candidate(
            Some(old),
            candidate,
            &self.catalog()?,
            &self.directory()?,
            crate::domain::now(),
        )?;
        let mut tickets = self.list()?;
        let index = tickets
            .iter()
            .position(|ticket| ticket.id == candidate.id)
            .ok_or("Ticket disappeared. Reload saved records.")?;
        tickets[index] = candidate.clone();
        next.validate_against(&tickets)?;
        self.save_unchecked(candidate)?;
        self.write_state("price_overrides", next)?;
        self.append_audit(
            Some(actor),
            AuditAction::PriceOverrideRecorded,
            &format!("price-approval:{}", event.id),
            &format!(
                "Ticket {}; line {}; {:?}; {} to {} cents. Reason recorded in price history.",
                candidate.id,
                event.line_id,
                event.action,
                event.before.effective_total_cents(),
                event.after.effective_total_cents()
            ),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn approve_line_price_as(
        &self,
        session: &Session,
        expected_revision: u64,
        ticket_id: u64,
        line_id: u64,
        approved_total_cents: i64,
        reason: &str,
        key: &str,
    ) -> Result<Ticket, String> {
        self.change_line_price_as(
            session,
            expected_revision,
            ticket_id,
            line_id,
            PriceOverrideAction::Apply {
                approved_total_cents,
            },
            reason,
            key,
        )
    }
    pub fn clear_line_price_as(
        &self,
        session: &Session,
        expected_revision: u64,
        ticket_id: u64,
        line_id: u64,
        reason: &str,
        key: &str,
    ) -> Result<Ticket, String> {
        self.change_line_price_as(
            session,
            expected_revision,
            ticket_id,
            line_id,
            PriceOverrideAction::Clear,
            reason,
            key,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn change_line_price_as(
        &self,
        session: &Session,
        expected_revision: u64,
        ticket_id: u64,
        line_id: u64,
        action: PriceOverrideAction,
        reason: &str,
        key: &str,
    ) -> Result<Ticket, String> {
        self.authorized(session, Permission::OverridePrices, |actor| {
            let (current, ticket) = self.price_context(expected_revision, ticket_id, key)?;
            let (next, candidate) = match action {
                PriceOverrideAction::Apply {
                    approved_total_cents,
                } => current.approve(
                    &ticket,
                    line_id,
                    approved_total_cents,
                    &actor,
                    reason,
                    key,
                    crate::domain::now(),
                )?,
                PriceOverrideAction::Clear => {
                    current.clear(&ticket, line_id, &actor, reason, key, crate::domain::now())?
                }
            };
            self.commit_price_override(&current, &next, &ticket, &candidate, &actor)?;
            Ok(candidate)
        })
    }
}
