use super::*;
#[cfg(feature = "extended-workflows")]
use crate::auth::{Permission, Session};
use crate::{operator_snapshot::OperatorSnapshot, security_audit::AuditAction};
impl Store {
    #[cfg(feature = "extended-workflows")]
    pub fn save_ticket_as(&self, session: &Session, input: &Ticket) -> Result<Ticket, String> {
        self.authorized(session, Permission::Counter, |actor| {
            self.save_ticket_recorded(input, &actor)
        })
    }

    #[cfg(not(feature = "extended-workflows"))]
    pub fn save_counter_ticket(&self, input: &Ticket) -> Result<Ticket, String> {
        let actor = OperatorSnapshot::local_counter();
        self.transaction(|| self.save_ticket_recorded(input, &actor))
    }

    fn save_ticket_recorded(
        &self,
        input: &Ticket,
        actor: &OperatorSnapshot,
    ) -> Result<Ticket, String> {
        let tickets = self.list()?;
        let old = tickets.iter().find(|t| t.id == input.id);
        let mut candidate = input.clone();
        let is_completed = candidate.state == State::Completed;
        let is_completing = is_completed && !old.is_some_and(|t| t.state == State::Completed);
        if !is_completed && candidate.receipt_snapshot.is_some() {
            return Err("Only a completed ticket can have a frozen receipt.".into());
        }
        let now = crate::domain::now();
        if old.is_none() {
            candidate.created_at = now;
        }
        if is_completing {
            candidate.completed_at = Some(now);
        }
        candidate.created_by = old
            .and_then(|t| t.created_by.clone())
            .or_else(|| old.is_none().then_some(actor.clone()));
        candidate.completed_by = if is_completing {
            Some(actor.clone())
        } else if is_completed {
            old.and_then(|t| t.completed_by.clone())
        } else {
            None
        };
        crate::counter_validation::validate_counter_candidate(
            old,
            &candidate,
            &self.catalog()?,
            &self.directory()?,
            crate::domain::now(),
        )?;
        crate::price_overrides::validate_ordinary_candidate(old, &candidate)?;
        #[cfg(feature = "extended-workflows")]
        let mut prospective = tickets.clone();
        #[cfg(feature = "extended-workflows")]
        {
            if let Some(index) = prospective
                .iter()
                .position(|ticket| ticket.id == candidate.id)
            {
                prospective[index] = candidate.clone();
            } else {
                prospective.push(candidate.clone());
            }
        }
        #[cfg(feature = "extended-workflows")]
        self.price_overrides()?.validate_against(&prospective)?;
        #[cfg(not(feature = "extended-workflows"))]
        self.validate_walkthrough_preservation(old, &candidate)?;
        if is_completing {
            candidate.receipt_snapshot = Some(candidate.render_receipt());
        }
        self.save_unchecked(&candidate)?;
        if old != Some(&candidate) {
            self.append_audit(
                Some(actor),
                AuditAction::TicketSaved,
                &format!("ticket:{}", candidate.id),
                &format!(
                    "Revision {}; state {:?}; {} line(s); total {:?} cents",
                    candidate.revision,
                    candidate.state,
                    candidate.lines.len(),
                    candidate.total()
                ),
            )?;
        }
        Ok(candidate)
    }
    #[cfg(feature = "extended-workflows")]
    pub fn save_directory_as(
        &self,
        session: &Session,
        previous: &Directory,
        next: &Directory,
    ) -> Result<(), String> {
        self.authorized(session, Permission::Counter, |actor| {
            let current = self.directory()?;
            let business = current.accounts != next.accounts
                || current.next_account_id != next.next_account_id;
            if business {
                self.require_session(session, Permission::ManageBusinessAccounts)?;
            }
            self.save_directory_recorded(previous, next, &actor)
        })
    }

    #[cfg(not(feature = "extended-workflows"))]
    pub fn save_counter_directory(
        &self,
        previous: &Directory,
        next: &Directory,
    ) -> Result<(), String> {
        let actor = OperatorSnapshot::local_counter();
        self.transaction(|| self.save_directory_recorded(previous, next, &actor))
    }

