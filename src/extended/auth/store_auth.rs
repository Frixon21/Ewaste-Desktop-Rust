use super::*;
#[cfg(feature = "extended-workflows")]
use crate::auth::Role;
use crate::{
    auth::{AuthDirectory, OperatorSnapshot, Permission, Session},
    security_audit::AuditAction,
};
#[derive(Serialize, Deserialize)]
enum AuthSetup {
    NeedsBootstrap,
    Ready,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct LoginThrottle {
    failures: u32,
    blocked_until: u64,
}
impl Store {
    pub(super) fn validate_security_state(&self) -> Result<(), String> {
        self.auth_directory()?;
        self.security_audit()?;
        let throttle: LoginThrottle = self.read_required("login_throttle")?;
        if throttle.failures > 5 || (throttle.failures == 5 && throttle.blocked_until == 0) {
            return Err("Saved sign-in throttle is invalid.".into());
        }
        Ok(())
    }
    pub fn auth_directory(&self) -> Result<AuthDirectory, String> {
        let directory: AuthDirectory = self.read_required("auth")?;
        directory.validate()?;
        let phase: AuthSetup = self.read_required("auth_setup")?;
        match phase {
            AuthSetup::NeedsBootstrap if directory!=AuthDirectory::default()=>return Err("Register setup state conflicts with existing operator records. Restore a verified backup.".into()),
            AuthSetup::Ready if directory.operators.is_empty()=>return Err("Initialized operator records are missing. Setup was not reopened.".into()),
            _=>(),
        }
        Ok(directory)
    }
    pub fn require_session(
        &self,
        session: &Session,
        permission: Permission,
    ) -> Result<OperatorSnapshot, String> {
        self.auth_directory()?.require(session, permission)
    }
    pub(super) fn authorized<T>(
        &self,
        session: &Session,
        permission: Permission,
        operation: impl FnOnce(OperatorSnapshot) -> Result<T, String>,
    ) -> Result<T, String> {
        self.transaction(|| {
            let actor = self.require_session(session, permission)?;
            operation(actor)
        })
    }
    pub fn bootstrap_administrator(
        &self,
        login: &str,
        name: &str,
        password: &str,
    ) -> Result<Session, String> {
        self.transaction(|| {
            let current = self.auth_directory()?;
            let (next, session) = current.bootstrap(login, name, password)?;
            self.write_state("auth", &next)?;
            self.write_state("auth_setup", &AuthSetup::Ready)?;
            self.append_audit(
                Some(session.actor()),
                AuditAction::BootstrapAdministrator,
                &format!("operator:{}", session.actor().id),
                "Created first administrator from entered credentials",
            )?;
            Ok(session)
        })
    }
    pub fn sign_in(&self, login: &str, password: &str) -> Result<Session, String> {
        self.transaction(|| {
            let directory = self.auth_directory()?;
            let mut throttle: LoginThrottle = self.read_required("login_throttle")?;
            if throttle.failures > 5 || (throttle.failures == 5 && throttle.blocked_until == 0) {
                return Err("Saved sign-in throttle is invalid.".into());
            }
            let now = crate::domain::now();
            if now < throttle.blocked_until {
                return Ok(Err(
                    "Sign-in is temporarily delayed after repeated failures. Try again shortly."
                        .into(),
                ));
            }
            if throttle.blocked_until > 0 {
                throttle = LoginThrottle::default();
            }
            match directory.authenticate(login, password) {
                Ok(session) => {
                    self.write_state("login_throttle", &LoginThrottle::default())?;
                    self.append_audit(
                        Some(session.actor()),
                        AuditAction::SignIn,
                        &format!("operator:{}", session.actor().id),
                        "Local operator signed in",
                    )?;
                    Ok(Ok(session))
                }
                Err(_) => {
                    throttle.failures += 1;
                    if throttle.failures >= 5 {
                        throttle.blocked_until = now
                            .checked_add(30)
                            .ok_or("System clock exceeds the supported range.")?;
                    }
                    self.write_state("login_throttle", &throttle)?;
                    self.append_audit(
                        None,
                        AuditAction::SignInFailed,
                        "register",
                        "Credentials rejected",
                    )?;
                    Ok(Err("Sign-in failed. Check the login and password.".into()))
                }
            }
        })?
    }
    pub fn sign_out(&self, session: &Session) -> Result<(), String> {
        let result = self.authorized(session, Permission::Counter, |actor| {
            self.append_audit(
                Some(&actor),
                AuditAction::SignOut,
                &format!("operator:{}", actor.id),
                "Local operator signed out",
            )
        });
        session.revoke();
        result
    }
    #[cfg(feature = "extended-workflows")]
    fn change_operators(
        &self,
        session: &Session,
        expected: &AuthDirectory,
        action: AuditAction,
        operation: impl FnOnce(&AuthDirectory) -> Result<(AuthDirectory, OperatorSnapshot), String>,
    ) -> Result<AuthDirectory, String> {
        self.authorized(session, Permission::ManageOperators, |actor| {
            let current = self.auth_directory()?;
            if &current != expected {
                return Err("Operator records changed. Reload before saving.".into());
            }
            let (next, changed) = operation(&current)?;
            next.validate()?;
            if next != current {
                self.write_state("auth", &next)?;
                self.append_audit(
                    Some(&actor),
                    action,
                    &format!("operator:{}", changed.id),
                    "Authorized operator account change",
                )?;
            }
            Ok(next)
        })
    }
    #[cfg(feature = "extended-workflows")]
    pub fn create_operator_as(
        &self,
        session: &Session,
        expected: &AuthDirectory,
        login: &str,
        name: &str,
        role: Role,
        password: &str,
    ) -> Result<AuthDirectory, String> {
        self.change_operators(session, expected, AuditAction::OperatorCreated, |current| {
            current.create_operator(session, login, name, role, password)
        })
    }
    #[cfg(feature = "extended-workflows")]
    pub fn update_operator_as(
        &self,
        session: &Session,
        expected: &AuthDirectory,
        id: u64,
        name: &str,
        role: Role,
        active: bool,
    ) -> Result<AuthDirectory, String> {
        self.change_operators(session, expected, AuditAction::OperatorUpdated, |current| {
            current.update_operator(session, id, name, role, active)
        })
    }
    #[cfg(feature = "extended-workflows")]
    pub fn reset_operator_password_as(
        &self,
        session: &Session,
        expected: &AuthDirectory,
        id: u64,
        password: &str,
    ) -> Result<AuthDirectory, String> {
        self.change_operators(session, expected, AuditAction::PasswordReset, |current| {
            current.reset_password(session, id, password)
        })
    }
}
