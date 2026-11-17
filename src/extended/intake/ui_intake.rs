use super::*;
use crate::{
    intake_review::{ReviewDisposition, ReviewLedger, ReviewSnapshot},
    intake_ui::{IntakeForm, IntakeUi},
};

pub(super) struct IntakeWork {
    pub details: IntakeUi,
    pub ledger: ReviewLedger,
    pub queue_open: bool,
    review: Option<ReviewDialog>,
    query: String,
    only_pending: bool,
    include_contacts: bool,
    message: Option<String>,
}
impl IntakeWork {
    pub fn load(store: &Store) -> Result<Self, String> {
        Ok(Self {
            details: Default::default(),
            ledger: store.intake_reviews()?,
            queue_open: false,
            review: None,
            query: String::new(),
            only_pending: true,
            include_contacts: false,
            message: None,
        })
    }
    pub fn locked(&mut self) {
        self.details.saved();
        self.queue_open = false;
        self.review = None;
        self.query.clear();
        self.include_contacts = false;
        self.message = None;
    }
    pub fn modal_open(&self) -> bool {
        self.details.open || self.review.is_some()
    }
}
struct ReviewDialog {
    original: Ticket,
    previous_event: Option<u64>,
    form: IntakeForm,
    disposition: ReviewDisposition,
    reason: String,
    request_key: String,
}
impl ReviewDialog {
    fn new(ticket: &Ticket, ledger: &ReviewLedger) -> Result<Self, String> {
        let event = ledger.latest(ticket.id);
        let snapshot = event
            .map(|e| e.snapshot.clone())
            .map_or_else(|| ReviewSnapshot::from_ticket(ticket), Ok)?;
        let snapshot = snapshot.normalized_for(ticket)?;
        let mut preview = ticket.clone();
        preview.source_intake = snapshot.source;
        for (line, material) in preview.lines.iter_mut().zip(snapshot.materials) {
            line.material_intake = material.details;
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "System clock is unavailable.")?
            .as_nanos();
        Ok(Self {
            original: ticket.clone(),
            previous_event: event.map(|e| e.id),
            form: IntakeForm::new(&preview),
            disposition: snapshot.disposition,
            reason: String::new(),
            request_key: format!("source-{}-{}-{nonce}", ticket.id, ledger.next_id),
        })
    }
    fn snapshot(&self) -> Result<ReviewSnapshot, String> {
        let form = self.form.normalized()?;
        let mut snapshot = ReviewSnapshot::from_ticket(&self.original)?;
        if snapshot.materials.len() != form.materials.len() {
            return Err("Material lines changed. Reload this review.".into());
        }
        snapshot.source = form.source;
        snapshot.disposition = self.disposition;
        for (material, details) in snapshot.materials.iter_mut().zip(form.materials) {
            material.details = details;
        }
        Ok(snapshot)
    }
}
enum IntakeAction {
    BeginReview(u64),
    ReloadReview,
    SaveReview,
    Refresh,
    Export,
    Open(u64),
}
fn disposition_label(value: ReviewDisposition) -> &'static str {
    match value {
        ReviewDisposition::PendingReview => "Pending documentation",
        ReviewDisposition::Reviewed => "Documentation reviewed",
        ReviewDisposition::NotForClaim => "Not for claim",
    }
}

