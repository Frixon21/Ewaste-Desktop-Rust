//! Observed source-detail setters; optional documentary review is compiled separately.
use crate::domain::Ticket;
pub use crate::record_compat::{
    DeliveryMode, IntakeReason, MaterialIntake, SourceCategory, SourceIntake,
};
#[cfg(any(test, feature = "extended-workflows"))]
pub use crate::record_compat::{MaterialClass, MaterialCondition};
#[cfg(feature = "extended-workflows")]
use crate::{domain::Line, workflow::CustomerChoice};
#[cfg(feature = "extended-workflows")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "extended-workflows")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntakeIssue {
    pub code: String,
    pub ticket_id: u64,
    pub line_index: Option<usize>,
    pub message: String,
}

#[cfg(feature = "extended-workflows")]
fn issue(
    ticket: &Ticket,
    line_index: Option<usize>,
    code: &str,
    message: impl Into<String>,
) -> IntakeIssue {
    IntakeIssue {
        code: code.into(),
        ticket_id: ticket.id,
        line_index,
        message: message.into(),
    }
}

impl Ticket {
    /// Return an edited draft without changing billing, customer, tender or the
    /// caller's original ticket. Taken tender and held/final tickets stay protected.
    pub fn with_source_intake(&self, source: &SourceIntake) -> Result<Self, String> {
        self.editable_intake()?;
        Ok(Self {
            source_intake: source.normalized()?,
            ..self.clone()
        })
    }

    #[cfg(feature = "extended-workflows")]
    pub fn with_material_intake(
        &self,
        index: usize,
        material: &MaterialIntake,
    ) -> Result<Self, String> {
        self.editable_intake()?;
        if index >= self.lines.len() {
            return Err("Material line not found.".into());
        }
        let material = material.normalized()?;
        let lines = self
            .lines
            .iter()
            .enumerate()
            .map(|(position, line)| {
                if position == index {
                    Line {
                        material_intake: material.clone(),
                        ..line.clone()
                    }
                } else {
                    line.clone()
                }
            })
            .collect();
        Ok(Self {
            lines,
            ..self.clone()
        })
    }

    /// Return a training review queue. Missing actual weight is legitimate and
    /// never filled from a billed quantity, piece count, or material shortcut.
    #[cfg(feature = "extended-workflows")]
    pub fn intake_issues(&self) -> Vec<IntakeIssue> {
        let source = &self.source_intake;
        let mut issues = Vec::new();
        if let Err(message) = source.validate() {
            issues.push(issue(self, None, "source-data", message));
        }
        if source.category == SourceCategory::Unspecified {
            issues.push(issue(
                self,
                None,
                "source-category",
                "Review the source category.",
            ));
        }
        if source.delivery == DeliveryMode::Unspecified {
            issues.push(issue(self, None, "delivery", "Review pickup or drop-off."));
        }
        if source.reason == IntakeReason::Unspecified {
            issues.push(issue(
                self,
                None,
                "reason",
                "Review the reason for this intake.",
            ));
        }
        if source.reason == IntakeReason::Other && source.reason_detail.trim().is_empty() {
            issues.push(issue(
                self,
                None,
                "reason-detail",
                "Describe the other intake reason.",
            ));
        }
        if source.category == SourceCategory::Anonymous {
            if source.linked_customer_id.is_some() || !source.source_name.trim().is_empty() {
                issues.push(issue(self, None, "anonymous-identity", "The Anonymous source category includes an explicit customer link or source name. Review the category and source identity together."));
            }
            if source.circumstances.trim().is_empty() {
                issues.push(issue(
                    self,
                    None,
                    "anonymous-circumstances",
                    "Document the circumstances of the anonymous source for review.",
                ));
            }
        } else {
            if source.source_name.trim().is_empty() {
                issues.push(issue(self, None, "source-name", "Review the source name."));
            }
            if source.source_address.trim().is_empty() {
                issues.push(issue(
                    self,
                    None,
                    "source-address",
                    "Review the source address.",
                ));
            }
        }
        if matches!(
            source.category,
            SourceCategory::Business | SourceCategory::OtherCollector
        ) {
            if source.organization.trim().is_empty() {
                issues.push(issue(
                    self,
                    None,
                    "organization",
                    "Review the source organization.",
                ));
            }
            if source.contact_name.trim().is_empty() {
                issues.push(issue(
                    self,
                    None,
                    "contact-name",
                    "Review the source contact person.",
                ));
            }
        }
        self.source_link_issues(&mut issues);
        self.material_issues(&mut issues);
        issues
    }

    #[cfg(feature = "extended-workflows")]
    fn source_link_issues(&self, issues: &mut Vec<IntakeIssue>) {
        let source = &self.source_intake;
        let Some(linked) = source.linked_customer_id else {
            return;
        };
        if self.customer_choice == CustomerChoice::Blank
            || self.customer_record.as_ref().map(|customer| customer.id) != Some(linked)
        {
            issues.push(issue(self, None, "linked-customer", "The explicitly copied source refers to a different customer. Review the source snapshot; it was not overwritten."));
            return;
        }
        if let Some(customer) = &self.customer_record
            && (source.source_name.trim() != customer.name.trim()
                || source.source_address.trim() != customer.address.trim()
                || source.phone.trim() != customer.phone.trim())
        {
            issues.push(issue(self, None, "linked-customer-details", "The linked customer's name, address or phone differs from the explicitly copied source. Review the retained source details or deliberately detach the source identity."));
        }
    }

    #[cfg(feature = "extended-workflows")]
    fn material_issues(&self, issues: &mut Vec<IntakeIssue>) {
        for (index, line) in self.lines.iter().enumerate() {
            let material = &line.material_intake;
            if let Err(message) = material.validate() {
                issues.push(issue(self, Some(index), "material-data", message));
            }
            if material.class == MaterialClass::Unclassified {
                issues.push(issue(
                    self,
                    Some(index),
                    "material-class",
                    "Review the reporting material stream separately from the billed material.",
                ));
            }
            if material.condition == MaterialCondition::Unspecified {
                issues.push(issue(
                    self,
                    Some(index),
                    "condition",
                    "Review the material condition.",
                ));
            }
            if material.reporting_units.is_none() {
                issues.push(issue(self, Some(index), "reporting-units", "Review the reporting unit count separately from billing quantity and piece count."));
            }
        }
    }
}

#[cfg(all(test, feature = "extended-workflows"))]
#[path = "../../tests/extended/intake/intake_tests.rs"]
mod tests;
