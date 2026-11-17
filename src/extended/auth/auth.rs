//! Local operator authentication. Persist only password hashes; sessions are
//! private, revocable runtime capabilities and are deliberately not serializable.

pub use crate::operator_snapshot::{OperatorSnapshot, Role};
use crate::operator_snapshot::{normalize_login, normalize_name};
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const AUTH_FAILED: &str = "Sign-in failed. Check the login and password.";
const SESSION_EXPIRED: &str = "This session is no longer authorized. Sign in again.";
const HASH_INVALID: &str = "A saved password hash is invalid or uses unsupported parameters.";
const MAX_OPERATORS: usize = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Counter,
    ManageBusinessAccounts,
    #[cfg(any(test, feature = "extended-workflows"))]
    ManageTariffs,
    #[cfg(any(test, feature = "extended-workflows"))]
    ReviewIntake,
    #[cfg(any(test, feature = "extended-workflows"))]
    ExportRecords,
    #[cfg(any(test, feature = "extended-workflows"))]
    ExportContacts,
    #[cfg(any(test, feature = "extended-workflows"))]
    Closeout,
    #[cfg(any(test, feature = "extended-workflows"))]
    AdjustFinancials,
    #[cfg(feature = "extended-workflows")]
    OverridePrices,
    #[cfg(feature = "extended-workflows")]
    ManageInventory,
    #[cfg(any(test, feature = "extended-workflows"))]
    Backup,
    ManageOperators,
}

