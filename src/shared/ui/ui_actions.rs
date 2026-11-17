use super::*;

impl RegisterApp {
    pub(super) fn run(&mut self, action: Act) -> Result<(), String> {
        #[cfg(not(feature = "extended-workflows"))]
        if self.scanner.active()
            && !matches!(
                action,
                Act::ScannerReview
                    | Act::ScannerCancel
                    | Act::ScannerClear
                    | Act::Open(_)
                    | Act::New
            )
        {
            return Err(
                "Review or cancel the active scanner capture before using the counter.".into(),
            );
        }
        #[cfg(feature = "extended-workflows")]
        if matches!(action, Act::SignOut) {
            return self.handle_auth_action(crate::auth_ui::AuthAction::Logout);
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            if matches!(action, Act::ScannerReview) {
                return self.review_scanner_capture();
            }
            if matches!(action, Act::ScannerCancel) {
                self.cancel_scanner_capture();
                return Ok(());
            }
            if matches!(action, Act::ScannerClear) {
                if let Some(capture) = &mut self.scanner.capture {
                    capture.clear();
                }
                self.scanner.message = None;
                return Ok(());
            }
        }
        #[cfg(feature = "extended-workflows")]
        self.store
            .require_session(self.operator_session()?, Permission::Counter)?;
        match action {
            #[cfg(feature = "extended-workflows")]
            Act::SignOut => unreachable!("sign out handled before authenticated commands"),
            #[cfg(feature = "extended-workflows")]
            Act::Backup => {
                let parent = self
                    .store
                    .path()
                    .parent()
                    .ok_or("Database folder is unavailable.")?;
                let directory = parent.join("backups");
                std::fs::create_dir_all(&directory)
                    .map_err(|e| format!("Could not create backup folder: {e}"))?;
                let nonce = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| "System clock is unavailable.")?
                    .as_nanos();
                let target = directory.join(format!("counter-{nonce}.sqlite3"));
                self.store.backup_as(self.operator_session()?, &target)?;
                self.notice = Some(format!("Backup verified: {}", target.display()));
            }
            Act::New => {
                // Reuse an untouched draft rather than producing empty history entries.
                if self.active().state == State::Draft
                    && self.active().lines.is_empty()
                    && self.active().customer.is_none()
                {
                    self.reset_fields();
                    self.focus_code = true;
                    return Ok(());
                }
                let id = self
                    .tickets
                    .iter()
                    .map(|ticket| ticket.id)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| "No ticket numbers remain.".to_owned())?;
                let t = Ticket::new(id);
                let t = self.persist_ticket(&t)?;
                self.tickets.push(t);
                self.active = id;
                self.reset_fields();
                self.focus_code = true;
            }
            Act::Open(id) => {
                if self.tickets.iter().any(|t| t.id == id) {
                    self.active = id;
                    self.reset_fields();
                    self.show_history = false;
                }
            }
            Act::Add => {
                let code = self.material_text.trim().to_ascii_uppercase();
                let quantity = self.quantity.trim().to_owned();
                let manual = self.manual.trim().to_owned();
                if code.is_empty() {
                    return Err("Choose a material or enter its code.".into());
                }
                let tare = self.tare.clone();
                let catalog = self.wf.catalog.clone();
                self.save_action(|t| {
                    t.add_catalog_line(
                        &catalog,
                        &code,
                        &quantity,
                        &tare,
                        &manual,
                        crate::domain::now(),
                    )
                })?;
                #[cfg(not(feature = "extended-workflows"))]
                if matches!(code.as_str(), "801" | "802") {
                    self.open_source_details(self.active().lines.len() - 1)?;
                }
                self.material = None;
                self.material_text.clear();
                self.quantity.clear();
                self.tare.clear();
                self.manual.clear();
                self.focus_code = true;
            }
            Act::Remove(index) => self.wf.pending_remove = Some(index),
            Act::Edit(index) => {
                let line = self
                    .active()
                    .lines
                    .get(index)
                    .ok_or_else(|| "Line not found.".to_owned())?;
                self.edit = Some(LineEdit {
                    index,
                    quantity: line
                        .gross_thousandths
                        .map(crate::domain::weight_string)
                        .unwrap_or_else(|| line.quantity.clone()),
                    tare: line
                        .tare_thousandths
                        .map(crate::domain::weight_string)
                        .unwrap_or_else(|| "0".into()),
                    manual: if line.rule_snapshot.as_ref().map_or_else(
                        || {
                            CATALOG
                                .iter()
                                .find(|m| m.code == line.code)
                                .is_none_or(|m| m.rate_cents.is_none())
                        },
                        |r| r.rate_cents.is_none(),
                    ) {
                        line.total_cents.map_or_else(String::new, |c| {
                            let amount = money(c);
                            amount.trim_start_matches('$').replace(',', "")
                        })
                    } else {
                        String::new()
                    },
                    class: line.classification.clone(),
                    pieces: line.piece_count.map_or_else(String::new, |v| v.to_string()),
                    note: line.notes.clone(),
                });
            }
            Act::SaveEdit(index) => {
                let edit = self
                    .edit
                    .as_ref()
                    .ok_or_else(|| "Open the line before editing.".to_owned())?;
                let quantity = edit.quantity.trim().to_owned();
                let tare = edit.tare.trim().to_owned();
                let weighted = self
                    .active()
                    .lines
                    .get(index)
                    .is_some_and(|l| l.unit.as_deref() == Some("lb"));
                let amount = edit.manual.trim().to_owned();
                let class = edit.class.trim().to_owned();
                let note = edit.note.trim().to_owned();
                let pieces = edit.pieces.trim().to_owned();
                self.save_action(|t| {
                    if weighted {
                        t.edit_weight_line(index, &quantity, &tare)?;
                    } else {
                        t.line_edit(index, &quantity, &amount)?;
                    }
                    t.line_intake(index, &class, &pieces, &note)
                })?;
                self.edit = None;
            }
            Act::Workflow(action) => self.run_workflow(action)?,
            #[cfg(feature = "extended-workflows")]
            Act::Hold => {
                self.save_action(|t| t.hold())?;
                self.edit = None;
            }
            Act::Resume => self.save_action(|t| t.resume())?,
            Act::Complete => {
                #[cfg(not(feature = "extended-workflows"))]
                self.ensure_source_choice()?;
                let payment = match self.tender {
                    Tender::Cash => Payment::Cash,
                    Tender::Card => Payment::ExternalCard {
                        reference: self
                            .active()
                            .tender_evidence
                            .card_attempts
                            .last()
                            .map(|a| a.reference.clone())
                            .unwrap_or_default(),
                    },
                    Tender::Charge => Payment::ChargeTo {
                        account: self
                            .active()
                            .account_record
                            .as_ref()
                            .map(|a| a.number.clone())
                            .unwrap_or_default(),
                    },
                };
                self.save_action(move |t| t.complete(payment))?;
                self.edit = None;
                let t = self.active();
                self.receipt = Some((t.id, false, t.receipt()));
                self.print_saved(crate::printing::CopyKind::Customer)?;
            }
            Act::Receipt => {
                let t = self.active();
                self.receipt = Some((t.id, false, t.receipt()));
            }
            Act::CustomerCopy => self.print_saved(crate::printing::CopyKind::Customer)?,
            Act::Reprint => {
                self.print_saved(crate::printing::CopyKind::Internal)?;
            }
            #[cfg(not(feature = "extended-workflows"))]
            Act::Details(index) => self.open_source_details(index)?,
            #[cfg(not(feature = "extended-workflows"))]
            Act::SaveDetails => self.save_source_details()?,
            #[cfg(not(feature = "extended-workflows"))]
            Act::ScannerOpen => self.open_scanner_capture()?,
            #[cfg(not(feature = "extended-workflows"))]
            Act::ScannerReview | Act::ScannerCancel | Act::ScannerClear => {
                unreachable!("local capture cleanup is handled before authentication")
            }
            #[cfg(not(feature = "extended-workflows"))]
            action @ (Act::PrinterSettings
            | Act::PrinterSaveChoice
            | Act::PrinterDiscover
            | Act::PrinterClose
            | Act::PrinterRetry(_)
            | Act::PrinterSaveResults
            | Act::PrinterRefresh
            | Act::PrinterRecover(_)
            | Act::PrinterConfirmRecovery
            | Act::PrinterCancelRecovery) => self.run_printer_action(action)?,
        }
        Ok(())
    }
}
