use super::*;
use crate::price_override_ui::PriceOverrideUiAction;

impl RegisterApp {
    pub(super) fn price_controls(&mut self, ui: &mut egui::Ui) {
        if self.active().state != State::Draft {
            return;
        }
        let enabled = self.can(Permission::OverridePrices)
            && self.active().editable_intake().is_ok()
            && !self.active().lines.is_empty();
        if ui
            .add_enabled(enabled, egui::Button::new("Supervisor line price approval"))
            .clicked()
        {
            self.price_editor.open = true;
        }
    }
    pub(super) fn run_price_override(
        &mut self,
        action: PriceOverrideUiAction,
    ) -> Result<(), String> {
        self.store
            .require_session(self.operator_session()?, Permission::OverridePrices)?;
        let saved = match action {
            PriceOverrideUiAction::Approve {
                ticket_id,
                revision,
                line_id,
                amount_cents,
                reason,
                request_key,
            } => Some(self.store.approve_line_price_as(
                self.operator_session()?,
                revision,
                ticket_id,
                line_id,
                amount_cents,
                &reason,
                &request_key,
            )?),
            PriceOverrideUiAction::Clear {
                ticket_id,
                revision,
                line_id,
                reason,
                request_key,
            } => Some(self.store.clear_line_price_as(
                self.operator_session()?,
                revision,
                ticket_id,
                line_id,
                &reason,
                &request_key,
            )?),
            PriceOverrideUiAction::Refresh => {
                self.store.price_overrides()?;
                let mut tickets = self.store.list()?;
                tickets.sort_by_key(|ticket| ticket.id);
                if !tickets.iter().any(|ticket| ticket.id == self.active) {
                    return Err("Active ticket is unavailable. Reopen the register.".into());
                }
                self.tickets = tickets;
                None
            }
        };
        if let Some(saved) = saved {
            let index = self
                .tickets
                .iter()
                .position(|ticket| ticket.id == saved.id)
                .ok_or("Approved ticket is unavailable. Refresh saved records.")?;
            self.tickets[index] = saved;
            self.reset_fields();
            self.notice =
                Some("Supervisor price decision saved with its reason and audit event.".into());
            self.price_editor.saved();
        } else {
            self.price_editor.saved();
            self.price_editor.open = true;
        }
        Ok(())
    }
    pub(super) fn price_panels(&mut self, ctx: &egui::Context) {
        if !self.can(Permission::OverridePrices) {
            self.price_editor.locked();
            return;
        }
        let ticket = self.active().clone();
        let actor = self
            .operator_session()
            .expect("authenticated operator")
            .actor()
            .clone();
        if let Some(action) = self.price_editor.show(ctx, &ticket, &actor)
            && let Err(error) = self.run_price_override(action)
        {
            self.price_editor.error(error);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/extended/pricing/ui_price_override_tests.rs"]
mod tests;
