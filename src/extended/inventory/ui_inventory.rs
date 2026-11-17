use super::*;
use crate::{
    inventory::{InventoryDirectory, InventoryLedger},
    inventory_ui::{InventoryUi, InventoryUiAction},
};

pub(super) struct InventoryWork {
    pub directory: InventoryDirectory,
    pub ledger: InventoryLedger,
    pub editor: InventoryUi,
}

impl InventoryWork {
    pub fn load(store: &Store) -> Result<Self, String> {
        Ok(Self {
            directory: store.inventory_directory()?,
            ledger: store.inventory_ledger()?,
            editor: Default::default(),
        })
    }
    pub fn locked(&mut self) {
        self.editor.locked();
    }
}
impl RegisterApp {
    pub(super) fn inventory_controls(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                self.can(Permission::ManageInventory),
                egui::Button::new("Training inventory / shipments"),
            )
            .clicked()
        {
            self.inventory.editor.open = true;
        }
    }
    fn refresh_inventory_records(&mut self) -> Result<(), String> {
        let mut tickets = self.store.list()?;
        tickets.sort_by_key(|ticket| ticket.id);
        if !tickets.iter().any(|ticket| ticket.id == self.active) {
            return Err("The active ticket is unavailable. Reopen the register.".into());
        }
        let directory = self.store.inventory_directory()?;
        let ledger = self.store.inventory_ledger()?;
        let reviews = self.store.intake_reviews()?;
        ledger.validate_against(&tickets, &reviews, &directory)?;
        self.tickets = tickets;
        self.intake.ledger = reviews;
        self.inventory.directory = directory;
        self.inventory.ledger = ledger;
        Ok(())
    }
    pub(super) fn run_inventory_action(&mut self, action: InventoryUiAction) -> Result<(), String> {
        self.store
            .require_session(self.operator_session()?, Permission::ManageInventory)?;
        match action {
            InventoryUiAction::SaveLocation { location } => {
                self.store.save_inventory_location_as(
                    self.operator_session()?,
                    &self.inventory.directory,
                    &location,
                )?;
            }
            InventoryUiAction::Receive { input, request_key } => {
                self.store.receive_inventory_lot_as(
                    self.operator_session()?,
                    &self.inventory.ledger,
                    &self.inventory.directory,
                    &input,
                    &request_key,
                )?;
            }
            InventoryUiAction::Move {
                lot_id,
                to_location_id,
                container,
                reason,
                request_key,
            } => {
                self.store.move_inventory_lot_as(
                    self.operator_session()?,
                    &self.inventory.ledger,
                    &self.inventory.directory,
                    lot_id,
                    to_location_id,
                    &container,
                    &reason,
                    &request_key,
                )?;
            }
            InventoryUiAction::Ship { input, request_key } => {
                self.store.ship_inventory_lots_as(
                    self.operator_session()?,
                    &self.inventory.ledger,
                    &self.inventory.directory,
                    &input,
                    &request_key,
                )?;
            }
            InventoryUiAction::Export { include_contacts } => {
                let folder = self.export_inventory_records(include_contacts)?;
                self.notice = Some(format!("Training custody CSVs saved: {}", folder.display()));
            }
            InventoryUiAction::Refresh => (),
        }
        self.refresh_inventory_records()?;
        self.inventory.editor.saved();
        self.inventory.editor.open = true;
        Ok(())
    }
    fn export_inventory_records(
        &self,
        include_contacts: bool,
    ) -> Result<std::path::PathBuf, String> {
        let exports = self
            .store
            .prepare_inventory_export_as(self.operator_session()?, include_contacts)?;
        let root = self
            .store
            .path()
            .parent()
            .ok_or("Register folder is unavailable.")?
            .join("exports");
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("Could not create exports folder: {error}"))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "System clock is unavailable.")?
            .as_nanos();
        let folder = root.join(format!("training-inventory-{stamp}"));
        std::fs::create_dir(&folder)
            .map_err(|error| format!("Could not create a new custody export folder: {error}"))?;
        use std::io::Write;
        for (name, text) in [
            ("lots.csv", exports.lots),
            ("movements.csv", exports.movements),
            ("shipments.csv", exports.shipments),
        ] {
            let result = (|| {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(folder.join(name))?;
                file.write_all(text.as_bytes())?;
                file.sync_all()
            })();
            result.map_err(|error: std::io::Error| {
                format!(
                    "Custody export failed. Partial output retained at {}: {error}",
                    folder.display()
                )
            })?;
        }
        Ok(folder)
    }
    pub(super) fn inventory_panels(&mut self, ctx: &egui::Context) {
        if !self.can(Permission::ManageInventory) {
            self.inventory.locked();
            return;
        }
        let actor = self
            .operator_session()
            .expect("authenticated operator")
            .actor()
            .clone();
        if let Some(action) = self.inventory.editor.show(
            ctx,
            &self.tickets,
            &self.intake.ledger,
            &self.inventory.directory,
            &self.inventory.ledger,
            &actor,
        ) && let Err(error) = self.run_inventory_action(action)
        {
            self.inventory.editor.error(error);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/extended/inventory/ui_inventory_tests.rs"]
mod tests;
