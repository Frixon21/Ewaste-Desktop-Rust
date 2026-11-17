//! Synthetic whole-lot custody, independent of billing and regulatory eligibility.
#[path = "inventory_types.rs"]
mod types;
use crate::{
    auth::OperatorSnapshot,
    domain::{State, Ticket},
    intake::MaterialClass,
    intake_review::{LineIdentity, ReviewDisposition, ReviewLedger},
};
pub use types::*;
use types::{actor_allowed, dates, measured_weight, request_key, text};

#[path = "inventory_replay.rs"]
mod replay;

fn origin_for(
    tickets: &[Ticket],
    reviews: &ReviewLedger,
    ticket_id: u64,
    identity: LineIdentity,
    review_id: Option<u64>,
    at: u64,
) -> Result<LotOrigin, String> {
    let ticket = tickets
        .iter()
        .find(|ticket| ticket.id == ticket_id && ticket.id != 0 && ticket.state == State::Completed)
        .ok_or("Select an existing completed ticket before receiving a lot.")?;
    ticket.validate_intake_records()?;
    if ticket.created_at > at || ticket.completed_at.unwrap_or(ticket.created_at) > at {
        return Err("The selected ticket completion is later than this inventory event.".into());
    }
    let line = match identity {
        LineIdentity::Stable(id) if id > 0 => ticket.lines.iter().find(|line| line.id == id),
        LineIdentity::LegacyOrdinal(index) => ticket.lines.get(index).filter(|line| line.id == 0),
        _ => None,
    }
    .ok_or("The inventory source does not identify a frozen ticket line.")?;
    let origin = if let Some(id) = review_id {
        let event = reviews
            .events
            .iter()
            .find(|event| event.id == id && event.ticket_id == ticket_id && event.at <= at)
            .ok_or("Select an existing documentary review recorded before this inventory event.")?;
        let material = event
            .snapshot
            .materials
            .iter()
            .find(|material| material.identity == identity)
            .ok_or("The selected review does not identify this material line.")?;
        LotOrigin {
            ticket_id,
            line_identity: identity,
            review_event_id: Some(id),
            review_disposition: event.snapshot.disposition,
            source: event.snapshot.source.clone(),
            material: material.details.clone(),
        }
    } else {
        LotOrigin {
            ticket_id,
            line_identity: identity,
            review_event_id: None,
            review_disposition: ReviewDisposition::PendingReview,
            source: ticket.source_intake.clone(),
            material: line.material_intake.clone(),
        }
    };
    origin.validate()?;
    Ok(origin)
}

fn same_request(
    event: &InventoryEvent,
    actor: &OperatorSnapshot,
    reason: &str,
) -> Result<(), String> {
    if &event.actor != actor || event.reason != reason {
        return Err(
            "This inventory request key was already used with different audit details.".into(),
        );
    }
    Ok(())
}

