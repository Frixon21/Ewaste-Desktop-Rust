//! Draft price decisions remain explicit actions for an authorized Store command.
//! This editor never changes line money, approves its own actor, or prints reasons.
use crate::{
    auth::{OperatorSnapshot, Role},
    domain::{Line, Ticket, money},
};
use eframe::egui;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_APPROVAL_CENTS: i64 = 99_999_999;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PriceOverrideUiAction {
    Approve {
        ticket_id: u64,
        revision: u64,
        line_id: u64,
        amount_cents: i64,
        reason: String,
        request_key: String,
    },
    Clear {
        ticket_id: u64,
        revision: u64,
        line_id: u64,
        reason: String,
        request_key: String,
    },
    Refresh,
}
#[derive(Default)]
pub struct PriceOverrideUi {
    pub open: bool,
    original: Option<Ticket>,
    actor: Option<OperatorSnapshot>,
    selected_line: Option<u64>,
    amount: String,
    reason: String,
    pending_action: Option<PriceOverrideUiAction>,
    message: Option<String>,
}
fn request_key(ticket_id: u64) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "price:{ticket_id}:{stamp}:{}",
        REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
fn parse_amount(input: &str) -> Result<i64, String> {
    let input = input.trim();
    let input = input.strip_prefix('$').unwrap_or(input);
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    if whole.len() > 6
        || fraction.len() > 2
        || (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(
            "Enter a nonnegative dollar amount with at most two decimal places, for example 3.00."
                .into(),
        );
    }
    let whole = if whole.is_empty() {
        0
    } else {
        whole
            .parse::<i64>()
            .map_err(|_| "The approved amount is outside the supported range.")?
    };
    let fraction_value = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i64>()
            .map_err(|_| "Review the approved cents.")?
    };
    let cents = whole
        .checked_mul(100)
        .and_then(|whole| {
            fraction_value
                .checked_mul(10i64.pow(2 - fraction.len() as u32))
                .and_then(|fraction| whole.checked_add(fraction))
        })
        .ok_or("The approved amount is outside the supported range.")?;
    if cents > MAX_APPROVAL_CENTS {
        return Err("The approved amount is outside the supported range.".into());
    }
    Ok(cents)
}
fn required_reason(reason: &str) -> Result<String, String> {
    if reason.chars().count() > 500
        || reason.chars().any(char::is_control)
        || reason.trim().is_empty()
    {
        return Err("Enter an approval or clearing reason using at most 500 characters without control characters.".into());
    }
    Ok(reason.trim().to_owned())
}
fn check_actor(actor: &OperatorSnapshot) -> Result<(), String> {
    actor.validate()?;
    if !matches!(actor.role, Role::Supervisor | Role::Administrator) {
        return Err("A supervisor or administrator must approve a draft price exception.".into());
    }
    Ok(())
}
fn baseline(line: &Line) -> Result<i64, String> {
    if line.id == 0 {
        return Err(
            "This material needs a stable saved line identity before price approval.".into(),
        );
    }
    let total = line
        .total_cents
        .ok_or("Resolve the baseline line price before approval.")?;
    let base = line
        .base_cents
        .ok_or("Review the baseline price calculation before approval.")?;
    let minimum = line
        .minimum_cents
        .ok_or("Review the baseline minimum adjustment before approval.")?;
    if total < 0
        || base < 0
        || minimum < 0
        || base.checked_add(minimum) != Some(total)
        || line
            .effective_total_cents()
            .is_none_or(|effective| effective < 0)
    {
        return Err(
            "The original tariff calculation is invalid. Review it before approval.".into(),
        );
    }
    Ok(total)
}
fn dollars(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}
impl PriceOverrideUi {
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
    fn reload(&mut self, ticket: &Ticket, actor: &OperatorSnapshot) {
        self.locked();
        self.open = true;
        self.original = Some(ticket.clone());
        self.actor = Some(actor.clone());
        if let Some(line) = ticket.lines.iter().find(|line| baseline(line).is_ok()) {
            let _ = self.select_line(ticket, line.id);
        }
    }
    fn select_line(&mut self, ticket: &Ticket, id: u64) -> Result<(), String> {
        if self.pending_action.is_some() {
            return Err("Retry or refresh the saved price action before changing lines.".into());
        }
        let line = ticket
            .lines
            .iter()
            .find(|line| line.id == id && line.id > 0)
            .ok_or("Choose a material line with a stable identity.")?;
        baseline(line)?;
        self.selected_line = Some(id);
        self.amount = dollars(
            line.effective_total_cents()
                .ok_or("Review the effective line price.")?,
        );
        self.reason.clear();
        Ok(())
    }
    fn stale(&self, ticket: &Ticket, actor: &OperatorSnapshot) -> bool {
        self.original.as_ref() != Some(ticket) || self.actor.as_ref() != Some(actor)
    }
    fn checked_line<'a>(
        &self,
        ticket: &'a Ticket,
        actor: &OperatorSnapshot,
    ) -> Result<&'a Line, String> {
        check_actor(actor)?;
        if self.stale(ticket, actor) {
            return Err("The ticket, revision or operator changed. Explicitly refresh before submitting this price action.".into());
        }
        ticket.editable_intake()?;
        let id = self
            .selected_line
            .ok_or("Choose a fully priced material line with a stable identity.")?;
        let line = ticket
            .lines
            .iter()
            .find(|line| line.id == id && id > 0)
            .ok_or("The selected material line changed. Refresh the saved ticket.")?;
        baseline(line)?;
        Ok(line)
    }
    fn approve_action(
        &mut self,
        ticket: &Ticket,
        actor: &OperatorSnapshot,
    ) -> Result<PriceOverrideUiAction, String> {
        if self.pending_action.is_some() {
            return Err("Retry or refresh the exact existing price action before creating another approval.".into());
        }
        let line = self.checked_line(ticket, actor)?;
        let action = PriceOverrideUiAction::Approve {
            ticket_id: ticket.id,
            revision: ticket.revision,
            line_id: line.id,
            amount_cents: parse_amount(&self.amount)?,
            reason: required_reason(&self.reason)?,
            request_key: request_key(ticket.id),
        };
        self.pending_action = Some(action.clone());
        Ok(action)
    }
    fn clear_action(
        &mut self,
        ticket: &Ticket,
        actor: &OperatorSnapshot,
    ) -> Result<PriceOverrideUiAction, String> {
        if self.pending_action.is_some() {
            return Err(
                "Retry or refresh the exact existing price action before clearing an approval."
                    .into(),
            );
        }
        let line = self.checked_line(ticket, actor)?;
        if line.price_approval.is_none() {
            return Err("This line has no active price approval to clear.".into());
        }
        let action = PriceOverrideUiAction::Clear {
            ticket_id: ticket.id,
            revision: ticket.revision,
            line_id: line.id,
            reason: required_reason(&self.reason)?,
            request_key: request_key(ticket.id),
        };
        self.pending_action = Some(action.clone());
        Ok(action)
    }
    fn retry_action(
        &self,
        ticket: &Ticket,
        actor: &OperatorSnapshot,
    ) -> Result<PriceOverrideUiAction, String> {
        self.checked_line(ticket, actor)?;
        self.pending_action
            .clone()
            .ok_or_else(|| "There is no price action to retry.".into())
    }
    fn refresh_action(&mut self) -> PriceOverrideUiAction {
        self.pending_action = None;
        self.message = None;
        PriceOverrideUiAction::Refresh
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        ticket: &Ticket,
        actor: &OperatorSnapshot,
    ) -> Option<PriceOverrideUiAction> {
        if !self.open {
            return None;
        }
        if check_actor(actor).is_err() {
            self.locked();
            return None;
        }
        if self.original.is_none() {
            self.reload(ticket, actor);
        }
        let stale = self.stale(ticket, actor);
        let editable = ticket.editable_intake().err();
        let pending = self.pending_action.is_some();
        let view = self
            .original
            .as_ref()
            .expect("original ticket initialized")
            .clone();
        let mut open = true;
        let mut close_requested = false;
        let mut command = None;
        egui::Window::new(format!("Supervisor line price - #{:06}", view.id))
            .open(&mut open)
            .default_size([700., 520.])
            .min_width(480.)
            .max_height((ctx.content_rect().height() - 40.).max(120.))
            .show(ctx, |ui| {
                ui.label("Approve one draft line price without replacing its original tariff calculation.");
                ui.small("The reason is kept in internal audit history, not the customer receipt.");
                if stale {
                    ui.colored_label(ui.visuals().error_fg_color, "The ticket or signed-in operator changed. Refresh the saved ticket before continuing.");
                }
                if let Some(message) = &editable {
                    ui.colored_label(ui.visuals().error_fg_color, format!("Price approval is unavailable: {message}"));
                }
                if let Some(message) = &self.message {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
                if pending {
                    ui.label("The exact price action is awaiting acknowledgment. Retry it or explicitly refresh saved data; fields are frozen.");
                }
                let body_height = (ui.available_height() - 76.).clamp(32., 380.);
                egui::ScrollArea::vertical().max_height(body_height).show(ui, |ui| {
                    ui.add_enabled_ui(!pending && !stale && editable.is_none(), |ui| self.form_fields(ui, &view));
                });
                (command, close_requested) = self.buttons(ui, &view, !stale && editable.is_none());
            });
        if !open || close_requested {
            self.open = false;
            if self.pending_action.is_none() {
                self.locked();
            }
            return None;
        }
        let result = match command? {
            Command::Approve => self.approve_action(ticket, actor),
            Command::Clear => self.clear_action(ticket, actor),
            Command::Retry => self.retry_action(ticket, actor),
            Command::Refresh => Ok(self.refresh_action()),
        };
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
    fn buttons(
        &self,
        ui: &mut egui::Ui,
        ticket: &Ticket,
        editable: bool,
    ) -> (Option<Command>, bool) {
        let mut command = None;
        let mut close = false;
        let pending = self.pending_action.is_some();
        let enabled = editable && self.selected_line.is_some();
        ui.horizontal_wrapped(|ui| {
            if pending {
                if ui
                    .add_enabled(enabled, egui::Button::new("Retry exact price approval"))
                    .clicked()
                {
                    command = Some(Command::Retry);
                }
            } else {
                if ui
                    .add_enabled(enabled, egui::Button::new("Approve line price"))
                    .clicked()
                {
                    command = Some(Command::Approve);
                }
                let approved = ticket.lines.iter().any(|line| {
                    Some(line.id) == self.selected_line && line.price_approval.is_some()
                });
                if ui
                    .add_enabled(
                        enabled && approved,
                        egui::Button::new("Clear approval to tariff"),
                    )
                    .clicked()
                {
                    command = Some(Command::Clear);
                }
            }
            if ui.button("Refresh saved ticket").clicked() {
                command = Some(Command::Refresh);
            }
            close = ui
                .button(if pending {
                    "Close; keep pending action"
                } else {
                    "Cancel price changes"
                })
                .clicked();
        });
        (command, close)
    }
    fn form_fields(&mut self, ui: &mut egui::Ui, ticket: &Ticket) {
        let label = ticket
            .lines
            .iter()
            .find(|line| Some(line.id) == self.selected_line)
            .map(|line| format!("{} / {}", line.code, line.name))
            .unwrap_or_else(|| "Choose a priced material".into());
        let mut selected = None;
        egui::ComboBox::from_id_salt("supervisor-price-line")
            .selected_text(label)
            .width(450.)
            .show_ui(ui, |ui| {
                for line in ticket.lines.iter().filter(|line| baseline(line).is_ok()) {
                    if ui
                        .selectable_label(
                            Some(line.id) == self.selected_line,
                            format!(
                                "{} / {} / baseline {}",
                                line.code,
                                line.name,
                                money(line.total_cents.unwrap_or(0))
                            ),
                        )
                        .clicked()
                    {
                        selected = Some(line.id);
                    }
                }
            });
        if let Some(id) = selected
            && let Err(message) = self.select_line(ticket, id)
        {
            self.message = Some(message);
        }
        let Some(line) = ticket
            .lines
            .iter()
            .find(|line| Some(line.id) == self.selected_line)
        else {
            ui.label(
                "Add or resolve a material line price before requesting a supervisor exception.",
            );
            return;
        };
        ui.label(format!(
            "Original base {} + minimum {} = tariff total {}",
            money(line.base_cents.unwrap_or(0)),
            money(line.minimum_cents.unwrap_or(0)),
            money(line.total_cents.unwrap_or(0))
        ));
        if let (Some(rate), Some(unit)) = (line.rate_cents, line.unit.as_deref()) {
            ui.label(format!(
                "Saved tariff rate: {}/{unit}; billed quantity {}",
                money(rate),
                line.quantity
            ));
        } else {
            ui.label("Baseline is an explicitly entered manual line amount.");
        }
        ui.strong(format!(
            "Current effective line amount: {}",
            line.effective_total_cents()
                .map_or_else(|| "Pending".into(), money)
        ));
        if let Some(approval) = &line.price_approval {
            ui.small(format!(
                "Active approval event #{}; clearing restores {}",
                approval.event_id,
                money(line.total_cents.unwrap_or(0))
            ));
        }
        ui.label("Approved line amount ($)");
        ui.add(
            egui::TextEdit::singleline(&mut self.amount)
                .char_limit(32)
                .desired_width(180.),
        );
        ui.label("Reason for approving or clearing the price");
        ui.add(
            egui::TextEdit::singleline(&mut self.reason)
                .char_limit(500)
                .desired_width(550.),
        );
    }
}
enum Command {
    Approve,
    Clear,
    Retry,
    Refresh,
}

#[cfg(test)]
#[path = "../../tests/extended/pricing/price_override_ui_tests.rs"]
mod tests;
