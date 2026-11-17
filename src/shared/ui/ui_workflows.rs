use super::*;
#[cfg(feature = "extended-workflows")]
use crate::customers::ScanFixture;
use crate::{
    customers::{BusinessAccount, Customer, Directory},
    printing::{CopyKind, PrintLog, PrintState},
    tender::CardOutcome,
};

pub(super) enum WorkflowAct {
    SelectCustomer(u64),
    SaveCustomer,
    Blank,
    #[cfg(feature = "extended-workflows")]
    Scan(ScanFixture),
    SaveAccount,
    SelectAccount(u64),
    Cash,
    BeginCard,
    CardResult(bool),
    Delete(usize),
    #[cfg(feature = "extended-workflows")]
    RetryPrint(u64),
}
pub(super) struct WorkflowUi {
    pub directory: Directory,
    pub catalog: crate::catalog::Catalog,
    pub prints: PrintLog,
    pub pending_remove: Option<usize>,
    pub(super) customer_edit: Option<Customer>,
    pub(super) account_edit: Option<BusinessAccount>,
    pub(super) scan_notice: String,
    pub(super) scan_matches: Vec<u64>,
    pub(super) cash: String,
    pub(super) card_amount: String,
    pub(super) outcome: CardOutcome,
    pub(super) verified: bool,
    #[cfg(feature = "extended-workflows")]
    fail_print: bool,
}
impl WorkflowUi {
    pub fn load(store: &Store) -> Result<Self, String> {
        Ok(Self {
            directory: store.directory()?,
            catalog: store.catalog()?,
            prints: store.print_log()?,
            pending_remove: None,
            customer_edit: None,
            account_edit: None,
            scan_notice: String::new(),
            scan_matches: vec![],
            cash: String::new(),
            card_amount: String::new(),
            outcome: CardOutcome::Approved,
            verified: false,
            #[cfg(feature = "extended-workflows")]
            fail_print: false,
        })
    }
    pub fn reset(&mut self) {
        self.customer_edit = None;
        self.account_edit = None;
        self.pending_remove = None;
        self.cash.clear();
        self.card_amount.clear();
        self.scan_notice.clear();
        self.scan_matches.clear();
        self.verified = false;
    }
    pub fn modal_open(&self) -> bool {
        self.customer_edit.is_some() || self.account_edit.is_some() || self.pending_remove.is_some()
    }
    fn printer_fails(&self) -> bool {
        #[cfg(feature = "extended-workflows")]
        {
            self.fail_print
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            false
        }
    }
}
impl RegisterApp {
    fn persist_directory(&mut self, next: Directory) -> Result<(), String> {
        #[cfg(feature = "extended-workflows")]
        self.store
            .save_directory_as(self.operator_session()?, &self.wf.directory, &next)?;
        #[cfg(not(feature = "extended-workflows"))]
        self.store
            .save_counter_directory(&self.wf.directory, &next)?;
        self.wf.directory = next;
        Ok(())
    }