impl RegisterApp {
    pub(super) fn intake_controls(&mut self, ui: &mut egui::Ui) {
        let ticket = self.active();
        let issue_count = self
            .intake
            .ledger
            .latest(ticket.id)
            .and_then(|e| e.snapshot.issues(ticket).ok())
            .unwrap_or_else(|| ticket.intake_issues())
            .len();
        let status = self
            .intake
            .ledger
            .latest(ticket.id)
            .map(|e| disposition_label(e.snapshot.disposition))
            .unwrap_or("Pending documentation");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Source / material intake").clicked() {
                self.intake.details.open = true;
            }
            if ui.button("Intake review queue").clicked() {
                self.intake.queue_open = true;
            }
            ui.small(format!("{status} / {issue_count} documentary flag(s)"));
        });
    }
    pub(super) fn begin_documentary_review(&mut self, id: u64) -> Result<(), String> {
        self.store
            .require_session(self.operator_session()?, Permission::ReviewIntake)?;
        let ticket = self
            .tickets
            .iter()
            .find(|t| t.id == id)
            .ok_or("Ticket not found.")?;
        self.intake.review = Some(ReviewDialog::new(ticket, &self.intake.ledger)?);
        self.intake.message = None;
        Ok(())
    }
    fn reload_intake_records(&mut self) -> Result<(), String> {
        let mut tickets = self.store.list()?;
        tickets.sort_by_key(|t| t.id);
        let ledger = self.store.intake_reviews()?;
        ledger.validate_against(&tickets)?;
        if !tickets.iter().any(|t| t.id == self.active) {
            return Err("Active ticket disappeared. Reopen the register.".into());
        }
        self.tickets = tickets;
        self.intake.ledger = ledger;
        Ok(())
    }
    pub(super) fn reload_documentary_review(&mut self) -> Result<(), String> {
        let id = self
            .intake
            .review
            .as_ref()
            .ok_or("Open a documentary review first.")?
            .original
            .id;
        self.reload_intake_records()?;
        self.begin_documentary_review(id)
    }
    pub(super) fn record_documentary_review(&mut self) -> Result<(), String> {
        let dialog = self
            .intake
            .review
            .as_ref()
            .ok_or("Open a documentary review first.")?;
        let ticket = self
            .tickets
            .iter()
            .find(|t| t.id == dialog.original.id)
            .ok_or("Ticket not found.")?;
        if ticket != &dialog.original
            || self.intake.ledger.latest(ticket.id).map(|e| e.id) != dialog.previous_event
        {
            return Err(
                "The saved ticket or source review changed. Reload before recording this review."
                    .into(),
            );
        }
        let actor = self
            .store
            .require_session(self.operator_session()?, Permission::ReviewIntake)?;
        let snapshot = dialog.snapshot()?;
        let (next, _) = self.intake.ledger.record(
            ticket,
            &snapshot,
            &actor.display_name,
            &dialog.reason,
            &dialog.request_key,
            crate::domain::now(),
        )?;
        let next = self.store.save_intake_reviews_as(
            self.operator_session()?,
            &self.intake.ledger,
            &next,
        )?;
        self.intake.ledger = next;
        self.intake.review = None;
        self.intake.message = Some(
            "Documentary review saved. Original ticket, payment and receipt are unchanged.".into(),
        );
        Ok(())
    }
    pub(super) fn export_intake_review(&mut self) -> Result<std::path::PathBuf, String> {
        let csv = self
            .store
            .prepare_intake_export_as(self.operator_session()?, self.intake.include_contacts)?;
        let folder = self
            .store
            .path()
            .parent()
            .ok_or("Register folder is unavailable.")?
            .join("exports");
        std::fs::create_dir_all(&folder)
            .map_err(|e| format!("Could not create export folder: {e}"))?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "System clock is unavailable.")?
            .as_nanos();
        let path = folder.join(format!("training-intake-review-{nonce}.csv"));
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("Could not create a new export: {e}"))?;
        file.write_all(csv.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| {
                format!(
                    "Export failed; partial output retained at {}: {e}",
                    path.display()
                )
            })?;
        self.intake.message = Some(format!("Training review CSV saved: {}", path.display()));
        Ok(path)
    }
    pub(super) fn intake_panels(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        let ticket = self.active().clone();
        if let Some(next) = self.intake.details.show(ctx, &ticket) {
            let result = self.save_action(move |current| {
                if current.id != next.id || current.revision != next.revision {
                    return Err("Ticket changed. Reload intake before saving.".into());
                }
                *current = next;
                Ok(())
            });
            match result {
                Ok(()) => self.intake.details.saved(),
                Err(error) => self.intake.details.error(error),
            }
        }
        let queue_action = self.intake_queue(ctx);
        let review_action = self.documentary_dialog(ctx);
        if let Some(request) = review_action.or(queue_action) {
            let result = match request {
                IntakeAction::BeginReview(id) => self.begin_documentary_review(id),
                IntakeAction::ReloadReview => self.reload_documentary_review(),
                IntakeAction::SaveReview => self.record_documentary_review(),
                IntakeAction::Refresh => self.reload_intake_records(),
                IntakeAction::Export => self.export_intake_review().map(|_| ()),
                IntakeAction::Open(id) => {
                    self.intake.details.saved();
                    self.intake.details.open = true;
                    *action = Some(Act::Open(id));
                    Ok(())
                }
            };
            if let Err(error) = result {
                self.intake.message = Some(error);
            }
        }
    }
    fn intake_queue(&mut self, ctx: &egui::Context) -> Option<IntakeAction> {
        if !self.intake.queue_open {
            return None;
        }
        let mut open = true;
        let mut action = None;
        egui::Window::new("E-waste intake review queue").open(&mut open).default_size([760.,530.]).max_height((ctx.content_rect().height()-40.).max(120.)).show(ctx,|ui|{
            ui.label("Training documentary review only. Payment, customer Blank and program eligibility are separate decisions.");
            ui.horizontal_wrapped(|ui|{
                ui.add(TextEdit::singleline(&mut self.intake.query).hint_text("Ticket number or customer").char_limit(120));
                ui.checkbox(&mut self.intake.only_pending,"Pending only");
                if ui.button("Refresh saved records").clicked(){action=Some(IntakeAction::Refresh);}
            });
            if let Some(message)=&self.intake.message{ui.label(message);}
            egui::ScrollArea::vertical().max_height((ui.available_height()-100.).clamp(32.,330.)).show(ui,|ui|{
                let query=self.intake.query.trim().trim_start_matches('#').to_lowercase();let mut found=false;
                for ticket in self.tickets.iter().rev().filter(|t|!t.lines.is_empty()){
                    let event=self.intake.ledger.latest(ticket.id);
                    let disposition=event.map(|e|e.snapshot.disposition).unwrap_or_default();
                    if self.intake.only_pending&&disposition!=ReviewDisposition::PendingReview{continue;}
                    if !query.is_empty()&&!format!("{:06}",ticket.id).contains(&query)&&!ticket.customer.as_ref().is_some_and(|name|name.to_lowercase().contains(&query)){continue;}
                    found=true;
                    let issues=event.map(|e|e.snapshot.issues(ticket)).unwrap_or_else(||Ok(ticket.intake_issues()));
                    let count=issues.as_ref().map_or(0,|i|i.len());
                    ui.group(|ui|{
                        ui.label(format!("#{:06} / {} / {} / {}",ticket.id,state_label(ticket.state),ticket.total().map_or_else(||"Price pending".into(),money),ticket.customer.as_deref().unwrap_or("Blank")));
                        ui.label(format!("{} / {count} documentary flag(s)",disposition_label(disposition)));
                        if let Err(error)=&issues{ui.colored_label(RED,error);}
                        ui.horizontal(|ui|{
                            if ui.button("View intake").clicked(){action=Some(IntakeAction::Open(ticket.id));}
                            if ui.add_enabled(ticket.state==State::Completed,egui::Button::new("Record documentary review")).clicked(){action=Some(IntakeAction::BeginReview(ticket.id));}
                        });
                    });
                }
                if !found{ui.label("No matching intake records.");}
            });
            ui.separator();
            ui.checkbox(&mut self.intake.include_contacts,"Include synthetic source/contact fields in local CSV");
            ui.small("Default CSV excludes source/contact details and all internal notes. This is not an official reporting form.");
            if ui.button("Export training review CSV").clicked(){action=Some(IntakeAction::Export);}
        });
        self.intake.queue_open = open;
        action
    }
    fn documentary_dialog(&mut self, ctx: &egui::Context) -> Option<IntakeAction> {
        let operator_name = self
            .security
            .session
            .as_ref()
            .map(|s| s.actor().display_name.clone())
            .unwrap_or_default();
        let dialog = self.intake.review.as_mut()?;
        let mut open = true;
        let mut action = None;
        egui::Window::new(format!("Documentary review - #{:06}",dialog.original.id)).open(&mut open).default_size([760.,650.]).max_height((ctx.content_rect().height()-40.).max(120.)).show(ctx,|ui|{
            ui.label("A new audit event corrects source documents. The financial ticket and original receipt remain frozen.");
            ui.small("The signed-in reviewer is recorded. Reviewed documents do not establish claim eligibility.");
            if let Some(message)=&self.intake.message{ui.colored_label(OCHRE,message);}
            egui::ScrollArea::vertical().max_height((ui.available_height()-125.).clamp(32.,430.)).show(ui,|ui|{
                dialog.form.fields(ui,&dialog.original);
                if let Ok(snapshot)=dialog.snapshot(){match snapshot.issues(&dialog.original){Ok(issues)=>{for issue in issues{ui.colored_label(OCHRE,issue.message);}},Err(error)=>{ui.colored_label(RED,error);}}}
                for event in self.intake.ledger.events.iter().rev().filter(|e|e.ticket_id==dialog.original.id){ui.small(format!("Review #{}: {} / {} / {}",event.id,disposition_label(event.snapshot.disposition),event.operator,event.reason));}
            });
            egui::ComboBox::from_id_salt("source-review-disposition").selected_text(disposition_label(dialog.disposition)).show_ui(ui,|ui|{
                for value in [ReviewDisposition::PendingReview,ReviewDisposition::Reviewed,ReviewDisposition::NotForClaim]{ui.selectable_value(&mut dialog.disposition,value,disposition_label(value));}
            });
            ui.label(format!("Reviewer: {operator_name}"));
            ui.add(TextEdit::singleline(&mut dialog.reason).hint_text("Reason / evidence checked").char_limit(500));
            ui.horizontal_wrapped(|ui|{
                if ui.button("Save new documentary review").clicked(){action=Some(IntakeAction::SaveReview);}
                if ui.button("Discard changes and reload saved review").clicked(){action=Some(IntakeAction::ReloadReview);}
            });
        });
        if !open {
            self.intake.review = None;
        }
        action
    }
}

#[cfg(test)]
#[path = "../../tests/extended/intake/ui_intake_tests.rs"]
mod tests;
