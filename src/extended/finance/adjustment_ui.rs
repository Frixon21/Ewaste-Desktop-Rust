//! Native evidence entry for a saved adjustment intent. Persistence and current
//! operator authorization belong to the caller; this dialog never returns money.
use crate::{
    adjustments::{
        AdjustmentAttempt, AdjustmentKind, AdjustmentLedger, AdjustmentMethod, AdjustmentOutcome,
    },
    auth::OperatorSnapshot,
    domain::{Payment, State, Ticket, amount, money, now},
};
use eframe::egui;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdjustmentUiAction {
    Start {
        ticket_id: u64,
        kind: AdjustmentKind,
        amount_cents: i64,
        reason: String,
        reference: String,
        request_key: String,
    },
    Result {
        ticket_id: u64,
        attempt_id: u64,
        outcome: AdjustmentOutcome,
        verified: bool,
        reference: String,
        request_key: String,
        reconcile: bool,
    },
    Refresh,
}

#[derive(Default)]
pub struct AdjustmentUi {
    pub open: bool,
    original: Option<Ticket>,
    request: RequestForm,
    result: Option<ResultForm>,
    pending_action: Option<AdjustmentUiAction>,
    message: Option<String>,
    selected_voucher: Option<u64>,
}

struct RequestForm {
    kind: AdjustmentKind,
    amount: String,
    reason: String,
    reference: String,
}
impl Default for RequestForm {
    fn default() -> Self {
        Self {
            kind: AdjustmentKind::Refund,
            amount: String::new(),
            reason: String::new(),
            reference: String::new(),
        }
    }
}
struct ResultForm {
    attempt_id: u64,
    outcome: AdjustmentOutcome,
    verified: bool,
}

enum Command {
    Reserve,
    Record,
    Retry,
    Refresh,
    Reload,
}

fn request_key(ticket_id: u64) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("adjustment:{ticket_id}:{stamp}:{sequence}")
}
fn method(ticket: &Ticket) -> Option<AdjustmentMethod> {
    ticket.payment.as_ref().map(|payment| match payment {
        Payment::Cash => AdjustmentMethod::CashRefund,
        Payment::ExternalCard { .. } => AdjustmentMethod::ExternalCardRefund,
        Payment::ChargeTo { .. } => AdjustmentMethod::BillingCredit,
    })
}
fn method_label(method: AdjustmentMethod) -> &'static str {
    match method {
        AdjustmentMethod::CashRefund => "Cash refund",
        AdjustmentMethod::ExternalCardRefund => "External card refund",
        AdjustmentMethod::BillingCredit => "Business billing credit",
    }
}
fn result_button(attempt: &AdjustmentAttempt) -> &'static str {
    if attempt.outcome == AdjustmentOutcome::Unknown {
        return "Reconcile existing adjustment";
    }
    match attempt.method {
        AdjustmentMethod::CashRefund => "Confirm cash returned",
        AdjustmentMethod::ExternalCardRefund => "Record external refund result",
        AdjustmentMethod::BillingCredit => "Confirm billing credit",
    }
}

