use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Material<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub unit: Option<&'a str>,
    pub rate_cents: Option<i64>,
    pub minimum_cents: Option<i64>,
}

const fn priced(
    code: &'static str,
    name: &'static str,
    unit: &'static str,
    rate: i64,
    min: i64,
) -> Material<'static> {
    Material {
        code,
        name,
        unit: Some(unit),
        rate_cents: Some(rate),
        minimum_cents: Some(min),
    }
}
const fn manual(code: &'static str, name: &'static str) -> Material<'static> {
    Material {
        code,
        name,
        unit: None,
        rate_cents: None,
        minimum_cents: None,
    }
}

pub const CATALOG: &[Material<'static>] = &[
    priced("800", "CRT TV", "each", 500, 0),
    priced("801", "Flat Screen", "each", 500, 0),
    priced("802", "Laptops", "lb", 40, 500),
    priced("803", "Other E-waste", "lb", 40, 500),
    manual("900", "Batteries"),
    priced("900-HH", "Household Batteries", "each", 0, 0),
    priced("900-CAR", "Car Batteries", "each", 100, 0),
    manual("901", "Metal Oil Filters"),
    manual("902", "Oil"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Draft,
    Held,
    Completed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Payment {
    Cash,
    ExternalCard { reference: String },
    ChargeTo { account: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    #[serde(default)]
    pub price_approval: Option<crate::price_overrides::PriceApproval>,
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub material_intake: crate::intake::MaterialIntake,
    #[serde(default)]
    pub rule_snapshot: Option<crate::catalog::PricingRule>,
    #[serde(default)]
    pub gross_thousandths: Option<i64>,
    #[serde(default)]
    pub tare_thousandths: Option<i64>,
    pub code: String,
    pub name: String,
    pub quantity: String,
    pub unit: Option<String>,
    #[serde(default)]
    pub rate_cents: Option<i64>,
    pub base_cents: Option<i64>,
    // Applied adjustment, not the configured threshold.
    pub minimum_cents: Option<i64>,
    pub total_cents: Option<i64>,
    pub classification: String,
    pub piece_count: Option<u32>,
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    #[serde(default)]
    pub created_by: Option<crate::operator_snapshot::OperatorSnapshot>,
    #[serde(default)]
    pub completed_by: Option<crate::operator_snapshot::OperatorSnapshot>,
    #[serde(default = "first_line_id")]
    pub next_line_id: u64,
    #[serde(default)]
    pub source_intake: crate::intake::SourceIntake,
    #[serde(default)]
    pub receipt_snapshot: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub customer_choice: crate::workflow::CustomerChoice,
    #[serde(default)]
    pub customer_record: Option<crate::customers::Customer>,
    #[serde(default)]
    pub account_record: Option<crate::customers::BusinessAccount>,
    #[serde(default)]
    pub tender_evidence: crate::tender::TenderEvidence,
    pub id: u64,
    pub created_at: u64,
    pub state: State,
    pub lines: Vec<Line>,
    pub customer: Option<String>,
    pub payment: Option<Payment>,
    pub reprints: u32,
    #[serde(default)]
    pub completed_at: Option<u64>,
}

impl Line {
    pub fn effective_total_cents(&self) -> Option<i64> {
        self.price_approval
            .as_ref()
            .map(|approval| approval.approved_total_cents)
            .or(self.total_cents)
    }
    pub(crate) fn same_price_basis(&self, other: &Self) -> bool {
        self.code == other.code
            && self.name == other.name
            && self.quantity == other.quantity
            && self.unit == other.unit
            && self.rule_snapshot == other.rule_snapshot
            && self.gross_thousandths == other.gross_thousandths
            && self.tare_thousandths == other.tare_thousandths
            && self.rate_cents == other.rate_cents
            && self.base_cents == other.base_cents
            && self.minimum_cents == other.minimum_cents
            && self.total_cents == other.total_cents
    }
    pub(crate) fn preserve_price_approval(&mut self, old: &Self) -> Result<(), String> {
        if old.price_approval.is_some() && !self.same_price_basis(old) {
            return Err("Clear the supervisor price approval before changing billed quantity, measurements or price.".into());
        }
        self.price_approval = old.price_approval.clone();
        Ok(())
    }
}

fn first_line_id() -> u64 {
    1
}

pub fn money(cents: i64) -> String {
    let mag = cents.unsigned_abs();
    format!(
        "{}${}.{:02}",
        if cents < 0 { "-" } else { "" },
        mag / 100,
        mag % 100
    )
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn decimal(input: &str, digits: u32, prompt: &str) -> Result<i64, String> {
    let input = input.trim();
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    if whole.len() > 6
        || fraction.len() > digits as usize
        || (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(prompt.into());
    }
    let signed: i64 = whole.parse().unwrap_or(0);
    let frac: i64 = fraction.parse().unwrap_or(0);
    Ok(signed * 10i64.pow(digits) + frac * 10i64.pow(digits - fraction.len() as u32))
}

pub(crate) fn amount(input: &str) -> Result<Option<i64>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    let input = input.strip_prefix('$').unwrap_or(input);
    decimal(input, 2, "Enter an amount in dollars, e.g. 2.75.").map(Some)
}

pub(crate) fn weight_string(thousandths: i64) -> String {
    if thousandths % 1000 == 0 {
        return (thousandths / 1000).to_string();
    }
    format!("{}.{:03}", thousandths / 1000, thousandths % 1000)
        .trim_end_matches('0')
        .to_owned()
}

fn material_line(material: &Material, qty: &str, manual_amount: &str) -> Result<Line, String> {
    let (quantity, base, adjustment, total) = match (material.unit, material.rate_cents) {
        (Some(unit), Some(rate)) => {
            if !manual_amount.trim().is_empty() {
                return Err("This material has a set rate.".into());
            }
            let (qty, label) = if unit == "each" {
                let units = if qty.trim().is_empty() {
                    1
                } else {
                    decimal(qty, 0, "Enter a positive whole item count.")?
                };
                if units == 0 {
                    return Err("Enter a positive whole item count.".into());
                }
                (units * 1000, units.to_string())
            } else {
                let weight = decimal(qty, 3, "Enter a positive weight in pounds.")?;
                if weight == 0 {
                    return Err("Enter a positive weight in pounds.".into());
                }
                (weight, weight_string(weight))
            };
            let subtotal = (qty * rate + 500) / 1000;
            let floor = (material.minimum_cents.unwrap_or(0) - subtotal).max(0);
            (label, Some(subtotal), Some(floor), Some(subtotal + floor))
        }
        _ => {
            let price = amount(manual_amount)?;
            (String::new(), price, price.map(|_| 0), price)
        }
    };
    Ok(Line {
        price_approval: None,
        id: 0,
        material_intake: Default::default(),
        rule_snapshot: None,
        gross_thousandths: None,
        tare_thousandths: None,
        code: material.code.into(),
        name: material.name.into(),
        quantity,
        unit: material.unit.map(str::to_owned),
        rate_cents: material.rate_cents,
        base_cents: base,
        minimum_cents: adjustment,
        total_cents: total,
        classification: String::new(),
        piece_count: None,
        notes: String::new(),
    })
}

pub(crate) fn rule_line(
    rule: &crate::catalog::PricingRule,
    qty: &str,
    manual: &str,
) -> Result<Line, String> {
    crate::catalog::Catalog {
        rules: vec![rule.clone()],
    }
    .validate()?;
    let material = Material {
        code: &rule.code,
        name: &rule.name,
        unit: rule.unit.as_deref(),
        rate_cents: rule.rate_cents,
        minimum_cents: rule.minimum_cents,
    };
    let mut line = material_line(&material, qty, manual)?;
    line.rule_snapshot = Some(rule.clone());
    Ok(line)
}

impl Ticket {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            created_by: None,
            completed_by: None,
            next_line_id: 1,
            source_intake: Default::default(),
            revision: 0,
            receipt_snapshot: None,
            customer_choice: Default::default(),
            customer_record: None,
            account_record: None,
            tender_evidence: Default::default(),
            created_at: now(),
            completed_at: None,
            state: State::Draft,
            lines: vec![],
            customer: None,
            payment: None,
            reprints: 0,
        }
    }
    pub(crate) fn draft(&self) -> Result<(), String> {
        if self.state == State::Draft {
            Ok(())
        } else {
            Err("Only open tickets can be edited.".into())
        }
    }
    pub fn total(&self) -> Option<i64> {
        self.lines.iter().try_fold(0i64, |sum, line| {
            sum.checked_add(line.effective_total_cents()?)
        })
    }
    pub fn add_line(&mut self, code: &str, quantity: &str, manual: &str) -> Result<(), String> {
        self.draft()?;
        CATALOG
            .iter()
            .find(|item| item.code == code)
            .ok_or("Choose a material.")?;
        self.editable_intake()?;
        let rule = crate::catalog::Catalog::default()
            .rule_for(code, 0)
            .cloned()
            .ok_or(if cfg!(feature = "extended-workflows") {
                "Choose a configured training material."
            } else {
                "Choose a configured material."
            })?;
        self.push_line(rule_line(&rule, quantity, manual)?)?;
        Ok(())
    }
    pub(crate) fn push_line(&mut self, mut line: Line) -> Result<(), String> {
        let highest = self.lines.iter().map(|l| l.id).max().unwrap_or(0);
        let id = self
            .next_line_id
            .max(
                highest
                    .checked_add(1)
                    .ok_or("Material line ID limit reached.")?,
            )
            .max(1);
        let next = id.checked_add(1).ok_or("Material line ID limit reached.")?;
        line.id = id;
        self.lines.push(line);
        self.next_line_id = next;
        Ok(())
    }
    pub fn validate_intake_records(&self) -> Result<(), String> {
        for actor in [&self.created_by, &self.completed_by].into_iter().flatten() {
            actor.validate()?;
        }
        self.source_intake.validate()?;
        let mut ids = std::collections::HashSet::new();
        for line in &self.lines {
            if let Some(approval) = &line.price_approval {
                approval.validate()?;
            }
            line.material_intake.validate()?;
            if line.id > 0 && (!ids.insert(line.id) || line.id >= self.next_line_id) {
                return Err(
                    "Material line identifiers are inconsistent. Reopen the ticket.".into(),
                );
            }
        }
        Ok(())
    }
    pub fn remove_line(&mut self, index: usize) -> Result<(), String> {
        self.draft()?;
        if index >= self.lines.len() {
            return Err("Material line not found.".into());
        }
        self.editable_intake()?;
        self.lines.remove(index);
        Ok(())
    }
    pub fn line_edit(&mut self, index: usize, quantity: &str, manual: &str) -> Result<(), String> {
        self.draft()?;
        self.editable_intake()?;
        let line = self
            .lines
            .get_mut(index)
            .ok_or("Material line not found.")?;
        let mut updated = if let Some(rule) = &line.rule_snapshot {
            rule_line(rule, quantity, manual)?
        } else {
            let material = CATALOG
                .iter()
                .find(|item| item.code == line.code)
                .ok_or("Material is not in the catalog.")?;
            material_line(material, quantity, manual)?
        };
        updated.id = line.id;
        updated.material_intake = line.material_intake.clone();
        updated.classification = line.classification.clone();
        updated.piece_count = line.piece_count;
        updated.notes = line.notes.clone();
        updated.preserve_price_approval(line)?;
        *line = updated;
        Ok(())
    }
    pub fn line_intake(
        &mut self,
        index: usize,
        classification: &str,
        count: &str,
        notes: &str,
    ) -> Result<(), String> {
        self.draft()?;
        let line = self
            .lines
            .get_mut(index)
            .ok_or("Material line not found.")?;
        if classification.chars().count() > 80
            || classification.chars().any(char::is_control)
            || notes.chars().count() > 500
            || notes
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
        {
            return Err("Classification or notes are too long.".into());
        }
        let count = count.trim();
        let piece_count = if count.is_empty() {
            None
        } else {
            let count = count
                .parse::<u32>()
                .map_err(|_| "Enter a positive piece count.")?;
            if count == 0 || count > 999_999 {
                return Err("Enter a positive piece count up to 999999.".into());
            }
            Some(count)
        };
        line.classification = classification.trim().to_owned();
        line.piece_count = piece_count;
        line.notes = notes.trim().to_owned();
        Ok(())
    }
    #[cfg(any(test, feature = "extended-workflows"))]
    pub fn hold(&mut self) -> Result<(), String> {
        self.draft()?;
        self.state = State::Held;
        Ok(())
    }
    pub fn resume(&mut self) -> Result<(), String> {
        if self.state != State::Held {
            return Err("Only held tickets can be resumed.".into());
        }
        self.state = State::Draft;
        Ok(())
    }
    pub fn complete(&mut self, mut payment: Payment) -> Result<(), String> {
        if self.state == State::Completed && self.payment.as_ref() == Some(&payment) {
            return Ok(());
        }
        self.draft()?;
        if self.lines.is_empty() {
            return Err("Add at least one material.".into());
        }
        if self.total().is_none() {
            return Err("Enter all line amounts before checkout.".into());
        }
        self.validate_completion(&payment)?;
        match &mut payment {
            Payment::Cash => (),
            Payment::ExternalCard { reference } => {
                *reference = reference.trim().to_owned();
                if reference.is_empty()
                    || reference.len() > 64
                    || !reference
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                {
                    return Err("Enter the external payment reference, not the card number.".into());
                }
                if (12..=19).contains(&reference.len())
                    && reference.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(if cfg!(feature = "extended-workflows") {
                        "Use a test reference with a letter. Do not enter a card number."
                    } else {
                        "Use the processor reference, not a card number."
                    }
                    .into());
                }
            }
            Payment::ChargeTo { account } => {
                *account = account.trim().to_owned();
                if account.is_empty() || account.chars().count() > 80 {
                    return Err("Select or enter a business account.".into());
                }
            }
        }
        self.customer = self
            .customer
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        self.state = State::Completed;
        self.completed_at = Some(now());
        self.payment = Some(payment);
        self.receipt_snapshot = Some(self.render_receipt());
        Ok(())
    }
    pub fn reprint(&mut self) -> Result<(), String> {
        if self.state != State::Completed {
            return Err("Complete the ticket before reprinting.".into());
        }
        self.reprints = self
            .reprints
            .checked_add(1)
            .ok_or("Reprint count limit reached.")?;
        Ok(())
    }
    pub fn receipt(&self) -> String {
        self.receipt_snapshot
            .clone()
            .unwrap_or_else(|| self.render_receipt())
    }
    pub(crate) fn render_receipt(&self) -> String {
        if self.state != State::Completed {
            return String::new();
        }
        let mut result = format!(
            "E-WASTE COUNTER\nTicket #{:06}\nCustomer: {}\n",
            self.id,
            self.customer.as_deref().unwrap_or("Blank")
        );
        if let Some(customer) = &self.customer_record {
            for address_line in customer
                .address
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                result.push_str(address_line);
                result.push('\n');
            }
            let phone = customer.phone.trim();
            if !phone.is_empty() {
                result.push_str(&format!("Phone #: {phone}\n"));
            }
        }
        result.push('\n');
        for line in &self.lines {
            let measure = line
                .unit
                .as_ref()
                .map(|unit| format!(" ({} {})", line.quantity, unit))
                .unwrap_or_default();
            result.push_str(&format!(
                "{} {}{}  {}\n",
                line.code,
                line.name,
                measure,
                line.effective_total_cents().map(money).unwrap_or_default()
            ));
            if let Some(note) = line
                .rule_snapshot
                .as_ref()
                .and_then(|rule| rule.placeholder_note())
            {
                result.push_str(&format!("  {note}\n"));
            }
            if let Some(approval) = &line.price_approval {
                result.push_str(&format!(
                    "Original line total {} / Authorized line total {}\n",
                    line.total_cents.map(money).unwrap_or_default(),
                    money(approval.approved_total_cents)
                ));
            }
        }
        for line in &self.lines {
            #[cfg(feature = "extended-workflows")]
            if !line.classification.is_empty() {
                result.push_str(&format!("  Classification: {}\n", line.classification));
            }
            #[cfg(not(feature = "extended-workflows"))]
            if matches!(line.code.as_str(), "801" | "802")
                && self.source_intake.category != crate::intake::SourceCategory::Unspecified
            {
                result.push_str(&format!(
                    "  Source category: {}\n",
                    self.source_intake.category.label()
                ));
            } else if !line.classification.is_empty() {
                result.push_str(&format!("  Classification: {}\n", line.classification));
            }
            if let Some(count) = line.piece_count {
                result.push_str(&format!("  Piece count: {count}\n"));
            }
            if let (Some(gross), Some(tare)) = (line.gross_thousandths, line.tare_thousandths) {
                result.push_str(&format!(
                    "  Gross {} lb / Tare {} lb / Net {} lb\n",
                    weight_string(gross),
                    weight_string(tare),
                    line.quantity
                ));
            }
            if let (Some(base), Some(adjustment)) = (line.base_cents, line.minimum_cents) {
                result.push_str(&format!(
                    "{} base {} + minimum adjustment {}\n",
                    line.code,
                    money(base),
                    money(adjustment)
                ));
            }
            if let (Some(rate), Some(unit)) = (line.rate_cents, line.unit.as_deref()) {
                result.push_str(&format!("  Rate {}/{}\n", money(rate), unit));
            }
        }
        if let Some(cash) = &self.tender_evidence.cash {
            result.push_str(&format!(
                "Cash received {} / Change {}\n",
                money(cash.received_cents),
                money(cash.change_cents)
            ));
        }
        if let (Some(account), Some(Payment::ChargeTo { .. })) =
            (&self.account_record, &self.payment)
        {
            result.push_str(&format!("Account {} / {}\n", account.number, account.name));
        }
        result.push_str(&format!("\nTotal  {}\n", money(self.total().unwrap_or(0))));
        match self.payment.as_ref() {
            Some(Payment::Cash) => result.push_str("Cash recorded\n"),
            Some(Payment::ExternalCard { reference }) => {
                result.push_str(&format!("External card recorded: {reference}\n"))
            }
            Some(Payment::ChargeTo { account }) => result.push_str(&format!(
                "Charge to: {account}\nPending billing\nSignature: _______________________\n"
            )),
            None => (),
        }
        #[cfg(feature = "extended-workflows")]
        result.push_str("\nTRAINING COPY - synthetic counter workflow\n");
        #[cfg(not(feature = "extended-workflows"))]
        result.push_str("\nE-waste Counter\n");
        result
    }
}

#[cfg(test)]
#[path = "../../tests/shared/domain_tests.rs"]
mod tests;
