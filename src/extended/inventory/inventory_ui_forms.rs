use super::*;
fn field(ui: &mut egui::Ui, label: &str, value: &mut String, max: usize) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .char_limit(max)
            .desired_width(600.),
    );
}
fn date_label(value: Option<u64>) -> String {
    value.map_or_else(
        || "Unknown".into(),
        |at| crate::catalog_ui::format_utc_date(at).unwrap_or_else(|_| "Invalid saved date".into()),
    )
}
fn weight_label(value: Option<i64>) -> String {
    value.map_or_else(
        || "Unknown".into(),
        |v| format!("{}.{:03} lb", v / 1000, v % 1000),
    )
}
impl InventoryUi {
    pub(super) fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (Tab::OnHand, "On hand"),
                (Tab::Receive, "Receive"),
                (Tab::Move, "Move"),
                (Tab::Ship, "Ship"),
                (Tab::Locations, "Locations"),
                (Tab::History, "History / issues"),
            ] {
                if ui.selectable_label(self.tab == tab, label).clicked() && self.tab != tab {
                    self.tab = tab;
                    self.confirmed = false;
                }
            }
        });
    }
    pub(super) fn form(&mut self, ui: &mut egui::Ui, state: Option<&InventoryState>) {
        let Some(base) = self.base.clone() else {
            return;
        };
        match self.tab {
            Tab::OnHand => self.on_hand(ui, state),
            Tab::Receive => self.receive_form(ui, &base),
            Tab::Move => self.move_form(ui, &base, state),
            Tab::Ship => self.ship_form(ui, state),
            Tab::Locations => self.locations_form(ui, &base.directory),
            Tab::History => self.history(ui, &base),
        }
    }
    fn on_hand(&mut self, ui: &mut egui::Ui, state: Option<&InventoryState>) {
        ui.label(
            "Whole-lot custody only. Unknown documentary counts, weights and dates stay unknown.",
        );
        let lots = state
            .map(|s| {
                s.lots
                    .iter()
                    .filter(|lot| lot.status == LotStatus::OnHand)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ui.strong(format!("{} on-hand lots", lots.len()));
        for lot in lots {
            lot_preview(ui, lot);
            ui.separator();
        }
        ui.checkbox(
            &mut self.include_contacts,
            "Include source contact details in this export",
        );
        ui.small("The default export omits contact details. Shipment net weight is recorded separately; no lot allocation or claim is created.");
    }
    fn receive_form(&mut self, ui: &mut egui::Ui, base: &Snapshot) {
        let before = self.ticket_id;
        egui::ComboBox::from_id_salt("inventory-ticket")
            .selected_text(self.ticket_id.map_or_else(
                || "Choose completed ticket".into(),
                |id| format!("Ticket #{id:06}"),
            ))
            .show_ui(ui, |ui| {
                for ticket in base.tickets.iter().filter(|t| t.state == State::Completed) {
                    ui.selectable_value(
                        &mut self.ticket_id,
                        Some(ticket.id),
                        format!("Ticket #{:06}", ticket.id),
                    );
                }
            });
        if before != self.ticket_id {
            self.identity = base
                .tickets
                .iter()
                .find(|t| Some(t.id) == self.ticket_id)
                .and_then(|t| LineIdentity::from_ticket(t, 0).ok());
            self.review_event_id = None;
            self.confirmed = false;
        }
        let previous = self.identity;
        egui::ComboBox::from_id_salt("inventory-material")
            .selected_text(self.identity.map_or_else(
                || "Choose frozen material".into(),
                |id| format!("Material {id:?}"),
            ))
            .show_ui(ui, |ui| {
                if let Some(ticket) = base.tickets.iter().find(|t| Some(t.id) == self.ticket_id) {
                    for (index, line) in ticket.lines.iter().enumerate() {
                        if let Ok(id) = LineIdentity::from_ticket(ticket, index) {
                            ui.selectable_value(
                                &mut self.identity,
                                Some(id),
                                format!("{} / {} / {id:?}", line.code, line.name),
                            );
                        }
                    }
                }
            });
        if previous != self.identity {
            self.confirmed = false;
        }
        let previous = self.review_event_id;
        egui::ComboBox::from_id_salt("inventory-review")
            .selected_text(self.review_event_id.map_or_else(
                || "Original frozen intake snapshot".into(),
                |id| format!("Exact documentary review event #{id}"),
            ))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.review_event_id,
                    None,
                    "Original frozen intake snapshot",
                );
                for event in base
                    .reviews
                    .events
                    .iter()
                    .filter(|e| Some(e.ticket_id) == self.ticket_id)
                {
                    ui.selectable_value(
                        &mut self.review_event_id,
                        Some(event.id),
                        format!("Review #{} / {:?}", event.id, event.snapshot.disposition),
                    );
                }
            });
        if previous != self.review_event_id {
            self.confirmed = false;
        }
        match self.selected_origin() {
            Ok(origin) => origin_preview(ui, &origin),
            Err(message) => {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
        }
        ui.checkbox(
            &mut self.confirmed,
            "I confirm this pinned source and material snapshot for this whole lot",
        );
        self.location_picker(ui, &base.directory);
        field(
            ui,
            "Container reference (optional)",
            &mut self.container,
            80,
        );
        field(
            ui,
            "Received date UTC (YYYY-MM-DD; blank = unknown)",
            &mut self.received_date,
            10,
        );
        field(
            ui,
            "First accumulation date UTC (independent; blank = unknown)",
            &mut self.accumulation_date,
            10,
        );
        field(ui, "Receiving reason", &mut self.reason, 500);
    }
    fn location_picker(&mut self, ui: &mut egui::Ui, directory: &InventoryDirectory) {
        let name = directory
            .locations
            .iter()
            .find(|l| l.id == self.location_id)
            .map_or_else(
                || "Choose active location".into(),
                |l| format!("{}{}", l.name, if l.active { "" } else { " (inactive)" }),
            );
        egui::ComboBox::from_id_salt("inventory-active-location")
            .selected_text(name)
            .show_ui(ui, |ui| {
                for location in directory.locations.iter().filter(|l| l.active) {
                    ui.selectable_value(&mut self.location_id, location.id, &location.name);
                }
            });
    }
    fn move_form(&mut self, ui: &mut egui::Ui, base: &Snapshot, state: Option<&InventoryState>) {
        ui.label("Move an entire on-hand lot. Documentary measurements remain unchanged.");
        let previous = self.move_lot;
        egui::ComboBox::from_id_salt("inventory-move-lot")
            .selected_text(self.move_lot.map_or_else(
                || "Choose on-hand lot".into(),
                |id| format!("Whole lot #{id}"),
            ))
            .show_ui(ui, |ui| {
                if let Some(state) = state {
                    for lot in state.lots.iter().filter(|l| l.status == LotStatus::OnHand) {
                        ui.selectable_value(
                            &mut self.move_lot,
                            Some(lot.id),
                            format!(
                                "Lot #{} / {} / {}",
                                lot.id, lot.location.name, lot.container
                            ),
                        );
                    }
                }
            });
        if previous != self.move_lot {
            self.confirmed = false;
            if let Some(lot) = state.and_then(|s| self.move_lot.and_then(|id| s.lot(id))) {
                self.container = lot.container.clone();
                self.location_id = lot.location.id;
            }
        }
        if let Some(lot) = state.and_then(|s| self.move_lot.and_then(|id| s.lot(id))) {
            lot_preview(ui, lot);
        }
        ui.label("Destination location");
        self.location_picker(ui, &base.directory);
        field(
            ui,
            "Destination container (optional)",
            &mut self.container,
            80,
        );
        field(ui, "Movement reason", &mut self.reason, 500);
        ui.checkbox(
            &mut self.confirmed,
            "I confirm movement of the entire selected lot",
        );
    }
    fn ship_form(&mut self, ui: &mut egui::Ui, state: Option<&InventoryState>) {
        ui.label("Select whole on-hand lots; no partial-lot allocation is performed.");
        if let Some(state) = state {
            for lot in state.lots.iter().filter(|l| l.status == LotStatus::OnHand) {
                let mut selected = self.ship_lots.contains(&lot.id);
                if ui
                    .checkbox(
                        &mut selected,
                        format!(
                            "Lot #{} / {} / {}",
                            lot.id, lot.location.name, lot.container
                        ),
                    )
                    .changed()
                {
                    self.confirmed = false;
                    if selected {
                        self.ship_lots.push(lot.id)
                    } else {
                        self.ship_lots.retain(|id| *id != lot.id)
                    }
                }
            }
        }
        ui.strong(format!("{} whole lots selected", self.ship_lots.len()));
        field(ui, "Destination name", &mut self.destination_name, 120);
        field(
            ui,
            "Destination address (optional)",
            &mut self.destination_address,
            240,
        );
        field(ui, "Carrier name (optional)", &mut self.carrier_name, 120);
        field(
            ui,
            "Carrier address (optional)",
            &mut self.carrier_address,
            240,
        );
        field(
            ui,
            "Shipment evidence reference (required)",
            &mut self.evidence,
            120,
        );
        field(
            ui,
            "Reported shipment date UTC (YYYY-MM-DD; blank = unknown)",
            &mut self.shipped_date,
            10,
        );
        field(
            ui,
            "Independently measured shipment net weight lb (blank = unknown)",
            &mut self.shipment_weight,
            16,
        );
        ui.small("Shipment net weight is independent of individual lot measurements and billing quantities.");
        field(ui, "Shipment reason", &mut self.reason, 500);
        ui.checkbox(
            &mut self.confirmed,
            "I confirm shipment of every selected whole lot",
        );
    }
    fn locations_form(&mut self, ui: &mut egui::Ui, directory: &InventoryDirectory) {
        let previous = self.location_edit_id;
        egui::ComboBox::from_id_salt("inventory-edit-location")
            .selected_text(self.location_edit_id.map_or_else(
                || "Create new location".into(),
                |id| format!("Edit location #{id}"),
            ))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.location_edit_id, None, "Create new location");
                for location in &directory.locations {
                    ui.selectable_value(
                        &mut self.location_edit_id,
                        Some(location.id),
                        format!(
                            "{} / {}",
                            location.name,
                            if location.active {
                                "Active"
                            } else {
                                "Inactive"
                            }
                        ),
                    );
                }
            });
        if previous != self.location_edit_id {
            if let Some(location) = directory
                .locations
                .iter()
                .find(|l| Some(l.id) == self.location_edit_id)
            {
                self.location_name = location.name.clone();
                self.location_active = location.active;
            } else {
                self.location_name.clear();
                self.location_active = true;
            }
        }
        field(ui, "Location name", &mut self.location_name, 120);
        ui.checkbox(
            &mut self.location_active,
            "Active location (available for new receipts and movements)",
        );
        ui.small("Deactivating a location preserves the original location snapshots and existing on-hand lots.");
        for location in &directory.locations {
            ui.label(format!(
                "#{} / {} / {}",
                location.id,
                location.name,
                if location.active {
                    "Active"
                } else {
                    "Inactive"
                }
            ));
        }
    }
    fn history(&self, ui: &mut egui::Ui, base: &Snapshot) {
        ui.heading("Custody issues");
        match base
            .ledger
            .issues(&base.tickets, &base.reviews, &base.directory)
        {
            Ok(issues) => {
                if issues.is_empty() {
                    ui.label("No recorded custody issues.");
                }
                for issue in issues {
                    ui.label(format!(
                        "Lot #{} / {} / {}",
                        issue.lot_id, issue.code, issue.message
                    ));
                }
            }
            Err(message) => {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
        }
        ui.heading("Append-only custody history");
        for event in base.ledger.events.iter().rev() {
            let action = match &event.action {
                InventoryAction::Received { lot } => format!("Received lot #{}", lot.id),
                InventoryAction::Moved {
                    lot_id,
                    to_location,
                    to_container,
                    ..
                } => format!(
                    "Moved lot #{lot_id} to {} / {to_container}",
                    to_location.name
                ),
                InventoryAction::Shipped { shipment } => format!(
                    "Shipment #{} / {} whole lots to {}",
                    shipment.id,
                    shipment.lot_ids.len(),
                    shipment.destination.name
                ),
            };
            ui.label(format!(
                "Event #{} / {} / {} / {}",
                event.id,
                date_label(Some(event.at)),
                event.actor.display_name,
                action
            ));
            ui.small(&event.reason);
        }
    }
}
fn origin_preview(ui: &mut egui::Ui, origin: &LotOrigin) {
    ui.group(|ui| {
        ui.strong(format!(
            "Pinned source / ticket #{} / {:?} / {}",
            origin.ticket_id,
            origin.line_identity,
            origin
                .review_event_id
                .map_or_else(|| "Original intake".into(), |id| format!("Review #{id}"))
        ));
        ui.label(format!(
            "Source: {:?} / {:?} / {:?}",
            origin.source.category, origin.source.reason, origin.source.delivery
        ));
        for (label, value) in [
            ("Source name", &origin.source.source_name),
            ("Address", &origin.source.source_address),
            ("Organization", &origin.source.organization),
            ("Contact", &origin.source.contact_name),
            ("Phone", &origin.source.phone),
            ("Reason detail", &origin.source.reason_detail),
            ("Circumstances", &origin.source.circumstances),
            ("Source notes", &origin.source.notes),
        ] {
            if !value.is_empty() {
                ui.label(format!("{label}: {value}"));
            }
        }
        ui.strong(format!(
            "Pinned material: {:?} / {:?} / {:?}",
            origin.material.class, origin.material.condition, origin.review_disposition
        ));
        ui.label(format!(
            "Documentary count: {} / actual documentary weight: {}",
            origin
                .material
                .reporting_units
                .map_or_else(|| "Unknown".into(), |v| v.to_string()),
            weight_label(origin.material.actual_weight_thousandths)
        ));
        if !origin.material.notes.is_empty() {
            ui.label(format!("Material notes: {}", origin.material.notes));
        }
    });
}
fn lot_preview(ui: &mut egui::Ui, lot: &InventoryLot) {
    ui.strong(format!(
        "Whole lot #{} / {:?} / {} / {}",
        lot.id,
        lot.status,
        lot.location.name,
        if lot.container.is_empty() {
            "Unknown container"
        } else {
            &lot.container
        }
    ));
    ui.label(format!(
        "Ticket #{} / {:?} / snapshot {} / {:?}",
        lot.origin.ticket_id,
        lot.origin.line_identity,
        lot.origin
            .review_event_id
            .map_or_else(|| "Original".into(), |id| format!("Review #{id}")),
        lot.origin.material.class
    ));
    ui.label(format!(
        "Count: {} / weight: {} / received: {} / first accumulation: {}",
        lot.origin
            .material
            .reporting_units
            .map_or_else(|| "Unknown".into(), |v| v.to_string()),
        weight_label(lot.origin.material.actual_weight_thousandths),
        date_label(lot.received_at),
        date_label(lot.first_accumulation_at)
    ));
}