impl AdjustmentUi {
    pub fn error(&mut self, message: String) {
        self.message = Some(message);
        self.open = true;
    }
    pub fn saved(&mut self) {
        if let Some(AdjustmentUiAction::Result {
            attempt_id,
            outcome: AdjustmentOutcome::Completed,
            ..
        }) = &self.pending_action
        {
            self.selected_voucher = Some(*attempt_id);
        }
        self.pending_action = None;
        self.request = RequestForm::default();
        self.result = None;
        self.message = None;
    }
    pub fn locked(&mut self) {
        *self = Self::default();
    }
    fn reload(&mut self, ticket: &Ticket) {
        self.locked();
        self.open = true;
        self.original = Some(ticket.clone());
    }
    fn check_ticket(&self, ticket: &Ticket) -> Result<(), String> {
        if self.original.as_ref() != Some(ticket) {
            return Err("The original ticket changed or another ticket is open. Explicitly reload before entering an adjustment.".into());
        }
        if ticket.state != State::Completed {
            return Err("Find a saved completed ticket before adjustment.".into());
        }
        Ok(())
    }
    fn prepare_result(&mut self, attempt: &AdjustmentAttempt) {
        if self
            .result
            .as_ref()
            .is_none_or(|form| form.attempt_id != attempt.id)
        {
            self.result = Some(ResultForm {
                attempt_id: attempt.id,
                outcome: AdjustmentOutcome::Completed,
                verified: false,
            });
        }
        if attempt.outcome == AdjustmentOutcome::Unknown
            && let Some(form) = &mut self.result
            && form.outcome.unresolved()
        {
            form.outcome = AdjustmentOutcome::Completed;
            form.verified = false;
        }
    }
    fn start_action(
        &mut self,
        ticket: &Ticket,
        ledger: &AdjustmentLedger,
        actor: &OperatorSnapshot,
    ) -> Result<AdjustmentUiAction, String> {
        self.check_ticket(ticket)?;
        if self.pending_action.is_some() {
            return Err(
                "Retry or refresh the existing save before creating another request.".into(),
            );
        }
        let remaining = ticket
            .total()
            .ok_or("Review the original amount.")?
            .checked_sub(ledger.total_adjusted(ticket.id)?)
            .ok_or("Review the remaining adjustment balance.")?;
        let cents = if self.request.kind == AdjustmentKind::Void {
            remaining
        } else {
            amount(&self.request.amount)?.ok_or("Enter the refund amount in dollars.")?
        };
        let key = request_key(ticket.id);
        let reference = if method(ticket) == Some(AdjustmentMethod::ExternalCardRefund) {
            self.request.reference.trim().to_owned()
        } else {
            String::new()
        };
        ledger.start(
            ticket,
            self.request.kind,
            cents,
            actor,
            &self.request.reason,
            &key,
            now(),
            &reference,
        )?;
        let action = AdjustmentUiAction::Start {
            ticket_id: ticket.id,
            kind: self.request.kind,
            amount_cents: cents,
            reason: self.request.reason.trim().to_owned(),
            reference,
            request_key: key,
        };
        self.pending_action = Some(action.clone());
        self.selected_voucher = None;
        Ok(action)
    }
    fn result_action(
        &mut self,
        ticket: &Ticket,
        ledger: &AdjustmentLedger,
        actor: &OperatorSnapshot,
    ) -> Result<AdjustmentUiAction, String> {
        self.check_ticket(ticket)?;
        if self.pending_action.is_some() {
            return Err(
                "Retry or refresh the existing result save. Do not repeat the payout.".into(),
            );
        }
        let form = self
            .result
            .as_ref()
            .ok_or("Find the saved adjustment intent first.")?;
        let attempt = ledger
            .attempts(ticket.id)?
            .into_iter()
            .find(|a| a.id == form.attempt_id && a.outcome.unresolved())
            .ok_or("This adjustment already has a final result. Refresh its history.")?;
        let reconcile = attempt.outcome == AdjustmentOutcome::Unknown;
        let verified = form.verified && !form.outcome.unresolved();
        if !form.outcome.unresolved() && !verified {
            return Err("Explicitly verify the final outcome of this saved adjustment.".into());
        }
        let key = request_key(ticket.id);
        ledger.record_result(
            ticket,
            attempt.id,
            form.outcome,
            verified,
            &attempt.external_reference,
            actor,
            &key,
            now(),
            reconcile,
        )?;
        let action = AdjustmentUiAction::Result {
            ticket_id: ticket.id,
            attempt_id: attempt.id,
            outcome: form.outcome,
            verified,
            reference: attempt.external_reference,
            request_key: key,
            reconcile,
        };
        self.pending_action = Some(action.clone());
        Ok(action)
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        ticket: &Ticket,
        ledger: &AdjustmentLedger,
        actor: &OperatorSnapshot,
    ) -> Option<AdjustmentUiAction> {
        if !self.open {
            return None;
        }
        if self.original.is_none() {
            self.reload(ticket);
        }
        let stale = self.original.as_ref() != Some(ticket);
        let attempts = ledger.attempts(ticket.id);
        let ledger_ok = attempts.is_ok();
        let attempts = attempts.unwrap_or_else(|message| {
            self.message = Some(message);
            vec![]
        });
        let unresolved = attempts.iter().find(|a| a.outcome.unresolved());
        if let Some(attempt) = unresolved {
            self.prepare_result(attempt);
        }
        let mut command = None;
        let mut open = true;
        egui::Window::new("Refunds and finalized ticket voids").open(&mut open).default_size([740., 640.]).min_width(540.)
            .max_height((ctx.content_rect().height() - 40.).max(120.)).show(ctx, |ui| {
                header(ui, ticket, ledger, actor);
                if let Some(message) = &self.message { ui.colored_label(ui.visuals().error_fg_color, message); }
                if stale {
                    ui.colored_label(ui.visuals().error_fg_color, "This form belongs to a different saved ticket snapshot. Its entries have been retained.");
                    if ui.button("Discard form and reload this ticket").clicked() { command = Some(Command::Reload); }
                }
                if self.pending_action.is_some() { ui.label("A save awaits confirmation. Refresh history or retry the same save; keep the existing payout request."); }
                let body_height = (ui.available_height() - 40.).clamp(32., 500.);
                egui::ScrollArea::vertical().max_height(body_height).show(ui, |ui| {
                    ui.add_enabled_ui(!stale && ledger_ok && self.pending_action.is_none(), |ui| {
                        if let Some(attempt) = unresolved { self.result_fields(ui, attempt); }
                        else if ticket.state == State::Completed { self.request_fields(ui, ticket, ledger); }
                    });
                    self.history(ui, ctx, ticket, ledger, &attempts);
                });
                ui.horizontal_wrapped(|ui| {
                    if !stale && ledger_ok {
                        if self.pending_action.is_some() {
                            if ui.button("Retry same save").clicked() { command = Some(Command::Retry); }
                        } else if let Some(attempt) = unresolved {
                            let ready = self.result.as_ref().is_some_and(|form| form.verified || form.outcome == AdjustmentOutcome::Unknown);
                            if ui.add_enabled(ready, egui::Button::new(result_button(attempt))).clicked() { command = Some(Command::Record); }
                        } else if ticket.state == State::Completed
                            && ui.add_enabled(!self.request.reason.trim().is_empty(), egui::Button::new("Reserve adjustment intent")).clicked()
                        {
                            command = Some(Command::Reserve);
                        }
                    }
                    if ui.button("Refresh history").clicked() { command = Some(Command::Refresh); }
                });
            });
        if !open {
            self.locked();
            return None;
        }
        let result = match command {
            Some(Command::Reserve) => self.start_action(ticket, ledger, actor).map(Some),
            Some(Command::Record) => self.result_action(ticket, ledger, actor).map(Some),
            Some(Command::Retry) => Ok(self.pending_action.clone()),
            Some(Command::Refresh) => Ok(Some(AdjustmentUiAction::Refresh)),
            Some(Command::Reload) => {
                self.reload(ticket);
                Ok(None)
            }
            None => Ok(None),
        };
        match result {
            Ok(action) => action,
            Err(message) => {
                self.message = Some(message);
                None
            }
        }
    }

