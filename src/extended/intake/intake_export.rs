//! Synthetic documentary review CSV. Billing snapshots are exported unchanged;
//! reviewed source/material fields are an overlay, never a claim approval.

use crate::{
    domain::{Line, Payment, State, Ticket},
    intake_review::{
        LineIdentity, ReviewDisposition, ReviewLedger, ReviewSnapshot, ReviewedMaterial,
    },
};

const HEADERS: &[&str] = &[
    "training_ticket_id",
    "training_ticket_state",
    "training_created_at_unix_seconds",
    "training_completed_at_unix_seconds",
    "training_tender_disposition",
    "training_ticket_total_cents",
    "training_line_identity",
    "training_billing_code",
    "training_billing_material_name",
    "training_billing_quantity",
    "training_billing_unit",
    "training_billing_rate_cents",
    "training_billing_base_cents",
    "training_billing_minimum_adjustment_cents",
    "training_billing_line_total_cents",
    "training_approved_line_total_cents",
    "training_effective_line_total_cents",
    "training_price_approval_event_id",
    "training_source_category",
    "training_intake_reason",
    "training_delivery_mode",
    "training_material_class",
    "training_material_condition",
    "training_reporting_unit_count",
    "training_actual_weight_lb",
    "training_documentary_disposition",
    "training_documentary_issue_count",
    "training_review_event_id",
    "training_review_at_unix_seconds",
];

const CONTACT_HEADERS: &[&str] = &[
    "training_source_name",
    "training_source_address",
    "training_source_organization",
    "training_source_contact_name",
    "training_source_phone",
];

/// Export all supplied saved tickets and their complete documentary ledger.
/// Contacts require a deliberate opt-in; internal prose never enters this CSV.
pub fn export_intake(
    tickets: &[Ticket],
    ledger: &ReviewLedger,
    include_contacts: bool,
) -> Result<String, String> {
    ledger.validate_against(tickets)?;
    for ticket in tickets {
        validate_ticket(ticket)?;
    }
    let mut headers = HEADERS
        .iter()
        .map(|header| (*header).to_owned())
        .collect::<Vec<_>>();
    if include_contacts {
        headers.extend(CONTACT_HEADERS.iter().map(|header| (*header).to_owned()));
    }
    let mut csv = csv_row(&headers);
    let mut ordered = tickets.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|ticket| (ticket.created_at, ticket.id));
    for ticket in ordered {
        if ticket.lines.is_empty() {
            continue;
        }
        let (snapshot, issue_count) = reviewed_snapshot(ticket, ledger)?;
        for (index, line) in ticket.lines.iter().enumerate() {
            let identity = identity_for(line, index);
            let material = snapshot
                .materials
                .iter()
                .find(|material| material.identity == identity)
                .ok_or("A documentary export line does not match its saved material identity.")?;
            let mut row = base_row(
                ticket,
                line,
                identity,
                &snapshot,
                material,
                issue_count,
                ledger,
            );
            if include_contacts {
                let source = &snapshot.source;
                row.extend([
                    source.source_name.clone(),
                    source.source_address.clone(),
                    source.organization.clone(),
                    source.contact_name.clone(),
                    source.phone.clone(),
                ]);
            }
            csv.push_str(&csv_row(&row));
        }
    }
    Ok(csv)
}

fn identity_for(line: &Line, index: usize) -> LineIdentity {
    if line.id == 0 {
        LineIdentity::LegacyOrdinal(index)
    } else {
        LineIdentity::Stable(line.id)
    }
}