    pub(super) fn run_workflow(&mut self, action: WorkflowAct) -> Result<(), String> {
        #[cfg(not(feature = "extended-workflows"))]
        if self.scanner.active() {
            return Err("Review or cancel the active scanner capture before using customer or payment actions.".into());
        }
        #[cfg(feature = "extended-workflows")]
        self.store
            .require_session(self.operator_session()?, Permission::Counter)?;
        match action {
            WorkflowAct::SelectCustomer(id) => {
                let customer = self
                    .wf
                    .directory
                    .customers
                    .iter()
                    .find(|c| c.id == id)
                    .ok_or("Customer not found.")?
                    .clone();
                self.save_action(|t| t.select_customer(&customer))?;
                self.query.clear();
                self.wf.customer_edit = None;
                self.wf.scan_notice.clear();
            }
            WorkflowAct::SaveCustomer => {
                self.active().editable_intake()?;
                let input = self
                    .wf
                    .customer_edit
                    .as_ref()
                    .ok_or("Open customer editor first.")?;
                let (next, customer) = self.wf.directory.save_customer(input)?;
                self.persist_directory(next)?;
                self.wf.customer_edit = Some(customer.clone());
                self.save_action(|t| t.select_customer(&customer))?;
                self.wf.customer_edit = None;
                self.wf.scan_notice.clear();
                self.query.clear();
            }
            WorkflowAct::Blank => self.save_action(|t| t.choose_blank())?,
            #[cfg(feature = "extended-workflows")]
            WorkflowAct::Scan(fixture) => {
                self.active().editable_intake()?;
                let review = self.wf.directory.scan_fixture(fixture)?;
                self.wf.scan_notice = review.prompt;
                self.wf.scan_matches = review.matches;
                self.query = review.customer.name.clone();
                self.wf.customer_edit = Some(review.customer);
            }
            WorkflowAct::SaveAccount => {
                self.active().editable_intake()?;
                let input = self
                    .wf
                    .account_edit
                    .as_ref()
                    .ok_or("Open account editor first.")?;
                let (next, account) = self.wf.directory.save_account(input)?;
                self.persist_directory(next)?;
                self.wf.account_edit = Some(account.clone());
                self.save_action(|t| t.select_account(&account))?;
                self.wf.account_edit = None;
                self.account.clear();
            }
            WorkflowAct::SelectAccount(id) => {
                let account = self
                    .wf
                    .directory
                    .accounts
                    .iter()
                    .find(|a| a.id == id)
                    .ok_or("Account not found.")?
                    .clone();
                self.save_action(|t| t.select_account(&account))?;
                self.account.clear();
            }
            WorkflowAct::Cash => {
                #[cfg(not(feature = "extended-workflows"))]
                self.ensure_source_choice()?;
                let amount = self.wf.cash.clone();
                self.save_action(|t| t.confirm_cash(&amount))?;
            }
            WorkflowAct::BeginCard => {
                #[cfg(not(feature = "extended-workflows"))]
                self.ensure_source_choice()?;
                let amount = self.wf.card_amount.clone();
                let reference = self.reference.clone();
                self.save_action(|t| t.start_card(&amount, &reference))?;
                self.wf.verified = false;
            }
            WorkflowAct::CardResult(reconcile) => {
                let outcome = self.wf.outcome;
                let verified = self.wf.verified;
                self.save_action(|t| t.card_result(outcome, verified, reconcile))?;
                self.wf.verified = false;
            }
            WorkflowAct::Delete(index) => {
                self.save_action(|t| t.remove_line(index))?;
                self.wf.pending_remove = None;
                self.edit = None;
            }
            #[cfg(feature = "extended-workflows")]
            WorkflowAct::RetryPrint(id) => {
                let (next, job) = self.wf.prints.retry(id, self.wf.printer_fails())?;
                self.store
                    .save_print_log_as(self.operator_session()?, &self.wf.prints, &next)?;
                self.wf.prints = next;
                self.receipt = Some((job.ticket_id, job.copy == CopyKind::Internal, job.receipt));
                if job.state == PrintState::Failed {
                    return Err("Simulated printer failure. Ticket is saved; retry the same job after clearing the failure switch.".into());
                }
            }
        }
        Ok(())
    }
    pub(super) fn print_saved(&mut self, copy: CopyKind) -> Result<(), String> {
        #[cfg(feature = "extended-workflows")]
        self.store
            .require_session(self.operator_session()?, Permission::Counter)?;
        let ticket = self.active();
        if ticket.state != State::Completed {
            return Err("Complete and save before printing.".into());
        }
        let key = match copy {
            CopyKind::Customer => format!("customer:{}", ticket.id),
            CopyKind::Internal => format!("internal:{}:{}", ticket.id, self.wf.prints.next_id),
        };
        let (next, job) = self.wf.prints.request(
            ticket.id,
            copy,
            &ticket.receipt(),
            &key,
            self.wf.printer_fails(),
        )?;
        #[cfg(feature = "extended-workflows")]
        self.store
            .save_print_log_as(self.operator_session()?, &self.wf.prints, &next)?;
        #[cfg(not(feature = "extended-workflows"))]
        self.store.save_counter_print_log(&self.wf.prints, &next)?;
        self.wf.prints = next;
        self.receipt = Some((job.ticket_id, copy == CopyKind::Internal, job.receipt));
        if job.state == PrintState::Failed {
            return Err("Simulated printer failure. Saved ticket and payment are unchanged. Retry the failed job.".into());
        }
        #[cfg(not(feature = "extended-workflows"))]
        self.request_physical_receipt(copy)?;
        Ok(())
    }
    pub(super) fn workflow_checkout(&mut self, ui: &mut egui::Ui, action: &mut Option<Act>) {
        let ticket = self.active().clone();
        ui.label(RichText::new("Customer").strong());
        match ticket.customer_choice {
            crate::workflow::CustomerChoice::Unset => {
                ui.colored_label(OCHRE, "Customer choice required");
            }
            crate::workflow::CustomerChoice::Blank => {
                ui.label("Blank (803)");
            }
            crate::workflow::CustomerChoice::Selected => {
                if let Some(c) = &ticket.customer_record {
                    ui.label(format!("{} / {}", c.number, c.name));
                    if !c.notice.is_empty() {
                        ui.colored_label(OCHRE, format!("Notice: {}", c.notice));
                    }
                    if ui.small_button("Edit selected customer").clicked() {
                        self.wf.customer_edit = self
                            .wf
                            .directory
                            .customers
                            .iter()
                            .find(|x| x.id == c.id)
                            .cloned();
                    }
                }
            }
        }
        ui.add(
            TextEdit::singleline(&mut self.query)
                .hint_text("Number, name, phone or address")
                .char_limit(240),
        );
        if !self.query.trim().is_empty() {
            let matches = self.wf.directory.search_customers(&self.query);
            if matches.is_empty() {
                ui.label("No customer matches");
            }
            for c in matches.into_iter().take(6) {
                if ui
                    .button(format!(
                        "{} / {}\n{} / {}",
                        c.number, c.name, c.phone, c.address
                    ))
                    .clicked()
                {
                    *action = Some(Act::Workflow(WorkflowAct::SelectCustomer(c.id)));
                }
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Create customer").clicked() {
                self.wf.customer_edit = Some(Customer {
                    name: self.query.clone(),
                    ..Default::default()
                });
            }
            if ui.button("Blank").clicked() {
                *action = Some(Act::Workflow(WorkflowAct::Blank));
            }
        });
        #[cfg(feature = "extended-workflows")]
        ui.collapsing("Synthetic scanner", |ui| {
            for (label, fixture) in [
                ("Review test scan", ScanFixture::Success),
                ("Duplicate test", ScanFixture::Duplicate),
                ("Unreadable test", ScanFixture::Unreadable),
                ("Disconnected test", ScanFixture::Disconnected),
            ] {
                if ui.button(label).clicked() {
                    *action = Some(Act::Workflow(WorkflowAct::Scan(fixture)));
                }
            }
        });
        #[cfg(not(feature = "extended-workflows"))]
        {
            if ui.button("Scan customer").clicked() {
                *action = Some(Act::ScannerOpen);
            }
            ui.small("Use Scan customer before scanning. Only the named keyboard profile is supported; customer lookup and manual entry are always available.");
        }
        ui.separator();
        ui.label(RichText::new("Tender").strong());
        ui.radio_value(&mut self.tender, Tender::Cash, "Cash");
        ui.radio_value(&mut self.tender, Tender::Card, "External card");
        ui.radio_value(&mut self.tender, Tender::Charge, "Charge to account");
        match self.tender {
            Tender::Cash => {
                ui.add(
                    TextEdit::singleline(&mut self.wf.cash)
                        .hint_text("Cash received ($)")
                        .char_limit(18),
                );
                if ui.button("Confirm cash received").clicked() {
                    *action = Some(Act::Workflow(WorkflowAct::Cash));
                }
                if let Some(cash) = &ticket.tender_evidence.cash {
                    ui.label(format!(
                        "Received {} / Change {}",
                        money(cash.received_cents),
                        money(cash.change_cents)
                    ));
                }
            }
            Tender::Card => {
                ui.small("Record the separate processor result. No card data is stored here.");
                ui.add(
                    TextEdit::singleline(&mut self.reference)
                        .hint_text("Processor reference")
                        .char_limit(64),
                );
                ui.add(
                    TextEdit::singleline(&mut self.wf.card_amount)
                        .hint_text("External amount ($)")
                        .char_limit(18),
                );
                if ui.button("Track external attempt").clicked() {
                    *action = Some(Act::Workflow(WorkflowAct::BeginCard));
                }
                if let Some(attempt) = ticket.tender_evidence.card_attempts.last() {
                    ui.label(format!(
                        "{} / {} / {:?}",
                        attempt.reference,
                        money(attempt.amount_cents),
                        attempt.outcome
                    ));
                    if attempt.outcome.unresolved() {
                        egui::ComboBox::from_id_salt("card-outcome")
                            .selected_text(format!("{:?}", self.wf.outcome))
                            .show_ui(ui, |ui| {
                                for outcome in [
                                    CardOutcome::Approved,
                                    CardOutcome::Declined,
                                    CardOutcome::Cancelled,
                                    CardOutcome::ZipMismatch,
                                    CardOutcome::Unknown,
                                ] {
                                    ui.selectable_value(
                                        &mut self.wf.outcome,
                                        outcome,
                                        format!("{outcome:?}"),
                                    );
                                }
                            });
                        ui.checkbox(&mut self.wf.verified, "I verified this processor result");
                        if ui
                            .button(if attempt.outcome == CardOutcome::Unknown {
                                "Reconcile existing attempt"
                            } else {
                                "Record result"
                            })
                            .clicked()
                        {
                            *action = Some(Act::Workflow(WorkflowAct::CardResult(
                                attempt.outcome == CardOutcome::Unknown,
                            )));
                        }
                    }
                }
            }
            Tender::Charge => {
                if let Some(a) = &ticket.account_record {
                    ui.label(format!("Selected: {} / {}", a.number, a.name));
                }
                ui.add(
                    TextEdit::singleline(&mut self.account)
                        .hint_text("Business name or account number")
                        .char_limit(120),
                );
                let matches = self.wf.directory.search_accounts(&self.account);
                if !self.account.trim().is_empty() && matches.is_empty() {
                    ui.label("No matching accounts");
                }
                for a in matches.into_iter().take(6) {
                    if ui
                        .add_enabled(
                            a.active,
                            egui::Button::new(format!(
                                "{} / {}{}",
                                a.number,
                                a.name,
                                if a.active { "" } else { " (inactive)" }
                            )),
                        )
                        .clicked()
                    {
                        *action = Some(Act::Workflow(WorkflowAct::SelectAccount(a.id)));
                    }
                }
                if ui.button("Create business account").clicked() {
                    self.wf.account_edit = Some(BusinessAccount {
                        name: self.account.clone(),
                        active: true,
                        ..Default::default()
                    });
                }
                ui.small("Signed receipt; pending billing. No automated invoice.");
            }
        }
    }
    pub(super) fn workflow_dialogs(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        let mut cancel = false;
        if let Some(c) = &mut self.wf.customer_edit {
            let mut open = true;
            egui::Window::new("Review customer")
                .open(&mut open)
                .default_width(400.0)
                .show(ctx, |ui| {
                    if let Some(error) = &self.error {
                        ui.colored_label(RED, error);
                    }
                    if !self.wf.scan_notice.is_empty() {
                        ui.label(&self.wf.scan_notice);
                    }
                    ui.label(if c.id == 0 { "New customer" } else { &c.number });
                    for (label, text, limit) in [
                        ("Name", &mut c.name, 120),
                        ("Phone", &mut c.phone, 40),
                        ("Address", &mut c.address, 240),
                        ("Account notice (internal)", &mut c.notice, 500),
                    ] {
                        ui.label(label);
                        ui.add(
                            TextEdit::singleline(text)
                                .char_limit(limit)
                                .desired_width(360.0),
                        );
                    }
                    if !self.wf.scan_notice.is_empty() {
                        for existing in self
                            .wf
                            .directory
                            .customers
                            .iter()
                            .filter(|x| self.wf.scan_matches.contains(&x.id))
                            .take(6)
                        {
                            if ui
                                .button(format!(
                                    "Select {} / {} / {}",
                                    existing.number, existing.name, existing.address
                                ))
                                .clicked()
                            {
                                *action =
                                    Some(Act::Workflow(WorkflowAct::SelectCustomer(existing.id)));
                            }
                        }
                    }
                    if ui.button("Save and select customer").clicked() {
                        *action = Some(Act::Workflow(WorkflowAct::SaveCustomer));
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            cancel |= !open;
        }
        if cancel {
            self.wf.customer_edit = None;
            self.wf.scan_notice.clear();
        }
        let mut cancel = false;
        if let Some(a) = &mut self.wf.account_edit {
            let mut open = true;
            egui::Window::new("Business account")
                .open(&mut open)
                .show(ctx, |ui| {
                    if let Some(error) = &self.error {
                        ui.colored_label(RED, error);
                    }
                    ui.label("Business name");
                    ui.add(TextEdit::singleline(&mut a.name).char_limit(120));
                    ui.label("Account number (blank generates one)");
                    ui.add(TextEdit::singleline(&mut a.number).char_limit(32));
                    ui.checkbox(&mut a.active, "Active for charge-to");
                    if ui.button("Save and select account").clicked() {
                        *action = Some(Act::Workflow(WorkflowAct::SaveAccount));
                    }
                });
            cancel = !open;
        }
        if cancel {
            self.wf.account_edit = None;
        }
        if let Some(index) = self.wf.pending_remove {
            let mut open = true;
            egui::Window::new("Confirm line deletion")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Remove material line {} from this draft?",
                        index + 1
                    ));
                    if ui.button("Remove selected line").clicked() {
                        *action = Some(Act::Workflow(WorkflowAct::Delete(index)));
                    }
                    if ui.button("Keep line").clicked() {
                        self.wf.pending_remove = None;
                    }
                });
            if !open {
                self.wf.pending_remove = None;
            }
        }
    }
    #[cfg(feature = "extended-workflows")]
    pub(super) fn workflow_status(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        egui::TopBottomPanel::bottom("print-status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                #[cfg(feature = "extended-workflows")]
                {
                    ui.label("Training printer simulation");
                    ui.checkbox(&mut self.wf.fail_print, "Simulate failure");
                }
                #[cfg(not(feature = "extended-workflows"))]
                ui.label("Preview only — physical printer not configured");
                for job in self
                    .wf
                    .prints
                    .jobs
                    .iter()
                    .filter(|j| j.ticket_id == self.active && j.state == PrintState::Failed)
                {
                    if ui
                        .button(format!("Retry {:?} job {}", job.copy, job.id))
                        .clicked()
                    {
                        *action = Some(Act::Workflow(WorkflowAct::RetryPrint(job.id)));
                    }
                }
                if let Some(job) = self
                    .wf
                    .prints
                    .jobs
                    .iter()
                    .rev()
                    .find(|j| j.ticket_id == self.active)
                {
                    ui.label(format!(
                        "{:?} copy: {:?} ({} attempt(s))",
                        job.copy, job.state, job.attempts
                    ));
                }
            });
        });
    }
    #[cfg(all(test, not(feature = "extended-workflows")))]
    pub(super) fn workflow_status(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        self.printer_status(ctx, action);
    }
}

#[cfg(test)]
#[path = "../../tests/shared/ui_workflows_tests.rs"]
mod tests;
