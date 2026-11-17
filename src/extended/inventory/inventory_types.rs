//! Synthetic whole-lot custody records, independent of financial tickets.
use crate::{
    auth::{OperatorSnapshot, Role},
    intake::{MaterialClass, MaterialIntake, SourceIntake},
    intake_review::{LineIdentity, ReviewDisposition},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryLocation {
    pub id: u64,
    pub name: String,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryDirectory {
    pub locations: Vec<InventoryLocation>,
    pub next_id: u64,
}

impl Default for InventoryDirectory {
    fn default() -> Self {
        Self {
            locations: vec![InventoryLocation {
                id: 1,
                name: "Training staging".into(),
                active: true,
            }],
            next_id: 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiveInput {
    pub ticket_id: u64,
    pub line_identity: LineIdentity,
    pub review_event_id: Option<u64>,
    pub location_id: u64,
    pub container: String,
    pub received_at: Option<u64>,
    pub first_accumulation_at: Option<u64>,
    pub reason: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartySnapshot {
    pub name: String,
    pub address: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipmentInput {
    pub lot_ids: Vec<u64>,
    pub destination: PartySnapshot,
    pub carrier: PartySnapshot,
    pub evidence_reference: String,
    pub shipped_at: Option<u64>,
    pub actual_weight_thousandths: Option<i64>,
    pub reason: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LotOrigin {
    pub ticket_id: u64,
    pub line_identity: LineIdentity,
    pub review_event_id: Option<u64>,
    pub review_disposition: ReviewDisposition,
    pub source: SourceIntake,
    pub material: MaterialIntake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LotStatus {
    OnHand,
    Shipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryLot {
    pub id: u64,
    pub origin: LotOrigin,
    pub location: InventoryLocation,
    pub container: String,
    pub received_at: Option<u64>,
    pub first_accumulation_at: Option<u64>,
    pub recorded_at: u64,
    pub received_by: OperatorSnapshot,
    pub status: LotStatus,
    pub shipment_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryShipment {
    pub id: u64,
    pub lot_ids: Vec<u64>,
    pub destination: PartySnapshot,
    pub carrier: PartySnapshot,
    pub evidence_reference: String,
    pub shipped_at: Option<u64>,
    pub actual_weight_thousandths: Option<i64>,
    pub recorded_at: u64,
    pub actor: OperatorSnapshot,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
// Preserve the published value-owned snapshot construction API shared by
// custody, export and recovery callers; this is a local training ledger.
#[allow(clippy::large_enum_variant)]
pub enum InventoryAction {
    Received {
        lot: InventoryLot,
    },
    Moved {
        lot_id: u64,
        from_location: InventoryLocation,
        from_container: String,
        to_location: InventoryLocation,
        to_container: String,
    },
    Shipped {
        shipment: InventoryShipment,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryEvent {
    pub id: u64,
    pub at: u64,
    pub actor: OperatorSnapshot,
    pub request_key: String,
    pub reason: String,
    pub action: InventoryAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryLedger {
    pub events: Vec<InventoryEvent>,
    pub next_id: u64,
    pub next_lot_id: u64,
    pub next_shipment_id: u64,
}

impl Default for InventoryLedger {
    fn default() -> Self {
        Self {
            events: vec![],
            next_id: 1,
            next_lot_id: 1,
            next_shipment_id: 1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InventoryState {
    pub lots: Vec<InventoryLot>,
    pub shipments: Vec<InventoryShipment>,
}

impl InventoryState {
    pub fn lot(&self, id: u64) -> Option<&InventoryLot> {
        self.lots.iter().find(|lot| lot.id == id)
    }
    pub fn shipment(&self, id: u64) -> Option<&InventoryShipment> {
        self.shipments.iter().find(|shipment| shipment.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryIssue {
    pub lot_id: u64,
    pub code: String,
    pub message: String,
}

pub(super) fn text(
    value: &str,
    label: &str,
    limit: usize,
    required: bool,
) -> Result<String, String> {
    if value.len() > limit * 4
        || value.chars().count() > limit
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} must contain at most {limit} characters without control characters."
        ));
    }
    let value = value.trim();
    if required && value.is_empty() {
        return Err(format!("Enter {label}."));
    }
    Ok(value.to_owned())
}

pub(super) fn actor_allowed(actor: &OperatorSnapshot) -> Result<(), String> {
    actor.validate()?;
    if !matches!(actor.role, Role::Supervisor | Role::Administrator) {
        return Err("A supervisor or administrator must record inventory movements.".into());
    }
    Ok(())
}

pub(super) fn request_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err("Use a unique inventory request key of 1–128 ASCII letters, digits, '-', '_', '.' or ':'.".into());
    }
    Ok(())
}

pub(super) fn dates(
    received: Option<u64>,
    accumulated: Option<u64>,
    at: u64,
) -> Result<(), String> {
    if received.is_some_and(|date| date > at)
        || accumulated.is_some_and(|date| date > at)
        || received
            .zip(accumulated)
            .is_some_and(|(received, accumulated)| accumulated > received)
    {
        return Err("Known receipt/accumulation dates must not be in the future, and accumulation must not follow the receipt date.".into());
    }
    Ok(())
}

pub(super) fn measured_weight(weight: Option<i64>) -> Result<(), String> {
    if weight.is_some_and(|weight| !(1..=999_999_999).contains(&weight)) {
        return Err(
            "Enter a positive measured shipment weight up to 999999.999 lb, or leave it unknown."
                .into(),
        );
    }
    Ok(())
}

impl PartySnapshot {
    pub(super) fn normalized(&self, required: bool) -> Result<Self, String> {
        Ok(Self {
            name: text(&self.name, "party name", 120, required)?,
            address: text(&self.address, "party address", 240, false)?,
        })
    }
}

impl LotOrigin {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.ticket_id == 0
            || self.line_identity == LineIdentity::Stable(0)
            || self.review_event_id == Some(0)
        {
            return Err("The inventory origin has invalid saved identities.".into());
        }
        self.source.validate()?;
        self.material.validate()?;
        if matches!(
            self.material.class,
            MaterialClass::LooseBattery | MaterialClass::Oil | MaterialClass::OilFilter
        ) {
            return Err("Loose batteries, oil and oil filters use a separate material workflow, outside this training e-waste inventory lane.".into());
        }
        Ok(())
    }
}

impl InventoryLocation {
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 || text(&self.name, "location name", 120, true)? != self.name {
            return Err("The inventory location has an invalid ID or name.".into());
        }
        Ok(())
    }
}

impl InventoryDirectory {
    pub fn location(&self, id: u64) -> Option<&InventoryLocation> {
        self.locations.iter().find(|location| location.id == id)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.next_id == 0 || self.locations.len() > 1_000 {
            return Err(
                "The inventory location directory has invalid sequencing or capacity.".into(),
            );
        }
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for location in &self.locations {
            location.validate()?;
            if location.id >= self.next_id
                || !ids.insert(location.id)
                || !names.insert(location.name.to_lowercase())
            {
                return Err(
                    "Inventory location IDs or names are duplicated or inconsistent.".into(),
                );
            }
        }
        Ok(())
    }

    pub fn save_location(
        &self,
        input: &InventoryLocation,
    ) -> Result<(Self, InventoryLocation), String> {
        self.validate()?;
        let name = text(&input.name, "location name", 120, true)?;
        let (id, next_id) = if input.id == 0 {
            (
                self.next_id,
                self.next_id
                    .checked_add(1)
                    .ok_or("Inventory location ID limit reached.")?,
            )
        } else {
            if self.location(input.id).is_none() {
                return Err("Inventory location not found. Reload before editing.".into());
            }
            (input.id, self.next_id)
        };
        let location = InventoryLocation {
            id,
            name,
            active: input.active,
        };
        let mut locations = self
            .locations
            .iter()
            .filter(|existing| existing.id != id)
            .cloned()
            .collect::<Vec<_>>();
        locations.push(location.clone());
        locations.sort_by_key(|location| location.id);
        let next = Self { locations, next_id };
        next.validate()?;
        Ok((next, location))
    }

    pub(super) fn active_location(&self, id: u64) -> Result<InventoryLocation, String> {
        self.validate()?;
        self.location(id)
            .filter(|location| location.active)
            .cloned()
            .ok_or_else(|| "Select an active training inventory location.".into())
    }
}
