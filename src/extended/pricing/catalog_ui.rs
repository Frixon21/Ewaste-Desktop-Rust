//! A reviewable editor for synthetic pricing schedules. Persistence belongs to
//! the caller; a returned candidate never closes the window by itself.

use crate::catalog::{Catalog, PricingRule, TRAINING_NOTICE};
use eframe::egui;

#[derive(Default)]
pub struct CatalogUi {
    pub open: bool,
    form: Option<RuleForm>,
    message: Option<String>,
}

struct RuleForm {
    rule: PricingRule,
    is_new: bool,
    from: String,
    until: String,
}

impl RuleForm {
    fn from_rule(rule: &PricingRule, is_new: bool) -> Self {
        Self {
            rule: rule.clone(),
            is_new,
            from: format_utc_date(rule.effective_from)
                .unwrap_or_else(|_| "Date outside editor range".into()),
            until: rule
                .effective_until
                .map(|at| {
                    format_utc_date(at).unwrap_or_else(|_| "Date outside editor range".into())
                })
                .unwrap_or_default(),
        }
    }

    fn new() -> Self {
        Self::from_rule(
            &PricingRule {
                id: String::new(),
                code: String::new(),
                name: String::new(),
                unit: Some("each".into()),
                rate_cents: Some(0),
                minimum_cents: Some(0),
                effective_from: 0,
                effective_until: None,
                provisional: true,
                classification_required: false,
                blank_allowed: false,
            },
            true,
        )
    }

    fn candidate(&self) -> Result<PricingRule, String> {
        // Existing schedules may contain times within a day. Merely opening and
        // saving their date fields must not silently truncate those timestamps.
        let effective_from = if format_utc_date(self.rule.effective_from).ok().as_deref()
            == Some(self.from.as_str())
        {
            self.rule.effective_from
        } else {
            parse_utc_date(&self.from)?
        };
        let effective_until = if self.until.is_empty() {
            None
        } else {
            match self.rule.effective_until {
                Some(original)
                    if format_utc_date(original).ok().as_deref() == Some(self.until.as_str()) =>
                {
                    Some(original)
                }
                _ => Some(parse_utc_date(&self.until)?),
            }
        };
        let candidate = PricingRule {
            effective_from,
            effective_until,
            ..self.rule.clone()
        };
        candidate.validate()?;
        Ok(candidate)
    }
}

impl CatalogUi {
    /// Call after a failed persistence attempt. The complete form remains open.
    pub fn error(&mut self, message: String) {
        self.message = Some(message);
        self.open = true;
    }

    /// Acknowledge that the returned catalog was saved successfully.
    pub fn saved(&mut self) {
        self.message = None;
        self.form = None;
        self.open = false;
    }

    pub fn show(&mut self, context: &egui::Context, catalog: &Catalog) -> Option<Catalog> {
        if !self.open {
            return None;
        }
        if self.form.is_none() {
            self.form = Some(
                catalog
                    .rules
                    .first()
                    .map(|rule| RuleForm::from_rule(rule, false))
                    .unwrap_or_else(RuleForm::new),
            );
        }
        let mut open = self.open;
        let mut result = None;
        egui::Window::new("Training catalog and prices")
            .open(&mut open).default_size([690., 600.]).min_width(520.)
            .show(context, |ui| {
                ui.label(TRAINING_NOTICE);
                ui.label("Dates use UTC. An end date excludes that day; adjacent rules may share the same boundary.");
                self.rule_picker(ui, catalog);
                ui.separator();
                if let Some(form) = self.form.as_mut() { edit_form(ui, form); }
                if let Some(message) = &self.message {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
                ui.separator();
                ui.label("Existing receipts retain their saved rules and prices. New rules must not overlap another schedule for the same material.");
                if ui.button("Validate and save training catalog").clicked() {
                    let candidate = self.form.as_ref().ok_or("Select a rule first.".to_owned())
                        .and_then(RuleForm::candidate)
                        .and_then(|rule| catalog.save_rule(&rule));
                    match candidate {
                        Ok(candidate) => { self.message = None; result = Some(candidate); }
                        Err(message) => self.message = Some(message),
                    }
                }
            });
        self.open = open;
        result
    }

    fn rule_picker(&mut self, ui: &mut egui::Ui, catalog: &Catalog) {
        let selected = self
            .form
            .as_ref()
            .map(|form| {
                if form.is_new {
                    "New training rule".into()
                } else {
                    format!("{} — {}", form.rule.id, form.rule.name)
                }
            })
            .unwrap_or_default();
        let mut selection = None;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("training-catalog-rules")
                .selected_text(selected)
                .width(370.)
                .show_ui(ui, |ui| {
                    for rule in &catalog.rules {
                        let chosen = self
                            .form
                            .as_ref()
                            .is_some_and(|form| !form.is_new && form.rule.id == rule.id);
                        if ui
                            .selectable_label(chosen, format!("{} — {}", rule.id, rule.name))
                            .clicked()
                        {
                            selection = Some(RuleForm::from_rule(rule, false));
                        }
                    }
                });
            if ui.button("Add rule").clicked() {
                selection = Some(RuleForm::new());
            }
        });
        if let Some(form) = selection {
            self.form = Some(form);
            self.message = None;
        }
    }
}

