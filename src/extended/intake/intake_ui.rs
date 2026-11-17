//! Reviewable source and material documentation, isolated from billing and receipts.
use crate::{
    domain::{Ticket, decimal, money, weight_string},
    intake::*,
};
use eframe::egui;

#[derive(Default)]
pub struct IntakeUi {
    pub open: bool,
    original: Option<Ticket>,
    form: Option<IntakeForm>,
    message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IntakeForm {
    pub source: SourceIntake,
    pub materials: Vec<MaterialIntake>,
    units: Vec<String>,
    weights: Vec<String>,
    message: Option<String>,
}

impl IntakeForm {
    pub fn new(ticket: &Ticket) -> Self {
        Self {
            source: ticket.source_intake.clone(),
            materials: ticket
                .lines
                .iter()
                .map(|line| line.material_intake.clone())
                .collect(),
            units: ticket
                .lines
                .iter()
                .map(|line| {
                    line.material_intake
                        .reporting_units
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                })
                .collect(),
            weights: ticket
                .lines
                .iter()
                .map(|line| {
                    line.material_intake
                        .actual_weight_thousandths
                        .map(weight_string)
                        .unwrap_or_default()
                })
                .collect(),
            message: None,
        }
    }

    pub fn normalized(&self) -> Result<Self, String> {
        if self.materials.len() != self.units.len() || self.materials.len() != self.weights.len() {
            return Err("Material lines changed. Reload the source form.".into());
        }
        let mut next = self.clone();
        next.source = self.source.normalized()?;
        for (index, material) in self.materials.iter().enumerate() {
            let units = self.units[index].trim();
            let weight = self.weights[index].trim();
            let data = MaterialIntake {
                reporting_units: if units.is_empty() {
                    None
                } else {
                    Some(units.parse::<u32>().map_err(
                        |_| "Enter a positive whole reporting unit count, or leave it blank.",
                    )?)
                },
                actual_weight_thousandths: if weight.is_empty() {
                    None
                } else {
                    Some(decimal(
                        weight,
                        3,
                        "Enter reporting pounds to three decimal places, or leave it blank.",
                    )?)
                },
                ..material.clone()
            };
            next.materials[index] = data.normalized()?;
        }
        next.message = None;
        Ok(next)
    }

    pub fn dirty(&self, ticket: &Ticket) -> bool {
        self.normalized().map_or(true, |form| {
            form.source != ticket.source_intake
                || form.materials
                    != ticket
                        .lines
                        .iter()
                        .map(|line| line.material_intake.clone())
                        .collect::<Vec<_>>()
        })
    }

    pub fn fields(&mut self, ui: &mut egui::Ui, ticket: &Ticket) {
        if self.materials.len() != ticket.lines.len() {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "Material lines changed. Reload the saved source form.",
            );
            return;
        }
        edit_source(ui, self, ticket);
        for index in 0..self.materials.len() {
            ui.push_id(index, |ui| edit_material(ui, self, ticket, index));
        }
        if let Some(message) = &self.message {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        if let Err(message) = self.normalized() {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
    }

    fn copy_customer(&mut self, ticket: &Ticket) -> Result<(), String> {
        let customer = ticket
            .customer_record
            .as_ref()
            .ok_or("Select a customer before explicitly copying approved source fields.")?;
        let copied = SourceIntake::from_customer(customer);
        self.source = SourceIntake {
            source_name: copied.source_name,
            source_address: copied.source_address,
            phone: copied.phone,
            linked_customer_id: copied.linked_customer_id,
            ..self.source.clone()
        };
        Ok(())
    }

    fn detach_customer_link(&mut self) {
        self.source = SourceIntake {
            linked_customer_id: None,
            ..self.source.clone()
        };
    }

    fn customer_link_label(&self, ticket: &Ticket) -> String {
        match self.source.linked_customer_id {
            None => "Independent source: no copied customer link".into(),
            Some(id) => match &ticket.customer_record {
                Some(customer) if customer.id == id => {
                    format!("Copied customer link: {} (record #{id})", customer.number)
                }
                _ => {
                    format!("Copied customer link: record #{id} (different from current customer)")
                }
            },
        }
    }

    fn copy_weight(&mut self, ticket: &Ticket, index: usize) -> Result<(), String> {
        let line = ticket.lines.get(index).ok_or("Material line not found.")?;
        let (gross, tare) = line
            .gross_thousandths
            .zip(line.tare_thousandths)
            .filter(|(gross, tare)| {
                line.unit.as_deref() == Some("lb") && *gross > *tare && *tare >= 0
            })
            .ok_or("No saved gross/tare scale reading is available for this material.")?;
        self.weights[index] = weight_string(gross - tare);
        Ok(())
    }
}

impl IntakeUi {
    pub fn error(&mut self, message: String) {
        self.message = Some(message);
        self.open = true;
    }
    pub fn saved(&mut self) {
        self.cancel();
    }
    fn cancel(&mut self) {
        self.open = false;
        self.form = None;
        self.original = None;
        self.message = None;
    }
    fn reload(&mut self, ticket: &Ticket) {
        self.form = Some(IntakeForm::new(ticket));
        self.original = Some(ticket.clone());
        self.message = None;
    }

