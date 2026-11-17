//! UI-thread dispatch coordination; native printer calls run only on workers.
use super::*;
use crate::{
    printing::CopyKind,
    receipt_dispatch::{
        DispatchJob, DispatchOutcome, DispatchState, PrepareDispatch, PreparedDispatch,
        ReceiptDispatchLedger,
    },
};
use std::{
    collections::HashSet,
    sync::{Arc, mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
type Discover = Arc<dyn Fn() -> Result<Vec<String>, String> + Send + Sync>;
type Submit = Arc<dyn Fn(&str, &str, &str) -> DispatchOutcome + Send + Sync>;
enum WorkerResult {
    Printers(Result<Vec<String>, String>),
    Submitted(PreparedDispatch, DispatchOutcome),
}
pub(super) struct PrinterUi {
    pub(super) open: bool,
    pub(super) queue: Option<String>,
    pub(super) ledger: ReceiptDispatchLedger,
    pub(super) recovery: Option<CheckedCopy>,
    pub(super) choice: Option<String>,
    settings_original: Option<Option<String>>,
    queues: Vec<String>,
    pub(super) discovering: bool,
    message: Option<String>,
    receiver: mpsc::Receiver<WorkerResult>,
    sender: mpsc::Sender<WorkerResult>,
    discover: Discover,
    submit: Submit,
    in_flight: HashSet<(u64, u32)>,
    uncommitted: Vec<(PreparedDispatch, DispatchOutcome)>,
}
pub(super) struct CheckedCopy {
    pub(super) prior_job_id: u64,
    pub(super) prior_attempt_id: u32,
    pub(super) queue: String,
    pub(super) checked: bool,
    ticket_id: u64,
    old_queue: String,
    copy: CopyKind,
    request_key: String,
}
fn unique_key(prefix: &str, id: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    format!(
        "{prefix}:{id}:{}:{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
fn state_caption(state: &DispatchState) -> String {
    match state {
        DispatchState::Dispatching => {
            "Submission unresolved; it may have reached Windows. Do not resend automatically."
                .into()
        }
        DispatchState::Accepted { windows_job_id } => {
            format!("Accepted by Windows (job {windows_job_id}); paper output is not confirmed.")
        }
        DispatchState::NotSubmitted { detail } => format!("Not submitted: {detail}"),
        DispatchState::Unknown { detail } => format!(
            "Submission uncertain: {detail}. Check the printer and Windows queue before another copy."
        ),
    }
}
impl PrinterUi {
    pub(super) fn load(store: &Store) -> Result<Self, String> {
        Self::with_adapter(
            store,
            Arc::new(crate::printer_windows::installed_printers),
            Arc::new(crate::printer_windows::submit),
        )
    }
    pub(super) fn with_adapter(
        store: &Store,
        discover: Discover,
        submit: Submit,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let queue = store.receipt_printer_queue()?;
        Ok(Self {
            open: false,
            choice: queue.clone(),
            settings_original: None,
            queue,
            ledger: store.receipt_dispatch_log()?,
            recovery: None,
            queues: vec![],
            discovering: false,
            message: None,
            receiver,
            sender,
            discover,
            submit,
            in_flight: HashSet::new(),
            uncommitted: vec![],
        })
    }
    pub(super) fn reload(&mut self, store: &Store) -> Result<(), String> {
        let queue = store.receipt_printer_queue()?;
        let ledger = store.receipt_dispatch_log()?;
        self.queue = queue;
        self.ledger = ledger;
        Ok(())
    }
    pub(super) fn ticket_changed(&mut self) {
        self.message = None;
        self.recovery = None;
    }
    fn discover(&mut self) -> Result<(), String> {
        if self.discovering {
            return Ok(());
        }
        self.discovering = true;
        self.message = None;
        let discover = self.discover.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("receipt-printer-list".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| discover()))
                    .unwrap_or_else(|_| {
                        Err("Installed-printer discovery stopped unexpectedly.".into())
                    });
                let _ = sender.send(WorkerResult::Printers(result));
            })
        {
            self.discovering = false;
            return Err(format!("Could not start printer discovery: {error}"));
        }
        Ok(())
    }
    fn start(&mut self, prepared: PreparedDispatch) {
        let queue = prepared.queue().to_owned();
        let body = prepared.receipt().to_owned();
        let copy = match prepared.copy() {
            CopyKind::Customer => "Customer receipt",
            CopyKind::Internal => "Internal copy",
        };
        let title = format!("E-waste Counter / {copy} #{:06}", prepared.ticket_id());
        let submit = self.submit.clone();
        let sender = self.sender.clone();
        let capability = prepared.clone();
        self.in_flight
            .insert((prepared.job_id(), prepared.attempt_id()));
        if let Err(error) = std::thread::Builder::new()
            .name("receipt-printer-submit".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    submit(&queue, &title, &body)
                }))
                .unwrap_or_else(|_| DispatchOutcome::Unknown {
                    detail: "The printer worker stopped before its submission result was known."
                        .into(),
                });
                let _ = sender.send(WorkerResult::Submitted(capability, outcome));
            })
        {
            let _ = self.sender.send(WorkerResult::Submitted(
                prepared,
                DispatchOutcome::NotSubmitted {
                    detail: format!(
                        "No printer call was made because the worker could not start: {error}"
                    ),
                },
            ));
        }
    }
    fn busy(&self) -> bool {
        self.discovering || !self.in_flight.is_empty()
    }
    fn job_busy(&self, id: u64) -> bool {
        self.in_flight.iter().any(|(job, _)| *job == id)
            || self
                .uncommitted
                .iter()
                .any(|(prepared, _)| prepared.job_id() == id)
    }
    fn latest(&self, job: &DispatchJob) -> bool {
        self.ledger
            .jobs
            .iter()
            .filter(|other| other.ticket_id == job.ticket_id && other.copy == job.copy)
            .max_by_key(|other| other.id)
            .is_some_and(|latest| latest.id == job.id)
    }
    fn recovery_kind(&self, job: &DispatchJob) -> bool {
        match job.state() {
            DispatchState::Unknown { .. } => true,
            DispatchState::Dispatching => !self.in_flight.iter().any(|(id, _)| *id == job.id),
            DispatchState::NotSubmitted { .. } => {
                self.queue.as_ref().is_some_and(|queue| queue != &job.queue)
            }
            DispatchState::Accepted { .. } => false,
        }
    }
}
impl RegisterApp {
    pub(super) fn open_checked_copy(&mut self, id: u64) -> Result<(), String> {
        self.printer.reload(&self.store)?;
        let queue = self
            .printer
            .queue
            .clone()
            .ok_or("Choose an installed printer with Printer... first.")?;
        let job = self
            .printer
            .ledger
            .job(id)
            .ok_or("Saved printer request not found.")?;
        if job.ticket_id != self.active
            || !self.printer.latest(job)
            || !self.printer.recovery_kind(job)
            || self.printer.job_busy(id)
        {
            return Err("This printer request is not available for a checked new copy. Finish saving any pending result and refresh its status first.".into());
        }
        self.printer.recovery = Some(CheckedCopy {
            prior_job_id: id,
            prior_attempt_id: job
                .attempts
                .last()
                .ok_or("Saved printer attempt not found.")?
                .id,
            queue,
            checked: false,
            ticket_id: job.ticket_id,
            old_queue: job.queue.clone(),
            copy: job.copy,
            request_key: unique_key("physical-checked", id),
        });
        Ok(())
    }
    pub(super) fn confirm_checked_copy(&mut self) -> Result<(), String> {
        let form = self
            .printer
            .recovery
            .as_ref()
            .ok_or("Open the checked-copy confirmation first.")?;
        if !form.checked {
            return Err("Confirm that you checked the printer, Windows queue and paper before requesting another copy.".into());
        }
        let job = self
            .printer
            .ledger
            .job(form.prior_job_id)
            .ok_or("Saved printer request not found.")?;
        if form.ticket_id != self.active
            || self.printer.queue.as_ref() != Some(&form.queue)
            || job
                .attempts
                .last()
                .is_none_or(|attempt| attempt.id != form.prior_attempt_id)
            || !self.printer.latest(job)
            || !self.printer.recovery_kind(job)
            || self.printer.job_busy(job.id)
        {
            return Err("The saved printer request or selected printer changed. Close and reopen this confirmation before continuing.".into());
        }
        let prepared = self.store.prepare_checked_receipt_copy(
            form.prior_job_id,
            form.prior_attempt_id,
            &form.request_key,
            &form.queue,
            true,
        )?;
        self.printer.recovery = None;
        self.accept_dispatch(prepared)
    }
    pub(super) fn request_physical_receipt(&mut self, copy: CopyKind) -> Result<(), String> {
        let Some(queue) = self.printer.queue.clone() else {
            self.printer.message=Some("Receipt preview saved. Select an installed printer with Printer... to enable paper copies.".into());
            return Ok(());
        };
        let id = self.active().id;
        let key = match copy {
            CopyKind::Customer => format!("physical-customer:{id}"),
            CopyKind::Internal => unique_key("physical-internal", id),
        };
        let prepared = self
            .store
            .prepare_receipt_dispatch(id, copy, &key, &queue)?;
        self.accept_dispatch(prepared)
    }
    fn accept_dispatch(&mut self, prepared: PrepareDispatch) -> Result<(), String> {
        self.printer.message = None;
        match prepared {
            PrepareDispatch::Submit(prepared) => self.printer.start(prepared),
            PrepareDispatch::Existing(job) => {
                self.printer.message = Some(format!(
                    "Saved printer request #{} to {}: {}",
                    job.id,
                    job.queue,
                    state_caption(job.state())
                ));
            }
        }
        self.printer.ledger = self.store.receipt_dispatch_log()?;
        Ok(())
    }
    pub(super) fn retry_physical_receipt(&mut self, id: u64) -> Result<(), String> {
        let key = unique_key("physical-retry", id);
        let prepared = self.store.retry_receipt_dispatch(id, &key)?;
        self.accept_dispatch(prepared)
    }
    pub(super) fn poll_printer_results(&mut self) -> Result<(), String> {
        let mut failure = None;
        while let Ok(result) = self.printer.receiver.try_recv() {
            match result {
                WorkerResult::Printers(result) => {
                    self.printer.discovering = false;
                    match result {
                        Ok(mut queues) => {
                            queues.sort();
                            queues.dedup();
                            self.printer.queues = queues;
                        }
                        Err(message) => self.printer.message = Some(message),
                    }
                }
                WorkerResult::Submitted(prepared, outcome) => {
                    self.printer
                        .in_flight
                        .remove(&(prepared.job_id(), prepared.attempt_id()));
                    if let Err(error) = self
                        .store
                        .record_receipt_dispatch_result(prepared.clone(), outcome.clone())
                    {
                        self.printer.uncommitted.push((prepared, outcome));
                        failure = Some(format!(
                            "The printer result could not be saved: {error}. Save the result again; this will not send another printer job."
                        ));
                    } else if let Err(error) = self.printer.reload(&self.store) {
                        failure = Some(error)
                    }
                }
            }
        }
        if let Some(error) = failure {
            self.printer.message = Some(error.clone());
            Err(error)
        } else {
            Ok(())
        }
    }
    fn save_printer_results_again(&mut self) -> Result<(), String> {
        let pending = std::mem::take(&mut self.printer.uncommitted);
        let mut failure = None;
        for (prepared, outcome) in pending {
            if let Err(error) = self
                .store
                .record_receipt_dispatch_result(prepared.clone(), outcome.clone())
            {
                self.printer.uncommitted.push((prepared, outcome));
                failure = Some(error);
            }
        }
        self.printer.reload(&self.store)?;
        if let Some(error) = failure {
            Err(error)
        } else {
            self.printer.message = None;
            Ok(())
        }
    }
    fn open_printer_settings(&mut self) -> Result<(), String> {
        self.printer.reload(&self.store)?;
        self.printer.choice = self.printer.queue.clone();
        self.printer.settings_original = Some(self.printer.queue.clone());
        self.printer.open = true;
        self.printer.discover()
    }
    fn save_printer_choice(&mut self) -> Result<(), String> {
        if self
            .printer
            .choice
            .as_ref()
            .is_some_and(|queue| !self.printer.queues.contains(queue))
        {
            return Err("Choose an installed printer, or select preview only.".into());
        }
        let original = self
            .printer
            .settings_original
            .as_ref()
            .ok_or("Open the printer choice before saving it.")?;
        self.store
            .save_receipt_printer_queue(original.as_deref(), self.printer.choice.as_deref())?;
        self.printer.queue = self.printer.choice.clone();
        self.printer.open = false;
        self.printer.settings_original = None;
        self.printer.message = None;
        Ok(())
    }
    pub(super) fn printer_tick(&mut self, ctx: &egui::Context) {
        let _ = self.poll_printer_results();
        if self.printer.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    pub(super) fn printer_status(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        egui::TopBottomPanel::bottom("physical-printer-status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Printer...").clicked() {
                    *action = Some(Act::PrinterSettings);
                }
                ui.label(self.printer.queue.as_deref().map_or_else(
                    || "Preview only — physical printer not configured".to_owned(),
                    |queue| format!("Printer: {queue}"),
                ));
                if ui.small_button("Refresh printer status").clicked() {
                    *action = Some(Act::PrinterRefresh);
                }
                if !self.printer.uncommitted.is_empty()
                    && ui.button("Save printer result again").clicked()
                {
                    *action = Some(Act::PrinterSaveResults);
                }
            });
            if let Some(message) = &self.printer.message {
                ui.colored_label(OCHRE, message);
            }
            for job in self
                .printer
                .ledger
                .jobs
                .iter()
                .filter(|job| job.ticket_id == self.active)
                .rev()
                .take(2)
            {
                ui.horizontal_wrapped(|ui| {
                    let copy = match job.copy {
                        CopyKind::Customer => "Customer receipt",
                        CopyKind::Internal => "Internal copy",
                    };
                    let state = state_caption(job.state());
                    ui.label(format!("{copy} / {} / {state}", job.queue));
                    if matches!(job.state(), DispatchState::NotSubmitted { .. })
                        && ui
                            .button(format!("Retry {copy} to saved printer"))
                            .clicked()
                    {
                        *action = Some(Act::PrinterRetry(job.id));
                    }
                    if self.printer.latest(job) && self.printer.recovery_kind(job) {
                        if ui
                            .add_enabled(
                                self.printer.queue.is_some() && !self.printer.job_busy(job.id),
                                egui::Button::new("Print another copy after checking"),
                            )
                            .clicked()
                        {
                            *action = Some(Act::PrinterRecover(job.id));
                        }
                        if self.printer.queue.is_none() {
                            ui.small("Choose Printer... before requesting another copy.");
                        }
                    }
                });
            }
        });
    }
    pub(super) fn printer_dialog(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        self.checked_copy_dialog(ctx, action);
        if !self.printer.open {
            return;
        }
        let mut open = true;
        egui::Window::new("Receipt printer").open(&mut open).default_width(540.).max_height((ctx.content_rect().height()-40.).max(120.)).show(ctx,|ui| {
            ui.label("Choose an installed Windows printer. Saving this choice does not print a test page.");
            if self.printer.discovering{ui.label("Reading installed printers...");}
            if let Some(message)=&self.printer.message{ui.colored_label(OCHRE,message);}
            let height=(ui.available_height()-64.).clamp(32.,320.);
            egui::ScrollArea::vertical().max_height(height).show(ui,|ui| {
                ui.radio_value(&mut self.printer.choice,None,"Preview only (no physical printer)");
                for queue in &self.printer.queues{ui.radio_value(&mut self.printer.choice,Some(queue.clone()),queue);}
                if !self.printer.discovering&&self.printer.queues.is_empty(){ui.label("No installed printers were found. Install or connect a printer in Windows, or use receipt preview.");}
                if self.printer.choice.as_ref().is_some_and(|queue|!self.printer.queues.contains(queue)){ui.colored_label(OCHRE,"The saved printer is not in the installed list. Choose another printer or preview only.");}
            });
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.printer.discovering,egui::Button::new("Save printer choice")).clicked(){*action=Some(Act::PrinterSaveChoice);}
                if ui.button("Refresh installed printers").clicked(){*action=Some(Act::PrinterDiscover);}
                if ui.button("Cancel").clicked(){*action=Some(Act::PrinterClose);}
            });
        });
        if !open {
            self.printer.open = false;
            self.printer.settings_original = None;
        }
    }
    fn checked_copy_dialog(&mut self, ctx: &egui::Context, action: &mut Option<Act>) {
        let busy = self
            .printer
            .recovery
            .as_ref()
            .is_some_and(|form| self.printer.job_busy(form.prior_job_id));
        let Some(form) = &mut self.printer.recovery else {
            return;
        };
        let mut open = true;
        egui::Window::new("Print another copy after checking")
            .open(&mut open).default_size([560., 340.])
            .max_height((ctx.content_rect().height() - 40.).max(120.))
            .show(ctx, |ui| {
                let copy = match form.copy { CopyKind::Customer => "Customer receipt", CopyKind::Internal => "Internal copy" };
                ui.strong(format!("{copy} / ticket #{:06}", form.ticket_id));
                let body_height = (ui.available_height() - 52.).clamp(32., 340.);
                egui::ScrollArea::vertical().max_height(body_height).show(ui, |ui| {
                    ui.label(format!("Original printer: {}", form.old_queue));
                    ui.label(format!("Selected printer for this new copy: {}", form.queue));
                    if let Some(error) = &self.error { ui.colored_label(OCHRE, error); }
                    ui.checkbox(&mut form.checked, "I checked the printer and Windows queue. The original may still print; I want another copy.");
                    if busy { ui.label("Wait for the original result to finish saving before requesting another copy."); }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(form.checked && !busy, egui::Button::new("Print checked new copy")).clicked() {
                        *action = Some(Act::PrinterConfirmRecovery);
                    }
                    if ui.button("Cancel").clicked() { *action = Some(Act::PrinterCancelRecovery); }
                });
            });
        if !open {
            self.printer.recovery = None;
        }
    }
    pub(super) fn run_printer_action(&mut self, action: Act) -> Result<(), String> {
        match action {
            Act::PrinterSettings => self.open_printer_settings(),
            Act::PrinterSaveChoice => self.save_printer_choice(),
            Act::PrinterDiscover => self.printer.discover(),
            Act::PrinterClose => {
                self.printer.open = false;
                self.printer.settings_original = None;
                Ok(())
            }
            Act::PrinterRetry(id) => self.retry_physical_receipt(id),
            Act::PrinterSaveResults => self.save_printer_results_again(),
            Act::PrinterRefresh => self.printer.reload(&self.store),
            Act::PrinterRecover(id) => self.open_checked_copy(id),
            Act::PrinterConfirmRecovery => self.confirm_checked_copy(),
            Act::PrinterCancelRecovery => {
                self.printer.recovery = None;
                Ok(())
            }
            _ => Err("Choose a printer action.".into()),
        }
    }
}
