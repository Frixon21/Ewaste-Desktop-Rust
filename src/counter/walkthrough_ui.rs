#[path = "../shared/ui/register_common.rs"]
mod register_common;
use register_common::*;
#[path = "../shared/ui/ui_actions.rs"]
mod actions;
#[path = "walkthrough_details.rs"]
mod details_form;
#[path = "../shared/ui/ui_dialogs.rs"]
mod dialogs;
#[path = "printer/printer_ui.rs"]
mod printers;
#[path = "scanner/scanner_ui.rs"]
mod scanners;
#[path = "../shared/ui/ui_workflows.rs"]
mod workflows;
use crate::{domain::*, store::Store};
use details_form::SourceDetails;
use eframe::egui::{self, Color32, Frame, Key, RichText, Stroke, TextEdit};
use printers::PrinterUi;
use scanners::ScannerUi;
use workflows::{WorkflowAct, WorkflowUi};

enum Act {
    New,
    Open(u64),
    Add,
    Remove(usize),
    Edit(usize),
    SaveEdit(usize),
    Workflow(WorkflowAct),
    Resume,
    Complete,
    Receipt,
    CustomerCopy,
    Reprint,
    Details(usize),
    SaveDetails,
    PrinterSettings,
    PrinterSaveChoice,
    PrinterDiscover,
    PrinterClose,
    PrinterRetry(u64),
    PrinterSaveResults,
    PrinterRefresh,
    PrinterRecover(u64),
    PrinterConfirmRecovery,
    PrinterCancelRecovery,
    ScannerOpen,
    ScannerReview,
    ScannerCancel,
    ScannerClear,
}

pub struct RegisterApp {
    store: Store,
    printer: PrinterUi,
    scanner: ScannerUi,
    tickets: Vec<Ticket>,
    active: u64,
    material: Option<&'static str>,
    material_text: String,
    quantity: String,
    tare: String,
    manual: String,
    wf: WorkflowUi,
    tender: Tender,
    reference: String,
    account: String,
    query: String,
    history: History,
    history_query: String,
    show_history: bool,
    focus_code: bool,
    focus_value: bool,
    error: Option<String>,
    notice: Option<String>,
    edit: Option<LineEdit>,
    receipt: Option<(u64, bool, String)>,
    styled: bool,
    details: Option<SourceDetails>,
}

impl RegisterApp {
    pub fn new(store: Store) -> Result<Self, String> {
        let (tickets, active) = load_initial_tickets(&store)?;
        let wf = WorkflowUi::load(&store)?;
        let printer = PrinterUi::load(&store)?;
        Ok(Self {
            wf,
            printer,
            scanner: ScannerUi::default(),
            store,
            tickets,
            active,
            material: None,
            material_text: String::new(),
            quantity: String::new(),
            tare: String::new(),
            manual: String::new(),
            tender: Tender::Cash,
            reference: String::new(),
            account: String::new(),
            query: String::new(),
            history: History::All,
            history_query: String::new(),
            show_history: false,
            focus_code: false,
            focus_value: false,
            error: None,
            notice: None,
            edit: None,
            receipt: None,
            styled: false,
            details: None,
        })
    }

    fn reset_fields(&mut self) {
        self.scanner.clear();
        self.printer.ticket_changed();
        self.reset_entry_fields();
        self.details = None;
        self.receipt = None;
        self.tender = Tender::Cash;
    }