    fn request_fields(&mut self, ui: &mut egui::Ui, ticket: &Ticket, ledger: &AdjustmentLedger) {
        ui.heading("New adjustment request");
        ui.label("Reserve and save the request first. Do not return money until its saved Pending intent appears here.");
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.request.kind, AdjustmentKind::Refund, "Refund");
            ui.radio_value(
                &mut self.request.kind,
                AdjustmentKind::Void,
                "Void remaining sale balance",
            );
        });
        if self.request.kind == AdjustmentKind::Refund {
            ui.label("Refund amount ($)");
            ui.add(egui::TextEdit::singleline(&mut self.request.amount).char_limit(18));
        } else if let (Some(total), Ok(adjusted)) =
            (ticket.total(), ledger.total_adjusted(ticket.id))
        {
            ui.label(format!(
                "Full remaining balance: {}",
                money(total.saturating_sub(adjusted))
            ));
        }
        ui.label("Operator reason");
        ui.add(
            egui::TextEdit::multiline(&mut self.request.reason)
                .char_limit(500)
                .desired_rows(2),
        );
        if method(ticket) == Some(AdjustmentMethod::ExternalCardRefund) {
            ui.label("Synthetic external refund reference (no card data)");
            ui.add(egui::TextEdit::singleline(&mut self.request.reference).char_limit(64));
            ui.small("Record the separate processor workflow. This application does not initiate a refund.");
        }
    }
    fn result_fields(&mut self, ui: &mut egui::Ui, attempt: &AdjustmentAttempt) {
        ui.heading(format!(
            "Saved request #{} — {}",
            attempt.id,
            method_label(attempt.method)
        ));
        ui.label(format!(
            "{:?} / {} / {:?}",
            attempt.kind,
            money(attempt.amount_cents),
            attempt.outcome
        ));
        if attempt.outcome == AdjustmentOutcome::Unknown {
            ui.label("Check the existing request and record a verified final outcome. Reconciliation keeps this same attempt and reference.");
        } else {
            ui.label("Confirm only after checking the actual outcome of this saved request. If a save failed after cash was returned, keep this request and reconcile the evidence.");
        }
        if !attempt.external_reference.is_empty() {
            ui.label(format!(
                "Saved refund reference: {}",
                attempt.external_reference
            ));
        }
        let form = self
            .result
            .as_mut()
            .expect("saved intent result initialized");
        egui::ComboBox::from_id_salt("adjustment-outcome")
            .selected_text(format!("{:?}", form.outcome))
            .show_ui(ui, |ui| {
                for outcome in [
                    AdjustmentOutcome::Completed,
                    AdjustmentOutcome::Declined,
                    AdjustmentOutcome::Cancelled,
                    AdjustmentOutcome::Unknown,
                ] {
                    if (outcome == AdjustmentOutcome::Declined
                        && attempt.method != AdjustmentMethod::ExternalCardRefund)
                        || (outcome == AdjustmentOutcome::Unknown
                            && attempt.outcome == AdjustmentOutcome::Unknown)
                    {
                        continue;
                    }
                    ui.selectable_value(&mut form.outcome, outcome, format!("{outcome:?}"));
                }
            });
        if form.outcome.unresolved() {
            form.verified = false;
            ui.label("Unknown remains unpaid and prevents another adjustment request.");
        } else {
            ui.checkbox(
                &mut form.verified,
                "I verified this final outcome for the existing request",
            );
        }
    }
    fn history(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        ticket: &Ticket,
        ledger: &AdjustmentLedger,
        attempts: &[AdjustmentAttempt],
    ) {
        ui.collapsing("Saved adjustment history", |ui| {
            for attempt in attempts {
                ui.label(format!(
                    "#{} {:?} / {} / {} / {:?}",
                    attempt.id,
                    attempt.kind,
                    money(attempt.amount_cents),
                    method_label(attempt.method),
                    attempt.outcome
                ));
                if attempt.outcome == AdjustmentOutcome::Completed
                    && ui.button(format!("View voucher #{}", attempt.id)).clicked()
                {
                    self.selected_voucher = Some(attempt.id);
                }
            }
            if attempts.is_empty() {
                ui.label("No saved adjustments");
            }
        });
        if let Some(attempt) = attempts.iter().find(|a| {
            Some(a.id) == self.selected_voucher && a.outcome == AdjustmentOutcome::Completed
        }) {
            match ledger.receipt(attempt, ticket) {
                Ok(mut voucher) => {
                    ui.separator();
                    ui.strong("Saved adjustment voucher");
                    if ui.button("Copy adjustment voucher").clicked() {
                        ctx.copy_text(voucher.clone());
                    }
                    ui.add(
                        egui::TextEdit::multiline(&mut voucher)
                            .code_editor()
                            .interactive(false)
                            .desired_rows(7),
                    );
                }
                Err(message) => {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
            }
        }
    }
}