impl Role {
    fn allows(self, permission: Permission) -> bool {
        match self {
            Self::Administrator => true,
            Self::Supervisor => permission != Permission::ManageOperators,
            Self::Cashier => permission == Permission::Counter,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operator {
    pub id: u64,
    pub login: String,
    pub display_name: String,
    pub role: Role,
    pub active: bool,
    pub revision: u64,
    password_hash: String,
}

impl std::fmt::Debug for Operator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Operator")
            .field("actor", &self.snapshot())
            .field("active", &self.active)
            .field("password_hash", &"[redacted]")
            .finish()
    }
}

impl Operator {
    pub fn snapshot(&self) -> OperatorSnapshot {
        OperatorSnapshot {
            id: self.id,
            login: self.login.clone(),
            display_name: self.display_name.clone(),
            role: self.role,
            revision: self.revision,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthDirectory {
    pub operators: Vec<Operator>,
    pub next_id: u64,
    pub register_id: String,
}

impl Default for AuthDirectory {
    fn default() -> Self {
        Self {
            operators: vec![],
            next_id: 1,
            register_id: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct Session {
    actor: OperatorSnapshot,
    register_id: String,
    credential_binding: String,
    revoked: Arc<AtomicBool>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("actor", &self.actor)
            .field("revoked", &self.is_revoked())
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn actor(&self) -> &OperatorSnapshot {
        &self.actor
    }
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }
}

fn session_for(operator: &Operator, register_id: &str) -> Session {
    Session {
        actor: operator.snapshot(),
        register_id: register_id.into(),
        credential_binding: operator.password_hash.clone(),
        revoked: Arc::new(AtomicBool::new(false)),
    }
}

fn validate_password(password: &str) -> Result<(), String> {
    if password.len() > 512 || !(15..=128).contains(&password.chars().count()) {
        return Err(
            "Use a password of 15–128 characters, at most 512 UTF-8 bytes. Spaces are significant."
                .into(),
        );
    }
    Ok(())
}

fn random_bytes() -> Result<[u8; 16], String> {
    let mut bytes = [0u8; 16];
    OsRng.try_fill_bytes(&mut bytes).map_err(|_| "The operating system could not provide secure randomness. Retry before creating credentials.")?;
    Ok(bytes)
}

fn password_hasher() -> Result<Argon2<'static>, String> {
    let params =
        Params::new(19_456, 2, 1, Some(32)).map_err(|_| "Password hashing is unavailable.")?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn make_hash(password: &str) -> Result<String, String> {
    validate_password(password)?;
    let salt = random_bytes()?;
    password_hasher()?
        .hash_password_with_salt(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "Password hashing failed. The operator was not changed.".into())
}

/// Reject persisted resource costs before invoking the verifier, which otherwise
/// takes its parameters from the PHC input rather than the hasher configuration.
fn checked_hash(encoded: &str) -> Result<PasswordHash, String> {
    if encoded.len() > 256 || !encoded.is_ascii() {
        return Err(HASH_INVALID.into());
    }
    let parsed = PasswordHash::new(encoded).map_err(|_| HASH_INVALID)?;
    if parsed.algorithm.as_str() != "argon2id"
        || parsed.version != Some(19)
        || parsed.params.as_str() != "m=19456,t=2,p=1"
        || parsed.hash.as_ref().is_none_or(|hash| hash.len() != 32)
    {
        return Err(HASH_INVALID.into());
    }
    if parsed.salt.as_ref().is_none_or(|salt| salt.len() != 16) {
        return Err(HASH_INVALID.into());
    }
    Ok(parsed)
}

impl AuthDirectory {
    pub fn validate(&self) -> Result<(), String> {
        if self.next_id == 0 || self.operators.len() > MAX_OPERATORS {
            return Err("The operator directory has invalid capacity or sequencing.".into());
        }
        if self.operators.is_empty() {
            return if self.register_id.is_empty() && self.next_id == 1 {
                Ok(())
            } else {
                Err("An initialized operator directory cannot be empty. Restore verified operator records; administrator setup was not reopened.".into())
            };
        }
        let valid_register = self.register_id.len() == 32
            && self
                .register_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !valid_register {
            return Err("The operator directory has an invalid register identity.".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut logins = std::collections::HashSet::new();
        for operator in &self.operators {
            operator.snapshot().validate()?;
            if operator.id >= self.next_id
                || !ids.insert(operator.id)
                || !logins.insert(operator.login.as_str())
            {
                return Err("Operator IDs or logins are duplicated or inconsistent.".into());
            }
            checked_hash(&operator.password_hash)?;
        }
        if !self.operators.is_empty()
            && !self
                .operators
                .iter()
                .any(|operator| operator.active && operator.role == Role::Administrator)
        {
            return Err("Keep at least one active administrator.".into());
        }
        Ok(())
    }

    pub fn bootstrap(
        &self,
        login: &str,
        name: &str,
        password: &str,
    ) -> Result<(Self, Session), String> {
        self.validate()?;
        if !self.operators.is_empty() {
            return Err(
                "The register already has operators. Sign in with an existing account.".into(),
            );
        }
        let operator = Operator {
            id: self.next_id,
            login: normalize_login(login)?,
            display_name: normalize_name(name)?,
            role: Role::Administrator,
            active: true,
            revision: 1,
            password_hash: make_hash(password)?,
        };
        let register_id = random_bytes()?
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let next = Self {
            operators: vec![operator.clone()],
            next_id: self
                .next_id
                .checked_add(1)
                .ok_or("Operator ID limit reached.")?,
            register_id,
        };
        next.validate()?;
        let session = session_for(&operator, &next.register_id);
        Ok((next, session))
    }

    pub fn authenticate(&self, login: &str, password: &str) -> Result<Session, String> {
        self.validate().map_err(|_| AUTH_FAILED)?;
        let login = normalize_login(login).map_err(|_| AUTH_FAILED)?;
        validate_password(password).map_err(|_| AUTH_FAILED)?;
        let candidate = self
            .operators
            .iter()
            .find(|operator| operator.login == login);
        // A valid unknown login incurs the same bounded verifier work without a
        // dummy account or default credential. It can never receive a session.
        let verifier = candidate.or(self.operators.first()).ok_or(AUTH_FAILED)?;
        let parsed = checked_hash(&verifier.password_hash).map_err(|_| AUTH_FAILED)?;
        let verified = password_hasher()
            .map_err(|_| AUTH_FAILED)?
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        let operator = candidate.ok_or(AUTH_FAILED)?;
        if !verified || !operator.active {
            return Err(AUTH_FAILED.into());
        }
        Ok(session_for(operator, &self.register_id))
    }

    pub fn require(
        &self,
        session: &Session,
        permission: Permission,
    ) -> Result<OperatorSnapshot, String> {
        self.validate()?;
        if session.is_revoked() || session.register_id != self.register_id {
            return Err(SESSION_EXPIRED.into());
        }
        let operator = self
            .operators
            .iter()
            .find(|operator| operator.id == session.actor.id)
            .ok_or(SESSION_EXPIRED)?;
        if !operator.active
            || operator.snapshot() != session.actor
            || operator.password_hash != session.credential_binding
        {
            return Err(SESSION_EXPIRED.into());
        }
        if !operator.role.allows(permission) {
            return Err("Your operator role is not permitted to perform this action.".into());
        }
        Ok(operator.snapshot())
    }

    #[cfg(any(test, feature = "extended-workflows"))]
    pub fn can(&self, session: &Session, permission: Permission) -> bool {
        self.require(session, permission).is_ok()
    }

    #[cfg(any(test, feature = "extended-workflows"))]
    pub fn create_operator(
        &self,
        session: &Session,
        login: &str,
        name: &str,
        role: Role,
        password: &str,
    ) -> Result<(Self, OperatorSnapshot), String> {
        self.require(session, Permission::ManageOperators)?;
        let login = normalize_login(login)?;
        let display_name = normalize_name(name)?;
        if self.operators.len() >= MAX_OPERATORS
            || self
                .operators
                .iter()
                .any(|operator| operator.login == login)
        {
            return Err("That login already exists or the operator limit was reached.".into());
        }
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Operator ID limit reached.")?;
        let operator = Operator {
            id: self.next_id,
            login,
            display_name,
            role,
            active: true,
            revision: 1,
            password_hash: make_hash(password)?,
        };
        let operators = self
            .operators
            .iter()
            .cloned()
            .chain(std::iter::once(operator.clone()))
            .collect();
        let next = Self {
            operators,
            next_id,
            ..self.clone()
        };
        next.validate()?;
        Ok((next, operator.snapshot()))
    }

    #[cfg(any(test, feature = "extended-workflows"))]
    pub fn update_operator(
        &self,
        session: &Session,
        id: u64,
        name: &str,
        role: Role,
        active: bool,
    ) -> Result<(Self, OperatorSnapshot), String> {
        self.require(session, Permission::ManageOperators)?;
        let original = self
            .operators
            .iter()
            .find(|operator| operator.id == id)
            .ok_or("Operator not found.")?;
        let display_name = normalize_name(name)?;
        if original.display_name == display_name
            && original.role == role
            && original.active == active
        {
            return Ok((self.clone(), original.snapshot()));
        }
        let operator = Operator {
            display_name,
            role,
            active,
            revision: original
                .revision
                .checked_add(1)
                .ok_or("Operator revision limit reached.")?,
            ..original.clone()
        };
        self.replace_operator(operator)
    }

    #[cfg(any(test, feature = "extended-workflows"))]
    pub fn reset_password(
        &self,
        session: &Session,
        id: u64,
        password: &str,
    ) -> Result<(Self, OperatorSnapshot), String> {
        self.require(session, Permission::ManageOperators)?;
        let original = self
            .operators
            .iter()
            .find(|operator| operator.id == id)
            .ok_or("Operator not found.")?;
        let revision = original
            .revision
            .checked_add(1)
            .ok_or("Operator revision limit reached.")?;
        let operator = Operator {
            password_hash: make_hash(password)?,
            revision,
            ..original.clone()
        };
        self.replace_operator(operator)
    }

    #[cfg(any(test, feature = "extended-workflows"))]
    fn replace_operator(&self, operator: Operator) -> Result<(Self, OperatorSnapshot), String> {
        let operators = self
            .operators
            .iter()
            .map(|existing| {
                if existing.id == operator.id {
                    operator.clone()
                } else {
                    existing.clone()
                }
            })
            .collect();
        let next = Self {
            operators,
            ..self.clone()
        };
        next.validate()?;
        Ok((next, operator.snapshot()))
    }
}

#[cfg(test)]
#[path = "../../tests/extended/auth/auth_tests.rs"]
mod tests;
