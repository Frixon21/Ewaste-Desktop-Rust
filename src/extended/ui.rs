#[path = "../shared/ui/register_common.rs"]
mod register_common;
use register_common::*;
#[path = "finance/ui_adjustments.rs"]
mod finances;
use finances::FinancialWork;
#[path = "auth/ui_auth.rs"]
mod operators;
#[path = "pricing/ui_price_overrides.rs"]
mod price_approvals;
#[path = "inventory/ui_inventory.rs"]
mod stock;
use crate::auth::Permission;
use operators::OperatorUi;
use stock::InventoryWork;
#[path = "intake/ui_intake.rs"]
mod intake;
use intake::IntakeWork;
#[path = "../shared/ui/ui_actions.rs"]
mod actions;
#[path = "../shared/ui/ui_dialogs.rs"]
mod dialogs;
#[path = "../shared/ui/ui_workflows.rs"]
mod workflows;
use crate::{domain::*, store::Store};
use eframe::egui::{self, Align, Color32, Frame, Key, Layout, RichText, Stroke, TextEdit};
use workflows::{WorkflowAct, WorkflowUi};

enum Act {
    New,
    SignOut,
    Backup,
    Open(u64),
    Add,
    Remove(usize),
    Edit(usize),
    SaveEdit(usize),
    Workflow(WorkflowAct),
    Hold,
    Resume,
    Complete,
    Receipt,
    CustomerCopy,
    Reprint,
}

pub struct RegisterApp {
    store: Store,
    security: OperatorUi,
    financial: FinancialWork,
    inventory: InventoryWork,
    price_editor: crate::price_override_ui::PriceOverrideUi,
    tickets: Vec<Ticket>,
    active: u64,
    material: Option<&'static str>,
    material_text: String,
    quantity: String,
    tare: String,
    manual: String,
    wf: WorkflowUi,
    intake: IntakeWork,
    catalog_editor: crate::catalog_ui::CatalogUi,
    closeout_editor: crate::closeout_ui::CloseoutUi,
    closeouts: Vec<crate::closeout::Closeout>,
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
}

impl RegisterApp {
    pub fn new(store: Store) -> Result<Self, String> {
        let (tickets, active) = load_initial_tickets(&store)?;
        let wf = WorkflowUi::load(&store)?;
        let security = OperatorUi::load(&store)?;
        let financial = FinancialWork::load(&store)?;
        let inventory = InventoryWork::load(&store)?;
        let intake = IntakeWork::load(&store)?;
        let closeouts = store.closeouts()?;
        Ok(Self {
            wf,
            security,
            financial,
            inventory,
            price_editor: Default::default(),
            intake,
            closeouts,
            catalog_editor: Default::default(),
            closeout_editor: Default::default(),
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
        })
    }

    fn reset_fields(&mut self) {
        self.reset_entry_fields();
        self.receipt = None;
        self.tender = Tender::Cash;
    }