    fn candidate(&self, ticket: &Ticket) -> Result<Ticket, String> {
        if self.original.as_ref() != Some(ticket) {
            return Err("This ticket changed while intake was open. Reload the saved ticket before editing.".into());
        }
        ticket.editable_intake()?;
        let form = self
            .form
            .as_ref()
            .ok_or("Open source intake first.")?
            .normalized()?;
        if form.materials.len() != ticket.lines.len() {
            return Err("Material lines changed. Reload the source form.".into());
        }
        let mut next = ticket.with_source_intake(&form.source)?;
        for (index, material) in form.materials.iter().enumerate() {
            next = next.with_material_intake(index, material)?;
        }
        Ok(next)
    }

    pub fn show(&mut self, ctx: &egui::Context, ticket: &Ticket) -> Option<Ticket> {
        if !self.open {
            self.cancel();
            return None;
        }
        if self.form.is_none() {
            self.reload(ticket);
        }
        let stale = self.original.as_ref() != Some(ticket);
        let blocked = ticket.editable_intake().err();
        let mut open = true;
        let mut reload = false;
        let mut cancel = false;
        let mut save = false;
        egui::Window::new(format!("Source and material intake — #{:06}", ticket.id))
            .open(&mut open).default_size([740., 650.]).min_width(540.)
            .max_height((ctx.content_rect().height() - 40.).max(120.)).show(ctx, |ui| {
                ui.label("Training documentation only. Source details stay off customer receipts. Billing and reporting quantities remain separate.");
                if stale {
                    ui.colored_label(ui.visuals().error_fg_color, "The ticket changed. Review the saved ticket before replacing this form.");
                    reload = ui.button("Discard form changes and reload saved ticket").clicked();
                }
                if let Some(reason) = &blocked {
                    ui.label(format!("Read-only intake: {reason}"));
                    if stale { ui.label("Reload saved intake to inspect the current snapshot. Your unsaved form remains available for review."); }
                }
                if let Some(message) = &self.message { ui.colored_label(ui.visuals().error_fg_color, message); }
                if self.form.as_ref().zip(self.original.as_ref()).is_some_and(|(form, original)| form.dirty(original)) {
                    ui.label("Unsaved source or material changes");
                }
                let form = self.form.as_mut().expect("intake form initialized");
                let body_height = (ui.available_height() - 36.).clamp(32., 520.);
                egui::ScrollArea::vertical().max_height(body_height).show(ui, |ui| {
                    ui.add_enabled_ui(blocked.is_none() && !stale, |ui| {
                        form.fields(ui, ticket);
                    });
                    ui.separator();
                    ui.strong("Documentation review");
                    let mut review = ticket.clone();
                    let normalized = form.normalized().unwrap_or_else(|_| form.clone());
                    review.source_intake = normalized.source;
                    for (line, material) in review.lines.iter_mut().zip(normalized.materials) { line.material_intake = material; }
                    let issues = review.intake_issues();
                    if issues.is_empty() { ui.label("No missing documentation flagged by the training checks. This does not establish program eligibility."); }
                    for issue in issues {
                        ui.label(match issue.line_index {
                            Some(index) => format!("Line {}: {}", index + 1, issue.message),
                            None => issue.message,
                        });
                    }
                });
                ui.horizontal(|ui| {
                    if blocked.is_none() && !stale { save = ui.button("Validate and save intake").clicked(); }
                    cancel = ui.button(if blocked.is_some() { "Close" } else { "Cancel changes" }).clicked();
                });
            });
        if !open || cancel {
            self.cancel();
            return None;
        }
        if reload {
            self.reload(ticket);
        }
        if save {
            match self.candidate(ticket) {
                Ok(candidate) => {
                    self.message = None;
                    return Some(candidate);
                }
                Err(message) => self.message = Some(message),
            }
        }
        None
    }
}

fn select<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    choices: &[(&str, T)],
) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(label)
            .selected_text(
                choices
                    .iter()
                    .find(|(_, item)| item == value)
                    .map(|(name, _)| *name)
                    .unwrap_or("Unspecified"),
            )
            .show_ui(ui, |ui| {
                for (name, item) in choices {
                    ui.selectable_value(value, *item, *name);
                }
            });
    });
}

fn text(ui: &mut egui::Ui, label: &str, value: &mut String, limit: usize) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .char_limit(limit)
            .desired_width(550.),
    );
}