    fn save_directory_recorded(
        &self,
        previous: &Directory,
        next: &Directory,
        actor: &OperatorSnapshot,
    ) -> Result<(), String> {
        let current = self.directory()?;
        let business =
            current.accounts != next.accounts || current.next_account_id != next.next_account_id;
        self.save_directory_unchecked(previous, next)?;
        if current != *next {
            self.append_audit(
                Some(actor),
                if business {
                    AuditAction::BusinessDirectorySaved
                } else {
                    AuditAction::CustomerDirectorySaved
                },
                "directory",
                "Saved approved directory fields",
            )?;
        }
        Ok(())
    }
    #[cfg(feature = "extended-workflows")]
    pub fn save_catalog_as(
        &self,
        session: &Session,
        previous: &crate::catalog::Catalog,
        next: &crate::catalog::Catalog,
    ) -> Result<(), String> {
        self.authorized(session, Permission::ManageTariffs, |actor| {
            let current = self.catalog()?;
            self.save_catalog_unchecked(previous, next)?;
            if current != *next {
                self.append_audit(
                    Some(&actor),
                    AuditAction::TariffsSaved,
                    "catalog",
                    "Updated dated tariff configuration",
                )?;
            }
            Ok(())
        })
    }
    #[cfg(feature = "extended-workflows")]
    pub fn save_print_log_as(
        &self,
        session: &Session,
        previous: &PrintLog,
        next: &PrintLog,
    ) -> Result<(), String> {
        self.authorized(session, Permission::Counter, |actor| {
            self.save_print_log_recorded(previous, next, &actor)
        })
    }

    #[cfg(not(feature = "extended-workflows"))]
    pub fn save_counter_print_log(
        &self,
        previous: &PrintLog,
        next: &PrintLog,
    ) -> Result<(), String> {
        let actor = OperatorSnapshot::local_counter();
        self.transaction(|| self.save_print_log_recorded(previous, next, &actor))
    }

