use super::*;
use crate::{
    auth::{OperatorSnapshot, Permission, Session},
    inventory::{
        InventoryAction, InventoryDirectory, InventoryLedger, InventoryLocation, InventoryLot,
        InventoryShipment, ReceiveInput, ShipmentInput,
    },
    inventory_export::{InventoryExports, export_inventory},
    security_audit::AuditAction,
};

impl Store {
    pub fn inventory_directory(&self) -> Result<InventoryDirectory, String> {
        let directory: InventoryDirectory = self.read_required("inventory_directory")?;
        directory.validate()?;
        Ok(directory)
    }
    pub fn inventory_ledger(&self) -> Result<InventoryLedger, String> {
        let ledger: InventoryLedger = self.read_required("inventory_ledger")?;
        ledger.validate_against(
            &self.list()?,
            &self.intake_reviews()?,
            &self.inventory_directory()?,
        )?;
        Ok(ledger)
    }
    pub fn save_inventory_location_as(
        &self,
        session: &Session,
        expected: &InventoryDirectory,
        input: &InventoryLocation,
    ) -> Result<InventoryDirectory, String> {
        self.authorized(session,Permission::ManageInventory,|actor|{
            let current=self.inventory_directory()?;
            if &current!=expected{return Err("Inventory locations changed. Refresh before editing a location.".into());}
            let(next,location)=current.save_location(input)?;
            self.inventory_ledger()?.validate_against(&self.list()?,&self.intake_reviews()?,&next)?;
            if current!=next{
                self.write_state("inventory_directory",&next)?;
                self.append_audit(Some(&actor),AuditAction::InventoryDirectorySaved,&format!("inventory-location:{}",location.id),"Updated training storage location metadata; historical movement snapshots preserved")?;
            }Ok(next)
        })
    }
    fn inventory_context(
        &self,
        expected: &InventoryLedger,
        expected_directory: &InventoryDirectory,
        key: &str,
    ) -> Result<(InventoryLedger, InventoryDirectory), String> {
        let current = self.inventory_ledger()?;
        let directory = self.inventory_directory()?;
        if !current.events.iter().any(|event| event.request_key == key)
            && (&current != expected || &directory != expected_directory)
        {
            return Err(
                "Inventory or locations changed. Refresh before recording a new custody event."
                    .into(),
            );
        }
        Ok((current, directory))
    }
    fn commit_inventory(
        &self,
        current: &InventoryLedger,
        next: &InventoryLedger,
        actor: &OperatorSnapshot,
    ) -> Result<(), String> {
        if current == next {
            return Ok(());
        }
        if next.events.len() != current.events.len() + 1
            || !next.events.starts_with(&current.events)
        {
            return Err("Inventory history is append-only, one custody action at a time.".into());
        }
        let event = next.events.last().ok_or("Inventory event is missing.")?;
        if &event.actor != actor {
            return Err("Inventory events require the current authenticated operator.".into());
        }
        next.validate_against(
            &self.list()?,
            &self.intake_reviews()?,
            &self.inventory_directory()?,
        )?;
        let detail = match &event.action {
            InventoryAction::Received { lot } => format!(
                "Received whole training lot {}; ticket {}; source review {:?}",
                lot.id, lot.origin.ticket_id, lot.origin.review_event_id
            ),
            InventoryAction::Moved {
                lot_id,
                from_location,
                to_location,
                ..
            } => format!(
                "Moved whole training lot {lot_id}; location {} to {}",
                from_location.id, to_location.id
            ),
            InventoryAction::Shipped { shipment } => format!(
                "Recorded training shipment {}; {} whole lot(s); no physical dispatch initiated",
                shipment.id,
                shipment.lot_ids.len()
            ),
        };
        self.write_state("inventory_ledger", next)?;
        self.append_audit(
            Some(actor),
            AuditAction::InventoryRecorded,
            &format!("inventory-event:{}", event.id),
            &detail,
        )
    }
    pub fn receive_inventory_lot_as(
        &self,
        session: &Session,
        expected: &InventoryLedger,
        expected_directory: &InventoryDirectory,
        input: &ReceiveInput,
        key: &str,
    ) -> Result<(InventoryLedger, InventoryLot), String> {
        self.authorized(session, Permission::ManageInventory, |actor| {
            let (current, directory) = self.inventory_context(expected, expected_directory, key)?;
            let (next, lot) = current.receive(
                &self.list()?,
                &self.intake_reviews()?,
                &directory,
                input,
                &actor,
                key,
                crate::domain::now(),
            )?;
            self.commit_inventory(&current, &next, &actor)?;
            Ok((next, lot))
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn move_inventory_lot_as(
        &self,
        session: &Session,
        expected: &InventoryLedger,
        expected_directory: &InventoryDirectory,
        lot_id: u64,
        to_location_id: u64,
        container: &str,
        reason: &str,
        key: &str,
    ) -> Result<(InventoryLedger, InventoryLot), String> {
        self.authorized(session, Permission::ManageInventory, |actor| {
            let (current, directory) = self.inventory_context(expected, expected_directory, key)?;
            let (next, lot) = current.move_lot(
                &directory,
                lot_id,
                to_location_id,
                container,
                reason,
                &actor,
                key,
                crate::domain::now(),
            )?;
            self.commit_inventory(&current, &next, &actor)?;
            Ok((next, lot))
        })
    }
    pub fn ship_inventory_lots_as(
        &self,
        session: &Session,
        expected: &InventoryLedger,
        expected_directory: &InventoryDirectory,
        input: &ShipmentInput,
        key: &str,
    ) -> Result<(InventoryLedger, InventoryShipment), String> {
        self.authorized(session, Permission::ManageInventory, |actor| {
            let (current, directory) = self.inventory_context(expected, expected_directory, key)?;
            let (next, shipment) =
                current.ship(&directory, input, &actor, key, crate::domain::now())?;
            self.commit_inventory(&current, &next, &actor)?;
            Ok((next, shipment))
        })
    }
    pub fn prepare_inventory_export_as(
        &self,
        session: &Session,
        include_contacts: bool,
    ) -> Result<InventoryExports, String> {
        self.authorized(
            session,
            if include_contacts {
                Permission::ExportContacts
            } else {
                Permission::ExportRecords
            },
            |actor| {
                let exports = export_inventory(
                    &self.list()?,
                    &self.intake_reviews()?,
                    &self.inventory_directory()?,
                    &self.inventory_ledger()?,
                    include_contacts,
                )?;
                self.append_audit(
                    Some(&actor),
                    AuditAction::RecordsExported,
                    "training-inventory",
                    if include_contacts {
                        "Prepared local custody CSVs with approved source/contact fields"
                    } else {
                        "Prepared local custody CSVs without source/contact fields"
                    },
                )?;
                Ok(exports)
            },
        )
    }
}
