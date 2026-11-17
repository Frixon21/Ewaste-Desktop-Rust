//! Native training closeout view. It proposes snapshots; the caller persists them.

use crate::{
    adjustments::AdjustmentLedger,
    closeout::{Closeout, ShiftReport},
    domain::{Ticket, amount, money, now},
};
use eframe::egui;

#[derive(Default, PartialEq)]
enum IntervalMode {
    #[default]
    SinceLast,
    AllHistory,
}

pub struct CloseoutUi {
    pub open: bool,
    mode: IntervalMode,
    window_end: Option<u64>,
    opening: String,
    counted: String,
    external_card: String,
    external_card_refunds: String,
    operator: String,
    reason: String,
    error: Option<String>,
    selected_history: Option<String>,
}

impl Default for CloseoutUi {
    fn default() -> Self {
        Self {
            open: false,
            mode: IntervalMode::default(),
            window_end: None,
            opening: "0.00".into(),
            counted: "0.00".into(),
            external_card: "0.00".into(),
            external_card_refunds: "0.00".into(),
            operator: String::new(),
            reason: String::new(),
            error: None,
            selected_history: None,
        }
    }
}

fn overlaps_saved(saved: &[Closeout], start: u64, end: u64) -> bool {
    saved
        .iter()
        .any(|c| start < c.report.end && c.report.start < end)
}

/// Calendar conversion is explicitly UTC and bounded to years 1970 through 9999.
fn utc_label(seconds: u64) -> String {
    if seconds >= 253_402_300_800 {
        return "Outside supported calendar range".into();
    }
    let days = (seconds / 86_400) as i64 + 719_468;
    let era = days / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let daytime = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        daytime / 3600,
        daytime / 60 % 60,
        daytime % 60
    )
}

fn dollars(input: &str, label: &str) -> Result<i64, String> {
    amount(input)
        .map_err(|_| format!("Enter {label} in dollars with at most two decimal places."))?
        .ok_or_else(|| format!("Enter {label}."))
}

fn ticket_queue(ui: &mut egui::Ui, label: &str, ids: &[u64]) {
    if !ids.is_empty() {
        let tickets = ids
            .iter()
            .map(|id| format!("#{id:06}"))
            .collect::<Vec<_>>()
            .join(", ");
        ui.colored_label(
            egui::Color32::from_rgb(139, 47, 32),
            format!("{label}: {tickets}"),
        );
    }
}

impl CloseoutUi {
    /// Attribution comes from the authenticated session, never an editable name.
    pub fn set_operator(&mut self, name: &str) {
        self.operator = name.to_owned();
    }

    pub fn error(&mut self, message: String) {
        self.error = Some(message);
    }

    /// Call only after persistence succeeds. Retain the operator for the next shift.
    pub fn saved(&mut self) {
        self.open = false;
        self.window_end = None;
        self.error = None;
        self.reason.clear();
        self.counted = "0.00".into();
        self.external_card = "0.00".into();
        self.external_card_refunds = "0.00".into();
    }

    fn interval(&mut self, saved: &[Closeout], clock: u64) -> Result<(u64, u64), String> {
        // A completion after saving may share this second-resolution timestamp;
        // excluding the current second keeps it available to the next closeout.
        let end = *self.window_end.get_or_insert(clock);
        let start = match self.mode {
            IntervalMode::AllHistory => 0,
            IntervalMode::SinceLast => saved.iter().map(|c| c.report.end).max().unwrap_or(0),
        };
        if start >= end {
            return Err(
                "No new interval is available. Refresh the cutoff after the last closeout.".into(),
            );
        }
        Ok((start, end))
    }

    fn candidate(&self, report: &ShiftReport) -> Result<Closeout, String> {
        report.reconcile_movements(
            dollars(&self.opening, "opening cash float")?,
            dollars(&self.counted, "counted cash")?,
            dollars(&self.external_card, "external card sales")?,
            dollars(&self.external_card_refunds, "external card refunds")?,
            &self.operator,
            &self.reason,
        )
    }

    pub fn show_with_adjustments(
        &mut self,
        ctx: &egui::Context,
        tickets: &[Ticket],
        saved: &[Closeout],
        ledger: &AdjustmentLedger,
    ) -> Option<Closeout> {
        self.show_using(ctx, tickets, saved, Some(ledger))
    }

