use super::*;

impl RegisterApp {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        if self.show_history {
            let mut open = true;
            egui::Window::new("Ticket history")
                .open(&mut open)
                .default_width(580.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.history, History::All, "All");
                        ui.selectable_value(&mut self.history, History::Draft, "Draft");
                        ui.selectable_value(&mut self.history, History::Held, "Held");
                        ui.selectable_value(&mut self.history, History::Completed, "Completed");
                    });
                    ui.add(
                        TextEdit::singleline(&mut self.history_query)
                            .hint_text("Ticket number, customer or business account")
                            .char_limit(120),
                    );
                    egui::ScrollArea::vertical()
                        .max_height(340.0)
                        .show(ui, |ui| {
                            let mut found = false;
                            for ticket in self.tickets.iter().rev() {
                                let include = match self.history {
                                    History::All => true,
                                    History::Draft => ticket.state == State::Draft,
                                    History::Held => ticket.state == State::Held,
                                    History::Completed => ticket.state == State::Completed,
                                };
                                if !include || !history_matches(ticket, &self.history_query) {
                                    continue;
                                }
                                found = true;
                                let caption = format!(
                                    "#{:06}   {}   {} line(s)   {}   {}",
                                    ticket.id,
                                    state_label(ticket.state),
                                    ticket.lines.len(),
                                    ticket.total().map_or_else(|| "Pending".into(), money),
                                    ticket.customer.as_deref().unwrap_or("Blank")
                                );
                                if ui
                                    .selectable_label(ticket.id == self.active, caption)
                                    .clicked()
                                {
                                    *action = Some(Act::Open(ticket.id));
                                }
                            }
                            if !found {
                                ui.label(RichText::new("No tickets").color(MUTED));
                            }
                        });
                });
            if !open {
                self.show_history = false;
            }
        }
        let editing_line = self
            .edit
            .as_ref()
            .and_then(|edit| self.active().lines.get(edit.index));
        let is_manual = editing_line.is_some_and(|line| line.unit.is_none());
        let is_weighted = editing_line.is_some_and(|line| line.unit.as_deref() == Some("lb"));
        let mut cancel_edit = false;
        if let Some(edit) = self.edit.as_mut() {
            let mut open = true;
            egui::Window::new("Edit material line")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(316.0)
                .show(ctx, |ui| {
                    if let Some(error) = &self.error {
                        ui.label(RichText::new(error).color(RED));
                    }
                    if is_manual {
                        ui.label("Manual amount ($)");
                        ui.add(
                            TextEdit::singleline(&mut edit.manual)
                                .desired_width(210.0)
                                .char_limit(18)
                                .hint_text("Blank keeps price pending"),
                        );
                    } else {
                        ui.label(if is_weighted {
                            "Gross pounds"
                        } else {
                            "Quantity"
                        });
                        ui.add(
                            TextEdit::singleline(&mut edit.quantity)
                                .desired_width(210.0)
                                .char_limit(18),
                        );
                    }
                    if is_weighted {
                        ui.label("Tare pounds");
                        ui.add(TextEdit::singleline(&mut edit.tare).char_limit(18));
                    }
                    #[cfg(feature = "extended-workflows")]
                    {
                        ui.label("Classification");
                        ui.add(
                            TextEdit::singleline(&mut edit.class)
                                .desired_width(210.0)
                                .char_limit(80),
                        );
                    }
                    ui.label("Piece count");
                    ui.add(
                        TextEdit::singleline(&mut edit.pieces)
                            .desired_width(210.0)
                            .char_limit(6),
                    );
                    ui.label("Internal notes");
                    ui.add(
                        TextEdit::multiline(&mut edit.note)
                            .desired_width(280.0)
                            .desired_rows(2)
                            .char_limit(500),
                    );
                    if control(ui, "Save line", true).clicked() {
                        *action = Some(Act::SaveEdit(edit.index));
                    }
                });
            if !open {
                cancel_edit = true;
            }
        }
        if cancel_edit {
            self.edit = None;
            self.error = None;
        }
        if let Some((id, copy, receipt)) = self.receipt.as_mut() {
            let mut open = true;
            let title = if *copy {
                format!("Receipt copy #{id:06}")
            } else {
                format!("Receipt #{id:06}")
            };
            egui::Window::new(title)
                .open(&mut open)
                .default_width(450.0)
                .show(ctx, |ui| {
                    if control(ui, "Copy receipt text", true).clicked() {
                        ctx.copy_text(receipt.clone());
                    }
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(350.0)
                        .show(ui, |ui| {
                            ui.add(
                                TextEdit::multiline(receipt)
                                    .code_editor()
                                    .desired_width(410.0)
                                    .interactive(false),
                            );
                        });
                });
            if !open {
                self.receipt = None;
            }
        }
    }
}

fn history_matches(ticket: &Ticket, query: &str) -> bool {
    let query = query.trim().trim_start_matches('#').to_lowercase();
    if query.is_empty() {
        return true;
    }
    format!("{:06}", ticket.id).contains(&query)
        || ticket
            .customer
            .as_ref()
            .is_some_and(|c| c.to_lowercase().contains(&query))
        || ticket
            .customer_record
            .as_ref()
            .is_some_and(|c| c.number.to_lowercase().contains(&query))
        || ticket.account_record.as_ref().is_some_and(|a| {
            a.number.to_lowercase().contains(&query) || a.name.to_lowercase().contains(&query)
        })
}
#[cfg(test)]
#[path = "../../tests/shared/ui_dialogs_tests.rs"]
mod tests;
