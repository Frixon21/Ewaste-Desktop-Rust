use super::*;
use crate::{
    auth::{AuthDirectory, Permission, Session},
    auth_ui::{AuthAction, AuthUi},
};
pub(super) struct OperatorUi {
    pub directory: AuthDirectory,
    pub session: Option<Session>,
    pub ui: AuthUi,
    pub show_audit: bool,
    last_activity: std::time::Instant,
}
impl OperatorUi {
    pub fn load(store: &Store) -> Result<Self, String> {
        Ok(Self {
            directory: store.auth_directory()?,
            session: None,
            ui: Default::default(),
            show_audit: false,
            last_activity: std::time::Instant::now(),
        })
    }
}
impl RegisterApp {
    pub(super) fn operator_session(&self) -> Result<&Session, String> {
        self.security
            .session
            .as_ref()
            .ok_or_else(|| "Sign in to the register first.".into())
    }
    pub(super) fn can(&self, permission: Permission) -> bool {
        self.security
            .session
            .as_ref()
            .is_some_and(|s| self.security.directory.can(s, permission))
    }
    pub(super) fn lock_register(&mut self) {
        if let Some(session) = self.security.session.take() {
            let _ = self.store.sign_out(&session);
            session.revoke();
        }
        self.reset_fields();
        self.show_history = false;
        self.history_query.clear();
        self.catalog_editor.saved();
        self.closeout_editor.saved();
        self.intake.locked();
        self.financial.locked();
        self.price_editor.locked();
        self.inventory.locked();
        self.security.show_audit = false;
        self.security.ui.locked();
        self.error = None;
        self.notice = None;
    }
    fn refresh_authenticated_records(&mut self) -> Result<(), String> {
        self.store
            .require_session(self.operator_session()?, Permission::Counter)?;
        let mut tickets = self.store.list()?;
        tickets.sort_by_key(|t| t.id);
        if !tickets.iter().any(|t| t.id == self.active) {
            self.active = tickets
                .last()
                .ok_or("The register contains no saved draft.")?
                .id;
        }
        self.tickets = tickets;
        self.wf = WorkflowUi::load(&self.store)?;
        self.intake = IntakeWork::load(&self.store)?;
        self.closeouts = self.store.closeouts()?;
        self.financial = FinancialWork::load(&self.store)?;
        self.inventory = InventoryWork::load(&self.store)?;
        Ok(())
    }
    pub(super) fn handle_auth_action(&mut self, action: AuthAction) -> Result<(), String> {
        let result = match action {
            AuthAction::Bootstrap {
                login,
                display_name,
                password,
            } => {
                let created = self
                    .store
                    .bootstrap_administrator(&login, &display_name, &password);
                match created {
                    Ok(session) => {
                        session.revoke();
                        self.security.directory = self.store.auth_directory()?;
                        self.security.ui.feedback(Some(
                            "Administrator created. Sign in to open the register.".into(),
                        ));
                        return Ok(());
                    }
                    Err(error) => Err(error),
                }
            }
            AuthAction::SignIn { login, password } => match self.store.sign_in(&login, &password) {
                Ok(session) => {
                    self.security.session = Some(session);
                    self.security.directory = self.store.auth_directory()?;
                    self.security.last_activity = std::time::Instant::now();
                    self.security.ui.signed_in();
                    self.refresh_authenticated_records()
                }
                Err(error) => Err(error),
            },
            AuthAction::CreateOperator {
                login,
                display_name,
                role,
                password,
            } => self
                .store
                .create_operator_as(
                    self.operator_session()?,
                    &self.security.directory,
                    &login,
                    &display_name,
                    role,
                    &password,
                )
                .map(|next| self.security.directory = next),
            AuthAction::UpdateOperator {
                id,
                display_name,
                role,
                active,
            } => self
                .store
                .update_operator_as(
                    self.operator_session()?,
                    &self.security.directory,
                    id,
                    &display_name,
                    role,
                    active,
                )
                .map(|next| self.security.directory = next),
            AuthAction::ResetPassword { id, password } => self
                .store
                .reset_operator_password_as(
                    self.operator_session()?,
                    &self.security.directory,
                    id,
                    &password,
                )
                .map(|next| self.security.directory = next),
            AuthAction::Logout => {
                self.lock_register();
                return Ok(());
            }
        };
        match result {
            Ok(()) => {
                self.security.ui.feedback(None);
                Ok(())
            }
            Err(error) => {
                if let Ok(directory) = self.store.auth_directory() {
                    self.security.directory = directory;
                }
                self.security.ui.feedback(Some(error.clone()));
                Err(error)
            }
        }
    }
    pub(super) fn authentication_screen(&mut self, ctx: &egui::Context) -> bool {
        match self.store.auth_directory() {
            Ok(directory) => self.security.directory = directory,
            Err(error) => {
                self.lock_register();
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.colored_label(RED, error);
                });
                return false;
            }
        }
        if self.security.session.as_ref().is_some_and(|s| {
            self.security
                .directory
                .require(s, Permission::Counter)
                .is_err()
        }) {
            self.lock_register();
            self.security.ui.feedback(Some(
                "Your session changed or was revoked. Sign in again.".into(),
            ));
        }
        if self.security.session.is_some() {
            if self.security.last_activity.elapsed() >= std::time::Duration::from_secs(15 * 60) {
                self.lock_register();
                self.security.ui.feedback(Some(
                    "Register locked after inactivity. Saved drafts are preserved.".into(),
                ));
            } else if ctx.input(|i| !i.events.is_empty()) {
                self.security.last_activity = std::time::Instant::now();
            }
        }
        if self.security.session.is_none() {
            let needs_bootstrap = self.security.directory.operators.is_empty();
            if let Some(action) = self.security.ui.sign_in(ctx, needs_bootstrap) {
                let _ = self.handle_auth_action(action);
            }
            return false;
        }
        true
    }
    pub(super) fn operator_panels(&mut self, ctx: &egui::Context) {
        let action = self.security.session.as_ref().and_then(|session| {
            self.security
                .ui
                .operators(ctx, &self.security.directory, session)
        });
        if let Some(action) = action {
            let _ = self.handle_auth_action(action);
        }
        if !self.security.show_audit {
            return;
        }
        if !self.can(Permission::AdjustFinancials) {
            self.security.show_audit = false;
            return;
        }
        let result = self
            .store
            .audit_as(self.operator_session().expect("authenticated operator"));
        let prices = self.store.price_overrides();
        let mut open = true;
        egui::Window::new("Register audit history").open(&mut open).default_size([720.,500.]).show(ctx,|ui|{
            ui.label("Authenticated application history. Windows file permissions protect the underlying database; this is not a tamper-proof external audit service.");
            if let Err(error)=&prices{ui.colored_label(RED,error);}
            match result{Ok(ref ledger)=>{egui::ScrollArea::vertical().max_height(380.).show(ui,|ui|{for event in ledger.events.iter().rev(){ui.label(format!("#{} / {:?} / {} / {}",event.id,event.action,event.actor.as_ref().map(|a|a.display_name.as_str()).unwrap_or("Rejected sign-in"),event.subject));ui.small(&event.detail);
                if event.action==crate::security_audit::AuditAction::PriceOverrideRecorded
                    && let Some(id)=event.subject.strip_prefix("price-approval:").and_then(|id|id.parse::<u64>().ok())
                    && let Some(decision)=prices.as_ref().ok().and_then(|ledger|ledger.events.iter().find(|decision|decision.id==id)){
                    ui.label(format!("Original tariff {} / Previous {} / New {}",money(decision.before.basis.total_cents),money(decision.before.effective_total_cents()),money(decision.after.effective_total_cents())));ui.label(format!("Reason: {}",decision.reason));
                }
                ui.separator();}});},Err(ref error)=>{ui.colored_label(RED,error);}}
        });
        self.security.show_audit = open;
    }
    #[cfg(test)]
    pub(super) fn test_sign_in(&mut self) {
        const PASSWORD: &str = "Synthetic native test operator password";
        self.security.session = Some(
            if self.store.auth_directory().unwrap().operators.is_empty() {
                self.store
                    .bootstrap_administrator("test-admin", "Synthetic test administrator", PASSWORD)
                    .unwrap()
            } else {
                self.store.sign_in("test-admin", PASSWORD).unwrap()
            },
        );
        self.security.directory = self.store.auth_directory().unwrap();
    }
}

#[cfg(test)]
#[path = "../../tests/extended/auth/ui_auth_tests.rs"]
mod tests;
