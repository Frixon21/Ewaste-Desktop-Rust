use super::*;
use crate::{
    adjustment_ui::{AdjustmentUi, AdjustmentUiAction},
    adjustments::AdjustmentLedger,
};
pub(super) struct FinancialWork {
    pub ledger: AdjustmentLedger,
    pub editor: AdjustmentUi,
}
impl FinancialWork {
    pub fn load(store: &Store) -> Result<Self, String> {
        Ok(Self {
            ledger: store.adjustments()?,
            editor: Default::default(),
        })
    }
    pub fn locked(&mut self) {
        self.editor.locked();
    }
}
impl RegisterApp {
    pub(super) fn financial_controls(&mut self, ui: &mut egui::Ui) {
        let ticket = self.active();
        if ticket.state != State::Completed {
            return;
        }
        let total = self.financial.ledger.total_adjusted(ticket.id);
        ui.horizontal_wrapped(|ui| {
            match total {
                Ok(amount) => {
                    ui.small(format!("Settled refunds / credits: {}", money(amount)));
                }
                Err(ref error) => {
                    ui.colored_label(RED, error);
                }
            }
            if ui
                .add_enabled(
                    self.can(Permission::AdjustFinancials),
                    egui::Button::new("Refund / void / billing credit"),
                )
                .clicked()
            {
                self.financial.editor.open = true;
            }
        });
    }
    pub(super) fn financial_panels(&mut self, ctx: &egui::Context) {
        if !self.can(Permission::AdjustFinancials) {
            self.financial.editor.locked();
            return;
        }
        let ticket = self.active().clone();
        let actor = self
            .operator_session()
            .expect("authenticated operator")
            .actor()
            .clone();
        let action = self
            .financial
            .editor
            .show(ctx, &ticket, &self.financial.ledger, &actor);
        if let Some(action) = action {
            let result = match action {
                AdjustmentUiAction::Start {
                    ticket_id,
                    kind,
                    amount_cents,
                    reason,
                    reference,
                    request_key,
                } => self
                    .store
                    .start_adjustment_as(
                        self.operator_session().expect("authenticated operator"),
                        &self.financial.ledger,
                        ticket_id,
                        kind,
                        amount_cents,
                        &reason,
                        &request_key,
                        &reference,
                    )
                    .map(|(ledger, _)| ledger),
                AdjustmentUiAction::Result {
                    ticket_id,
                    attempt_id,
                    outcome,
                    verified,
                    reference,
                    request_key,
                    reconcile,
                } => self
                    .store
                    .record_adjustment_result_as(
                        self.operator_session().expect("authenticated operator"),
                        &self.financial.ledger,
                        ticket_id,
                        attempt_id,
                        outcome,
                        verified,
                        &reference,
                        &request_key,
                        reconcile,
                    )
                    .map(|(ledger, _)| ledger),
                AdjustmentUiAction::Refresh => self.store.adjustments(),
            };
            match result {
                Ok(ledger) => {
                    self.financial.ledger = ledger;
                    self.financial.editor.saved();
                }
                Err(error) => self.financial.editor.error(error),
            }
        }
    }
}