    fn show_using(
        &mut self,
        ctx: &egui::Context,
        tickets: &[Ticket],
        saved: &[Closeout],
        ledger: Option<&AdjustmentLedger>,
    ) -> Option<Closeout> {
        if !self.open {
            self.window_end = None;
            return None;
        }
        let mut open = self.open;
        let mut candidate = None;
        egui::Window::new("Shift reconciliation (training)").open(&mut open)
            .default_width(620.0).default_height(650.0).vscroll(true).show(ctx, |ui| {
                ui.label("Local review only. No accounting or billing is submitted.");
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.mode,IntervalMode::SinceLast,"Since last closeout");
                    ui.selectable_value(&mut self.mode,IntervalMode::AllHistory,"All history");
                    if ui.button("Refresh cutoff").clicked() { self.window_end=None; }
                });
                match self.interval(saved,now()).and_then(|(start,end)| match ledger {
                    Some(ledger) => ShiftReport::from_tickets_and_adjustments(tickets,ledger,start,end),
                    None => ShiftReport::from_tickets(tickets,start,end),
                }) {
                    Ok(report) => {
                        ui.label(format!("From {}", utc_label(report.start)));
                        ui.label(format!("Until {} (exclusive)", utc_label(report.end)));
                        ui.small("The current second stays outside this closeout. Refresh the cutoff to include a just-completed ticket.");
                        ui.small("Completed tickets use completion time. Open and held backlog carries forward from before the cutoff. Older completions without a timestamp require review.");
                        ui.small("Refunds and billing credits use their completed result time, including adjustments to earlier sales. Unresolved adjustment backlog must be settled first.");
                        self.report(ui,&report);
                        let overlap = overlaps_saved(saved,report.start,report.end);
                        if overlap { ui.colored_label(egui::Color32::from_rgb(139,47,32),"This interval overlaps a saved closeout. Use Since last closeout to finalize new work."); }
                        self.declarations(ui,&report);
                        let ready = !overlap && !self.operator.trim().is_empty() && report.review_ticket_ids.is_empty() && report.unresolved_card_ticket_ids.is_empty() && report.paid_draft_ticket_ids.is_empty() && report.unresolved_adjustment_ticket_ids.is_empty();
                        if ui.add_enabled(ready,egui::Button::new("Finalize local closeout snapshot")).clicked() {
                            match self.candidate(&report) {
                                Ok(value) => candidate=Some(value),
                                Err(error) => self.error=Some(error),
                            }
                        }
                    }
                    Err(error) => { ui.colored_label(egui::Color32::from_rgb(139,47,32),error); }
                }
                if let Some(error)=&self.error { ui.colored_label(egui::Color32::from_rgb(139,47,32),error); }
                self.history(ui,saved);
            });
        self.open = open;
        if !open {
            self.window_end = None;
        }
        candidate
    }

    fn report(&self, ui: &mut egui::Ui, report: &ShiftReport) {
        ui.separator();
        egui::Grid::new("shift-totals")
            .num_columns(2)
            .show(ui, |ui| {
                for (label, value) in [
                    ("Cash sales", report.cash_sales_cents),
                    ("Completed cash refunds", report.cash_refunds_cents),
                    ("Cash received", report.cash_received_cents),
                    ("Change given", report.cash_change_cents),
                    ("Verified external card sales", report.verified_card_cents),
                    ("Completed external card refunds", report.card_refunds_cents),
                    (
                        "Net card movement",
                        report.verified_card_cents - report.card_refunds_cents,
                    ),
                    ("Pending business billing", report.pending_billing_cents),
                    ("Completed billing credits", report.billing_credits_cents),
                    (
                        "Net pending billing movement",
                        report.pending_billing_cents - report.billing_credits_cents,
                    ),
                    (
                        "Completed amount needing review",
                        report.unverified_completed_cents,
                    ),
                ] {
                    ui.label(label);
                    ui.label(money(value));
                    ui.end_row();
                }
            });
        ui.label(format!(
            "{} completed / {} open drafts / {} held / {} unpriced",
            report.completed_tickets,
            report.open_drafts,
            report.held_tickets,
            report.unpriced_tickets
        ));
        ticket_queue(
            ui,
            "Unresolved external card",
            &report.unresolved_card_ticket_ids,
        );
        ticket_queue(
            ui,
            "Taken tender on unfinished tickets (complete these first)",
            &report.paid_draft_ticket_ids,
        );
        ticket_queue(
            ui,
            "Completed tickets needing evidence review",
            &report.review_ticket_ids,
        );
        ticket_queue(
            ui,
            "Unresolved refunds or billing credits",
            &report.unresolved_adjustment_ticket_ids,
        );
        if !report.adjustment_event_ids.is_empty() {
            ui.small(format!(
                "Settled adjustment event IDs: {}",
                report
                    .adjustment_event_ids
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    fn declarations(&mut self, ui: &mut egui::Ui, report: &ShiftReport) {
        ui.separator();
        egui::Grid::new("shift-declarations")
            .num_columns(2)
            .show(ui, |ui| {
                for (label, value) in [
                    ("Opening cash float ($)", &mut self.opening),
                    ("Counted cash in drawer ($)", &mut self.counted),
                    ("External reported card sales ($)", &mut self.external_card),
                    (
                        "External reported card refunds ($)",
                        &mut self.external_card_refunds,
                    ),
                ] {
                    ui.label(label);
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .char_limit(18)
                            .desired_width(150.0),
                    );
                    ui.end_row();
                }
            });
        if let Ok(opening) = dollars(&self.opening, "opening cash float")
            && let Some(expected) = opening
                .checked_add(report.cash_sales_cents)
                .and_then(|cash| cash.checked_sub(report.cash_refunds_cents))
        {
            if expected >= 0 {
                ui.label(format!(
                    "Expected drawer: {} (opening + cash sales − cash refunds)",
                    money(expected)
                ));
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(139, 47, 32),
                    "Opening float and cash sales do not cover the recorded cash refunds.",
                );
            }
        }
        ui.label(format!(
            "Authenticated operator: {}",
            if self.operator.is_empty() {
                "Sign in before finalizing"
            } else {
                &self.operator
            }
        ));
        ui.label("Discrepancy reason (required if either total differs)");
        ui.add(
            egui::TextEdit::multiline(&mut self.reason)
                .char_limit(500)
                .desired_rows(2)
                .desired_width(450.0),
        );
    }

    fn history(&mut self, ui: &mut egui::Ui, saved: &[Closeout]) {
        ui.separator();
        ui.collapsing(format!("Saved closeouts ({})", saved.len()), |ui| {
            for closeout in saved.iter().rev() {
                if ui
                    .selectable_label(
                        self.selected_history.as_deref() == Some(&closeout.id),
                        format!(
                            "{} → {} / {}",
                            utc_label(closeout.report.start),
                            utc_label(closeout.report.end),
                            closeout.operator
                        ),
                    )
                    .clicked()
                {
                    self.selected_history = Some(closeout.id.clone());
                }
            }
            if let Some(closeout) = saved
                .iter()
                .find(|c| self.selected_history.as_deref() == Some(c.id.as_str()))
            {
                ui.label("Saved snapshot (read only)");
                ui.label(format!("Operator: {}", closeout.operator));
                ui.label(format!(
                    "Opening {} / expected cash {} / counted {} / variance {}",
                    money(closeout.opening_cash_cents),
                    money(closeout.expected_cash_cents),
                    money(closeout.counted_cash_cents),
                    money(closeout.cash_variance_cents)
                ));
                ui.label(format!(
                    "Card sales reported {} / refunds reported {} / expected net {} / variance {}",
                    money(closeout.external_card_cents),
                    money(closeout.external_card_refunds_cents),
                    money(closeout.expected_card_cents),
                    money(closeout.card_variance_cents)
                ));
                ui.label(format!(
                    "Pending billing: {}",
                    money(closeout.report.pending_billing_cents)
                ));
                ui.label(format!(
                    "Cash refunds {} / card refunds {} / billing credits {}",
                    money(closeout.report.cash_refunds_cents),
                    money(closeout.report.card_refunds_cents),
                    money(closeout.report.billing_credits_cents)
                ));
                if !closeout.reason.is_empty() {
                    ui.label(format!("Reason: {}", closeout.reason));
                }
            }
        });
    }
}

#[cfg(test)]
impl CloseoutUi {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tickets: &[Ticket],
        saved: &[Closeout],
    ) -> Option<Closeout> {
        self.show_using(ctx, tickets, saved, None)
    }
}

#[cfg(test)]
#[path = "../../tests/extended/finance/closeout_ui_tests.rs"]
mod tests;
