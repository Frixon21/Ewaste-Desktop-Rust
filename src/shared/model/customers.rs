//! Reusable counter identities. Approved scanner fields produce a review, never a write.
//! No credential number, raw barcode, image, or birth date enters this model.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customer {
    pub id: u64,
    pub number: String,
    pub name: String,
    pub phone: String,
    pub address: String,
    pub notice: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusinessAccount {
    pub id: u64,
    pub number: String,
    pub name: String,
    pub active: bool,
}

fn first_id() -> u64 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directory {
    #[serde(default)]
    pub customers: Vec<Customer>,
    #[serde(default)]
    pub accounts: Vec<BusinessAccount>,
    #[serde(default = "first_id")]
    pub next_customer_id: u64,
    #[serde(default = "first_id")]
    pub next_account_id: u64,
}

impl Default for Directory {
    fn default() -> Self {
        Self {
            customers: vec![],
            accounts: vec![],
            next_customer_id: 1,
            next_account_id: 1,
        }
    }
}

/// Synthetic inputs retained for the optional extended UI and tests.
#[cfg(any(test, feature = "extended-workflows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanFixture {
    Success,
    Duplicate,
    Unreadable,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReview {
    pub customer: Customer,
    /// Candidate IDs are suggestions. The caller must select one or create explicitly.
    pub matches: Vec<u64>,
    pub prompt: String,
}

fn field(input: &str, label: &str, limit: usize, required: bool) -> Result<String, String> {
    if input.chars().any(char::is_control) || input.chars().count() > limit {
        return Err(format!(
            "{label} must contain at most {limit} characters and no control characters."
        ));
    }
    let cleaned = input.trim();
    if required && cleaned.is_empty() {
        return Err(format!("Enter {label}."));
    }
    Ok(cleaned.to_owned())
}

fn number(input: &str) -> Result<String, String> {
    let value = field(input, "record number", 32, false)?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("Record numbers use letters, digits, hyphens or underscores.".into());
    }
    Ok(value)
}

fn phone_digits(input: &str) -> String {
    input.chars().filter(char::is_ascii_digit).collect()
}

#[cfg(not(feature = "extended-workflows"))]
fn scan_match_parts(input: &str) -> Vec<String> {
    input
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(not(feature = "extended-workflows"))]
fn scan_name_parts(input: &str) -> Vec<String> {
    let mut parts = scan_match_parts(input);
    parts.sort_unstable();
    parts
}

