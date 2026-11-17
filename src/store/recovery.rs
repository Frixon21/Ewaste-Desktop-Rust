//! Consistent local backups and restore rehearsals into a new file. Restoring
//! never changes the source or replaces the database used by the running app.

use super::*;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;

fn checked_path(path: &Path) -> Result<&str, String> {
    if !path.is_absolute() {
        return Err("Use an absolute database file path.".into());
    }
    let text = path
        .to_str()
        .ok_or("The database path is not valid Unicode.")?;
    CString::new(text).map_err(|_| "The database path contains a NUL character.")?;
    Ok(text)
}

fn companion_path(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

fn refuse_journals(source: &Path) -> Result<(), String> {
    for suffix in ["-journal", "-wal"] {
        let companion = companion_path(source, suffix);
        if companion
            .try_exists()
            .map_err(|error| format!("Could not check the backup's journal files: {error}"))?
        {
            return Err("Restore requires a standalone backup without journal or WAL files. Create a fresh backup from the register.".into());
        }
    }
    Ok(())
}

fn retained(target: &Path, error: impl std::fmt::Display) -> String {
    format!(
        "{error} The new output was retained for inspection at {}. Use a different new output path to retry.",
        target.display()
    )
}

fn validate_snapshot(path: &Path) -> Result<(), String> {
    // SQLite accepts an empty file as a new database. Require a real snapshot
    // before opening the copy, so a failed backup cannot become an empty restore.
    let mut header = [0u8; 16];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| format!("The output is not a complete SQLite snapshot: {error}"))?;
    if &header != b"SQLite format 3\0" {
        return Err("The output is not a SQLite database snapshot.".into());
    }
    // Recognize an existing register schema before any migration. Otherwise an
    // unrelated SQLite file with user_version=0 could become an empty register.
    let recognized = Store::open_readonly(path)?;
    drop(recognized);
    let store = Store::open(path)?;
    let statement = store.statement("PRAGMA quick_check")?;
    if statement.step()? != ROW || statement.read_text(0)? != "ok" || statement.step()? != DONE {
        return Err("The copied database failed its SQLite integrity check.".into());
    }
    store.list()?;
    store.directory()?;
    store.catalog()?;
    store.print_log()?;
    Ok(())
}

impl Store {
    /// Open an existing supported register database without creating or migrating
    /// the source. The backup output may subsequently migrate independently.
    pub fn open_readonly(path: &Path) -> Result<Self, String> {
        let store = Self::connect(path, true)?;
        {
            let statement = store.statement("PRAGMA user_version")?;
            if statement.step()? != ROW {
                return Err("Could not read the backup source database version.".into());
            }
            let version = unsafe { (store.api.column_int)(statement.stmt.as_ptr(), 0) };
            if !matches!(version, 1..=7) {
                return Err(format!(
                    "Choose an existing supported register database (source version {version})."
                ));
            }
            if version == 7 {
                store.receipt_dispatch_log()?;
                store.receipt_printer_queue()?;
            }
        }
        store.list()?;
        Ok(store)
    }

    /// Create a consistent SQLite snapshot at a new absolute path. Reserving an
    /// empty file with create_new prevents overwriting even an existing empty file.
    pub fn backup(&self, target: &Path) -> Result<(), String> {
        let target_text = checked_path(target)?;
        let reservation = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(3) // Allow SQLite's read/write access, deny deletion/replacement.
            .open(target)
            .map_err(|error| format!("Backup requires a new output file: {error}"))?;
        let operation = (|| {
            let mut statement = self.statement("VACUUM INTO ?1")?;
            statement.text(1, target_text)?;
            statement.finish()?;
            reservation
                .sync_all()
                .map_err(|error| format!("Could not sync the backup: {error}"))
        })();
        drop(reservation);
        operation.map_err(|error| retained(target, error))?;
        validate_snapshot(target).map_err(|error| retained(target, error))
    }
}

/// Rehearse recovery into a new database file. The source is read-only and held
/// without write/delete sharing during the copy, so an active register is refused.
/// Invalid/partial outputs are retained for diagnosis and never replace live data.
pub fn restore_copy(source: &Path, target: &Path) -> Result<(), String> {
    checked_path(source)?;
    checked_path(target)?;
    refuse_journals(source)?;
    let mut input = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(source)
        .map_err(|error| format!("Could not read a closed standalone backup: {error}"))?;
    if !input
        .metadata()
        .map_err(|error| format!("Could not inspect the backup: {error}"))?
        .is_file()
    {
        return Err("Choose a standalone backup file.".into());
    }
    // Recheck after acquiring the read-only handle that excludes new writers.
    refuse_journals(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(target)
        .map_err(|error| format!("Restore requires a new output file: {error}"))?;
    let copied = io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    drop(output);
    drop(input);
    copied.map_err(|error| retained(target, format!("Could not copy the backup: {error}")))?;
    validate_snapshot(target).map_err(|error| retained(target, error))
}

#[cfg(all(test, feature = "extended-workflows"))]
#[path = "../tests/store/recovery_tests.rs"]
mod tests;
