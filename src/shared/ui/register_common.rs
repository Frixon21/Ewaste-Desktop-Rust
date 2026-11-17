//! Register mechanics shared by the basic and optional extended shells.
use super::RegisterApp;
use crate::{
    domain::{State, Ticket},
    store::Store,
};
use eframe::egui::{self, Color32, RichText, Stroke};

pub(super) const FOREST: Color32 = Color32::from_rgb(31, 83, 67);
pub(super) const MINT: Color32 = Color32::from_rgb(230, 242, 232);
pub(super) const OCHRE: Color32 = Color32::from_rgb(107, 63, 13);
pub(super) const INK: Color32 = Color32::from_rgb(27, 45, 39);
pub(super) const MUTED: Color32 = Color32::from_rgb(78, 97, 90);
pub(super) const CANVAS: Color32 = Color32::from_rgb(243, 246, 242);
pub(super) const BORDER: Color32 = Color32::from_rgb(207, 218, 213);
pub(super) const RED: Color32 = Color32::from_rgb(139, 47, 32);

#[derive(Default, PartialEq)]
pub(super) enum Tender {
    #[default]
    Cash,
    Card,
    Charge,
}

#[derive(Default, PartialEq)]
pub(super) enum History {
    #[default]
    All,
    Draft,
    Held,
    Completed,
}

#[derive(Default)]
pub(super) struct LineEdit {
    pub(super) index: usize,
    pub(super) quantity: String,
    pub(super) tare: String,
    pub(super) manual: String,
    pub(super) class: String,
    pub(super) pieces: String,
    pub(super) note: String,
}

pub(super) fn load_initial_tickets(store: &Store) -> Result<(Vec<Ticket>, u64), String> {
    let mut tickets = store.list()?;
    tickets.sort_by_key(|t| t.id);
    let active = if let Some(t) = tickets.iter().rev().find(|t| t.state == State::Draft) {
        t.id
    } else if let Some(t) = tickets.last() {
        t.id
    } else {
        let t = store.seed_empty_register()?;
        tickets.push(t);
        1
    };
    Ok((tickets, active))
}

impl RegisterApp {
    pub(super) fn active(&self) -> &Ticket {
        self.tickets
            .iter()
            .find(|t| t.id == self.active)
            .expect("active ticket is stored")
    }

    pub(super) fn persist_ticket(&self, ticket: &Ticket) -> Result<Ticket, String> {
        #[cfg(feature = "extended-workflows")]
        {
            self.store.save_ticket_as(self.operator_session()?, ticket)
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            self.store.save_counter_ticket(ticket)
        }
    }

    pub(super) fn save_action(
        &mut self,
        action: impl FnOnce(&mut Ticket) -> Result<(), String>,
    ) -> Result<(), String> {
        let index = self
            .tickets
            .iter()
            .position(|t| t.id == self.active)
            .ok_or_else(|| "Ticket not found.".to_string())?;
        let mut candidate = self.tickets[index].clone();
        action(&mut candidate)?;
        if candidate != self.tickets[index] {
            candidate.revision = candidate
                .revision
                .checked_add(1)
                .ok_or("Ticket revision limit reached.")?;
        }
        let stored = self.persist_ticket(&candidate)?;
        self.tickets[index] = stored;
        Ok(())
    }

    pub(super) fn reset_entry_fields(&mut self) {
        self.material = None;
        self.material_text.clear();
        self.quantity.clear();
        self.manual.clear();
        self.reference.clear();
        self.account.clear();
        self.query.clear();
        self.tare.clear();
        self.wf.reset();
        self.edit = None;
    }

    pub(super) fn select(&mut self, code: &'static str) {
        if self.material != Some(code) {
            self.quantity.clear();
            self.tare.clear();
            self.manual.clear();
            self.material = Some(code);
        }
        self.material_text = code.to_owned();
        let unit = self
            .wf
            .catalog
            .rule_for(code, crate::domain::now())
            .and_then(|m| m.unit.as_deref());
        if unit == Some("each") && self.quantity.is_empty() {
            self.quantity.push('1');
        }
        self.focus_value = true;
    }

    pub(super) fn apply_initial_style(&mut self, ctx: &egui::Context) {
        if !self.styled {
            let mut visuals = egui::Visuals::light();
            visuals.panel_fill = Color32::WHITE;
            visuals.override_text_color = Some(INK);
            visuals.selection.bg_fill = MINT;
            visuals.selection.stroke = Stroke::new(1_f32, FOREST);
            ctx.set_visuals(visuals);
            self.styled = true;
        }
    }
}

pub(super) fn state_label(state: State) -> &'static str {
    match state {
        State::Draft => "Draft",
        State::Held => "Held",
        State::Completed => "Completed",
    }
}

pub(super) fn button(label: &str, primary: bool) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).strong().color(if primary {
        Color32::WHITE
    } else {
        FOREST
    }))
    .fill(if primary { FOREST } else { MINT })
    .stroke(Stroke::new(1.0_f32, if primary { FOREST } else { BORDER }))
}

pub(super) fn full_button(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    ui.add(button(label, primary).min_size(egui::vec2(ui.available_width(), 33.0)))
}

pub(super) fn control(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    ui.add(button(label, primary).min_size(egui::vec2(75.0, 31.0)))
}