impl Directory {
    /// Suggestions never establish identity or update an existing record. Only
    /// approved name/address fields enter a new, unsaved customer review.
    #[cfg(not(feature = "extended-workflows"))]
    pub fn review_scan(
        &self,
        candidate: &crate::scanner::ScanCandidate,
    ) -> Result<ScanReview, String> {
        candidate.validate().map_err(|error| error.to_string())?;
        self.validate()?;
        let customer = Customer {
            name: field(&candidate.name, "customer name", 120, true)?,
            address: field(&candidate.address, "address", 240, true)?,
            ..Default::default()
        };
        let name = scan_name_parts(&customer.name);
        let address = scan_match_parts(&customer.address);
        if name.is_empty() || address.is_empty() {
            return Err("Review the scanned name and address before continuing.".into());
        }
        let mut matches: Vec<u64> = self
            .customers
            .iter()
            .filter(|existing| {
                scan_name_parts(&existing.name) == name
                    || scan_match_parts(&existing.address) == address
            })
            .map(|existing| existing.id)
            .collect();
        matches.sort_unstable();
        let prompt=if matches.is_empty(){"Review the scanned name and address, then explicitly create a customer or cancel."}else{"Possible existing customers. Review the names, addresses and phone numbers, then explicitly select an existing record or create another."}.into();
        Ok(ScanReview {
            customer,
            matches,
            prompt,
        })
    }
    /// Validate restored records before exposing them to the counter. Customer and
    /// business sequences are independent; duplicate names are intentionally valid.
    pub fn validate(&self) -> Result<(), String> {
        let mut customer_ids = std::collections::HashSet::new();
        let mut customer_numbers = std::collections::HashSet::new();
        for customer in &self.customers {
            if customer.id == 0 || !customer_ids.insert(customer.id) {
                return Err("Customer directory contains an invalid or duplicate ID.".into());
            }
            let record_number = number(&customer.number)?;
            if record_number.is_empty() || !customer_numbers.insert(record_number.to_lowercase()) {
                return Err(
                    "Customer directory contains an empty or duplicate record number.".into(),
                );
            }
            field(&customer.name, "customer name", 120, true)?;
            field(&customer.phone, "phone", 40, false)?;
            field(&customer.address, "address", 240, false)?;
            field(&customer.notice, "account notice", 500, false)?;
        }
        let mut account_ids = std::collections::HashSet::new();
        let mut account_numbers = std::collections::HashSet::new();
        for account in &self.accounts {
            if account.id == 0 || !account_ids.insert(account.id) {
                return Err("Business directory contains an invalid or duplicate ID.".into());
            }
            let record_number = number(&account.number)?;
            if record_number.is_empty() || !account_numbers.insert(record_number.to_lowercase()) {
                return Err(
                    "Business directory contains an empty or duplicate account number.".into(),
                );
            }
            field(&account.name, "business name", 120, true)?;
        }
        Ok(())
    }

    /// id=0 creates a record. Existing IDs update that record and retain its number.
    /// Returns a new directory so persistence can succeed before replacing UI state.
    pub fn save_customer(&self, input: &Customer) -> Result<(Self, Customer), String> {
        let mut customer = Customer {
            id: input.id,
            number: number(&input.number)?,
            name: field(&input.name, "customer name", 120, true)?,
            phone: field(&input.phone, "phone", 40, false)?,
            address: field(&input.address, "address", 240, false)?,
            notice: field(&input.notice, "account notice", 500, false)?,
        };
        let mut result = self.clone();
        if customer.id == 0 {
            let mut id = self.next_customer_id.max(1);
            while self
                .customers
                .iter()
                .any(|c| c.id == id || c.number.eq_ignore_ascii_case(&format!("C{id:06}")))
            {
                id = id.checked_add(1).ok_or("Customer ID limit reached.")?;
            }
            result.next_customer_id = id.checked_add(1).ok_or("Customer ID limit reached.")?;
            customer.id = id;
            if customer.number.is_empty() {
                customer.number = format!("C{id:06}");
            }
        } else {
            let original = self
                .customers
                .iter()
                .find(|c| c.id == customer.id)
                .ok_or("Customer not found. Search again before saving.")?;
            if customer.number != original.number {
                return Err("A customer's stable record number cannot be changed.".into());
            }
        }
        if self
            .customers
            .iter()
            .any(|c| c.id != customer.id && c.number.eq_ignore_ascii_case(&customer.number))
        {
            return Err("That customer number already exists. Select the existing record.".into());
        }
        if let Some(index) = result.customers.iter().position(|c| c.id == customer.id) {
            result.customers[index] = customer.clone();
        } else {
            result.customers.push(customer.clone());
        }
        Ok((result, customer))
    }

