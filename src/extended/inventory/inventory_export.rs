//! Training custody CSVs. Source records and actual measurements come only from
//! pinned lot snapshots; financial quantities and prices are never weight inputs.
use crate::{
    domain::Ticket,
    intake_review::{LineIdentity, ReviewLedger},
    inventory::{
        InventoryAction, InventoryDirectory, InventoryEvent, InventoryLedger, InventoryLocation,
        InventoryLot, InventoryShipment, InventoryState,
    },
};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryExports {
    pub lots: String,
    pub movements: String,
    pub shipments: String,
}

const LOT_HEADERS: &[&str] = &[
    "training_lot_id",
    "training_origin_ticket_id",
    "training_origin_line_identity",
    "training_origin_review_event_id",
    "training_documentary_disposition",
    "training_material_class",
    "training_material_condition",
    "training_reporting_unit_count",
    "training_actual_weight_lb",
    "training_lot_status",
    "training_location_id",
    "training_location_name",
    "training_container",
    "training_received_at_unix_seconds",
    "training_first_accumulation_at_unix_seconds",
    "training_recorded_at_unix_seconds",
    "training_received_actor_id",
    "training_received_actor_name",
    "training_shipment_id",
    "training_shipment_evidence_reference",
    "training_documentary_issue_count",
];
const CONTACT_HEADERS: &[&str] = &[
    "training_source_name",
    "training_source_address",
    "training_source_organization",
    "training_source_contact_name",
    "training_source_phone",
];
const MOVEMENT_HEADERS: &[&str] = &[
    "training_event_id",
    "training_event_type",
    "training_recorded_at_unix_seconds",
    "training_actor_id",
    "training_actor_name",
    "training_lot_id",
    "training_shipment_id",
    "training_from_location_id",
    "training_from_location_name",
    "training_from_container",
    "training_to_location_id",
    "training_to_location_name",
    "training_to_container",
    "training_evidence_reference",
    "training_reason_recorded",
];
const SHIPMENT_HEADERS: &[&str] = &[
    "training_shipment_id",
    "training_lot_ids",
    "training_shipped_at_unix_seconds",
    "training_recorded_at_unix_seconds",
    "training_actor_id",
    "training_actor_name",
    "training_actual_weight_lb",
    "training_evidence_reference",
    "training_destination_name",
    "training_carrier_name",
];

pub fn export_inventory(
    tickets: &[Ticket],
    reviews: &ReviewLedger,
    directory: &InventoryDirectory,
    ledger: &InventoryLedger,
    include_contacts: bool,
) -> Result<InventoryExports, String> {
    ledger.validate_against(tickets, reviews, directory)?;
    let state = ledger.project()?;
    let mut issue_counts: HashMap<u64, usize> = HashMap::new();
    for issue in ledger.issues(tickets, reviews, directory)? {
        *issue_counts.entry(issue.lot_id).or_default() += 1;
    }
    let lots = export_lots(&state, &issue_counts, include_contacts)?;
    let movements = export_movements(ledger)?;
    let shipments = export_shipments(&state, include_contacts);
    Ok(InventoryExports {
        lots,
        movements,
        shipments,
    })
}