    fn top(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        egui::TopBottomPanel::top("masthead")
            .frame(
                Frame::new()
                    .fill(FOREST)
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("E-waste Counter")
                            .size(19.)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    if control(ui, "F2  New ticket", true).clicked() {
                        *action = Some(Act::New);
                    }
                    if control(ui, "F3  History", false).clicked() {
                        self.show_history = !self.show_history;
                    }
                });
            });
    }

    fn catalog(&mut self, ctx: &egui::Context, draft: bool) {
        egui::SidePanel::left("catalog")
            .resizable(false)
            .default_width(206.0)
            .frame(
                Frame::new()
                    .fill(CANVAS)
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.heading("Materials");
                ui.small("Choose a material, then enter its quantity or weight.");
                ui.add_space(8.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for m in CATALOG {
                        if m.code == "900-HH" || m.code == "900-CAR" {
                            continue;
                        }
                        self.material_button(ui, m, draft);
                    }
                });
            });
    }

    fn material_button(&mut self, ui: &mut egui::Ui, material: &Material<'static>, draft: bool) {
        let selected = self.material == Some(material.code);
        let rule = self
            .wf
            .catalog
            .rule_for(material.code, crate::domain::now());
        let rate = rule
            .map(|r| match (r.rate_cents, r.unit.as_deref()) {
                (Some(cents), Some(unit)) => format!(
                    "{}/{}{}",
                    money(cents),
                    unit,
                    if r.provisional { " (placeholder)" } else { "" }
                ),
                _ => "Manual (placeholder)".into(),
            })
            .unwrap_or_else(|| "No effective tariff".into());
        let caption = format!("{}   {}\n{}", material.code, material.name, rate);
        let response = ui.add_enabled(
            draft,
            egui::Button::new(RichText::new(caption).size(13.0).color(INK))
                .fill(if selected { MINT } else { Color32::WHITE })
                .stroke(Stroke::new(1.0_f32, if selected { FOREST } else { BORDER }))
                .min_size(egui::vec2(ui.available_width(), 39.0)),
        );
        if response.clicked() {
            self.select(material.code);
        }
        ui.add_space(3.0);
    }

    fn checkout(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        let ticket = self.active();
        let state = ticket.state;
        let total = ticket.total();
        let ready = !ticket.lines.is_empty() && total.is_some();
        let reprints =
            ticket.reprints as usize + self.wf.prints.successful_internal_copies(ticket.id);
        egui::SidePanel::right("checkout")
            .resizable(false)
            .default_width(244.)
            .frame(
                Frame::new()
                    .fill(CANVAS)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                let footer = if state == State::Completed { 124. } else { 48. };
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - footer).max(40.))
                    .show(ui, |ui| {
                        ui.heading("Ticket total");
                        ui.label(
                            RichText::new(total.map_or_else(|| "Pending amount".into(), money))
                                .size(29.)
                                .strong()
                                .color(if total.is_some() { FOREST } else { OCHRE }),
                        );
                        ui.separator();
                        if state == State::Draft {
                            self.workflow_checkout(ui, action);
                        } else if state == State::Completed {
                            ui.label(format!("Saved copy previews: {reprints}"));
                        } else {
                            ui.label("Previously held ticket");
                        }
                    });
                ui.separator();
                match state {
                    State::Draft => {
                        if ui
                            .add_enabled(
                                ready,
                                button("F12  Complete", true)
                                    .min_size(egui::vec2(ui.available_width(), 33.)),
                            )
                            .clicked()
                        {
                            *action = Some(Act::Complete);
                        }
                    }
                    State::Held => {
                        if full_button(ui, "Resume ticket", true).clicked() {
                            *action = Some(Act::Resume);
                        }
                    }
                    State::Completed => {
                        for (label, command) in [
                            ("View receipt", Act::Receipt),
                            ("Customer copy", Act::CustomerCopy),
                            ("F8  Internal copy", Act::Reprint),
                        ] {
                            if full_button(ui, label, false).clicked() {
                                *action = Some(command);
                            }
                        }
                    }
                }
            });
    }

    fn line_entry(&mut self, ui: &mut egui::Ui, action: &mut Option<Act>) {
        let selected = self
            .wf
            .catalog
            .rule_for(self.material_text.trim(), crate::domain::now())
            .cloned();
        let unconfigured = selected.as_ref().is_some_and(|m| m.rate_cents.is_none());
        let unit = selected.as_ref().and_then(|m| m.unit.as_deref());
        Frame::new()
            .fill(CANVAS)
            .inner_margin(egui::Margin::same(12))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let editor = ui.add(
                        TextEdit::singleline(&mut self.material_text)
                            .hint_text("Material code")
                            .desired_width(116.0)
                            .char_limit(32)
                            .id_salt("material-code"),
                    );
                    if self.focus_code {
                        editor.request_focus();
                        self.focus_code = false;
                    }
                    if editor.changed() {
                        self.material = None;
                        self.quantity.clear();
                        self.tare.clear();
                        self.manual.clear();
                        let code = self.material_text.trim();
                        if let Some(material) =
                            CATALOG.iter().find(|m| m.code.eq_ignore_ascii_case(code))
                        {
                            self.material = Some(material.code);
                            if self
                                .wf
                                .catalog
                                .rule_for(material.code, crate::domain::now())
                                .is_some_and(|r| r.unit.as_deref() == Some("each"))
                            {
                                self.quantity = "1".into();
                            }
                        }
                    }
                    if editor.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.focus_value = true;
                    }
                    match &selected {
                        Some(m) => {
                            ui.label(RichText::new(&m.name).strong());
                        }
                        None => {
                            ui.label(RichText::new("Select material").color(MUTED));
                        }
                    }
                    if unit == Some("lb") {
                        ui.add(
                            TextEdit::singleline(&mut self.tare)
                                .hint_text("Tare lb (0)")
                                .desired_width(80.0)
                                .char_limit(18),
                        );
                    }
                    let value = if unconfigured {
                        ui.add(
                            TextEdit::singleline(&mut self.manual)
                                .hint_text("Manual $")
                                .desired_width(83.0)
                                .char_limit(18),
                        )
                    } else {
                        ui.add_enabled(
                            selected.is_some(),
                            TextEdit::singleline(&mut self.quantity)
                                .hint_text(if unit == Some("lb") {
                                    "Gross (lb)"
                                } else {
                                    "Count"
                                })
                                .desired_width(79.0)
                                .char_limit(18),
                        )
                    };
                    if self.focus_value {
                        if selected.is_some() {
                            value.request_focus();
                        } else {
                            editor.request_focus();
                        }
                        self.focus_value = false;
                    }
                    if control(ui, "Add line", true).clicked()
                        || (value.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                    {
                        *action = Some(Act::Add);
                    }
                });
                if let Some(note) = selected.as_ref().and_then(|rule| rule.placeholder_note()) {
                    ui.small(note);
                }
            });
    }

    fn work(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        let state = self.active().state;
        let editable = self.active().editable_intake().is_ok();
        let id = self.active;
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(Color32::WHITE)
                    .inner_margin(egui::Margin::same(17)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading(format!("Ticket #{id:06}"));
                        let tag_color = if state == State::Completed {
                            FOREST
                        } else {
                            OCHRE
                        };
                        ui.label(RichText::new(state_label(state)).strong().color(tag_color));
                    });
                    if let Some(notice) = &self.notice {
                        ui.colored_label(FOREST, notice);
                    }
                    if let Some(error) = &self.error {
                        ui.label(RichText::new(error).color(RED));
                    }
                    ui.add_space(8.0);
                    if editable {
                        self.line_entry(ui, action);
                        ui.add_space(13.0);
                    } else if state == State::Draft {
                        ui.small("Saved material details are read-only.");
                    }
                    ui.add_space(8.0);
                    ui.heading("Material lines");
                    let lines = &self.active().lines;
                    if lines.is_empty() {
                        ui.label(RichText::new("No materials added").color(MUTED));
                        return;
                    }
                    let table = |ui: &mut egui::Ui| {
                        egui::Grid::new("material-lines")
                            .striped(true)
                            .spacing(egui::vec2(9.0, 9.0))
                            .min_col_width(48.0)
                            .show(ui, |ui| {
                                for title in ["Material", "Qty", "Rate", "Amount"] {
                                    ui.label(RichText::new(title).small().strong().color(MUTED));
                                }
                                if editable {
                                    ui.label("");
                                }
                                ui.end_row();
                                for (index, line) in lines.iter().enumerate() {
                                    ui.vertical(|ui| {
                                        ui.set_max_width(180.0);
                                        ui.add(
                                            egui::Label::new(format!(
                                                "{}  {}",
                                                line.code, line.name
                                            ))
                                            .wrap(),
                                        );
                                        if let Some(note) = line
                                            .rule_snapshot
                                            .as_ref()
                                            .and_then(|rule| rule.placeholder_note())
                                        {
                                            ui.add(
                                                egui::Label::new(RichText::new(note).small())
                                                    .wrap(),
                                            );
                                        }
                                    });
                                    let qty = match &line.unit {
                                        Some(unit) => format!("{} {}", line.quantity, unit),
                                        None => line
                                            .piece_count
                                            .map_or_else(|| "·".to_owned(), |n| n.to_string()),
                                    };
                                    ui.label(qty);
                                    let rate = match (line.rate_cents, line.unit.as_deref()) {
                                        (Some(0), _) => "Free".into(),
                                        (Some(cents), Some(unit)) => {
                                            format!("{}/{unit}", money(cents))
                                        }
                                        (Some(cents), None) => money(cents),
                                        (None, _) => "Manual".into(),
                                    };
                                    ui.label(rate);
                                    ui.vertical(|ui| {
                                        let line_total = line
                                            .effective_total_cents()
                                            .map_or_else(|| "Pending".into(), money);
                                        ui.label(RichText::new(line_total).strong());
                                        if let Some(adjustment) =
                                            line.minimum_cents.filter(|value| *value > 0)
                                        {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} min adj.",
                                                    money(adjustment)
                                                ))
                                                .small()
                                                .color(MUTED),
                                            );
                                        }
                                    });
                                    if editable {
                                        ui.horizontal(|ui| {
                                            if ui.small_button("Edit").clicked() {
                                                *action = Some(Act::Edit(index));
                                            }
                                            if matches!(line.code.as_str(), "801" | "802")
                                                && ui.small_button("Source details").clicked()
                                            {
                                                *action = Some(Act::Details(index));
                                            }
                                            if ui.small_button("Remove").clicked() {
                                                *action = Some(Act::Remove(index));
                                            }
                                        });
                                    }
                                    ui.end_row();
                                }
                            });
                    };
                    egui::ScrollArea::horizontal()
                        .id_salt("line-grid-scroll")
                        .show(ui, table);
                });
            });
    }
}