impl InventoryLedger {
    pub fn validate_against(
        &self,
        tickets: &[Ticket],
        reviews: &ReviewLedger,
        directory: &InventoryDirectory,
    ) -> Result<(), String> {
        directory.validate()?;
        reviews.validate_against(tickets)?;
        self.project()?;
        let mut ticket_ids = std::collections::HashSet::new();
        if tickets
            .iter()
            .any(|ticket| ticket.id == 0 || !ticket_ids.insert(ticket.id))
        {
            return Err("Inventory input contains invalid or duplicate ticket identities.".into());
        }
        for event in &self.events {
            match &event.action {
                InventoryAction::Received { lot } => {
                    let expected = origin_for(
                        tickets,
                        reviews,
                        lot.origin.ticket_id,
                        lot.origin.line_identity,
                        lot.origin.review_event_id,
                        event.at,
                    )?;
                    if lot.origin != expected {
                        return Err(
                            "A received lot no longer matches its exact frozen source snapshot."
                                .into(),
                        );
                    }
                    if directory.location(lot.location.id).is_none() {
                        return Err(
                            "A historical inventory location is missing from the directory.".into(),
                        );
                    }
                }
                InventoryAction::Moved {
                    from_location,
                    to_location,
                    ..
                } => {
                    if directory.location(from_location.id).is_none()
                        || directory.location(to_location.id).is_none()
                    {
                        return Err(
                            "A historical movement location is missing from the directory.".into(),
                        );
                    }
                }
                InventoryAction::Shipped { .. } => (),
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn receive(
        &self,
        tickets: &[Ticket],
        reviews: &ReviewLedger,
        directory: &InventoryDirectory,
        input: &ReceiveInput,
        actor: &OperatorSnapshot,
        key: &str,
        at: u64,
    ) -> Result<(Self, InventoryLot), String> {
        actor_allowed(actor)?;
        request_key(key)?;
        if !input.confirmed {
            return Err("Explicitly confirm the source, reporting measurements and whole lot before receiving.".into());
        }
        self.validate_against(tickets, reviews, directory)?;
        let container = text(&input.container, "container reference", 80, false)?;
        let reason = text(&input.reason, "inventory reason", 500, true)?;
        if let Some(event) = self.events.iter().find(|event| event.request_key == key) {
            same_request(event, actor, &reason)?;
            match &event.action {
                InventoryAction::Received { lot }
                    if lot.origin.ticket_id == input.ticket_id
                        && lot.origin.line_identity == input.line_identity
                        && lot.origin.review_event_id == input.review_event_id
                        && lot.location.id == input.location_id
                        && lot.container == container
                        && lot.received_at == input.received_at
                        && lot.first_accumulation_at == input.first_accumulation_at =>
                {
                    return Ok((
                        self.clone(),
                        self.project()?
                            .lot(lot.id)
                            .ok_or("Received lot missing.")?
                            .clone(),
                    ));
                }
                _ => {
                    return Err(
                        "This inventory request key belongs to a different receive or movement."
                            .into(),
                    );
                }
            }
        }
        dates(input.received_at, input.first_accumulation_at, at)?;
        let origin = origin_for(
            tickets,
            reviews,
            input.ticket_id,
            input.line_identity,
            input.review_event_id,
            at,
        )?;
        let location = directory.active_location(input.location_id)?;
        let lot = InventoryLot {
            id: self.next_lot_id,
            origin,
            location,
            container,
            received_at: input.received_at,
            first_accumulation_at: input.first_accumulation_at,
            recorded_at: at,
            received_by: actor.clone(),
            status: LotStatus::OnHand,
            shipment_id: None,
        };
        let event = InventoryEvent {
            id: self.next_id,
            at,
            actor: actor.clone(),
            request_key: key.into(),
            reason,
            action: InventoryAction::Received { lot: lot.clone() },
        };
        let next = self.append(event)?;
        next.validate_against(tickets, reviews, directory)?;
        Ok((next, lot))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn move_lot(
        &self,
        directory: &InventoryDirectory,
        lot_id: u64,
        to_location_id: u64,
        container: &str,
        reason: &str,
        actor: &OperatorSnapshot,
        key: &str,
        at: u64,
    ) -> Result<(Self, InventoryLot), String> {
        actor_allowed(actor)?;
        request_key(key)?;
        directory.validate()?;
        let state = self.project()?;
        let container = text(container, "container reference", 80, false)?;
        let reason = text(reason, "inventory reason", 500, true)?;
        if let Some(event) = self.events.iter().find(|event| event.request_key == key) {
            same_request(event, actor, &reason)?;
            match &event.action {
                InventoryAction::Moved {
                    lot_id: old_id,
                    to_location,
                    to_container,
                    ..
                } if *old_id == lot_id
                    && to_location.id == to_location_id
                    && *to_container == container =>
                {
                    return Ok((
                        self.clone(),
                        state.lot(lot_id).ok_or("Moved lot missing.")?.clone(),
                    ));
                }
                _ => {
                    return Err(
                        "This inventory request key belongs to a different movement.".into(),
                    );
                }
            }
        }
        let previous = state.lot(lot_id).ok_or("Inventory lot not found.")?;
        let location = directory.active_location(to_location_id)?;
        let event = InventoryEvent {
            id: self.next_id,
            at,
            actor: actor.clone(),
            request_key: key.into(),
            reason,
            action: InventoryAction::Moved {
                lot_id,
                from_location: previous.location.clone(),
                from_container: previous.container.clone(),
                to_location: location,
                to_container: container,
            },
        };
        let next = self.append(event)?;
        let current = next
            .project()?
            .lot(lot_id)
            .ok_or("Moved lot missing.")?
            .clone();
        Ok((next, current))
    }

    pub fn ship(
        &self,
        directory: &InventoryDirectory,
        input: &ShipmentInput,
        actor: &OperatorSnapshot,
        key: &str,
        at: u64,
    ) -> Result<(Self, InventoryShipment), String> {
        actor_allowed(actor)?;
        request_key(key)?;
        directory.validate()?;
        let state = self.project()?;
        if !input.confirmed {
            return Err("Explicitly confirm the selected whole lots and outbound evidence.".into());
        }
        let reason = text(&input.reason, "inventory reason", 500, true)?;
        let destination = input.destination.normalized(true)?;
        let carrier = input.carrier.normalized(false)?;
        let evidence_reference = text(
            &input.evidence_reference,
            "shipment evidence reference",
            120,
            true,
        )?;
        measured_weight(input.actual_weight_thousandths)?;
        let mut lot_ids = input.lot_ids.clone();
        lot_ids.sort_unstable();
        if lot_ids.is_empty()
            || lot_ids.len() > 10_000
            || lot_ids.windows(2).any(|pair| pair[0] == pair[1])
            || lot_ids.contains(&0)
        {
            return Err(
                "Select each existing whole lot once, up to 10000 lots per shipment.".into(),
            );
        }
        if let Some(event) = self.events.iter().find(|event| event.request_key == key) {
            same_request(event, actor, &reason)?;
            match &event.action {
                InventoryAction::Shipped { shipment }
                    if shipment.lot_ids == lot_ids
                        && shipment.destination == destination
                        && shipment.carrier == carrier
                        && shipment.evidence_reference == evidence_reference
                        && shipment.shipped_at == input.shipped_at
                        && shipment.actual_weight_thousandths
                            == input.actual_weight_thousandths =>
                {
                    return Ok((
                        self.clone(),
                        state
                            .shipment(shipment.id)
                            .ok_or("Saved shipment missing.")?
                            .clone(),
                    ));
                }
                _ => {
                    return Err(
                        "This inventory request key belongs to a different shipment.".into(),
                    );
                }
            }
        }
        let shipment = InventoryShipment {
            id: self.next_shipment_id,
            lot_ids,
            destination,
            carrier,
            evidence_reference,
            shipped_at: input.shipped_at,
            actual_weight_thousandths: input.actual_weight_thousandths,
            recorded_at: at,
            actor: actor.clone(),
            reason: reason.clone(),
        };
        let event = InventoryEvent {
            id: self.next_id,
            at,
            actor: actor.clone(),
            request_key: key.into(),
            reason,
            action: InventoryAction::Shipped {
                shipment: shipment.clone(),
            },
        };
        let next = self.append(event)?;
        Ok((next, shipment))
    }

    fn append(&self, event: InventoryEvent) -> Result<Self, String> {
        let receiving = matches!(&event.action, InventoryAction::Received { .. });
        let shipping = matches!(&event.action, InventoryAction::Shipped { .. });
        let next = Self {
            events: self
                .events
                .iter()
                .cloned()
                .chain(std::iter::once(event))
                .collect(),
            next_id: self
                .next_id
                .checked_add(1)
                .ok_or("Inventory event ID limit reached.")?,
            next_lot_id: if receiving {
                self.next_lot_id
                    .checked_add(1)
                    .ok_or("Inventory lot ID limit reached.")?
            } else {
                self.next_lot_id
            },
            next_shipment_id: if shipping {
                self.next_shipment_id
                    .checked_add(1)
                    .ok_or("Shipment ID limit reached.")?
            } else {
                self.next_shipment_id
            },
        };
        next.project()?;
        Ok(next)
    }

    pub fn issues(
        &self,
        tickets: &[Ticket],
        reviews: &ReviewLedger,
        directory: &InventoryDirectory,
    ) -> Result<Vec<InventoryIssue>, String> {
        self.validate_against(tickets, reviews, directory)?;
        let mut issues = Vec::new();
        for lot in self.project()?.lots {
            let mut add = |code: &str, message: &str| {
                issues.push(InventoryIssue {
                    lot_id: lot.id,
                    code: code.into(),
                    message: message.into(),
                })
            };
            if lot.received_at.is_none() {
                add(
                    "received-date-unknown",
                    "Actual receipt date is unknown; the recording time is separate.",
                );
            }
            if lot.first_accumulation_at.is_none() {
                add(
                    "accumulation-date-unknown",
                    "First accumulation date is unknown and was not inferred.",
                );
            }
            if lot.container.is_empty() {
                add("container-unknown", "Container reference is unknown.");
            }
            if lot.origin.material.reporting_units.is_none() {
                add(
                    "reporting-units-unknown",
                    "Reporting units are unknown and were not copied from billing.",
                );
            }
            if lot.origin.material.actual_weight_thousandths.is_none() {
                add(
                    "weight-unknown",
                    "Actual intake weight is unknown; a per-unit item need not have a recorded weight.",
                );
            }
            if matches!(
                lot.origin.material.class,
                MaterialClass::Unclassified | MaterialClass::Other
            ) {
                add(
                    "material-review",
                    "Material stream requires documentary review; no routing or eligibility decision was made.",
                );
            }
            if lot.origin.review_disposition == ReviewDisposition::PendingReview {
                add(
                    "documentary-review",
                    "The received source snapshot is pending documentary review.",
                );
            }
            if lot.status == LotStatus::OnHand
                && directory
                    .location(lot.location.id)
                    .is_some_and(|location| !location.active)
            {
                add(
                    "location-inactive",
                    "The on-hand lot references an inactive location; its historical snapshot is preserved.",
                );
            }
            if let Some(latest) = reviews.latest(lot.origin.ticket_id) {
                let material = latest
                    .snapshot
                    .materials
                    .iter()
                    .find(|material| material.identity == lot.origin.line_identity)
                    .ok_or(
                        "A current documentary review has an unmatched inventory source line.",
                    )?;
                if latest.snapshot.source != lot.origin.source
                    || material.details != lot.origin.material
                    || latest.snapshot.disposition != lot.origin.review_disposition
                {
                    add(
                        "source-review-changed",
                        "A later documentary review differs from the pinned source; the received lot and shipment were not rewritten.",
                    );
                }
            }
        }
        Ok(issues)
    }
}

#[cfg(test)]
#[path = "../../tests/extended/inventory/inventory_tests.rs"]
mod tests;
