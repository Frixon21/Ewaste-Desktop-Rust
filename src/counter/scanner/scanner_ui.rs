//! Explicit app-local keyboard capture. No barcode text is rendered or persisted.
use super::*;
use crate::scanner::{CaptureSession, KEYBOARD_PROFILE, MAX_CAPTURE_BYTES};
use zeroize::Zeroize;
const MANUAL_FALLBACK: &str = "Scanner input is unsupported or incomplete. Capture was cleared; look up the customer or enter details manually.";
const CAPTURE_ENDED: &str =
    "Scanner capture was cleared. Stop the scanner, then click Continue with manual entry.";
#[derive(Default)]
pub(super) struct ScannerUi {
    pub(super) capture: Option<CaptureSession>,
    pub(super) ticket_id: Option<u64>,
    pub(super) message: Option<&'static str>,
    pub(super) quarantined: bool,
}
impl ScannerUi {
    pub(super) fn clear(&mut self) {
        let active = self.active();
        self.erase_raw();
        if active {
            self.quarantine(CAPTURE_ENDED);
        }
    }
    fn erase_raw(&mut self) {
        if let Some(capture) = &mut self.capture {
            capture.clear();
        }
        self.capture = None;
        self.ticket_id = None;
    }
    pub(super) fn active(&self) -> bool {
        self.capture.is_some() || self.quarantined
    }
    fn quarantine(&mut self, message: &'static str) {
        self.erase_raw();
        self.quarantined = true;
        self.message = Some(message);
    }
    fn acknowledge(&mut self) {
        self.erase_raw();
        self.quarantined = false;
        self.message = None;
    }
    fn append(&mut self, text: &str) -> bool {
        self.capture
            .as_mut()
            .is_some_and(|capture| capture.append_text(text).is_ok())
    }
}
impl Drop for ScannerUi {
    fn drop(&mut self) {
        self.erase_raw();
    }
}
fn keyboard_event(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { .. }
            | egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Ime(_)
    )
}
fn erase_event_text(event: &mut egui::Event) {
    match event {
        egui::Event::Text(text)
        | egui::Event::Paste(text)
        | egui::Event::Ime(egui::ImeEvent::Preedit(text) | egui::ImeEvent::Commit(text)) => {
            text.zeroize()
        }
        _ => {}
    }
}
impl RegisterApp {
    pub(super) fn open_scanner_capture(&mut self) -> Result<(), String> {
        self.active().editable_intake()?;
        if self.scanner.active() {
            return Err(
                "Click Continue with manual entry or Cancel before starting another capture."
                    .into(),
            );
        }
        if self.edit.is_some()
            || self.details.is_some()
            || self.receipt.is_some()
            || self.wf.modal_open()
            || self.printer.open
            || self.printer.recovery.is_some()
        {
            return Err("Close the open form before scanning a customer.".into());
        }
        self.scanner.acknowledge();
        self.scanner.capture = Some(CaptureSession::new());
        self.scanner.ticket_id = Some(self.active);
        self.error = None;
        self.notice = None;
        Ok(())
    }
    pub(super) fn cancel_scanner_capture(&mut self) {
        self.scanner.acknowledge();
    }
    pub(super) fn review_scanner_capture(&mut self) -> Result<(), String> {
        let mut capture = self
            .scanner
            .capture
            .take()
            .ok_or("Open Scan customer before reviewing input.")?;
        let ticket_id = self.scanner.ticket_id.take();
        self.scanner.message = None;
        let permitted = ticket_id == Some(self.active) && self.active().editable_intake().is_ok();
        if !permitted {
            capture.clear();
            self.scanner.quarantine(MANUAL_FALLBACK);
            return Err(MANUAL_FALLBACK.into());
        }
        let candidate = match capture.review() {
            Ok(candidate) => candidate,
            Err(_) => {
                self.scanner.quarantine(MANUAL_FALLBACK);
                return Err(MANUAL_FALLBACK.into());
            }
        };
        // The capture has been wiped before any approved fields reach customer review.
        let review = match self.wf.directory.review_scan(&candidate) {
            Ok(review) => review,
            Err(_) => {
                self.scanner.quarantine(MANUAL_FALLBACK);
                return Err(MANUAL_FALLBACK.into());
            }
        };
        self.wf.scan_notice = review.prompt;
        self.wf.scan_matches = review.matches;
        self.query = review.customer.name.clone();
        self.wf.customer_edit = Some(review.customer);
        Ok(())
    }
    pub(super) fn scanner_capture_tick(&mut self, ctx: &egui::Context) {
        if !self.scanner.active() {
            return;
        }
        let capturing = self.scanner.capture.is_some();
        let mut failed = capturing
            && (self.scanner.ticket_id != Some(self.active)
                || !ctx.input(|input| input.raw.focused));
        let mut cancel = false;
        ctx.input_mut(|input| {
            for mut event in std::mem::take(&mut input.events) {
                match &mut event {
                    egui::Event::Text(text)
                    | egui::Event::Paste(text)
                    | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                        if capturing && !failed && !cancel && !self.scanner.append(text) {
                            failed = true;
                        }
                    }
                    egui::Event::Key {
                        key, pressed: true, ..
                    } => match key {
                        Key::Escape => cancel = true,
                        Key::Enter if capturing && !failed && !cancel => {
                            if !self.scanner.append("\n") {
                                failed = true;
                            }
                        }
                        Key::Tab if capturing && !failed && !cancel => {
                            if !self.scanner.append("\t") {
                                failed = true;
                            }
                        }
                        Key::Backspace if capturing && !failed && !cancel => {
                            if let Some(capture) = &mut self.scanner.capture {
                                capture.backspace();
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
                if keyboard_event(&event) {
                    erase_event_text(&mut event);
                } else {
                    input.events.push(event);
                }
            }
            input.raw.events.retain_mut(|event| {
                let keyboard = keyboard_event(event);
                if keyboard {
                    erase_event_text(event);
                }
                !keyboard
            });
            input.keys_down.clear();
            input.modifiers = egui::Modifiers::default();
            input.raw.modifiers = egui::Modifiers::default();
        });
        if failed || cancel {
            self.scanner.quarantine(if failed {
                MANUAL_FALLBACK
            } else {
                CAPTURE_ENDED
            });
            if failed {
                self.error = Some(MANUAL_FALLBACK.into());
            }
            ctx.request_repaint();
        }
    }
    pub(super) fn scanner_capture_panel(&mut self, ctx: &egui::Context) {
        if !self.scanner.active() {
            return;
        }
        let capturing = self.scanner.capture.is_some();
        let mut action = None;
        let response = egui::Modal::new(egui::Id::new("scanner-capture-modal")).show(ctx, |ui| {
            ui.set_max_width((ctx.content_rect().width() - 80.).clamp(280., 560.));
            if capturing {
                ui.heading("Scan customer");
                ui.strong("Keyboard-input capture");
                ui.label(format!("Supported profile: {KEYBOARD_PROFILE}"));
                ui.label("Scanner connection is not verified. Use a device configured for this profile, or enter the customer manually.");
                ui.label("Only name and address proceed to review. Barcode content stays hidden and is discarded after review.");
                let count = self.scanner.capture.as_ref().map_or(0, CaptureSession::len);
                ui.label(format!("Captured {count} of {MAX_CAPTURE_BYTES} bytes (content hidden)"));
                if self.scanner.capture.as_ref().is_some_and(|capture| !capture.is_empty()) {
                    ui.monospace("\u{2022}".repeat(count.min(32)));
                } else {
                    ui.label("Waiting for keyboard input or paste...");
                }
                ui.small("Enter remains part of the capture. Click Review when the scan is complete.");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Review captured customer").clicked() { action = Some(Act::ScannerReview); }
                    if ui.button("Clear capture").clicked() { action = Some(Act::ScannerClear); }
                    if ui.button("Cancel / manual entry").clicked() { action = Some(Act::ScannerCancel); }
                });
            } else {
                ui.heading("Scanner capture cleared");
                ui.label(self.scanner.message.unwrap_or(CAPTURE_ENDED));
                ui.label("Keyboard input remains blocked. Stop the scanner before returning to manual entry.");
                if ui.button("Continue with manual entry").clicked() { action = Some(Act::ScannerCancel); }
            }
        });
        // Escape or a backdrop click may erase capture, but cannot release later input.
        // Only a deliberate button click acknowledges the capture boundary.
        if capturing && response.backdrop_response.clicked() {
            self.scanner.quarantine(CAPTURE_ENDED);
            ctx.request_repaint();
        }
        if let Some(action) = action {
            self.error = self.run(action).err();
            ctx.request_repaint();
        }
    }
}
