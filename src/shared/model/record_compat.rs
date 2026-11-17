//! Persisted record shapes shared by the walkthrough and optional workflows.
//! These DTOs preserve stored fields; they do not run reporting or approval engines.
#[cfg(feature = "extended-workflows")]
use crate::customers::Customer;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceCategory {
    #[default]
    Unspecified,
    Residential,
    Education,
    Government,
    NonProfit,
    Business,
    Anonymous,
    OtherCollector,
}

#[cfg(not(feature = "extended-workflows"))]
impl SourceCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unspecified => "Unspecified",
            Self::Residential => "Residential",
            Self::Education => "Education",
            Self::Government => "Government",
            Self::NonProfit => "Nonprofit",
            Self::Business => "Business",
            Self::Anonymous => "Anonymous",
            Self::OtherCollector => "Other collector",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntakeReason {
    #[default]
    Unspecified,
    EmptyingGarage,
    Faulty,
    OfficeClosure,
    SmallBusiness,
    ItUpgrade,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryMode {
    #[default]
    Unspecified,
    DropOff,
    Pickup,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceIntake {
    pub category: SourceCategory,
    pub reason: IntakeReason,
    pub reason_detail: String,
    pub delivery: DeliveryMode,
    pub source_name: String,
    pub source_address: String,
    pub organization: String,
    pub contact_name: String,
    pub phone: String,
    pub circumstances: String,
    pub notes: String,
    pub linked_customer_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaterialClass {
    #[default]
    Unclassified,
    CrtDisplay,
    NonCrtDisplay,
    OtherElectronics,
    LooseBattery,
    EmbeddedBatteryProduct,
    Oil,
    OilFilter,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaterialCondition {
    #[default]
    Unspecified,
    Intact,
    Damaged,
    Leaking,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialIntake {
    pub class: MaterialClass,
    pub condition: MaterialCondition,
    pub reporting_units: Option<u32>,
    pub actual_weight_thousandths: Option<i64>,
    pub notes: String,
}

fn text(input: &str, label: &str, limit: usize) -> Result<String, String> {
    if input.chars().any(char::is_control) || input.chars().count() > limit {
        return Err(format!(
            "{label} must contain at most {limit} characters and no control characters."
        ));
    }
    Ok(input.trim().to_owned())
}

impl SourceIntake {
    pub fn normalized(&self) -> Result<Self, String> {
        if self.linked_customer_id == Some(0) {
            return Err("A linked customer must have a saved customer ID.".into());
        }
        Ok(Self {
            source_name: text(&self.source_name, "Source name", 120)?,
            source_address: text(&self.source_address, "Source address", 240)?,
            organization: text(&self.organization, "Organization", 120)?,
            contact_name: text(&self.contact_name, "Contact name", 120)?,
            phone: text(&self.phone, "Phone", 40)?,
            reason_detail: text(&self.reason_detail, "Reason detail", 240)?,
            circumstances: text(&self.circumstances, "Source circumstances", 500)?,
            notes: text(&self.notes, "Internal source notes", 1_000)?,
            ..self.clone()
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        self.normalized().map(|_| ())
    }

    /// Copy only when explicitly requested. This does not infer the source type,
    /// organization, collection purpose, delivery mode, or documentary approval.
    #[cfg(feature = "extended-workflows")]
    pub fn from_customer(customer: &Customer) -> Self {
        Self {
            source_name: customer.name.clone(),
            source_address: customer.address.clone(),
            phone: customer.phone.clone(),
            linked_customer_id: (customer.id != 0).then_some(customer.id),
            ..Default::default()
        }
    }
}

impl MaterialIntake {
    pub fn normalized(&self) -> Result<Self, String> {
        if self
            .reporting_units
            .is_some_and(|count| !(1..=999_999).contains(&count))
        {
            return Err(
                "Enter a positive reporting unit count up to 999999, or leave it unknown.".into(),
            );
        }
        if self
            .actual_weight_thousandths
            .is_some_and(|weight| !(1..=999_999_999).contains(&weight))
        {
            return Err(
                "Enter a positive actual weight up to 999999.999 lb, or leave it unknown.".into(),
            );
        }
        Ok(Self {
            notes: text(&self.notes, "Internal material notes", 1_000)?,
            ..self.clone()
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        self.normalized().map(|_| ())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceApproval {
    pub event_id: u64,
    pub approved_total_cents: i64,
}
impl PriceApproval {
    pub fn validate(&self) -> Result<(), String> {
        if self.event_id == 0 || !(0..=99_999_999).contains(&self.approved_total_cents) {
            return Err("The approved price needs a valid event ID and nonnegative amount within the supported limit.".into());
        }
        Ok(())
    }
}

/// Carry opaque fields through a core record update without interpreting them.
/// Array members with stable IDs are matched by ID; legacy members retain order.
#[cfg(not(feature = "extended-workflows"))]
pub(crate) fn preserve_unknown_fields(next: &mut serde_json::Value, previous: &serde_json::Value) {
    match (next, previous) {
        (serde_json::Value::Object(next), serde_json::Value::Object(previous)) => {
            for (key, old) in previous {
                match next.get_mut(key) {
                    Some(value) => preserve_unknown_fields(value, old),
                    None => {
                        next.insert(key.clone(), old.clone());
                    }
                }
            }
        }
        (serde_json::Value::Array(next), serde_json::Value::Array(previous)) => {
            for (index, value) in next.iter_mut().enumerate() {
                let old = match value
                    .get("id")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|id| *id > 0)
                {
                    Some(id) => previous
                        .iter()
                        .find(|old| old.get("id").and_then(serde_json::Value::as_u64) == Some(id)),
                    None => previous.get(index),
                };
                if let Some(old) = old {
                    preserve_unknown_fields(value, old);
                }
            }
        }
        _ => (),
    }
}

#[cfg(not(feature = "extended-workflows"))]
pub(crate) fn has_unknown_fields(raw: &serde_json::Value, known: &serde_json::Value) -> bool {
    match (raw, known) {
        (serde_json::Value::Object(raw), serde_json::Value::Object(known)) => {
            raw.iter().any(|(key, value)| {
                known
                    .get(key)
                    .is_none_or(|known| has_unknown_fields(value, known))
            })
        }
        (serde_json::Value::Array(raw), serde_json::Value::Array(known)) => raw
            .iter()
            .zip(known)
            .any(|(raw, known)| has_unknown_fields(raw, known)),
        _ => false,
    }
}
