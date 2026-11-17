//! Attribution stored on tickets and print requests in either build.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Cashier,
    Supervisor,
    Administrator,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorSnapshot {
    pub id: u64,
    pub login: String,
    pub display_name: String,
    pub role: Role,
    pub revision: u64,
}

impl OperatorSnapshot {
    #[cfg(not(feature = "extended-workflows"))]
    pub(crate) fn local_counter() -> Self {
        // A provenance marker in the existing v7 format. The operator directory
        // cannot allocate this ID because allocation requires a successor.
        Self {
            id: u64::MAX,
            login: "local-counter".into(),
            display_name: "Local counter (no sign-in)".into(),
            role: Role::Cashier,
            revision: 1,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0
            || self.revision == 0
            || normalize_login(&self.login)? != self.login
            || normalize_name(&self.display_name)? != self.display_name
        {
            return Err("The operator snapshot contains invalid identity metadata.".into());
        }
        Ok(())
    }
}

pub(crate) fn normalize_login(input: &str) -> Result<String, String> {
    if input.len() > 128 {
        return Err("Use a login of 1–48 ASCII letters, digits, '.', '_', '-' or '@'.".into());
    }
    let login = input.trim();
    if input.chars().any(char::is_control)
        || login.is_empty()
        || login.len() > 48
        || !login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-@".contains(&byte))
    {
        return Err("Use a login of 1–48 ASCII letters, digits, '.', '_', '-' or '@'.".into());
    }
    Ok(login.to_ascii_lowercase())
}

pub(crate) fn normalize_name(input: &str) -> Result<String, String> {
    if input.len() > 480
        || input.chars().count() > 120
        || input.trim().is_empty()
        || input.chars().any(char::is_control)
    {
        return Err("Enter a display name of 1–120 characters without control characters.".into());
    }
    Ok(input.trim().to_owned())
}