fn edit_source(ui: &mut egui::Ui, form: &mut IntakeForm, ticket: &Ticket) {
    ui.heading("Source and delivery");
    ui.label(form.customer_link_label(ticket));
    if form.source.linked_customer_id.is_some()
        && ui
            .button("Use independent source (clear copied customer link)")
            .clicked()
    {
        form.detach_customer_link();
    }
    if ui
        .add_enabled(
            ticket.customer_record.is_some(),
            egui::Button::new("Copy selected customer name, address and phone for review"),
        )
        .clicked()
    {
        form.message = form.copy_customer(ticket).err();
    }
    let source = &mut form.source;
    select(
        ui,
        "Source category",
        &mut source.category,
        &[
            ("Unspecified", SourceCategory::Unspecified),
            ("Residential", SourceCategory::Residential),
            ("Education", SourceCategory::Education),
            ("Government", SourceCategory::Government),
            ("Nonprofit", SourceCategory::NonProfit),
            ("Business", SourceCategory::Business),
            ("Anonymous", SourceCategory::Anonymous),
            ("Other collector", SourceCategory::OtherCollector),
        ],
    );
    select(
        ui,
        "Intake reason",
        &mut source.reason,
        &[
            ("Unspecified", IntakeReason::Unspecified),
            ("Emptying garage", IntakeReason::EmptyingGarage),
            ("Faulty equipment", IntakeReason::Faulty),
            ("Office closure", IntakeReason::OfficeClosure),
            ("Small business", IntakeReason::SmallBusiness),
            ("IT upgrade", IntakeReason::ItUpgrade),
            ("Other", IntakeReason::Other),
        ],
    );
    select(
        ui,
        "Delivery",
        &mut source.delivery,
        &[
            ("Unspecified", DeliveryMode::Unspecified),
            ("Drop-off", DeliveryMode::DropOff),
            ("Pickup", DeliveryMode::Pickup),
        ],
    );
    for (label, value, limit) in [
        ("Reason detail", &mut source.reason_detail, 240),
        ("Source name", &mut source.source_name, 120),
        ("Source address", &mut source.source_address, 240),
        ("Organization", &mut source.organization, 120),
        ("Contact person", &mut source.contact_name, 120),
        ("Phone", &mut source.phone, 40),
        (
            "Circumstances / source evidence",
            &mut source.circumstances,
            500,
        ),
        ("Internal source notes", &mut source.notes, 1_000),
    ] {
        text(ui, label, value, limit);
    }
}

fn edit_material(ui: &mut egui::Ui, form: &mut IntakeForm, ticket: &Ticket, index: usize) {
    ui.separator();
    let line = &ticket.lines[index];
    ui.heading(format!("Line {}: {} {}", index + 1, line.code, line.name));
    ui.label(format!(
        "Billing: {} {} / {}",
        line.quantity,
        line.unit.as_deref().unwrap_or("manual amount"),
        line.effective_total_cents()
            .map(money)
            .unwrap_or_else(|| "Pending".into())
    ));
    if line.price_approval.is_some() {
        ui.label(format!(
            "Original tariff line total: {}",
            line.total_cents.map_or_else(|| "Pending".into(), money)
        ));
    }
    if ui
        .add_enabled(
            line.gross_thousandths.zip(line.tare_thousandths).is_some(),
            egui::Button::new("Copy saved scale net to reporting weight for review"),
        )
        .clicked()
    {
        form.message = form.copy_weight(ticket, index).err();
    }
    let data = &mut form.materials[index];
    select(
        ui,
        "Reporting class",
        &mut data.class,
        &[
            ("Unclassified", MaterialClass::Unclassified),
            ("CRT display", MaterialClass::CrtDisplay),
            ("Non-CRT display", MaterialClass::NonCrtDisplay),
            ("Other electronics", MaterialClass::OtherElectronics),
            ("Loose battery", MaterialClass::LooseBattery),
            (
                "Embedded battery product",
                MaterialClass::EmbeddedBatteryProduct,
            ),
            ("Oil", MaterialClass::Oil),
            ("Oil filter", MaterialClass::OilFilter),
            ("Other", MaterialClass::Other),
        ],
    );
    select(
        ui,
        "Material condition",
        &mut data.condition,
        &[
            ("Unspecified", MaterialCondition::Unspecified),
            ("Intact", MaterialCondition::Intact),
            ("Damaged", MaterialCondition::Damaged),
            ("Leaking", MaterialCondition::Leaking),
        ],
    );
    text(
        ui,
        "Reporting units (optional; independent of billed count)",
        &mut form.units[index],
        6,
    );
    text(
        ui,
        "Actual reporting weight in lb (optional)",
        &mut form.weights[index],
        18,
    );
    text(ui, "Internal material notes", &mut data.notes, 1_000);
}

#[cfg(test)]
#[path = "../../tests/extended/intake/intake_ui_tests.rs"]
mod tests;