impl eframe::App for RegisterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

impl RegisterApp {
    fn frame(&mut self, ctx: &egui::Context) {
        let capture_frame = self.scanner.active();
        if capture_frame {
            self.scanner_capture_tick(ctx);
        }
        self.printer_tick(ctx);
        self.apply_initial_style(ctx);
        // Scanner input stays isolated after capture interruptions until acknowledgement.
        if capture_frame {
            self.scanner_capture_panel(ctx);
            return;
        }
        let mut action = None;
        let state = self.active().state;
        let modal = self.printer.open
            || self.printer.recovery.is_some()
            || self.edit.is_some()
            || self.details.is_some()
            || self.receipt.is_some()
            || self.wf.modal_open();
        if !modal {
            ctx.input_mut(|input| {
                let modifiers = egui::Modifiers::NONE;
                if input.consume_key(modifiers, Key::F2) {
                    action = Some(Act::New);
                }
                if input.consume_key(modifiers, Key::F3) {
                    self.show_history = !self.show_history;
                }
                if state == State::Completed && input.consume_key(modifiers, Key::F8) {
                    action = Some(Act::Reprint);
                }
                if state == State::Draft && input.consume_key(modifiers, Key::F12) {
                    action = Some(Act::Complete);
                }
            });
        }
        self.top(ctx, &mut action);
        self.printer_status(ctx, &mut action);
        self.catalog(ctx, self.active().editable_intake().is_ok());
        self.checkout(ctx, &mut action);
        self.work(ctx, &mut action);
        self.dialogs(ctx, &mut action);
        self.workflow_dialogs(ctx, &mut action);
        self.source_details_dialog(ctx, &mut action);
        self.printer_dialog(ctx, &mut action);
        if let Some(action) = action {
            self.error = self.run(action).err();
        }
    }
}

#[cfg(test)]
#[path = "../tests/counter/walkthrough_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/counter/receipt_dispatch_integration_tests.rs"]
mod receipt_dispatch_integration_tests;

#[cfg(test)]
#[path = "../tests/counter/scanner_integration_tests.rs"]
mod scanner_integration_tests;