fn edit_form(ui: &mut egui::Ui, form: &mut RuleForm) {
    egui::Grid::new("training-catalog-fields")
        .num_columns(2)
        .spacing([14., 10.])
        .show(ui, |ui| {
            ui.label("Stable rule ID");
            if form.is_new {
                ui.text_edit_singleline(&mut form.rule.id);
            } else {
                ui.label(&form.rule.id);
            }
            ui.end_row();
            ui.label("Material code");
            if form.is_new {
                ui.text_edit_singleline(&mut form.rule.code);
            } else {
                ui.label(&form.rule.code);
            }
            ui.end_row();
            ui.label("Material name");
            ui.text_edit_singleline(&mut form.rule.name);
            ui.end_row();
            ui.label("Billing unit");
            let previous = form.rule.unit.clone().unwrap_or_default();
            let mut unit = previous.clone();
            egui::ComboBox::from_id_salt("training-unit")
                .selected_text(if unit.is_empty() {
                    "Manual amount"
                } else {
                    &unit
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut unit, String::new(), "Manual amount");
                    ui.selectable_value(&mut unit, "each".into(), "Each (whole item count)");
                    ui.selectable_value(&mut unit, "lb".into(), "Pounds");
                });
            if previous != unit {
                form.rule.unit = (!unit.is_empty()).then_some(unit);
                form.rule.rate_cents = form
                    .rule
                    .unit
                    .as_ref()
                    .map(|_| form.rule.rate_cents.unwrap_or(0));
                form.rule.minimum_cents = form
                    .rule
                    .unit
                    .as_ref()
                    .map(|_| form.rule.minimum_cents.unwrap_or(0));
            }
            ui.end_row();
            ui.label("Rate (cents per billing unit)");
            if let Some(rate) = form.rule.rate_cents.as_mut() {
                ui.add(egui::DragValue::new(rate).range(0..=99_999_999));
            } else {
                ui.label("Cashier enters a line amount");
            }
            ui.end_row();
            ui.label("Minimum (cents per line)");
            if form.rule.unit.is_some() {
                let mut minimum = form.rule.minimum_cents.unwrap_or(0);
                if ui
                    .add(egui::DragValue::new(&mut minimum).range(0..=99_999_999))
                    .changed()
                {
                    form.rule.minimum_cents = Some(minimum);
                }
            } else {
                ui.label("No automatic minimum");
            }
            ui.end_row();
            ui.label("Effective from (YYYY-MM-DD)");
            ui.text_edit_singleline(&mut form.from);
            ui.end_row();
            ui.label("Effective until (exclusive; optional)");
            ui.text_edit_singleline(&mut form.until);
            ui.end_row();
        });
    ui.checkbox(
        &mut form.rule.classification_required,
        "Require classification at completion",
    );
    ui.checkbox(
        &mut form.rule.blank_allowed,
        "Allow deliberate Blank customer for this training material",
    );
    ui.checkbox(
        &mut form.rule.provisional,
        "Provisional fixture requiring policy review",
    );
}

fn leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn month_days(year: u32, month: u32) -> u32 {
    match month {
        2 => {
            if leap_year(year) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub(crate) fn parse_utc_date(input: &str) -> Result<u64, String> {
    let error = "Enter a valid UTC date as YYYY-MM-DD, between 1970 and 9999.";
    let bytes = input.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return Err(error.into());
    }
    let year: u32 = input[..4].parse().map_err(|_| error)?;
    let month: u32 = input[5..7].parse().map_err(|_| error)?;
    let day: u32 = input[8..].parse().map_err(|_| error)?;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > month_days(year, month)
    {
        return Err(error.into());
    }
    let years: u64 = (1970..year)
        .map(|year| if leap_year(year) { 366 } else { 365 })
        .sum();
    let months: u64 = (1..month)
        .map(|month| u64::from(month_days(year, month)))
        .sum();
    Ok((years + months + u64::from(day - 1)) * 86_400)
}

pub(crate) fn format_utc_date(timestamp: u64) -> Result<String, String> {
    if timestamp > 253_402_300_799 {
        return Err("Date outside the editor's supported range.".into());
    }
    let mut days = timestamp / 86_400;
    let mut year = 1970;
    loop {
        let length = if leap_year(year) { 366 } else { 365 };
        if days < length {
            break;
        }
        days -= length;
        year += 1;
    }
    let mut month = 1;
    while days >= u64::from(month_days(year, month)) {
        days -= u64::from(month_days(year, month));
        month += 1;
    }
    Ok(format!("{year:04}-{month:02}-{:02}", days + 1))
}

#[cfg(test)]
#[path = "../../tests/extended/pricing/catalog_ui_tests.rs"]
mod tests;