fn reviewed_snapshot(
    ticket: &Ticket,
    ledger: &ReviewLedger,
) -> Result<(ReviewSnapshot, usize), String> {
    if let Some(review) = ledger.latest(ticket.id) {
        let snapshot = review.snapshot.normalized_for(ticket)?;
        let count = snapshot.issues(ticket)?.len();
        return Ok((snapshot, count));
    }
    if ticket.state == State::Completed {
        let snapshot = ReviewSnapshot::from_ticket(ticket)?;
        let count = snapshot.issues(ticket)?.len();
        return Ok((snapshot, count));
    }
    // Draft/held tickets have no frozen review event. Export their own documentary
    // fields as pending and retain their current line identities without reviewing.
    let materials = ticket
        .lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            Ok(ReviewedMaterial {
                identity: identity_for(line, index),
                details: line.material_intake.normalized()?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((
        ReviewSnapshot {
            source: ticket.source_intake.normalized()?,
            materials,
            disposition: ReviewDisposition::PendingReview,
        },
        ticket.intake_issues().len(),
    ))
}

fn base_row(
    ticket: &Ticket,
    line: &Line,
    identity: LineIdentity,
    snapshot: &ReviewSnapshot,
    material: &ReviewedMaterial,
    issue_count: usize,
    ledger: &ReviewLedger,
) -> Vec<String> {
    let source = &snapshot.source;
    let details = &material.details;
    let event = ledger.latest(ticket.id);
    vec![
        ticket.id.to_string(),
        format!("{:?}", ticket.state),
        ticket.created_at.to_string(),
        optional(ticket.completed_at),
        tender_disposition(ticket),
        optional(ticket.total()),
        match identity {
            LineIdentity::Stable(id) => format!("stable:{id}"),
            LineIdentity::LegacyOrdinal(index) => format!("legacy-ordinal:{index}"),
        },
        line.code.clone(),
        line.name.clone(),
        line.quantity.clone(),
        line.unit.clone().unwrap_or_default(),
        optional(line.rate_cents),
        optional(line.base_cents),
        optional(line.minimum_cents),
        optional(line.total_cents),
        optional(
            line.price_approval
                .as_ref()
                .map(|approval| approval.approved_total_cents),
        ),
        optional(line.effective_total_cents()),
        optional(
            line.price_approval
                .as_ref()
                .map(|approval| approval.event_id),
        ),
        format!("{:?}", source.category),
        format!("{:?}", source.reason),
        format!("{:?}", source.delivery),
        format!("{:?}", details.class),
        format!("{:?}", details.condition),
        optional(details.reporting_units),
        details
            .actual_weight_thousandths
            .map(|weight| format!("{}.{:03}", weight / 1000, weight % 1000))
            .unwrap_or_default(),
        format!("{:?}", snapshot.disposition),
        issue_count.to_string(),
        optional(event.map(|event| event.id)),
        optional(event.map(|event| event.at)),
    ]
}

fn optional(value: Option<impl ToString>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn tender_disposition(ticket: &Ticket) -> String {
    if ticket.state == State::Completed {
        return match &ticket.payment {
            Some(Payment::Cash)
                if ticket
                    .total()
                    .is_some_and(|total| ticket.tender_evidence.cash_for_total(total).is_ok()) =>
            {
                "CashReceived"
            }
            Some(Payment::Cash) => "UnverifiedCash",
            Some(Payment::ExternalCard { reference })
                if ticket.total().is_some_and(|total| {
                    ticket
                        .tender_evidence
                        .approved_card_for_total(total)
                        .is_ok_and(|attempt| attempt.reference == *reference)
                }) =>
            {
                "VerifiedExternalCard"
            }
            Some(Payment::ExternalCard { .. }) => "UnverifiedExternalCard",
            Some(Payment::ChargeTo { .. }) => "PendingBilling",
            None => "Pending",
        }
        .into();
    }
    if let Some(total) = ticket.total() {
        if ticket.tender_evidence.cash_for_total(total).is_ok() {
            return "CashReceivedAwaitingCompletion".into();
        }
        if ticket
            .tender_evidence
            .approved_card_for_total(total)
            .is_ok()
        {
            return "VerifiedExternalCardAwaitingCompletion".into();
        }
    }
    if let Some(attempt) = ticket.tender_evidence.card_attempts.last() {
        return format!("ExternalCard{:?}", attempt.outcome);
    }
    match &ticket.payment {
        Some(Payment::ChargeTo { .. }) => "PendingBilling",
        Some(Payment::Cash) => "UnverifiedCash",
        Some(Payment::ExternalCard { .. }) => "UnverifiedExternalCard",
        None => "Pending",
    }
    .into()
}

fn validate_ticket(ticket: &Ticket) -> Result<(), String> {
    if ticket.id == 0 {
        return Err("A documentary export ticket has an invalid ID.".into());
    }
    ticket.validate_intake_records()?;
    if ticket.state == State::Completed
        && (ticket.lines.is_empty() || ticket.payment.is_none() || ticket.total().is_none())
    {
        return Err(
            "A completed documentary export ticket is missing its billing snapshot.".into(),
        );
    }
    for line in &ticket.lines {
        validate_billing(line)?;
    }
    Ok(())
}

fn validate_billing(line: &Line) -> Result<(), String> {
    for (value, limit) in [(&line.code, 32), (&line.name, 120), (&line.quantity, 32)] {
        if value.chars().count() > limit || value.chars().any(char::is_control) {
            return Err("A documentary export contains invalid billing text.".into());
        }
    }
    if line.code.trim().is_empty() || line.name.trim().is_empty() {
        return Err("A billing line is missing its material code or name.".into());
    }
    for amount in [
        line.rate_cents,
        line.base_cents,
        line.minimum_cents,
        line.total_cents,
    ]
    .into_iter()
    .flatten()
    {
        if amount < 0 {
            return Err("A documentary export contains a negative billing amount.".into());
        }
    }
    if let (Some(base), Some(adjustment), Some(total)) =
        (line.base_cents, line.minimum_cents, line.total_cents)
        && base.checked_add(adjustment) != Some(total)
    {
        return Err(
            "A saved billing line's base, minimum adjustment and total do not agree.".into(),
        );
    }
    match line.unit.as_deref() {
        Some("each") => {
            if line
                .quantity
                .parse::<u32>()
                .ok()
                .is_none_or(|quantity| quantity == 0 || quantity > 999_999)
            {
                return Err("A saved each quantity must be a positive whole count.".into());
            }
        }
        Some("lb") => validate_billing_weight(&line.quantity)?,
        Some(_) => return Err("A saved billing line uses an unknown unit.".into()),
        None => (),
    }
    Ok(())
}

fn validate_billing_weight(quantity: &str) -> Result<(), String> {
    let (whole, fraction) = quantity.split_once('.').unwrap_or((quantity, ""));
    if whole.len() > 6
        || fraction.len() > 3
        || whole.len() + fraction.len() == 0
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .any(|byte| byte != b'0')
    {
        return Err(
            "A saved billing weight must be a positive pound quantity to three decimal places."
                .into(),
        );
    }
    Ok(())
}

fn csv_row(values: &[String]) -> String {
    format!(
        "{}\r\n",
        values
            .iter()
            .map(|value| csv_cell(value))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn csv_cell(value: &str) -> String {
    let trimmed = value
        .trim_start_matches(|character: char| character.is_whitespace() || character == '\u{feff}');
    let formula = trimmed.starts_with(['=', '+', '-', '@'])
        || value.chars().next().is_some_and(char::is_control);
    let protected = if formula {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if protected.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", protected.replace('"', "\"\""))
    } else {
        protected
    }
}

#[cfg(test)]
#[path = "../../tests/extended/intake/intake_export_tests.rs"]
mod tests;
