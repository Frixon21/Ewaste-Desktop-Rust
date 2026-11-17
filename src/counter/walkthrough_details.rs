use super::*;
use crate::intake::{DeliveryMode, IntakeReason, SourceCategory, SourceIntake};
pub(super) struct SourceDetails {
    pub(super) index: usize,
    pub(super) source: SourceIntake,
    pub(super) pieces: String,
    pub(super) notes: String,
    original: Ticket,
}
fn category_label(category: SourceCategory) -> &'static str {
    match category {
        SourceCategory::Unspecified => "Choose category",
        SourceCategory::Residential => "Residential",
        SourceCategory::Education => "Education",
        SourceCategory::Government => "Government",
        SourceCategory::NonProfit => "Non-profit",
        SourceCategory::Business => "Business",
        SourceCategory::Anonymous => "Anonymous",
        SourceCategory::OtherCollector => "Other collectors",
    }
}
fn field(ui: &mut egui::Ui, label: &str, value: &mut String, limit: usize) {
    ui.label(label);
    ui.add(
        TextEdit::singleline(value)
            .char_limit(limit)
            .desired_width(530.),
    );
}
impl RegisterApp {
    pub(super) fn open_source_details(&mut self, index: usize) -> Result<(), String> {
        let ticket = self.active();
        ticket.editable_intake()?;
        let line = ticket.lines.get(index).ok_or("Material line not found.")?;
        if !matches!(line.code.as_str(), "801" | "802") {
            return Err("Source category details apply to TV and laptop lines.".into());
        }
        let previous = self.details.as_ref().filter(|form| {
            form.original.id == ticket.id
                && form.index == index
                && form
                    .original
                    .lines
                    .get(index)
                    .is_some_and(|before| before.id == line.id && before.code == line.code)
        });
        let mut source =
            previous.map_or_else(|| ticket.source_intake.clone(), |form| form.source.clone());
        if let Some(customer) = &ticket.customer_record {
            let linked = source.linked_customer_id.is_some()
                || (source.source_name.is_empty()
                    && source.source_address.is_empty()
                    && source.phone.is_empty());
            if linked || source.source_name.is_empty() {
                source.source_name = customer.name.clone();
            }
            if linked || source.source_address.is_empty() {
                source.source_address = customer.address.clone();
            }
            if linked || source.phone.is_empty() {
                source.phone = customer.phone.clone();
            }
            if linked {
                source.linked_customer_id = Some(customer.id);
            }
        }
        let pieces = previous.map_or_else(
            || {
                line.piece_count
                    .map_or_else(String::new, |count| count.to_string())
            },
            |form| form.pieces.clone(),
        );
        let notes = previous.map_or_else(|| line.notes.clone(), |form| form.notes.clone());
        self.details = Some(SourceDetails {
            index,
            source,
            pieces,
            notes,
            original: ticket.clone(),
        });
        Ok(())
    }
    pub(super) fn save_source_details(&mut self) -> Result<(), String> {
        let form = self
            .details
            .as_ref()
            .ok_or("Open the source category details first.")?;
        if self.active() != &form.original {
            return Err(
                "This ticket changed. Close and reopen its source details before saving.".into(),
            );
        }
        if form.source.category == SourceCategory::Unspecified {
            return Err("Choose Residential, Education, Government, Non-profit, Business, Anonymous or Other collectors.".into());
        }
        let mut source = form.source.normalized()?;
        if source.linked_customer_id.is_some()
            && self
                .active()
                .customer_record
                .as_ref()
                .is_none_or(|customer| {
                    source.source_name != customer.name
                        || source.source_address != customer.address
                        || source.phone != customer.phone
                })
        {
            source.linked_customer_id = None;
        }
        if source.reason == IntakeReason::Unspecified && !source.reason_detail.is_empty() {
            source.reason = IntakeReason::Other;
        }
        let index = form.index;
        let pieces = form.pieces.clone();
        let notes = form.notes.clone();
        let label = category_label(source.category);
        self.save_action(|ticket| {
            *ticket = ticket.with_source_intake(&source)?;
            ticket.line_intake(index, label, &pieces, &notes)
        })?;
        self.details = None;
        Ok(())
    }
    pub(super) fn ensure_source_choice(&mut self) -> Result<(), String> {
        let missing = self
            .active()
            .lines
            .iter()
            .enumerate()
            .find(|(_, line)| {
                matches!(line.code.as_str(), "801" | "802")
                    && self.active().source_intake.category == SourceCategory::Unspecified
            })
            .map(|(index, _)| index);
        if let Some(index) = missing {
            self.open_source_details(index)?;
            return Err("Choose the TV/laptop source category before taking payment or completing the ticket.".into());
        }
        Ok(())
    }
    pub(super) fn source_details_dialog(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        let mut close = false;
        if let Some(form) = &mut self.details {
            let mut open = true;
            let mut save = false;
            egui::Window::new("TV / laptop source details")
                .open(&mut open)
                .default_size([590., 510.])
                .min_width(430.)
                .max_height((ctx.content_rect().height() - 40.).max(120.))
                .show(ctx, |ui| {
                    if let Some(error) = &self.error {
                        ui.colored_label(RED, error);
                    }
                    ui.strong("Source category for this ticket");
                    ui.label("TV and laptop lines in this load share this source category.");
                    let height = (ui.available_height() - 45.).clamp(32., 380.);
                    egui::ScrollArea::vertical()
                        .max_height(height)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for category in [
                                    SourceCategory::Residential,
                                    SourceCategory::Education,
                                    SourceCategory::Government,
                                    SourceCategory::NonProfit,
                                    SourceCategory::Business,
                                    SourceCategory::Anonymous,
                                    SourceCategory::OtherCollector,
                                ] {
                                    ui.radio_value(
                                        &mut form.source.category,
                                        category,
                                        category_label(category),
                                    );
                                }
                            });
                            field(ui, "Source name", &mut form.source.source_name, 120);
                            field(ui, "Address", &mut form.source.source_address, 240);
                            field(ui, "Phone", &mut form.source.phone, 40);
                            field(ui, "Contact (optional)", &mut form.source.contact_name, 120);
                            field(
                                ui,
                                "Organization (optional)",
                                &mut form.source.organization,
                                120,
                            );
                            field(ui, "Reason (optional)", &mut form.source.reason_detail, 240);
                            ui.horizontal_wrapped(|ui| {
                                ui.label("Delivery (optional)");
                                ui.radio_value(
                                    &mut form.source.delivery,
                                    DeliveryMode::Unspecified,
                                    "Unspecified",
                                );
                                ui.radio_value(
                                    &mut form.source.delivery,
                                    DeliveryMode::DropOff,
                                    "Drop-off",
                                );
                                ui.radio_value(
                                    &mut form.source.delivery,
                                    DeliveryMode::Pickup,
                                    "Pickup",
                                );
                            });
                            field(ui, "Source notes (optional)", &mut form.source.notes, 1_000);
                            field(ui, "Piece count (optional)", &mut form.pieces, 6);
                            field(ui, "Material notes (optional)", &mut form.notes, 500);
                        });
                    ui.horizontal(|ui| {
                        save = control(ui, "Save source details", true).clicked();
                        close = ui.button("Cancel source details").clicked();
                    });
                });
            close |= !open;
            if save {
                *action = Some(Act::SaveDetails);
            }
        }
        if close {
            self.details = None;
            self.error = None;
        }
    }
}
