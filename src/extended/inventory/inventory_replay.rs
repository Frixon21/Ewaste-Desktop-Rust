//! Deterministic custody replay. Every transition preserves whole-lot identity.
use super::types::{actor_allowed, dates, measured_weight, request_key, text};
use super::*;
use std::collections::HashSet;

impl InventoryLedger {
    pub fn project(&self) -> Result<InventoryState, String> {
        let mut state = InventoryState::default();
        let mut next_id = 1u64;
        let mut next_lot = 1u64;
        let mut next_shipment = 1u64;
        let mut last_at = 0;
        let mut keys = HashSet::new();
        let mut origins = HashSet::new();
        for event in &self.events {
            actor_allowed(&event.actor)?;
            request_key(&event.request_key)?;
            if event.id != next_id
                || event.at < last_at
                || !keys.insert(event.request_key.as_str())
                || text(&event.reason, "inventory reason", 500, true)? != event.reason
            {
                return Err("Inventory event sequence, chronology, request keys or audit details are inconsistent.".into());
            }
            match &event.action {
                InventoryAction::Received { lot } => {
                    validate_received(event, lot, next_lot)?;
                    if !origins.insert((lot.origin.ticket_id, lot.origin.line_identity)) {
                        return Err("A frozen ticket line has already been received into inventory, including previously shipped lots.".into());
                    }
                    state.lots.push(lot.clone());
                    next_lot = next_lot
                        .checked_add(1)
                        .ok_or("Inventory lot sequence exhausted.")?;
                }
                InventoryAction::Moved {
                    lot_id,
                    from_location,
                    from_container,
                    to_location,
                    to_container,
                } => {
                    let index = state
                        .lots
                        .iter()
                        .position(|lot| lot.id == *lot_id)
                        .ok_or("Moved inventory lot not found.")?;
                    let lot = &state.lots[index];
                    to_location.validate()?;
                    if lot.status != LotStatus::OnHand
                        || lot.location != *from_location
                        || lot.container != *from_container
                        || !to_location.active
                        || text(to_container, "container reference", 80, false)? != *to_container
                        || lot.location.id == to_location.id && lot.container == *to_container
                    {
                        return Err("A whole-lot move must start at its recorded location/container and make an actual change to an active destination.".into());
                    }
                    state.lots[index] = InventoryLot {
                        location: to_location.clone(),
                        container: to_container.clone(),
                        ..lot.clone()
                    };
                }
                InventoryAction::Shipped { shipment } => {
                    validate_shipment(event, shipment, next_shipment, &state)?;
                    for lot in &mut state.lots {
                        if shipment.lot_ids.contains(&lot.id) {
                            *lot = InventoryLot {
                                status: LotStatus::Shipped,
                                shipment_id: Some(shipment.id),
                                ..lot.clone()
                            };
                        }
                    }
                    state.shipments.push(shipment.clone());
                    next_shipment = next_shipment
                        .checked_add(1)
                        .ok_or("Inventory shipment sequence exhausted.")?;
                }
            }
            last_at = event.at;
            next_id = next_id
                .checked_add(1)
                .ok_or("Inventory event sequence exhausted.")?;
        }
        if self.next_id != next_id
            || self.next_lot_id != next_lot
            || self.next_shipment_id != next_shipment
        {
            return Err("Saved inventory counters do not match their append-only history.".into());
        }
        Ok(state)
    }
}

fn validate_received(
    event: &InventoryEvent,
    lot: &InventoryLot,
    expected_id: u64,
) -> Result<(), String> {
    lot.origin.validate()?;
    lot.location.validate()?;
    dates(lot.received_at, lot.first_accumulation_at, event.at)?;
    if lot.id != expected_id
        || lot.recorded_at != event.at
        || lot.received_by != event.actor
        || lot.status != LotStatus::OnHand
        || lot.shipment_id.is_some()
        || !lot.location.active
        || text(&lot.container, "container reference", 80, false)? != lot.container
    {
        return Err("A received lot must retain its original identity, actor, dates and active location snapshot.".into());
    }
    Ok(())
}

fn validate_shipment(
    event: &InventoryEvent,
    shipment: &InventoryShipment,
    expected_id: u64,
    state: &InventoryState,
) -> Result<(), String> {
    measured_weight(shipment.actual_weight_thousandths)?;
    if shipment.id != expected_id
        || shipment.recorded_at != event.at
        || shipment.actor != event.actor
        || shipment.reason != event.reason
        || shipment.destination.normalized(true)? != shipment.destination
        || shipment.carrier.normalized(false)? != shipment.carrier
        || text(
            &shipment.evidence_reference,
            "shipment evidence reference",
            120,
            true,
        )? != shipment.evidence_reference
        || shipment.shipped_at.is_some_and(|date| date > event.at)
    {
        return Err("A shipment must retain valid independently recorded party, evidence, date and actor snapshots.".into());
    }
    let ids = &shipment.lot_ids;
    if ids.is_empty()
        || ids.len() > 10_000
        || ids.contains(&0)
        || ids.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(
            "A shipment must identify each existing whole lot exactly once in stable order.".into(),
        );
    }
    for id in ids {
        let lot = state
            .lot(*id)
            .ok_or("A selected shipment lot does not exist.")?;
        if lot.status != LotStatus::OnHand || lot.shipment_id.is_some() {
            return Err("A shipped lot cannot be shipped or allocated again.".into());
        }
        if shipment.shipped_at.is_some_and(|date| {
            lot.received_at.is_some_and(|received| date < received)
                || lot
                    .first_accumulation_at
                    .is_some_and(|accumulated| date < accumulated)
        }) {
            return Err("A reported shipment date cannot precede a known physical receipt or accumulation date.".into());
        }
    }
    Ok(())
}
