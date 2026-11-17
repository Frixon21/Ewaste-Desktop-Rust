//! Whole-lot custody editor. Documentary snapshots are pinned, never inferred from billing.
use crate::{
    auth::{OperatorSnapshot, Role},
    domain::{State, Ticket},
    intake_review::{LineIdentity, ReviewLedger, ReviewSnapshot},
    inventory::*,
};
use eframe::egui;
use std::sync::atomic::{AtomicU64, Ordering};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InventoryUiAction {
    SaveLocation {
        location: InventoryLocation,
    },
    Receive {
        input: ReceiveInput,
        request_key: String,
    },
    Move {
        lot_id: u64,
        to_location_id: u64,
        container: String,
        reason: String,
        request_key: String,
    },
    Ship {
        input: ShipmentInput,
        request_key: String,
    },
    Refresh,
    Export {
        include_contacts: bool,
    },
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    OnHand,
    Receive,
    Move,
    Ship,
    Locations,
    History,
}
#[derive(Clone, PartialEq, Eq)]
struct Snapshot {
    tickets: Vec<Ticket>,
    reviews: ReviewLedger,
    directory: InventoryDirectory,
    ledger: InventoryLedger,
    actor: OperatorSnapshot,
}
pub struct InventoryUi {
    pub open: bool,
    base: Option<Snapshot>,
    pending: Option<InventoryUiAction>,
    message: Option<String>,
    tab: Tab,
    ticket_id: Option<u64>,
    identity: Option<LineIdentity>,
    review_event_id: Option<u64>,
    location_id: u64,
    container: String,
    received_date: String,
    accumulation_date: String,
    reason: String,
    confirmed: bool,
    move_lot: Option<u64>,
    ship_lots: Vec<u64>,
    destination_name: String,
    destination_address: String,
    carrier_name: String,
    carrier_address: String,
    evidence: String,
    shipped_date: String,
    shipment_weight: String,
    location_edit_id: Option<u64>,
    location_name: String,
    location_active: bool,
    include_contacts: bool,
}
impl Default for InventoryUi {
    fn default() -> Self {
        Self {
            open: false,
            base: None,
            pending: None,
            message: None,
            tab: Tab::default(),
            ticket_id: None,
            identity: None,
            review_event_id: None,
            location_id: 0,
            container: String::new(),
            received_date: String::new(),
            accumulation_date: String::new(),
            reason: String::new(),
            confirmed: false,
            move_lot: None,
            ship_lots: vec![],
            destination_name: String::new(),
            destination_address: String::new(),
            carrier_name: String::new(),
            carrier_address: String::new(),
            evidence: String::new(),
            shipped_date: String::new(),
            shipment_weight: String::new(),
            location_edit_id: None,
            location_name: String::new(),
            location_active: true,
            include_contacts: false,
        }
    }
}
fn allowed(actor: &OperatorSnapshot) -> Result<(), String> {
    actor.validate()?;
    if matches!(actor.role, Role::Supervisor | Role::Administrator) {
        Ok(())
    } else {
        Err("A supervisor or administrator is required for inventory custody.".into())
    }
}
fn bounded(input: &str, label: &str, max: usize, required: bool) -> Result<String, String> {
    if input.chars().count() > max
        || input.chars().any(char::is_control)
        || required && input.trim().is_empty()
    {
        return Err(format!(
            "Enter {label} using {}at most {max} characters without control characters.",
            if required { "1 to " } else { "" }
        ));
    }
    Ok(input.trim().to_owned())
}
fn optional_date(input: &str) -> Result<Option<u64>, String> {
    if input.trim().is_empty() {
        Ok(None)
    } else {
        crate::catalog_ui::parse_utc_date(input.trim()).map(Some)
    }
}
fn shipment_weight(input: &str) -> Result<Option<i64>, String> {
    if input.trim().is_empty() {
        return Ok(None);
    }
    let value = crate::domain::decimal(
        input,
        3,
        "Enter a positive shipment net weight in pounds with up to three decimal places.",
    )?;
    if value <= 0 {
        return Err("Shipment net weight must be positive, or blank when unknown.".into());
    }
    Ok(Some(value))
}
fn key() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!(
        "inventory:{}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}
impl InventoryUi {
    pub fn error(&mut self, message: String) {
        self.message = Some(message);
        self.open = true;
    }
    pub fn saved(&mut self) {
        self.locked();
    }
    pub fn locked(&mut self) {
        *self = Self::default();
    }
    fn bind(
        &mut self,
        tickets: &[Ticket],
        reviews: &ReviewLedger,
        directory: &InventoryDirectory,
        ledger: &InventoryLedger,
        actor: &OperatorSnapshot,
    ) {
        self.locked();
        self.open = true;
        self.base = Some(Snapshot {
            tickets: tickets.to_vec(),
            reviews: reviews.clone(),
            directory: directory.clone(),
            ledger: ledger.clone(),
            actor: actor.clone(),
        });
        self.location_id = directory
            .locations
            .iter()
            .find(|l| l.active)
            .map_or(0, |l| l.id);
        if let Some(ticket) = tickets
            .iter()
            .find(|t| t.state == State::Completed && !t.lines.is_empty())
        {
            self.ticket_id = Some(ticket.id);
            self.identity = LineIdentity::from_ticket(ticket, 0).ok();
        }
    }
    fn stale(
        &self,
        tickets: &[Ticket],
        reviews: &ReviewLedger,
        directory: &InventoryDirectory,
        ledger: &InventoryLedger,
        actor: &OperatorSnapshot,
    ) -> bool {
        self.base.as_ref().is_none_or(|base| {
            base.tickets != tickets
                || &base.reviews != reviews
                || &base.directory != directory
                || &base.ledger != ledger
                || &base.actor != actor
        })
    }
    fn base(&self) -> Result<&Snapshot, String> {
        self.base
            .as_ref()
            .ok_or_else(|| "Open saved inventory before continuing.".into())
    }
    fn guard(&self) -> Result<&Snapshot, String> {
        let base = self.base()?;
        allowed(&base.actor)?;
        if self.pending.is_some() {
            return Err(
                "Retry the exact pending action or refresh before making another change.".into(),
            );
        }
        base.ledger
            .validate_against(&base.tickets, &base.reviews, &base.directory)?;
        Ok(base)
    }
    fn state(&self) -> Result<InventoryState, String> {
        let base = self.base()?;
        base.ledger
            .validate_against(&base.tickets, &base.reviews, &base.directory)?;
        base.ledger.project()
    }
    fn selected_origin(&self) -> Result<LotOrigin, String> {
        let base = self.base()?;
        let ticket = base
            .tickets
            .iter()
            .find(|t| Some(t.id) == self.ticket_id && t.state == State::Completed)
            .ok_or("Choose a saved completed ticket.")?;
        let identity = self.identity.ok_or("Choose a frozen material line.")?;
        let snapshot = match self.review_event_id {
            Some(id) => base
                .reviews
                .events
                .iter()
                .find(|e| e.id == id && e.ticket_id == ticket.id)
                .ok_or("The selected documentary review no longer belongs to this ticket.")?
                .snapshot
                .normalized_for(ticket)?,
            None => ReviewSnapshot::from_ticket(ticket)?,
        };
        let material = snapshot
            .materials
            .iter()
            .find(|m| m.identity == identity)
            .ok_or("Choose a material present in this exact documentary snapshot.")?
            .details
            .clone();
        Ok(LotOrigin {
            ticket_id: ticket.id,
            line_identity: identity,
            review_event_id: self.review_event_id,
            review_disposition: snapshot.disposition,
            source: snapshot.source,
            material,
        })
    }
    fn active_location(&self) -> Result<u64, String> {
        if self
            .base()?
            .directory
            .locations
            .iter()
            .any(|l| l.id == self.location_id && l.active)
        {
            Ok(self.location_id)
        } else {
            Err("Choose an active saved location.".into())
        }
    }
    fn freeze(&mut self, action: InventoryUiAction) -> Result<InventoryUiAction, String> {
        self.pending = Some(action.clone());
        Ok(action)
    }
    fn receive_action(&mut self) -> Result<InventoryUiAction, String> {
        self.guard()?;
        let origin = self.selected_origin()?;
        if self.state()?.lots.iter().any(|lot| {
            lot.origin.ticket_id == origin.ticket_id
                && lot.origin.line_identity == origin.line_identity
        }) {
            return Err("This frozen material line was already received. Use its existing whole lot for movement or shipment.".into());
        }
        if !self.confirmed {
            return Err(
                "Confirm the pinned source and material snapshot before receiving this whole lot."
                    .into(),
            );
        }
        let received_at = optional_date(&self.received_date)?;
        let first_accumulation_at = optional_date(&self.accumulation_date)?;
        if matches!((first_accumulation_at, received_at), (Some(first), Some(received)) if first > received)
        {
            return Err("First accumulation cannot be after the known received date. Leave unknown dates blank.".into());
        }
        let input = ReceiveInput {
            ticket_id: origin.ticket_id,
            line_identity: origin.line_identity,
            review_event_id: origin.review_event_id,
            location_id: self.active_location()?,
            container: bounded(&self.container, "a container reference", 80, false)?,
            received_at,
            first_accumulation_at,
            reason: bounded(&self.reason, "a receiving reason", 500, true)?,
            confirmed: true,
        };
        self.freeze(InventoryUiAction::Receive {
            input,
            request_key: key(),
        })
    }
    fn move_action(&mut self) -> Result<InventoryUiAction, String> {
        self.guard()?;
        let state = self.state()?;
        let lot = state
            .lot(self.move_lot.ok_or("Choose an on-hand lot.")?)
            .filter(|l| l.status == LotStatus::OnHand)
            .ok_or("Choose an on-hand whole lot.")?;
        if !self.confirmed {
            return Err("Confirm movement of the entire selected lot.".into());
        }
        let to_location_id = self.active_location()?;
        let container = bounded(&self.container, "a container reference", 80, false)?;
        if lot.location.id == to_location_id && lot.container == container {
            return Err("Choose a different location or container for this movement.".into());
        }
        self.freeze(InventoryUiAction::Move {
            lot_id: lot.id,
            to_location_id,
            container,
            reason: bounded(&self.reason, "a movement reason", 500, true)?,
            request_key: key(),
        })
    }
    fn ship_action(&mut self) -> Result<InventoryUiAction, String> {
        self.guard()?;
        let state = self.state()?;
        if self.ship_lots.is_empty() {
            return Err("Select at least one on-hand whole lot.".into());
        }
        if !self.confirmed {
            return Err("Confirm shipment of every selected whole lot.".into());
        }
        let mut lot_ids = self.ship_lots.clone();
        lot_ids.sort_unstable();
        lot_ids.dedup();
        if lot_ids.iter().any(|id| {
            state
                .lot(*id)
                .is_none_or(|lot| lot.status != LotStatus::OnHand)
        }) {
            return Err("A selected lot is no longer on hand. Refresh saved inventory.".into());
        }
        let shipped_at = optional_date(&self.shipped_date)?;
        if shipped_at.is_some_and(|shipped| {
            lot_ids.iter().filter_map(|id| state.lot(*id)).any(|lot| {
                [lot.received_at, lot.first_accumulation_at]
                    .into_iter()
                    .flatten()
                    .any(|date| date > shipped)
            })
        }) {
            return Err("The reported shipment date cannot precede a selected lot's known received or first accumulation date.".into());
        }
        let input = ShipmentInput {
            lot_ids,
            destination: PartySnapshot {
                name: bounded(&self.destination_name, "a destination name", 120, true)?,
                address: bounded(
                    &self.destination_address,
                    "a destination address",
                    240,
                    false,
                )?,
            },
            carrier: PartySnapshot {
                name: bounded(&self.carrier_name, "a carrier name", 120, false)?,
                address: bounded(&self.carrier_address, "a carrier address", 240, false)?,
            },
            evidence_reference: bounded(
                &self.evidence,
                "a shipment evidence reference",
                120,
                true,
            )?,
            shipped_at,
            actual_weight_thousandths: shipment_weight(&self.shipment_weight)?,
            reason: bounded(&self.reason, "a shipment reason", 500, true)?,
            confirmed: true,
        };
        self.freeze(InventoryUiAction::Ship {
            input,
            request_key: key(),
        })
    }
    fn location_action(&mut self) -> Result<InventoryUiAction, String> {
        let base = self.guard()?;
        let id = self.location_edit_id.unwrap_or(0);
        if self.location_edit_id.is_some() && !base.directory.locations.iter().any(|l| l.id == id) {
            return Err("The location was removed. Refresh saved inventory.".into());
        }
        let location = InventoryLocation {
            id,
            name: bounded(&self.location_name, "a location name", 120, true)?,
            active: self.location_active,
        };
        self.freeze(InventoryUiAction::SaveLocation { location })
    }
    fn retry_action(&self) -> Result<InventoryUiAction, String> {
        allowed(&self.base()?.actor)?;
        self.pending
            .clone()
            .ok_or_else(|| "There is no inventory action to retry.".into())
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tickets: &[Ticket],
        reviews: &ReviewLedger,
        directory: &InventoryDirectory,
        ledger: &InventoryLedger,
        actor: &OperatorSnapshot,
    ) -> Option<InventoryUiAction> {
        if !self.open {
            return None;
        }
        if allowed(actor).is_err() {
            self.locked();
            return None;
        }
        if self.base.is_none() {
            self.bind(tickets, reviews, directory, ledger, actor)
        }
        let stale = self.stale(tickets, reviews, directory, ledger, actor);
        let state = self.state();
        let mut open = true;
        let mut close = false;
        let mut command = None;
        egui::Window::new("Inventory custody / whole lots")
            .open(&mut open)
            .default_size([750., 520.])
            .min_width(520.)
            .max_height((ctx.content_rect().height() - 40.).max(120.))
            .show(ctx, |ui| {
                self.tabs(ui);
                self.notices(ui, stale, state.as_ref().err());
                let height = (ui.available_height() - 72.).clamp(32., 380.);
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(
                            !stale && state.is_ok() && self.pending.is_none(),
                            |ui| {
                                self.form(ui, state.as_ref().ok());
                            },
                        );
                    });
                (command, close) = self.buttons(ui, !stale && state.is_ok());
            });
        if !open || close {
            self.open = false;
            if self.pending.is_none() {
                self.locked()
            }
            return None;
        }
        match command? {
            Command::Refresh => {
                self.pending = None;
                self.message = None;
                Some(InventoryUiAction::Refresh)
            }
            Command::Retry => self.result(self.retry_action()),
            Command::Submit => {
                let result = match self.tab {
                    Tab::Receive => self.receive_action(),
                    Tab::Move => self.move_action(),
                    Tab::Ship => self.ship_action(),
                    Tab::Locations => self.location_action(),
                    Tab::OnHand => Ok(InventoryUiAction::Export {
                        include_contacts: self.include_contacts,
                    }),
                    Tab::History => return None,
                };
                self.result(result)
            }
        }
    }
    fn notices(&self, ui: &mut egui::Ui, stale: bool, invalid: Option<&String>) {
        if stale {
            ui.colored_label(ui.visuals().error_fg_color, "Saved tickets, reviews, locations, custody history or operator changed. Refresh before continuing.");
        }
        for message in [self.message.as_ref(), invalid].into_iter().flatten() {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        if self.pending.is_some() {
            ui.label("An exact action awaits acknowledgment. Fields are frozen; retry it or explicitly refresh saved data.");
        }
    }
    fn buttons(&self, ui: &mut egui::Ui, enabled: bool) -> (Option<Command>, bool) {
        let mut command = None;
        let mut close = false;
        ui.horizontal_wrapped(|ui| {
            let label = if self.pending.is_some() {
                Some("Retry exact inventory action")
            } else {
                self.submit_label()
            };
            if let Some(label) = label
                && ui.add_enabled(enabled, egui::Button::new(label)).clicked()
            {
                command = Some(if self.pending.is_some() {
                    Command::Retry
                } else {
                    Command::Submit
                });
            }
            if ui.button("Refresh saved inventory").clicked() {
                command = Some(Command::Refresh);
            }
            close = ui
                .button(if self.pending.is_some() {
                    "Close; keep pending"
                } else {
                    "Close inventory"
                })
                .clicked();
        });
        (command, close)
    }
    fn result(&mut self, result: Result<InventoryUiAction, String>) -> Option<InventoryUiAction> {
        match result {
            Ok(action) => {
                self.message = None;
                Some(action)
            }
            Err(message) => {
                self.message = Some(message);
                None
            }
        }
    }
    fn submit_label(&self) -> Option<&'static str> {
        match self.tab {
            Tab::OnHand => Some("Export inventory CSV"),
            Tab::Receive => Some("Receive confirmed whole lot"),
            Tab::Move => Some("Move confirmed whole lot"),
            Tab::Ship => Some("Ship confirmed whole lots"),
            Tab::Locations => Some("Save location"),
            Tab::History => None,
        }
    }
}
enum Command {
    Submit,
    Retry,
    Refresh,
}
#[path = "inventory_ui_forms.rs"]
mod forms;
#[cfg(test)]
#[path = "../../tests/extended/inventory/inventory_ui_tests.rs"]
mod tests;