    fn save_print_log_recorded(
        &self,
        previous: &PrintLog,
        next: &PrintLog,
        actor: &OperatorSnapshot,
    ) -> Result<(), String> {
        let current = self.print_log()?;
        validate_print_transition(&current, next, &self.list()?)?;
        self.save_print_log_unchecked(previous, next)?;
        if current != *next {
            self.append_audit(
                Some(actor),
                AuditAction::PrintJobSaved,
                "receipt-spool",
                "Recorded saved-receipt output or retry",
            )?;
        }
        Ok(())
    }
    #[cfg(feature = "extended-workflows")]
    pub fn save_intake_reviews_as(
        &self,
        session: &Session,
        previous: &crate::intake_review::ReviewLedger,
        input: &crate::intake_review::ReviewLedger,
    ) -> Result<crate::intake_review::ReviewLedger, String> {
        self.authorized(session, Permission::ReviewIntake, |actor| {
            let current = self.intake_reviews()?;
            let mut next = input.clone();
            if let Some(event) = next.events.last_mut() {
                event.actor = Some(actor.clone());
                event.operator = actor.display_name.clone();
            }
            self.save_intake_reviews_unchecked(previous, &next)?;
            if current != next {
                self.append_audit(
                    Some(&actor),
                    AuditAction::IntakeReviewSaved,
                    &format!("source-review:{}", next.next_id - 1),
                    "Appended documentary review; financial ticket unchanged",
                )?;
            }
            Ok(next)
        })
    }
    #[cfg(feature = "extended-workflows")]
    pub fn save_closeout_as(
        &self,
        session: &Session,
        input: &crate::closeout::Closeout,
    ) -> Result<crate::closeout::Closeout, String> {
        self.authorized(session, Permission::Closeout, |actor| {
            if input.report.end > crate::domain::now() {
                return Err("A closeout cannot include future time. Refresh the cutoff.".into());
            }
            if input.report.calculation_version != 2 {
                return Err("Use the refund-aware closeout report for new reconciliations.".into());
            }
            let mut record = input.clone();
            record.actor = Some(actor.clone());
            record.operator = actor.display_name.clone();
            record.validate()?;
            let current = self.closeouts()?;
            self.save_closeout_unchecked(&record)?;
            if !current.contains(&record) {
                self.append_audit(
                    Some(&actor),
                    AuditAction::CloseoutSaved,
                    &record.id,
                    "Finalized immutable local reconciliation",
                )?;
            }
            Ok(record)
        })
    }
    #[cfg(feature = "extended-workflows")]
    pub fn prepare_intake_export_as(
        &self,
        session: &Session,
        include_contacts: bool,
    ) -> Result<String, String> {
        self.authorized(
            session,
            if include_contacts {
                Permission::ExportContacts
            } else {
                Permission::ExportRecords
            },
            |actor| {
                self.price_overrides()?;
                let csv = crate::intake_export::export_intake(
                    &self.list()?,
                    &self.intake_reviews()?,
                    include_contacts,
                )?;
                self.append_audit(
                    Some(&actor),
                    AuditAction::RecordsExported,
                    "intake-review",
                    if include_contacts {
                        "Prepared local CSV with approved contact fields"
                    } else {
                        "Prepared local CSV without contact fields"
                    },
                )?;
                Ok(csv)
            },
        )
    }
    #[cfg(feature = "extended-workflows")]
    pub fn audit_as(
        &self,
        session: &Session,
    ) -> Result<crate::security_audit::AuditLedger, String> {
        self.authorized(session, Permission::AdjustFinancials, |_| {
            self.security_audit()
        })
    }
    #[cfg(feature = "extended-workflows")]
    pub fn backup_as(&self, session: &Session, target: &Path) -> Result<(), String> {
        self.authorized(session, Permission::Backup, |_| Ok(()))?;
        // VACUUM INTO cannot run inside a write transaction. Authorization starts
        // this operation; CLI recovery remains a filesystem-owner operation.
        self.backup(target)?;
        self.authorized(session, Permission::Backup, |actor| {
            self.append_audit(
                Some(&actor),
                AuditAction::BackupCreated,
                "register-backup",
                "Created and verified a new local backup",
            )
        })
        .map_err(|e| {
            format!(
                "Backup exists at {}, but its audit record could not be saved: {e}",
                target.display()
            )
        })
    }
}
fn validate_print_transition(
    previous: &PrintLog,
    next: &PrintLog,
    tickets: &[Ticket],
) -> Result<(), String> {
    next.validate()?;
    if previous == next {
        return Ok(());
    }
    if next.jobs.len() < previous.jobs.len() || next.jobs.len() > previous.jobs.len() + 1 {
        return Err("Receipt history is append-only, one request or retry at a time.".into());
    }
    let mut changes = 0;
    for (index, job) in next.jobs.iter().enumerate() {
        if let Some(old) = previous.jobs.get(index) {
            if old == job {
                continue;
            }
            if old.state != crate::printing::PrintState::Failed
                || old.id != job.id
                || old.request_key != job.request_key
                || old.ticket_id != job.ticket_id
                || old.copy != job.copy
                || old.receipt != job.receipt
                || old.attempts.checked_add(1) != Some(job.attempts)
            {
                return Err("A saved receipt job can only retry its original snapshot.".into());
            }
        } else {
            let ticket = tickets
                .iter()
                .find(|t| t.id == job.ticket_id && t.state == State::Completed)
                .ok_or("Print a saved completed ticket only.")?;
            if job.receipt != ticket.receipt() || job.id != previous.next_id || job.attempts != 1 {
                return Err(
                    "A receipt request must use the original saved ticket snapshot.".into(),
                );
            }
        }
        changes += 1;
    }
    if changes != 1 {
        return Err("Record one receipt output action at a time.".into());
    }
    Ok(())
}
