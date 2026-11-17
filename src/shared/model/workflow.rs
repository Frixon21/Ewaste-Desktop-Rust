//! Training workflow policies, separate from payment and customer identity.
use crate::{
    customers::{BusinessAccount, Customer},
    domain::*,
    tender::CardOutcome,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CustomerChoice {
    #[default]
    Unset,
    Blank,
    Selected,
}

impl Ticket {
    pub fn editable_intake(&self) -> Result<(), String> {
        self.draft()?;
        #[cfg(not(feature = "extended-workflows"))]
        if self.lines.iter().any(|line| line.price_approval.is_some()) {
            return Err(
                "This saved draft has price approval and is read-only in the walkthrough build."
                    .into(),
            );
        }
        self.tender_evidence.guard_for_noncard()
    }
    pub fn choose_blank(&mut self) -> Result<(), String> {
        self.editable_intake()?;
        if self.lines.is_empty()
            || self.lines.iter().any(|l| {
                !l.rule_snapshot
                    .as_ref()
                    .map_or(l.code == "803", |r| r.blank_allowed)
            })
        {
            return Err("The selected material rules require a customer. Select a customer or choose an eligible Blank load.".into());
        }
        self.customer_choice = CustomerChoice::Blank;
        self.customer_record = None;
        self.customer = None;
        Ok(())
    }
    pub fn select_customer(&mut self, customer: &Customer) -> Result<(), String> {
        self.editable_intake()?;
        if customer.id == 0 || customer.number.trim().is_empty() || customer.name.trim().is_empty()
        {
            return Err("Save or select a customer record first.".into());
        }
        crate::customers::Directory {
            customers: vec![customer.clone()],
            ..Default::default()
        }
        .validate()?;
        #[cfg(not(feature = "extended-workflows"))]
        if self.source_intake.linked_customer_id.is_some() {
            self.source_intake = crate::intake::SourceIntake {
                source_name: customer.name.clone(),
                source_address: customer.address.clone(),
                phone: customer.phone.clone(),
                linked_customer_id: Some(customer.id),
                ..self.source_intake.clone()
            };
        }
        self.customer_record = Some(customer.clone());
        self.customer = Some(customer.name.clone());
        self.customer_choice = CustomerChoice::Selected;
        Ok(())
    }
    pub fn select_account(&mut self, account: &BusinessAccount) -> Result<(), String> {
        self.editable_intake()?;
        if account.id == 0
            || !account.active
            || account.number.is_empty()
            || account.name.is_empty()
        {
            return Err("Select an active saved business account.".into());
        }
        crate::customers::Directory {
            accounts: vec![account.clone()],
            ..Default::default()
        }
        .validate()?;
        self.account_record = Some(account.clone());
        Ok(())
    }
    pub fn add_weight_line(&mut self, code: &str, gross: &str, tare: &str) -> Result<(), String> {
        self.editable_intake()?;
        let material = CATALOG
            .iter()
            .find(|m| m.code == code && m.unit == Some("lb"))
            .ok_or("Select a material billed by pounds.")?;
        let gross = decimal(gross, 3, "Enter gross pounds to three decimal places.")?;
        let tare = decimal(
            if tare.trim().is_empty() { "0" } else { tare },
            3,
            "Enter tare pounds to three decimal places.",
        )?;
        if gross <= tare {
            return Err("Gross must exceed tare; net weight must be positive.".into());
        }
        self.add_line(material.code, &weight_string(gross - tare), "")?;
        let index = self.lines.len() - 1;
        self.lines[index] = Line {
            gross_thousandths: Some(gross),
            tare_thousandths: Some(tare),
            ..self.lines[index].clone()
        };
        Ok(())
    }
    pub fn add_catalog_line(
        &mut self,
        catalog: &crate::catalog::Catalog,
        code: &str,
        gross: &str,
        tare: &str,
        manual: &str,
        at: u64,
    ) -> Result<(), String> {
        self.editable_intake()?;
        let rule = catalog
            .rule_for(code, at)
            .ok_or("No tariff is effective for this material at this time.")?;
        let mut line = if rule.unit.as_deref() == Some("lb") {
            let gross = decimal(gross, 3, "Enter gross pounds to three decimal places.")?;
            let tare = decimal(
                if tare.trim().is_empty() { "0" } else { tare },
                3,
                "Enter tare pounds to three decimal places.",
            )?;
            if gross <= tare {
                return Err("Gross must exceed tare; net weight must be positive.".into());
            }
            let mut l = rule_line(rule, &weight_string(gross - tare), manual)?;
            l.gross_thousandths = Some(gross);
            l.tare_thousandths = Some(tare);
            l
        } else {
            rule_line(rule, gross, manual)?
        };
        line.rule_snapshot = Some(rule.clone());
        self.push_line(line)?;
        Ok(())
    }
    pub fn edit_weight_line(
        &mut self,
        index: usize,
        gross: &str,
        tare: &str,
    ) -> Result<(), String> {
        self.editable_intake()?;
        let old = self
            .lines
            .get(index)
            .ok_or("Material line not found.")?
            .clone();
        let mut candidate = self.clone();
        if let Some(rule) = &old.rule_snapshot {
            candidate.add_catalog_line(
                &crate::catalog::Catalog {
                    rules: vec![rule.clone()],
                },
                &old.code,
                gross,
                tare,
                "",
                rule.effective_from,
            )?;
        } else {
            candidate.add_weight_line(&old.code, gross, tare)?;
        }
        let updated = candidate.lines.pop().ok_or("Weight update failed.")?;
        let mut replacement = Line {
            id: old.id,
            rule_snapshot: old.rule_snapshot.clone(),
            material_intake: old.material_intake.clone(),
            classification: old.classification.clone(),
            piece_count: old.piece_count,
            notes: old.notes.clone(),
            ..updated
        };
        replacement.preserve_price_approval(&old)?;
        self.lines[index] = replacement;
        Ok(())
    }
    pub fn validate_intake(&self) -> Result<(), String> {
        self.validate_intake_records()?;
        #[cfg(not(feature = "extended-workflows"))]
        {
            if self.state != State::Completed
                && self.lines.iter().any(|line| line.price_approval.is_some())
            {
                return Err("This saved draft has price approval and is read-only in the walkthrough build.".into());
            }
            if self
                .lines
                .iter()
                .any(|line| matches!(line.code.as_str(), "801" | "802"))
                && self.source_intake.category == crate::intake::SourceCategory::Unspecified
            {
                return Err(
                    "Select the TV/laptop source category before payment or completion.".into(),
                );
            }
        }

        if self.lines.is_empty() || self.total().is_none() {
            return Err("Add materials and resolve every price first.".into());
        }
        match self.customer_choice {
            CustomerChoice::Unset => {
                return Err("Select a customer or explicitly choose Blank.".into());
            }
            CustomerChoice::Blank
                if self.lines.iter().any(|l| {
                    !l.rule_snapshot
                        .as_ref()
                        .map_or(l.code == "803", |r| r.blank_allowed)
                }) =>
            {
                return Err("At least one material rule requires a customer. Select a saved customer record.".into());
            }
            CustomerChoice::Selected if self.customer_record.as_ref().is_none_or(|c| c.id == 0) => {
                return Err("Select a saved customer record.".into());
            }
            _ => (),
        }
        #[cfg(feature = "extended-workflows")]
        if self.lines.iter().any(|l| {
            l.rule_snapshot
                .as_ref()
                .map_or(l.code == "801", |r| r.classification_required)
                && l.classification.trim().is_empty()
        }) {
            return Err(
                "Training policy requires classification. Edit each material line that requires intake details."
                    .into(),
            );
        }
        Ok(())
    }
    pub fn confirm_cash(&mut self, received: &str) -> Result<(), String> {
        self.draft()?;
        self.validate_intake()?;
        let received = amount(received)?.ok_or("Enter the cash received.")?;
        self.tender_evidence = self
            .tender_evidence
            .record_cash(self.total().ok_or("Resolve prices first.")?, received)?;
        Ok(())
    }
    pub fn start_card(&mut self, entered_amount: &str, reference: &str) -> Result<(), String> {
        self.draft()?;
        self.validate_intake()?;
        let entered = amount(entered_amount)?.ok_or("Enter the external card amount.")?;
        if self.total() != Some(entered) {
            return Err(
                "External amount must equal the ticket total. Check the separate processor window."
                    .into(),
            );
        }
        self.tender_evidence = self.tender_evidence.begin_card(entered, reference)?;
        Ok(())
    }
    pub fn card_result(
        &mut self,
        outcome: CardOutcome,
        verified: bool,
        reconcile: bool,
    ) -> Result<(), String> {
        self.draft()?;
        self.tender_evidence = if reconcile {
            self.tender_evidence.reconcile_card(outcome, verified)?
        } else {
            self.tender_evidence.record_card_result(outcome, verified)?
        };
        Ok(())
    }
    pub fn validate_completion(&self, payment: &Payment) -> Result<(), String> {
        self.validate_intake()?;
        let total = self.total().ok_or("Resolve prices first.")?;
        match payment {
            Payment::Cash => {
                self.tender_evidence.cash_for_total(total)?;
            }
            Payment::ExternalCard { reference } => {
                if self
                    .tender_evidence
                    .approved_card_for_total(total)?
                    .reference
                    != *reference
                {
                    return Err("Complete using the verified attempt's reference.".into());
                }
            }
            Payment::ChargeTo { account } => {
                self.tender_evidence.guard_for_noncard()?;
                if self
                    .account_record
                    .as_ref()
                    .is_none_or(|a| !a.active || a.number != *account)
                {
                    return Err("Select and confirm an active business account.".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/shared/workflow_unit_tests.rs"]
mod tests;