fn header(ui: &mut egui::Ui, ticket: &Ticket, ledger: &AdjustmentLedger, actor: &OperatorSnapshot) {
    if let Ok(ids) = ledger.unresolved_ticket_ids()
        && ids.contains(&ticket.id)
    {
        ui.strong("An existing adjustment for this ticket still needs a final result");
    }
    ui.label(format!(
        "Operator: {} / {} / {:?}",
        actor.display_name, actor.login, actor.role
    ));
    ui.label(format!(
        "Original ticket #{:06} / {:?}",
        ticket.id, ticket.state
    ));
    if let Some(payment) = &ticket.payment {
        let label = match payment {
            Payment::Cash => "Cash",
            Payment::ExternalCard { .. } => "External card",
            Payment::ChargeTo { .. } => "Business charge-to (pending billing)",
        };
        ui.label(format!("Original tender: {label}"));
    }
    if let (Some(total), Ok(adjusted)) = (ticket.total(), ledger.total_adjusted(ticket.id)) {
        ui.label(format!(
            "Original {} / Confirmed adjustments {} / Remaining {}",
            money(total),
            money(adjusted),
            money(total.saturating_sub(adjusted))
        ));
    }
}

#[cfg(test)]
#[path = "../../tests/extended/finance/adjustment_ui_tests.rs"]
mod tests;