    fn top(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        egui::TopBottomPanel::top("masthead")
            .frame(
                Frame::new()
                    .fill(FOREST)
                    .inner_margin(egui::Margin::symmetric(18, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("E-waste Counter - Training")
                            .size(19.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if control(ui, "F2  New ticket", true).clicked() {
                            *action = Some(Act::New);
                        }
                        if ui
                            .add_enabled(self.can(Permission::Backup), egui::Button::new("Backup"))
                            .clicked()
                        {
                            *action = Some(Act::Backup);
                        }
                        if ui
                            .add_enabled(
                                self.can(Permission::ManageTariffs),
                                egui::Button::new("Training tariffs"),
                            )
                            .clicked()
                        {
                            self.catalog_editor.open = true;
                        }
                        if ui
                            .add_enabled(
                                self.can(Permission::Closeout),
                                egui::Button::new("Shift closeout"),
                            )
                            .clicked()
                        {
                            self.closeout_editor.open = true;
                        }
                        if control(ui, "F3  History", false).clicked() {
                            self.show_history = !self.show_history;
                        }
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    if let Some(session) = &self.security.session {
                        ui.colored_label(
                            Color32::WHITE,
                            format!(
                                "{} / {:?}",
                                session.actor().display_name,
                                session.actor().role
                            ),
                        );
                    }
                    if ui.button("Lock / sign out").clicked() {
                        *action = Some(Act::SignOut);
                    }
                    if self.can(Permission::ManageOperators) && ui.button("Operators").clicked() {
                        self.security.ui.show_operators = true;
                    }
                    if self.can(Permission::AdjustFinancials)
                        && ui.button("Audit history").clicked()
                    {
                        self.security.show_audit = true;
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
                ui.small("Synthetic fixtures. Other material rates are provisional.");
                ui.add_space(8.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for m in CATALOG {
                        if m.code == "900-HH" || m.code == "900-CAR" {
                            continue;
                        }
                        self.material_button(ui, m, draft);
                        if m.code == "900" {
                            for subtype in CATALOG
                                .iter()
                                .filter(|m| matches!(m.code, "900-HH" | "900-CAR"))
                            {
                                ui.indent(subtype.code, |ui| {
                                    self.material_button(ui, subtype, draft)
                                });
                            }
                        }
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
                    if r.provisional { " (provisional)" } else { "" }
                ),
                _ => "Manual (provisional)".into(),
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
        let t = self.active();
        let state = t.state;
        let total = t.total();
        let empty = t.lines.is_empty();
        let reprints = t.reprints as usize + self.wf.prints.successful_internal_copies(t.id);
        egui::SidePanel::right("checkout")
            .resizable(false)
            .default_width(244.0)
            .frame(
                Frame::new()
                    .fill(CANVAS)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Ticket total");
                    let total_caption = total.map_or("Pending amount".into(), money);
                    ui.label(
                        RichText::new(total_caption)
                            .size(29.0)
                            .strong()
                            .color(if total.is_some() { FOREST } else { OCHRE }),
                    );
                    ui.separator();
                    if state == State::Draft {
                        self.workflow_checkout(ui, action);
                        ui.add_space(8.0);
                        let ready = !empty && total.is_some();
                        ui.add_enabled_ui(ready, |ui| {
                            if full_button(ui, "F12  Complete", true).clicked() {
                                *action = Some(Act::Complete);
                            }
                        });
                        if full_button(ui, "F4  Hold ticket", false).clicked() {
                            *action = Some(Act::Hold);
                        }
                    } else if state == State::Held {
                        if full_button(ui, "Resume ticket", true).clicked() {
                            *action = Some(Act::Resume);
                        }
                    } else {
                        ui.label(format!("Reprints: {reprints}"));
                        if full_button(ui, "View receipt", true).clicked() {
                            *action = Some(Act::Receipt);
                        }
                        if full_button(ui, "Customer copy (test)", false).clicked() {
                            *action = Some(Act::CustomerCopy);
                        }
                        if full_button(ui, "F8  Internal copy (test)", false).clicked() {
                            *action = Some(Act::Reprint);
                        }
                    }
                })
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
            });
    }

    fn work(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        let state = self.active().state;
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
                    if state == State::Draft {
                        self.line_entry(ui, action);
                        ui.add_space(13.0);
                    }
                    self.intake_controls(ui);
                    self.financial_controls(ui);
                    self.price_controls(ui);
                    self.inventory_controls(ui);
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
                                if state == State::Draft {
                                    ui.label("");
                                }
                                ui.end_row();
                                for (index, line) in lines.iter().enumerate() {
                                    ui.label(format!("{}  {}", line.code, line.name));
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
                                        if line.price_approval.is_some() {
                                            ui.small(format!(
                                                "Approved; original {}",
                                                line.total_cents
                                                    .map_or_else(|| "Pending".into(), money)
                                            ));
                                        }
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
                                    if state == State::Draft {
                                        ui.horizontal(|ui| {
                                            if ui.small_button("Edit").clicked() {
                                                *action = Some(Act::Edit(index));
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
        self.apply_initial_style(ctx);
        if !self.authentication_screen(ctx) {
            return;
        }
        let mut action = None;
        let state = self.active().state;
        if self.edit.is_none()
            && self.receipt.is_none()
            && !self.wf.modal_open()
            && !self.catalog_editor.open
            && !self.closeout_editor.open
            && !self.intake.modal_open()
            && !self.security.ui.show_operators
            && !self.security.show_audit
            && !self.financial.editor.open
            && !self.price_editor.open
            && !self.inventory.editor.open
        {
            ctx.input_mut(|i| {
                let m = egui::Modifiers::NONE;
                if i.consume_key(m, Key::F2) {
                    action = Some(Act::New);
                }
                if i.consume_key(m, Key::F3) {
                    self.show_history = !self.show_history;
                }
                if state == State::Draft && i.consume_key(m, Key::F4) {
                    action = Some(Act::Hold);
                }
                if state == State::Completed && i.consume_key(m, Key::F8) {
                    action = Some(Act::Reprint);
                }
                if state == State::Draft && i.consume_key(m, Key::F12) {
                    action = Some(Act::Complete);
                }
            });
        }
        self.top(ctx, &mut action);
        self.workflow_status(ctx, &mut action);
        self.catalog(ctx, state == State::Draft);
        self.checkout(ctx, &mut action);
        self.work(ctx, &mut action);
        self.dialogs(ctx, &mut action);
        self.workflow_dialogs(ctx, &mut action);
        self.intake_panels(ctx, &mut action);
        self.operator_panels(ctx);
        self.financial_panels(ctx);
        self.price_panels(ctx);
        self.inventory_panels(ctx);
        if let Some(session) = &self.security.session {
            self.closeout_editor
                .set_operator(&session.actor().display_name);
        }
        if self.can(Permission::ManageTariffs)
            && let Some(next) = self.catalog_editor.show(ctx, &self.wf.catalog)
        {
            match self.store.save_catalog_as(
                self.operator_session().expect("authenticated operator"),
                &self.wf.catalog,
                &next,
            ) {
                Ok(()) => {
                    self.wf.catalog = next;
                    self.catalog_editor.saved();
                }
                Err(error) => self.catalog_editor.error(error),
            }
        }
        if self.can(Permission::Closeout)
            && let Some(record) = self.closeout_editor.show_with_adjustments(
                ctx,
                &self.tickets,
                &self.closeouts,
                &self.financial.ledger,
            )
        {
            match self.store.save_closeout_as(
                self.operator_session().expect("authenticated operator"),
                &record,
            ) {
                Ok(_) => match self.store.closeouts() {
                    Ok(records) => {
                        self.closeouts = records;
                        self.closeout_editor.saved();
                    }
                    Err(error) => self.closeout_editor.error(error),
                },
                Err(error) => self.closeout_editor.error(error),
            }
        }
        if let Some(action) = action {
            self.error = self.run(action).err();
        }
    }
}