    /// Business identity is separate from the person delivering material.
    /// Inactive accounts remain searchable to explain why they cannot be charged.
    pub fn save_account(&self, input: &BusinessAccount) -> Result<(Self, BusinessAccount), String> {
        let mut account = BusinessAccount {
            id: input.id,
            number: number(&input.number)?,
            name: field(&input.name, "business name", 120, true)?,
            active: input.active,
        };
        let mut result = self.clone();
        if account.id == 0 {
            let mut id = self.next_account_id.max(1);
            while self
                .accounts
                .iter()
                .any(|a| a.id == id || a.number.eq_ignore_ascii_case(&format!("A{id:06}")))
            {
                id = id.checked_add(1).ok_or("Account ID limit reached.")?;
            }
            result.next_account_id = id.checked_add(1).ok_or("Account ID limit reached.")?;
            account.id = id;
            if account.number.is_empty() {
                account.number = format!("A{id:06}");
            }
        } else {
            let original = self
                .accounts
                .iter()
                .find(|a| a.id == account.id)
                .ok_or("Business account not found. Search again before saving.")?;
            if account.number != original.number {
                return Err("A business account's stable number cannot be changed.".into());
            }
        }
        if self
            .accounts
            .iter()
            .any(|a| a.id != account.id && a.number.eq_ignore_ascii_case(&account.number))
        {
            return Err(
                "That business account number already exists. Select the existing record.".into(),
            );
        }
        if let Some(index) = result.accounts.iter().position(|a| a.id == account.id) {
            result.accounts[index] = account.clone();
        } else {
            result.accounts.push(account.clone());
        }
        Ok((result, account))
    }

    /// Empty searches return no matches; selection is always a deliberate UI action.
    pub fn search_customers(&self, query: &str) -> Vec<&Customer> {
        let query = query.trim().to_lowercase();
        if query.is_empty() || query.chars().count() > 240 {
            return vec![];
        }
        let digits = phone_digits(&query);
        self.customers
            .iter()
            .filter(|customer| {
                [
                    &customer.number,
                    &customer.name,
                    &customer.phone,
                    &customer.address,
                ]
                .iter()
                .any(|value| value.to_lowercase().contains(&query))
                    || (!digits.is_empty()
                        && query
                            .chars()
                            .all(|c| c.is_ascii_digit() || " +()-./".contains(c))
                        && phone_digits(&customer.phone).contains(&digits))
            })
            .collect()
    }

    pub fn search_accounts(&self, query: &str) -> Vec<&BusinessAccount> {
        let query = query.trim().to_lowercase();
        if query.is_empty() || query.chars().count() > 120 {
            return vec![];
        }
        self.accounts
            .iter()
            .filter(|account| {
                account.number.to_lowercase().contains(&query)
                    || account.name.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// Repeating the success fixture exercises duplicate detection without retaining
    /// raw scanner input. Failures return no partial identity and never mutate data.
    #[cfg(any(test, feature = "extended-workflows"))]
    pub fn scan_fixture(&self, fixture: ScanFixture) -> Result<ScanReview, String> {
        match fixture {
            ScanFixture::Unreadable => return Err("Synthetic scan unreadable. Retry or enter the customer manually; the draft is preserved.".into()),
            ScanFixture::Disconnected => return Err("Synthetic scanner disconnected. Enter the customer manually; the draft is preserved.".into()),
            ScanFixture::Success | ScanFixture::Duplicate => (),
        }
        let customer = Customer {
            name: "Synthetic Customer".into(),
            phone: "555-0100".into(),
            address: "100 Example Street, Example City, CA".into(),
            ..Default::default()
        };
        let matches: Vec<u64> = self
            .customers
            .iter()
            .filter(|existing| {
                existing.name.eq_ignore_ascii_case(&customer.name)
                    || (!existing.phone.is_empty()
                        && phone_digits(&existing.phone) == phone_digits(&customer.phone))
            })
            .map(|existing| existing.id)
            .collect();
        let prompt = if matches.is_empty() { "Review the synthetic customer fields, then explicitly create or cancel." } else { "Possible existing customer. Review the candidates and explicitly select an existing record or create another." }.into();
        Ok(ScanReview {
            customer,
            matches,
            prompt,
        })
    }
}

#[cfg(all(test, not(feature = "extended-workflows")))]
#[path = "../../tests/counter/customer_scan_tests.rs"]
mod scan_tests;

#[cfg(test)]
#[path = "../../tests/shared/customers_tests.rs"]
mod tests;