fn export_lots(
    state: &InventoryState,
    issues: &HashMap<u64, usize>,
    contacts: bool,
) -> Result<String, String> {
    let mut headers = LOT_HEADERS.to_vec();
    if contacts {
        headers.extend_from_slice(CONTACT_HEADERS);
    }
    let mut csv = header(&headers);
    let mut lots = state.lots.iter().collect::<Vec<_>>();
    lots.sort_by_key(|lot| lot.id);
    for lot in lots {
        let reference = match lot.shipment_id {
            Some(id) => state
                .shipment(id)
                .ok_or("An inventory lot refers to a missing saved shipment.")?
                .evidence_reference
                .clone(),
            None => String::new(),
        };
        let mut row = lot_row(lot, &reference, issues.get(&lot.id).copied().unwrap_or(0));
        if contacts {
            let source = &lot.origin.source;
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
    Ok(csv)
}

fn lot_row(lot: &InventoryLot, reference: &str, issues: usize) -> Vec<String> {
    let origin = &lot.origin;
    let material = &origin.material;
    vec![
        lot.id.to_string(),
        origin.ticket_id.to_string(),
        identity(origin.line_identity),
        optional(origin.review_event_id),
        format!("{:?}", origin.review_disposition),
        format!("{:?}", material.class),
        format!("{:?}", material.condition),
        optional(material.reporting_units),
        weight(material.actual_weight_thousandths),
        format!("{:?}", lot.status),
        lot.location.id.to_string(),
        lot.location.name.clone(),
        lot.container.clone(),
        optional(lot.received_at),
        optional(lot.first_accumulation_at),
        lot.recorded_at.to_string(),
        lot.received_by.id.to_string(),
        lot.received_by.display_name.clone(),
        optional(lot.shipment_id),
        reference.into(),
        issues.to_string(),
    ]
}

fn export_movements(ledger: &InventoryLedger) -> Result<String, String> {
    let mut csv = header(MOVEMENT_HEADERS);
    let mut positions: HashMap<u64, (&InventoryLocation, &str)> = HashMap::new();
    let mut events = ledger.events.iter().collect::<Vec<_>>();
    events.sort_by_key(|event| event.id);
    for event in events {
        match &event.action {
            InventoryAction::Received { lot } => {
                positions.insert(lot.id, (&lot.location, &lot.container));
                csv.push_str(&csv_row(&movement_row(
                    event,
                    "Received",
                    (Some(lot.id), None),
                    (None, Some(&lot.location)),
                    ("", &lot.container),
                    "",
                )));
            }
            InventoryAction::Moved {
                lot_id,
                from_location,
                from_container,
                to_location,
                to_container,
            } => {
                positions.insert(*lot_id, (to_location, to_container));
                csv.push_str(&csv_row(&movement_row(
                    event,
                    "Moved",
                    (Some(*lot_id), None),
                    (Some(from_location), Some(to_location)),
                    (from_container, to_container),
                    "",
                )));
            }
            InventoryAction::Shipped { shipment } => {
                let mut ids = shipment.lot_ids.clone();
                ids.sort_unstable();
                for id in ids {
                    let (location, container) = positions
                        .get(&id)
                        .copied()
                        .ok_or("A shipment movement has no received lot location.")?;
                    csv.push_str(&csv_row(&movement_row(
                        event,
                        "Shipped",
                        (Some(id), Some(shipment.id)),
                        (Some(location), None),
                        (container, ""),
                        &shipment.evidence_reference,
                    )));
                }
            }
        }
    }
    Ok(csv)
}

fn movement_row(
    event: &InventoryEvent,
    kind: &str,
    ids: (Option<u64>, Option<u64>),
    locations: (Option<&InventoryLocation>, Option<&InventoryLocation>),
    containers: (&str, &str),
    reference: &str,
) -> Vec<String> {
    vec![
        event.id.to_string(),
        kind.into(),
        event.at.to_string(),
        event.actor.id.to_string(),
        event.actor.display_name.clone(),
        optional(ids.0),
        optional(ids.1),
        optional(locations.0.map(|location| location.id)),
        locations
            .0
            .map(|location| location.name.clone())
            .unwrap_or_default(),
        containers.0.into(),
        optional(locations.1.map(|location| location.id)),
        locations
            .1
            .map(|location| location.name.clone())
            .unwrap_or_default(),
        containers.1.into(),
        reference.into(),
        (!event.reason.trim().is_empty()).to_string(),
    ]
}

fn export_shipments(state: &InventoryState, contacts: bool) -> String {
    let mut headers = SHIPMENT_HEADERS.to_vec();
    if contacts {
        headers.extend(["training_destination_address", "training_carrier_address"]);
    }
    let mut csv = header(&headers);
    let mut shipments = state.shipments.iter().collect::<Vec<_>>();
    shipments.sort_by_key(|shipment| shipment.id);
    for shipment in shipments {
        let mut row = shipment_row(shipment);
        if contacts {
            row.extend([
                shipment.destination.address.clone(),
                shipment.carrier.address.clone(),
            ]);
        }
        csv.push_str(&csv_row(&row));
    }
    csv
}

fn shipment_row(shipment: &InventoryShipment) -> Vec<String> {
    let mut lots = shipment.lot_ids.clone();
    lots.sort_unstable();
    vec![
        shipment.id.to_string(),
        lots.iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("|"),
        optional(shipment.shipped_at),
        shipment.recorded_at.to_string(),
        shipment.actor.id.to_string(),
        shipment.actor.display_name.clone(),
        weight(shipment.actual_weight_thousandths),
        shipment.evidence_reference.clone(),
        shipment.destination.name.clone(),
        shipment.carrier.name.clone(),
    ]
}

fn identity(value: LineIdentity) -> String {
    match value {
        LineIdentity::Stable(id) => format!("stable:{id}"),
        LineIdentity::LegacyOrdinal(index) => format!("legacy-ordinal:{index}"),
    }
}
fn optional(value: Option<impl ToString>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}
fn weight(value: Option<i64>) -> String {
    value
        .map(|value| format!("{}.{:03}", value / 1000, value % 1000))
        .unwrap_or_default()
}
fn header(columns: &[&str]) -> String {
    csv_row(
        &columns
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
}
fn csv_row(cells: &[String]) -> String {
    format!(
        "{}\r\n",
        cells
            .iter()
            .map(|cell| csv_cell(cell))
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn csv_cell(value: &str) -> String {
    let significant =
        value.trim_start_matches(|c: char| c.is_whitespace() || c.is_control() || c == '\u{feff}');
    let formula = significant.starts_with(['=', '+', '-', '@'])
        || value
            .chars()
            .next()
            .is_some_and(|c| c.is_control() || c == '\u{feff}');
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
#[path = "../../tests/extended/inventory/inventory_export_tests.rs"]
mod tests;
