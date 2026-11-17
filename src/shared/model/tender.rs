//! Synthetic payment evidence; this module never initiates a processor charge.
//! All transitions return a new value so failed validation preserves the draft.

use serde::{Deserialize, Serialize};

const MAX_CENTS: i64 = 99_999_999_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardOutcome {
    Pending,
    Approved,
    Declined,
    Cancelled,
    ZipMismatch,
    Unknown,
}

impl CardOutcome {
    pub fn unresolved(self) -> bool {
        matches!(self, Self::Pending | Self::Unknown)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashTender {
    pub amount_due_cents: i64,
    pub received_cents: i64,
    pub change_cents: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardAttempt {
    pub id: u64,
    pub amount_cents: i64,
    pub reference: String,
    pub outcome: CardOutcome,
    pub operator_verified: bool,
    pub reconciled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TenderEvidence {
    pub cash: Option<CashTender>,
    pub card_attempts: Vec<CardAttempt>,
}

fn validate_amount(cents: i64) -> Result<(), String> {
    if !(0..=MAX_CENTS).contains(&cents) {
        return Err("Enter a nonnegative amount within the supported limit.".into());
    }
    Ok(())
}

fn validate_reference(reference: &str) -> Result<String, String> {
    if reference.chars().any(char::is_control) {
        return Err("A processor reference cannot contain control characters.".into());
    }
    let reference = reference.trim();
    if reference.is_empty()
        || reference.len() > 64
        || !reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(
            if cfg!(feature = "extended-workflows") { "Enter a test processor reference of 1–64 letters, digits, '-', '_' or '.'." }
            else { "Use the processor transaction reference, not a card number (up to 64 letters, digits, '-', '_' or '.')." }.into(),
        );
    }
    let digits = reference.bytes().filter(|b| b.is_ascii_digit()).count();
    let only_digits_and_separators = reference
        .bytes()
        .all(|b| b.is_ascii_digit() || b"-_.".contains(&b));
    if (12..=19).contains(&digits) && only_digits_and_separators {
        return Err(if cfg!(feature = "extended-workflows") {
            "Do not enter card numbers. Use a synthetic processor reference."
        } else {
            "Use the processor transaction reference, not a card number."
        }
        .into());
    }
    Ok(reference.to_owned())
}

impl TenderEvidence {
    /// Block charge-to or other non-card fallback while money may already have
    /// been taken. A failed, resolved card attempt alone does not block fallback.
    pub fn guard_for_noncard(&self) -> Result<(), String> {
        if self.cash.is_some()
            || self.card_attempts.iter().any(|attempt| {
                attempt.outcome.unresolved() || attempt.outcome == CardOutcome::Approved
            })
        {
            return Err(
                "Resolve the existing tender before selecting another payment method.".into(),
            );
        }
        Ok(())
    }

    /// Record explicit cash confirmation, including the amount received and change.
    /// An identical retry is harmless; changed evidence requires a fresh draft.
    pub fn record_cash(&self, amount_due_cents: i64, received_cents: i64) -> Result<Self, String> {
        validate_amount(amount_due_cents)?;
        validate_amount(received_cents)?;
        if received_cents < amount_due_cents {
            return Err("Cash received is less than the amount due.".into());
        }
        if self
            .card_attempts
            .iter()
            .any(|attempt| attempt.outcome.unresolved() || attempt.outcome == CardOutcome::Approved)
        {
            return Err(
                "Resolve the existing card attempt before recording another tender.".into(),
            );
        }
        let cash = CashTender {
            amount_due_cents,
            received_cents,
            change_cents: received_cents - amount_due_cents,
        };
        if let Some(existing) = &self.cash {
            return if existing == &cash {
                Ok(self.clone())
            } else {
                Err("Cash has already been recorded for this draft.".into())
            };
        }
        Ok(Self {
            cash: Some(cash),
            ..self.clone()
        })
    }

    /// Start tracking a synthetic external attempt. This does not charge a card.
    /// A pending/unknown attempt must be reconciled before another attempt starts.
    pub fn begin_card(&self, amount_cents: i64, reference: &str) -> Result<Self, String> {
        validate_amount(amount_cents)?;
        let reference = validate_reference(reference)?;
        if self.cash.is_some() {
            return Err("Cash has already been recorded for this draft.".into());
        }
        if self.card_attempts.len() >= 100 {
            return Err("Card attempt limit reached; review this draft before continuing.".into());
        }
        if self
            .card_attempts
            .iter()
            .any(|attempt| attempt.outcome.unresolved() || attempt.outcome == CardOutcome::Approved)
        {
            return Err(
                "Resolve the existing card attempt; do not submit a duplicate charge.".into(),
            );
        }
        if self
            .card_attempts
            .iter()
            .any(|attempt| attempt.reference.eq_ignore_ascii_case(&reference))
        {
            return Err("That reference is already recorded. Review the original attempt.".into());
        }
        let id = self
            .card_attempts
            .iter()
            .map(|attempt| attempt.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Card attempt identifier limit reached.")?;
        let mut attempts = self.card_attempts.clone();
        attempts.push(CardAttempt {
            id,
            amount_cents,
            reference,
            outcome: CardOutcome::Pending,
            operator_verified: false,
            reconciled: false,
        });
        Ok(Self {
            card_attempts: attempts,
            ..self.clone()
        })
    }

    /// Record the external result. Approval always needs explicit operator review.
    pub fn record_card_result(
        &self,
        outcome: CardOutcome,
        operator_verified: bool,
    ) -> Result<Self, String> {
        let attempt = self
            .card_attempts
            .last()
            .ok_or("Start a card attempt first.")?;
        if attempt.outcome == outcome && attempt.operator_verified == operator_verified {
            return Ok(self.clone());
        }
        if attempt.outcome != CardOutcome::Pending {
            return Err(
                "This result is already recorded; use reconciliation for an unknown result.".into(),
            );
        }
        self.with_card_result(outcome, operator_verified, false)
    }

    /// Resolve a timeout/unknown or pending attempt after checking its existing
    /// processor result. This updates evidence without creating another attempt.
    pub fn reconcile_card(
        &self,
        outcome: CardOutcome,
        operator_verified: bool,
    ) -> Result<Self, String> {
        let attempt = self
            .card_attempts
            .last()
            .ok_or("There is no card attempt to reconcile.")?;
        if !attempt.outcome.unresolved() {
            return Err("Only a pending or unknown card attempt needs reconciliation.".into());
        }
        if !operator_verified || outcome.unresolved() {
            return Err("Verify a final processor result before resolving this attempt.".into());
        }
        self.with_card_result(outcome, true, true)
    }

    fn with_card_result(
        &self,
        outcome: CardOutcome,
        operator_verified: bool,
        reconciled: bool,
    ) -> Result<Self, String> {
        if outcome == CardOutcome::Approved && !operator_verified {
            return Err("Approval requires explicit verification of the processor result.".into());
        }
        let mut attempts = self.card_attempts.clone();
        let attempt = attempts.last_mut().ok_or("Start a card attempt first.")?;
        *attempt = CardAttempt {
            outcome,
            operator_verified,
            reconciled,
            ..attempt.clone()
        };
        Ok(Self {
            card_attempts: attempts,
            ..self.clone()
        })
    }

    /// Validate cash evidence against the current ticket, detecting later edits.
    pub fn cash_for_total(&self, total_cents: i64) -> Result<&CashTender, String> {
        validate_amount(total_cents)?;
        let cash = self
            .cash
            .as_ref()
            .ok_or("Confirm cash received before completing.")?;
        validate_amount(cash.received_cents)?;
        if cash.amount_due_cents != total_cents
            || cash.received_cents < total_cents
            || cash.change_cents != cash.received_cents - total_cents
            || self.card_attempts.iter().any(|attempt| {
                attempt.outcome.unresolved() || attempt.outcome == CardOutcome::Approved
            })
        {
            return Err(
                "Recorded tender does not match this ticket. Review payment evidence.".into(),
            );
        }
        Ok(cash)
    }

    /// Validate verified card approval and matching amount. A reference alone fails.
    pub fn approved_card_for_total(&self, total_cents: i64) -> Result<&CardAttempt, String> {
        validate_amount(total_cents)?;
        let attempt = self
            .card_attempts
            .last()
            .ok_or("Record and verify the external card result first.")?;
        validate_reference(&attempt.reference)?;
        if self.cash.is_some()
            || attempt.outcome != CardOutcome::Approved
            || !attempt.operator_verified
            || attempt.amount_cents != total_cents
            || self
                .card_attempts
                .iter()
                .any(|entry| entry.outcome.unresolved())
        {
            return Err("A verified approval for the exact ticket amount is required.".into());
        }
        Ok(attempt)
    }
}

#[cfg(test)]
#[path = "../../tests/shared/tender_tests.rs"]
mod tests;
