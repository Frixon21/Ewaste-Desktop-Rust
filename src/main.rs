#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// Shared counter logic and persistence.
#[path = "shared/model/catalog.rs"]
mod catalog;
#[path = "shared/model/counter_validation.rs"]
mod counter_validation;
#[path = "shared/model/customers.rs"]
mod customers;
#[path = "shared/model/domain.rs"]
mod domain;
#[path = "shared/model/intake.rs"]
mod intake;
#[path = "shared/model/operator_snapshot.rs"]
mod operator_snapshot;
#[path = "shared/receipts/printing.rs"]
mod printing;
#[path = "shared/receipts/receipt_dispatch.rs"]
mod receipt_dispatch;
#[path = "shared/model/record_compat.rs"]
mod record_compat;
#[path = "store/security_audit.rs"]
mod security_audit;
mod store;
#[path = "shared/model/tender.rs"]
mod tender;
#[path = "shared/model/workflow.rs"]
mod workflow;

// Basic register and its devices.
#[cfg(not(feature = "extended-workflows"))]
#[path = "counter/walkthrough_price_snapshot.rs"]
mod price_overrides;
#[cfg(not(feature = "extended-workflows"))]
#[path = "counter/printer/printer_windows.rs"]
mod printer_windows;
#[cfg(not(feature = "extended-workflows"))]
#[path = "counter/scanner/scanner.rs"]
mod scanner;
#[cfg(not(feature = "extended-workflows"))]
#[path = "counter/walkthrough_ui.rs"]
mod ui;

// Optional register and extended workflows.
#[cfg(feature = "extended-workflows")]
#[path = "extended/finance/adjustment_ui.rs"]
mod adjustment_ui;
#[cfg(feature = "extended-workflows")]
#[path = "extended/finance/adjustments.rs"]
mod adjustments;
#[cfg(feature = "extended-workflows")]
#[path = "extended/auth/auth.rs"]
mod auth;
#[cfg(feature = "extended-workflows")]
#[path = "extended/auth/auth_ui.rs"]
mod auth_ui;
#[cfg(feature = "extended-workflows")]
#[path = "extended/pricing/catalog_ui.rs"]
mod catalog_ui;
#[cfg(feature = "extended-workflows")]
#[path = "extended/finance/closeout.rs"]
mod closeout;
#[cfg(feature = "extended-workflows")]
#[path = "extended/finance/closeout_ui.rs"]
mod closeout_ui;
#[cfg(feature = "extended-workflows")]
#[path = "extended/intake/intake_export.rs"]
mod intake_export;
#[cfg(feature = "extended-workflows")]
#[path = "extended/intake/intake_review.rs"]
mod intake_review;
#[cfg(feature = "extended-workflows")]
#[path = "extended/intake/intake_ui.rs"]
mod intake_ui;
#[cfg(feature = "extended-workflows")]
#[path = "extended/inventory/inventory.rs"]
mod inventory;
#[cfg(feature = "extended-workflows")]
#[path = "extended/inventory/inventory_export.rs"]
mod inventory_export;
#[cfg(feature = "extended-workflows")]
#[path = "extended/inventory/inventory_ui.rs"]
mod inventory_ui;
#[cfg(feature = "extended-workflows")]
#[path = "extended/pricing/price_override_ui.rs"]
mod price_override_ui;
#[cfg(feature = "extended-workflows")]
#[path = "extended/pricing/price_overrides.rs"]
mod price_overrides;
#[cfg(feature = "extended-workflows")]
#[path = "extended/ui.rs"]
mod ui;

// Tests that exercise more than one top-level module.
#[cfg(all(test, feature = "extended-workflows"))]
#[path = "tests/extended/intake/intake_integration_tests.rs"]
mod intake_integration_tests;
#[cfg(all(test, feature = "extended-workflows"))]
#[path = "tests/extended/pricing/price_integration_tests.rs"]
mod price_integration_tests;
#[cfg(test)]
#[path = "tests/shared/workflow_tests.rs"]
mod workflow_tests;

use std::{error::Error, io, path::PathBuf};

fn path_for_counter() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("EWASTE_COUNTER_DB") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return Ok(path);
        }
        return Err("EWASTE_COUNTER_DB must be an absolute path.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or("Could not find the executable folder.")?;
    Ok(dir.join("data").join("counter.sqlite3"))
}

fn register() -> Result<(), Box<dyn Error>> {
    let path = path_for_counter().map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let store = store::Store::open(&path).map_err(io::Error::other)?;
    let app = ui::RegisterApp::new(store).map_err(io::Error::other)?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1250., 780.])
            .with_min_inner_size([820., 570.]),
        ..Default::default()
    };
    eframe::run_native(
        "E-waste Counter",
        options,
        Box::new(move |_| Ok(Box::new(app))),
    )
    .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(())
}

fn command_line() -> Result<bool, String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        return Ok(false);
    }
    if args.len() == 2 && args[0] == "--backup" {
        let source = path_for_counter()?;
        let destination = PathBuf::from(&args[1]);
        store::Store::open_readonly(&source)?.backup(&destination)?;
        return Ok(true);
    }
    if args.len() == 4 && args[0] == "--restore-copy" && args[2] == "--to" {
        store::restore_copy(&PathBuf::from(&args[1]), &PathBuf::from(&args[3]))?;
        return Ok(true);
    }
    Err("Usage: EwasteCounter.exe [--backup NEW_ABSOLUTE_PATH | --restore-copy BACKUP_PATH --to NEW_ABSOLUTE_PATH]".into())
}
fn main() {
    let result = command_line()
        .map_err(io::Error::other)
        .map_err(|e| Box::new(e) as Box<dyn Error>)
        .and_then(|handled| if handled { Ok(()) } else { register() });
    if let Err(error) = result {
        eprintln!("Could not start E-waste Counter: {error}");
        if let Ok(path) = path_for_counter() {
            let startup_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let _ = std::fs::create_dir_all(startup_dir);
            let _ = std::fs::write(startup_dir.join("startup-error.txt"), error.to_string());
        }
        std::process::exit(1);
    }
}
