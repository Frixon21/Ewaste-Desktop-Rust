//! Native credential forms. This module never authenticates, persists, or logs
//! passwords. Actions move credentials to the application service once.
#[cfg(feature = "extended-workflows")]
use crate::auth::{AuthDirectory, Operator, OperatorSnapshot, Permission, Role, Session};
use eframe::egui;

pub enum AuthAction {
    Bootstrap {
        login: String,
        display_name: String,
        password: String,
    },
    SignIn {
        login: String,
        password: String,
    },
    #[cfg(feature = "extended-workflows")]
    CreateOperator {
        login: String,
        display_name: String,
        role: Role,
        password: String,
    },
    #[cfg(feature = "extended-workflows")]
    UpdateOperator {
        id: u64,
        display_name: String,
        role: Role,
        active: bool,
    },
    #[cfg(feature = "extended-workflows")]
    ResetPassword {
        id: u64,
        password: String,
    },
    Logout,
}
#[derive(Default)]
pub struct AuthUi {
    #[cfg(feature = "extended-workflows")]
    pub show_operators: bool,
    login: String,
    display_name: String,
    password: String,
    confirmation: String,
    message: Option<String>,
    #[cfg(feature = "extended-workflows")]
    form: Option<OperatorForm>,
}
#[cfg(feature = "extended-workflows")]
struct OperatorForm {
    original: Option<OperatorSnapshot>,
    original_active: bool,
    login: String,
    display_name: String,
    role: Role,
    active: bool,
    reset_mode: bool,
    password: String,
    confirmation: String,
}
#[cfg(feature = "extended-workflows")]
impl OperatorForm {
    fn new() -> Self {
        Self {
            original: None,
            original_active: true,
            login: String::new(),
            display_name: String::new(),
            role: Role::Cashier,
            active: true,
            reset_mode: false,
            password: String::new(),
            confirmation: String::new(),
        }
    }
    fn existing(operator: &Operator) -> Self {
        Self {
            original: Some(operator.snapshot()),
            original_active: operator.active,
            login: operator.login.clone(),
            display_name: operator.display_name.clone(),
            role: operator.role,
            active: operator.active,
            reset_mode: false,
            password: String::new(),
            confirmation: String::new(),
        }
    }
    fn stale(&self, directory: &AuthDirectory) -> bool {
        self.original.as_ref().is_some_and(|original| {
            directory
                .operators
                .iter()
                .find(|operator| operator.id == original.id)
                .is_none_or(|operator| {
                    operator.snapshot() != *original || operator.active != self.original_active
                })
        })
    }
    fn clear_passwords(&mut self) {
        self.password.clear();
        self.confirmation.clear();
    }
}
fn confirmed_password(password: &str, confirmation: &str) -> Result<(), String> {
    if password != confirmation {
        return Err("The passwords do not match. Enter both again.".into());
    }
    if !(15..=128).contains(&password.chars().count()) || password.len() > 512 {
        return Err("Use a password of 15 to 128 characters. Spaces are allowed.".into());
    }
    Ok(())
}
#[cfg(feature = "extended-workflows")]
fn role_label(role: Role) -> &'static str {
    match role {
        Role::Cashier => "Cashier",
        Role::Supervisor => "Supervisor",
        Role::Administrator => "Administrator",
    }
}
fn edit_text(ui: &mut egui::Ui, label: &str, value: &mut String, limit: usize) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .char_limit(limit)
            .desired_width(390.),
    );
}
fn password_fields(
    ui: &mut egui::Ui,
    password: &mut String,
    confirmation: &mut String,
    confirm: bool,
) -> bool {
    ui.label("Password");
    let password_edit = ui.add(
        egui::TextEdit::singleline(password)
            .password(true)
            .char_limit(128)
            .desired_width(390.),
    );
    let mut submitted = !confirm
        && password_edit.lost_focus()
        && ui.input(|input| input.key_pressed(egui::Key::Enter));
    if confirm {
        ui.label("Confirm password");
        let confirmation_edit = ui.add(
            egui::TextEdit::singleline(confirmation)
                .password(true)
                .char_limit(128)
                .desired_width(390.),
        );
        submitted =
            confirmation_edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        ui.small("15 to 128 characters. Spaces are allowed.");
    }
    submitted
}
impl AuthUi {
    fn clear_passwords(&mut self) {
        self.password.clear();
        self.confirmation.clear();
        #[cfg(feature = "extended-workflows")]
        if let Some(form) = &mut self.form {
            form.clear_passwords();
        }
    }
    /// Service feedback never includes credentials. Failures retain entered
    /// account metadata, while all password fields require fresh entry.
    pub fn feedback(&mut self, message: Option<String>) {
        self.clear_passwords();
        #[cfg(feature = "extended-workflows")]
        if message.is_none() {
            self.form = None;
        }
        self.message = message;
    }
    pub fn signed_in(&mut self) {
        self.clear_passwords();
        self.login.clear();
        self.display_name.clear();
        #[cfg(feature = "extended-workflows")]
        {
            self.form = None;
            self.show_operators = false;
        }
        self.message = None;
    }
    pub fn locked(&mut self) {
        self.signed_in();
    }
    fn sign_in_action(&mut self, bootstrap: bool) -> Result<AuthAction, String> {
        if bootstrap {
            confirmed_password(&self.password, &self.confirmation)?;
            if self.login.trim().is_empty() || self.display_name.trim().is_empty() {
                return Err("Enter the administrator login and display name.".into());
            }
            self.confirmation.clear();
            Ok(AuthAction::Bootstrap {
                login: self.login.clone(),
                display_name: self.display_name.clone(),
                password: std::mem::take(&mut self.password),
            })
        } else {
            if self.login.trim().is_empty() || self.password.is_empty() {
                return Err("Enter your operator login and password.".into());
            }
            self.confirmation.clear();
            Ok(AuthAction::SignIn {
                login: self.login.clone(),
                password: std::mem::take(&mut self.password),
            })
        }
    }
    /// Render this instead of the register while no authenticated session exists.
    pub fn sign_in(&mut self, ctx: &egui::Context, needs_bootstrap: bool) -> Option<AuthAction> {
        let mut submit = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(if needs_bootstrap {
                "Set up this register"
            } else {
                "Operator sign-in"
            });
            if needs_bootstrap {
                ui.label(
                    "Create the first local register account. No default credentials are provided.",
                );
            } else {
                ui.label("Sign in with your local register account to open the counter.");
            }
            if let Some(message) = &self.message {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
            ui.add_space(10.);
            egui::ScrollArea::vertical()
                .max_height((ui.available_height() - 65.).clamp(32., 400.))
                .show(ui, |ui| {
                    edit_text(ui, "Operator login", &mut self.login, 48);
                    if needs_bootstrap {
                        edit_text(ui, "Display name", &mut self.display_name, 120);
                    }
                    submit = password_fields(
                        ui,
                        &mut self.password,
                        &mut self.confirmation,
                        needs_bootstrap,
                    );
                });
            ui.add_space(8.);
            submit |= ui
                .button(if needs_bootstrap {
                    "Create first administrator"
                } else {
                    "Sign in"
                })
                .clicked();
        });
        if !submit {
            return None;
        }
        match self.sign_in_action(needs_bootstrap) {
            Ok(action) => {
                self.message = None;
                Some(action)
            }
            Err(message) => {
                self.clear_passwords();
                self.message = Some(message);
                None
            }
        }
    }
    #[cfg(feature = "extended-workflows")]
    fn operator_action(
        &mut self,
        directory: &AuthDirectory,
        session: &Session,
    ) -> Result<AuthAction, String> {
        directory.require(session, Permission::ManageOperators)?;
        let form = self
            .form
            .as_mut()
            .ok_or("Select an operator or create a new one.")?;
        if form.stale(directory) {
            return Err(
                "This operator changed. Select the saved operator again before editing.".into(),
            );
        }
        if let Some(original) = &form.original {
            if form.reset_mode {
                confirmed_password(&form.password, &form.confirmation)?;
                form.confirmation.clear();
                return Ok(AuthAction::ResetPassword {
                    id: original.id,
                    password: std::mem::take(&mut form.password),
                });
            }
            Ok(AuthAction::UpdateOperator {
                id: original.id,
                display_name: form.display_name.clone(),
                role: form.role,
                active: form.active,
            })
        } else {
            confirmed_password(&form.password, &form.confirmation)?;
            if form.login.trim().is_empty() || form.display_name.trim().is_empty() {
                return Err("Enter an operator login and display name.".into());
            }
            form.confirmation.clear();
            Ok(AuthAction::CreateOperator {
                login: form.login.clone(),
                display_name: form.display_name.clone(),
                role: form.role,
                password: std::mem::take(&mut form.password),
            })
        }
    }
    /// Authorization is checked before even exposing operator metadata. The
    /// application service repeats the permission/revision check when saving.
    #[cfg(feature = "extended-workflows")]
    pub fn operators(
        &mut self,
        ctx: &egui::Context,
        directory: &AuthDirectory,
        session: &Session,
    ) -> Option<AuthAction> {
        if !self.show_operators {
            self.form = None;
            return None;
        }
        if directory
            .require(session, Permission::ManageOperators)
            .is_err()
        {
            self.show_operators = false;
            self.form = None;
            self.clear_passwords();
            return None;
        }
        let mut open = true;
        let mut selection = None;
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new("Operator accounts").open(&mut open).default_size([650.,520.]).min_width(480.).max_height((ctx.content_rect().height()-40.).max(120.)).show(ctx,|ui|{
            ui.label("Only administrators manage local operator access. Changes revoke affected sign-in sessions.");
            if let Some(message)=&self.message{ui.colored_label(ui.visuals().error_fg_color,message);}
            ui.horizontal_wrapped(|ui|{
                let selected=self.form.as_ref().map(|form|if form.original.is_some(){form.login.as_str()}else{"New operator"}).unwrap_or("Select operator");
                egui::ComboBox::from_id_salt("operator-account-picker").selected_text(selected).width(310.).show_ui(ui,|ui|{
                    for operator in &directory.operators{
                        let caption=format!("{} / {} / {}{}",operator.login,operator.display_name,role_label(operator.role),if operator.active{""}else{" (inactive)"});
                        if ui.selectable_label(self.form.as_ref().is_some_and(|form|form.original.as_ref().is_some_and(|original|original.id==operator.id)),caption).clicked(){selection=Some(OperatorForm::existing(operator));}
                    }
                });
                if ui.button("New operator").clicked(){selection=Some(OperatorForm::new());}
            });
            if let Some(form)=&mut self.form{
                let stale=form.stale(directory);
                if stale{ui.colored_label(ui.visuals().error_fg_color,"This operator changed. Select the saved operator again to reload before saving.");}
                let body=(ui.available_height()-50.).clamp(32.,360.);
                egui::ScrollArea::vertical().max_height(body).show(ui,|ui|{ui.add_enabled_ui(!stale,|ui|edit_operator(ui,form));});
                ui.horizontal_wrapped(|ui|{
                    submit=ui.add_enabled(!stale,egui::Button::new(if form.original.is_none(){"Create operator"}else if form.reset_mode{"Reset operator password"}else{"Save operator access"})).clicked();
                    cancel=ui.button("Cancel operator changes").clicked();
                });
            }else{ui.label("Select an existing operator or create a new account.");}
        });
        self.show_operators = open;
        if !open || cancel {
            self.form = None;
            self.clear_passwords();
            return None;
        }
        if let Some(form) = selection {
            self.form = Some(form);
            self.message = None;
        }
        if !submit {
            return None;
        }
        match self.operator_action(directory, session) {
            Ok(action) => {
                self.message = None;
                Some(action)
            }
            Err(message) => {
                self.clear_passwords();
                self.message = Some(message);
                None
            }
        }
    }
}
#[cfg(feature = "extended-workflows")]
fn edit_operator(ui: &mut egui::Ui, form: &mut OperatorForm) {
    if form.original.is_some() {
        ui.label(format!("Login: {}", form.login));
    } else {
        edit_text(ui, "Operator login", &mut form.login, 48);
    }
    edit_text(ui, "Display name", &mut form.display_name, 120);
    egui::ComboBox::from_id_salt("operator-role")
        .selected_text(role_label(form.role))
        .show_ui(ui, |ui| {
            for role in [Role::Cashier, Role::Supervisor, Role::Administrator] {
                ui.selectable_value(&mut form.role, role, role_label(role));
            }
        });
    if form.original.is_some() {
        ui.checkbox(&mut form.active, "Active operator");
        let resetting = form.reset_mode;
        ui.checkbox(&mut form.reset_mode, "Reset this operator's password");
        if resetting && !form.reset_mode {
            form.clear_passwords();
        }
    }
    if form.original.is_none() || form.reset_mode {
        password_fields(ui, &mut form.password, &mut form.confirmation, true);
    }
    if form.reset_mode {
        ui.small("Password reset changes credentials only. Save access or display-name changes separately.");
    }
}

#[cfg(test)]
#[path = "../../tests/extended/auth/auth_ui_tests.rs"]
mod tests;
