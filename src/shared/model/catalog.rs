//! Dated pricing for synthetic counter rehearsals. These are training fixtures,
//! never authorization to activate a production tariff or regulated fee.

use serde::{Deserialize, Serialize};

#[cfg(any(test, feature = "extended-workflows"))]
pub const TRAINING_NOTICE: &str = "Training only: minimum adjustments apply per line. Production tariff activation is unsupported.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingRule {
    pub id: String,
    pub code: String,
    pub name: String,
    pub unit: Option<String>,
    pub rate_cents: Option<i64>,
    pub minimum_cents: Option<i64>,
    pub effective_from: u64,
    /// Exclusive end: a rule ending at t and its successor starting at t may coexist.
    pub effective_until: Option<u64>,
    pub provisional: bool,
    pub classification_required: bool,
    pub blank_allowed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub rules: Vec<PricingRule>,
}

fn training_rule(
    code: &str,
    name: &str,
    unit: Option<&str>,
    rate: Option<i64>,
    minimum: Option<i64>,
) -> PricingRule {
    PricingRule {
        id: format!("training-{code}-v1"),
        code: code.into(),
        name: name.into(),
        unit: unit.map(str::to_owned),
        rate_cents: rate,
        minimum_cents: minimum,
        effective_from: 0,
        effective_until: None,
        provisional: !matches!(code, "801" | "803"),
        classification_required: code == "801",
        blank_allowed: code == "803",
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            rules: vec![
                training_rule("800", "CRT TV", Some("each"), Some(500), Some(0)),
                training_rule("801", "Flat Screen", Some("each"), Some(500), Some(0)),
                training_rule("802", "Laptops", Some("lb"), Some(40), Some(500)),
                training_rule("803", "Other E-waste", Some("lb"), Some(40), Some(500)),
                training_rule("900", "Batteries", None, None, None),
                training_rule(
                    "900-HH",
                    "Household Batteries",
                    Some("each"),
                    Some(0),
                    Some(0),
                ),
                training_rule("900-CAR", "Car Batteries", Some("each"), Some(100), Some(0)),
                training_rule("901", "Metal Oil Filters", None, None, None),
                training_rule("902", "Oil", None, None, None),
            ],
        }
    }
}

fn identifier(value: &str, label: &str, limit: usize) -> Result<(), String> {
    if value.is_empty()
        || value.len() > limit
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(format!(
            "{label} must contain 1–{limit} letters, digits, '-', '_' or '.'."
        ));
    }
    Ok(())
}

impl PricingRule {
    /// Explain unconfirmed fixture values without changing the saved calculation.
    pub fn placeholder_note(&self) -> Option<&'static str> {
        let per_line_minimum = self.minimum_cents.is_some_and(|minimum| minimum > 0);
        if self.provisional {
            Some(if per_line_minimum {
                "Placeholder unit and pricing; minimum applied per line (placeholder rule)"
            } else {
                "Placeholder unit and pricing"
            })
        } else if self.id == "training-803-v1" && self.code == "803" && per_line_minimum {
            Some("Minimum applied per line (placeholder rule)")
        } else {
            None
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        identifier(&self.id, "Rule ID", 80)?;
        identifier(&self.code, "Material code", 32)?;
        if self.name.trim().is_empty()
            || self.name.chars().count() > 120
            || self.name.chars().any(char::is_control)
        {
            return Err(
                "Enter a material name of at most 120 characters without control characters."
                    .into(),
            );
        }
        if self
            .effective_until
            .is_some_and(|until| until <= self.effective_from)
        {
            return Err("A pricing rule must end after its effective start.".into());
        }
        match (&self.unit, self.rate_cents, self.minimum_cents) {
            (None, None, None) => (),
            (Some(unit), Some(rate), minimum) if matches!(unit.as_str(), "each" | "lb") => {
                if !(0..=99_999_999).contains(&rate) || minimum.is_some_and(|cents| !(0..=99_999_999).contains(&cents)) {
                    return Err("Training rate and minimum must be nonnegative amounts within the supported limit.".into());
                }
            }
            _ => return Err("Use each or lb with a rate, or leave unit, rate and minimum empty for manual pricing.".into()),
        }
        Ok(())
    }
}

impl Catalog {
    /// Reject ambiguous schedules when loading or editing. Empty schedules and
    /// intentional date gaps are valid; checkout must handle no matching rule.
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        for (index, rule) in self.rules.iter().enumerate() {
            rule.validate()?;
            if !ids.insert(rule.id.to_ascii_lowercase()) {
                return Err("Pricing rule IDs must be unique.".into());
            }
            for other in &self.rules[..index] {
                if !rule.code.eq_ignore_ascii_case(&other.code) {
                    continue;
                }
                let rule_before_other_end = other
                    .effective_until
                    .is_none_or(|end| rule.effective_from < end);
                let other_before_rule_end = rule
                    .effective_until
                    .is_none_or(|end| other.effective_from < end);
                if rule_before_other_end && other_before_rule_end {
                    return Err(format!("Pricing rules overlap for material {}.", rule.code));
                }
            }
        }
        Ok(())
    }

    /// Select a rule from a previously validated catalog using [start, end).
    pub fn rule_for(&self, code: &str, at: u64) -> Option<&PricingRule> {
        self.rules.iter().find(|rule| {
            rule.code.eq_ignore_ascii_case(code)
                && at >= rule.effective_from
                && rule.effective_until.is_none_or(|end| at < end)
        })
    }

    /// Return a validated new schedule. Historical ticket snapshots remain owned
    /// by their ticket and are never repriced through this module.
    #[cfg(any(test, feature = "extended-workflows"))]
    pub fn save_rule(&self, rule: &PricingRule) -> Result<Self, String> {
        rule.validate()?;
        let mut rules = self.rules.clone();
        if let Some(index) = rules
            .iter()
            .position(|existing| existing.id.eq_ignore_ascii_case(&rule.id))
        {
            if rules[index].id != rule.id || rules[index].code != rule.code {
                return Err(
                    "An existing pricing rule's ID and material code cannot be changed.".into(),
                );
            }
            rules[index] = rule.clone();
        } else {
            rules.push(rule.clone());
        }
        let result = Self { rules };
        result.validate()?;
        Ok(result)
    }
}

#[cfg(test)]
#[path = "../../tests/shared/catalog_tests.rs"]
mod tests;
