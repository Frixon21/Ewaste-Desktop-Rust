//! Validate ordinary cashier candidates at the persistence boundary. UI controls
//! are not authority to change a saved tariff, identity, or payment history.
use crate::{
    catalog::{Catalog, PricingRule},
    customers::Directory,
    domain::{Line, Payment, State, Ticket, decimal, rule_line},
    tender::TenderEvidence,
    workflow::CustomerChoice,
};

pub fn validate_counter_candidate(
    previous: Option<&Ticket>,
    candidate: &Ticket,
    catalog: &Catalog,
    directory: &Directory,
    at: u64,
) -> Result<(), String> {
    catalog.validate()?;
    directory.validate()?;
    candidate.validate_intake_records()?;
    if let Some(old) = previous {
        if old.id != candidate.id || old.created_at != candidate.created_at {
            return Err("A counter edit cannot replace a different ticket.".into());
        }
        if old.state == State::Held && candidate.state == State::Completed {
            return Err("Resume a held ticket before completion.".into());
        }
        if old.state != State::Draft && old.tender_evidence != candidate.tender_evidence {
            return Err("Resume the draft before changing tender evidence.".into());
        }
        if intake_changed(old, candidate) {
            old.editable_intake()?;
        }
    }
    validate_customer(previous, candidate, directory)?;
    validate_account(previous, candidate, directory)?;
    let mut used = std::collections::HashSet::new();
    for (index, line) in candidate.lines.iter().enumerate() {
        validate_line_fields(line)?;
        let old = previous.and_then(|ticket| matching_line(ticket, line, index, &mut used));
        validate_line(old, line, catalog, at)?;
    }
    if candidate
        .lines
        .iter()
        .all(|line| line.total_cents.is_some())
        && candidate.total().is_none()
    {
        return Err("Ticket amount exceeds the supported range.".into());
    }
    validate_tender(previous.map(|ticket| &ticket.tender_evidence), candidate)?;
    Ok(())
}
fn intake_changed(old: &Ticket, next: &Ticket) -> bool {
    old.lines != next.lines
        || old.source_intake != next.source_intake
        || old.customer_choice != next.customer_choice
        || old.customer_record != next.customer_record
        || old.customer != next.customer
        || old.account_record != next.account_record
}
fn matching_line<'a>(
    ticket: &'a Ticket,
    line: &Line,
    index: usize,
    used: &mut std::collections::HashSet<usize>,
) -> Option<&'a Line> {
    let position = if line.id > 0 {
        ticket.lines.iter().position(|old| old.id == line.id)
    } else {
        ticket
            .lines
            .iter()
            .enumerate()
            .find(|(position, old)| {
                !used.contains(position) && old.id == 0 && billing_equal(old, line)
            })
            .map(|(position, _)| position)
            .or_else(|| {
                ticket
                    .lines
                    .get(index)
                    .filter(|old| old.id == 0 && old.code == line.code && !used.contains(&index))
                    .map(|_| index)
            })
            .or_else(|| {
                ticket
                    .lines
                    .iter()
                    .enumerate()
                    .find(|(position, old)| {
                        !used.contains(position) && old.id == 0 && old.code == line.code
                    })
                    .map(|(position, _)| position)
            })
    }?;
    if !used.insert(position) {
        return None;
    }
    ticket.lines.get(position)
}
fn billing_equal(a: &Line, b: &Line) -> bool {
    a.rule_snapshot == b.rule_snapshot
        && a.gross_thousandths == b.gross_thousandths
        && a.tare_thousandths == b.tare_thousandths
        && price_equal(a, b)
}
fn price_equal(a: &Line, b: &Line) -> bool {
    a.code == b.code
        && a.name == b.name
        && a.quantity == b.quantity
        && a.unit == b.unit
        && a.rate_cents == b.rate_cents
        && a.base_cents == b.base_cents
        && a.minimum_cents == b.minimum_cents
        && a.total_cents == b.total_cents
}
fn validate_line_fields(line: &Line) -> Result<(), String> {
    for (value, limit) in [
        (&line.code, 32),
        (&line.name, 120),
        (&line.quantity, 32),
        (&line.classification, 80),
    ] {
        if value.chars().count() > limit || value.chars().any(char::is_control) {
            return Err("A counter line contains invalid text.".into());
        }
    }
    if line
        .unit
        .as_ref()
        .is_some_and(|unit| unit.len() > 16 || unit.chars().any(char::is_control))
    {
        return Err("A counter line contains an invalid unit.".into());
    }
    if line.code.trim().is_empty()
        || line.name.trim().is_empty()
        || line.notes.chars().count() > 500
        || line
            .notes
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
        || line
            .piece_count
            .is_some_and(|count| count == 0 || count > 999_999)
    {
        return Err("Review the material code, description, notes and piece count.".into());
    }
    if [
        line.rate_cents,
        line.base_cents,
        line.minimum_cents,
        line.total_cents,
    ]
    .into_iter()
    .flatten()
    .any(|cents| cents < 0)
    {
        return Err("Counter amounts must be nonnegative.".into());
    }
    match (line.gross_thousandths, line.tare_thousandths) {
        (None, None) => (),
        (Some(gross), Some(tare))
            if line.unit.as_deref() == Some("lb")
                && gross > tare
                && tare >= 0
                && gross <= 999_999_999 =>
        {
            let net = decimal(&line.quantity, 3, "Review the saved net pounds.")?;
            if net <= 0 || gross - tare != net {
                return Err("Gross, tare and billed net pounds do not agree.".into());
            }
        }
        _ => return Err(
            "Record both gross and tare for a positive pound measurement, or leave both unknown."
                .into(),
        ),
    }
    Ok(())
}
fn validate_line(
    old: Option<&Line>,
    line: &Line,
    catalog: &Catalog,
    at: u64,
) -> Result<(), String> {
    let rule = match old {
        Some(old) => {
            if old.rule_snapshot != line.rule_snapshot {
                return Err(
                    "A saved line's pricing rule cannot be replaced by a counter edit.".into(),
                );
            }
            if old.rule_snapshot.is_none() && billing_equal(old, line) {
                return Ok(());
            }
            match &old.rule_snapshot {
                Some(rule) => rule.clone(),
                None => Catalog::default().rule_for(&line.code, 0).cloned().ok_or(
                    "This legacy material needs a verified pricing rule before correction.",
                )?,
            }
        }
        None => {
            let current = catalog
                .rule_for(&line.code, at)
                .ok_or("No tariff is effective for this material.")?;
            if line.rule_snapshot.as_ref() != Some(current) {
                return Err("A new material line must use the current configured tariff.".into());
            }
            current.clone()
        }
    };
    compare_recomputed(&rule, line)
}
fn compare_recomputed(rule: &PricingRule, line: &Line) -> Result<(), String> {
    let manual = if rule.rate_cents.is_none() {
        line.total_cents
            .map(|cents| format!("{}.{:02}", cents / 100, cents % 100))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let expected = rule_line(rule, &line.quantity, &manual)?;
    if !price_equal(&expected, line) {
        return Err("Material amounts do not match the saved pricing rule. Use a permitted price override workflow.".into());
    }
    Ok(())
}
fn same_customer_header(old: &Ticket, next: &Ticket) -> bool {
    old.customer_choice == next.customer_choice
        && old.customer_record == next.customer_record
        && old.customer == next.customer
}
fn validate_customer(
    previous: Option<&Ticket>,
    candidate: &Ticket,
    directory: &Directory,
) -> Result<(), String> {
    let changed = previous.is_none_or(|old| !same_customer_header(old, candidate));
    match candidate.customer_choice {
        CustomerChoice::Blank => {
            if candidate.customer_record.is_some() || candidate.customer.is_some() {
                return Err("Blank customer choice cannot retain a named customer.".into());
            }
        }
        CustomerChoice::Selected => {
            let customer = candidate
                .customer_record
                .as_ref()
                .ok_or("Select a saved customer record.")?;
            if candidate.customer.as_deref() != Some(customer.name.as_str()) {
                return Err("The selected customer and ticket name do not match.".into());
            }
            if changed && !directory.customers.iter().any(|saved| saved == customer) {
                return Err("The customer snapshot changed or is not saved. Select the current customer record.".into());
            }
        }
        CustomerChoice::Unset => {
            if changed && (candidate.customer_record.is_some() || candidate.customer.is_some()) {
                return Err(
                    "Select a saved customer instead of entering an unattached name.".into(),
                );
            }
        }
    }
    Ok(())
}
fn validate_account(
    previous: Option<&Ticket>,
    candidate: &Ticket,
    directory: &Directory,
) -> Result<(), String> {
    let changed = previous.is_none_or(|old| old.account_record != candidate.account_record);
    if let Some(account) = &candidate.account_record
        && changed
        && !directory
            .accounts
            .iter()
            .any(|saved| saved == account && saved.active)
    {
        return Err("Select the current active saved business account.".into());
    }
    let new_completion = candidate.state == State::Completed
        && previous.is_none_or(|old| old.state != State::Completed);
    if new_completion && let Some(Payment::ChargeTo { account }) = &candidate.payment {
        let selected = candidate
            .account_record
            .as_ref()
            .ok_or("Select a saved business account before charge-to completion.")?;
        if !directory.accounts.iter().any(|current| {
            current.id == selected.id && current.number == *account && current.active
        }) {
            return Err("The selected billing account is no longer active or does not match. Review it before completion.".into());
        }
    }
    Ok(())
}
fn validate_tender(previous: Option<&TenderEvidence>, candidate: &Ticket) -> Result<(), String> {
    let empty = TenderEvidence::default();
    let old = previous.unwrap_or(&empty);
    let next = &candidate.tender_evidence;
    if old == next {
        return Ok(());
    }
    if old.cash.is_some() {
        return Err("Recorded cash evidence cannot be erased or replaced.".into());
    }
    if let Some(cash) = &next.cash {
        if candidate.total() != Some(cash.amount_due_cents) {
            return Err("Cash confirmation must match the current ticket total.".into());
        }
        let expected = old.record_cash(cash.amount_due_cents, cash.received_cents)?;
        if &expected != next {
            return Err(
                "Cash confirmation cannot rewrite card history or change calculation.".into(),
            );
        }
        return Ok(());
    }
    let old_count = old.card_attempts.len();
    let new_count = next.card_attempts.len();
    let mut replay = if new_count
        == old_count
            .checked_add(1)
            .ok_or("Card attempt limit reached.")?
    {
        if next.card_attempts[..old_count] != old.card_attempts {
            return Err("Previous card attempts cannot be rewritten.".into());
        }
        let attempt = next.card_attempts.last().ok_or("Card attempt missing.")?;
        if candidate.total() != Some(attempt.amount_cents) {
            return Err("New external card amount must match the ticket total.".into());
        }
        old.begin_card(attempt.amount_cents, &attempt.reference)?
    } else if new_count == old_count && old_count > 0 {
        if next.card_attempts[..old_count - 1] != old.card_attempts[..old_count - 1] {
            return Err("Previous card outcomes cannot be rewritten.".into());
        }
        old.clone()
    } else {
        return Err("Card history must be preserved and attempts added one at a time.".into());
    };
    let expected_last = replay.card_attempts.last().ok_or("Card attempt missing.")?;
    let actual = next.card_attempts.last().ok_or("Card attempt missing.")?;
    if expected_last.id != actual.id
        || expected_last.reference != actual.reference
        || expected_last.amount_cents != actual.amount_cents
    {
        return Err("A card attempt's identity, reference and amount cannot be replaced.".into());
    }
    if &replay != next {
        replay = if actual.reconciled {
            replay.reconcile_card(actual.outcome, actual.operator_verified)?
        } else {
            replay.record_card_result(actual.outcome, actual.operator_verified)?
        };
    }
    if &replay != next {
        return Err("The tender transition does not match a permitted counter command.".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/shared/counter_validation_tests.rs"]
mod tests;
