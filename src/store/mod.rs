//! SQLite persistence via the Windows 11 system SQLite library.
//!
//! The native API stays private. Statements live for less time than their connection,
//! and bound text uses owned buffers retained until after statement finalization.

use crate::customers::Directory;
use crate::domain::{State, Ticket};
use crate::printing::PrintLog;
use libloading::os::windows::Library;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::ffi::{CStr, CString, c_char, c_int, c_uchar, c_void};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::rc::Rc;

type Db = *mut c_void;
type Stmt = *mut c_void;
type Destructor = Option<unsafe extern "C" fn(*mut c_void)>;

const OK: c_int = 0;
const ROW: c_int = 100;
const DONE: c_int = 101;
const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;

struct Api {
    open: unsafe extern "C" fn(*const c_char, *mut Db) -> c_int,
    open_v2: unsafe extern "C" fn(*const c_char, *mut Db, c_int, *const c_char) -> c_int,
    close: unsafe extern "C" fn(Db) -> c_int,
    errmsg: unsafe extern "C" fn(Db) -> *const c_char,
    get_autocommit: unsafe extern "C" fn(Db) -> c_int,
    busy_timeout: unsafe extern "C" fn(Db, c_int) -> c_int,
    prepare: unsafe extern "C" fn(Db, *const c_char, c_int, *mut Stmt, *mut *const c_char) -> c_int,
    bind_text: unsafe extern "C" fn(Stmt, c_int, *const c_char, c_int, Destructor) -> c_int,
    step: unsafe extern "C" fn(Stmt) -> c_int,
    column_text: unsafe extern "C" fn(Stmt, c_int) -> *const c_uchar,
    column_bytes: unsafe extern "C" fn(Stmt, c_int) -> c_int,
    column_int: unsafe extern "C" fn(Stmt, c_int) -> c_int,
    finalize: unsafe extern "C" fn(Stmt) -> c_int,
    // The pointers above may not outlive this module handle.
    _library: Library,
}

impl Api {
    fn load() -> Result<Self, String> {
        // Search only Windows' own System32, never the working directory or PATH.
        let library =
            unsafe { Library::load_with_flags("winsqlite3.dll", LOAD_LIBRARY_SEARCH_SYSTEM32) }
                .map_err(|err| format!("Could not load Windows SQLite: {err}"))?;
        unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
            // Each caller supplies the corresponding SQLite C ABI function signature.
            unsafe { library.get::<T>(name) }
                .map(|symbol| *symbol)
                .map_err(|err| format!("Windows SQLite function unavailable: {err}"))
        }
        // The API signatures match SQLite's C declarations.
        unsafe {
            Ok(Self {
                open: load_symbol(&library, b"sqlite3_open\0")?,
                open_v2: load_symbol(&library, b"sqlite3_open_v2\0")?,
                close: load_symbol(&library, b"sqlite3_close\0")?,
                errmsg: load_symbol(&library, b"sqlite3_errmsg\0")?,
                get_autocommit: load_symbol(&library, b"sqlite3_get_autocommit\0")?,
                busy_timeout: load_symbol(&library, b"sqlite3_busy_timeout\0")?,
                prepare: load_symbol(&library, b"sqlite3_prepare_v2\0")?,
                bind_text: load_symbol(&library, b"sqlite3_bind_text\0")?,
                step: load_symbol(&library, b"sqlite3_step\0")?,
                column_text: load_symbol(&library, b"sqlite3_column_text\0")?,
                column_bytes: load_symbol(&library, b"sqlite3_column_bytes\0")?,
                column_int: load_symbol(&library, b"sqlite3_column_int\0")?,
                finalize: load_symbol(&library, b"sqlite3_finalize\0")?,
                _library: library,
            })
        }
    }
}

pub struct Store {
    api: Api,
    db: NonNull<c_void>,
    #[cfg(any(test, not(feature = "extended-workflows")))]
    dispatch_binding: std::sync::Arc<()>,
    #[cfg(feature = "extended-workflows")]
    path: std::path::PathBuf,
    _ui_thread_only: PhantomData<Rc<()>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        let store = Self::connect(path, false)?;
        store.migrate()?;
        #[cfg(feature = "extended-workflows")]
        store.validate_security_state()?;
        #[cfg(not(feature = "extended-workflows"))]
        store.security_audit()?;
        store.validate_optional_workflow_records()?;
        Ok(store)
    }

    fn validate_optional_workflow_records(&self) -> Result<(), String> {
        self.receipt_dispatch_log()?;
        self.receipt_printer_queue()?;
        #[cfg(feature = "extended-workflows")]
        {
            self.closeouts()?;
            self.intake_reviews()?;
            self.adjustments()?;
            self.price_overrides()?;
            self.inventory_ledger()?;
        }
        #[cfg(not(feature = "extended-workflows"))]
        self.validate_compat_records()?;
        Ok(())
    }

    fn completion_is_closed(&self, at: u64) -> Result<bool, String> {
        #[cfg(feature = "extended-workflows")]
        {
            Ok(self
                .closeouts()?
                .iter()
                .any(|record| record.report.start <= at && at < record.report.end))
        }
        #[cfg(not(feature = "extended-workflows"))]
        {
            Ok(self
                .closed_periods()?
                .iter()
                .any(|(start, end)| *start <= at && at < *end))
        }
    }

    fn connect(path: &Path, readonly: bool) -> Result<Self, String> {
        #[cfg(feature = "extended-workflows")]
        let absolute_path = std::path::absolute(path)
            .map_err(|error| format!("Could not resolve database path: {error}"))?;
        let name = path
            .to_str()
            .ok_or("The database path is not valid Unicode.")?;
        let name = CString::new(name).map_err(|_| "The database path contains a NUL character.")?;
        let api = Api::load()?;
        let mut db = ptr::null_mut();
        // SQLite may provide a handle even on open failure; the Drop implementation closes it.
        let status = if readonly {
            // SQLITE_OPEN_READONLY does not create a missing file or permit
            // migrations/writes against a selected backup source.
            unsafe { (api.open_v2)(name.as_ptr(), &mut db, 0x0000_0001, ptr::null()) }
        } else {
            unsafe { (api.open)(name.as_ptr(), &mut db) }
        };
        let db =
            NonNull::new(db).ok_or_else(|| format!("Could not open SQLite (error {status})."))?;
        let store = Self {
            api,
            db,
            #[cfg(any(test, not(feature = "extended-workflows")))]
            dispatch_binding: std::sync::Arc::new(()),
            #[cfg(feature = "extended-workflows")]
            path: absolute_path,
            _ui_thread_only: PhantomData,
        };
        store.check(status, "Could not open the counter database")?;
        store.check(
            unsafe { (store.api.busy_timeout)(store.db.as_ptr(), 3000) },
            "Could not configure the database timeout",
        )?;
        Ok(store)
    }

    #[cfg(feature = "extended-workflows")]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn check(&self, status: c_int, action: &str) -> Result<(), String> {
        if status == OK {
            Ok(())
        } else {
            Err(self.error(status, action))
        }
    }

    fn error(&self, status: c_int, action: &str) -> String {
        let message = unsafe { (self.api.errmsg)(self.db.as_ptr()) };
        if message.is_null() {
            format!("{action} (SQLite error {status}).")
        } else {
            // SQLite's error message is valid until the next call on this connection.
            format!(
                "{action}: {} (SQLite {status}).",
                unsafe { CStr::from_ptr(message) }.to_string_lossy()
            )
        }
    }

    fn statement(&self, sql: &str) -> Result<Statement<'_>, String> {
        let sql = CString::new(sql).map_err(|_| "Invalid internal SQL.")?;
        let mut stmt = ptr::null_mut();
        let status = unsafe {
            (self.api.prepare)(
                self.db.as_ptr(),
                sql.as_ptr(),
                -1,
                &mut stmt,
                ptr::null_mut(),
            )
        };
        self.check(status, "Could not prepare a database statement")?;
        let stmt = NonNull::new(stmt).ok_or("Could not prepare an empty database statement.")?;
        Ok(Statement {
            store: self,
            stmt,
            bound: Vec::new(),
        })
    }

    fn execute(&self, sql: &str) -> Result<(), String> {
        self.statement(sql)?.finish()
    }

    fn transaction<T>(&self, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        // Register services own the outer write transaction. Nested private
        // persistence commands propagate errors into that transaction's rollback.
        if unsafe { (self.api.get_autocommit)(self.db.as_ptr()) } == 0 {
            return operation();
        }
        self.execute("BEGIN IMMEDIATE")?;
        match operation().and_then(|answer| self.execute("COMMIT").map(|()| answer)) {
            Ok(answer) => Ok(answer),
            Err(err) => match self.execute("ROLLBACK") {
                Ok(()) => Err(err),
                Err(rollback) => Err(format!("{err} Rollback also failed: {rollback}")),
            },
        }
    }

    fn migrate(&self) -> Result<(), String> {
        self.transaction(|| {
            let version = {
                let stmt = self.statement("PRAGMA user_version")?;
                if stmt.step()? != ROW {
                    return Err("Could not read the local database version.".into());
                }
                unsafe { (self.api.column_int)(stmt.stmt.as_ptr(), 0) }
            };
            match version {
                0=>{self.execute("CREATE TABLE tickets (id TEXT PRIMARY KEY NOT NULL, created_at TEXT NOT NULL, payload TEXT NOT NULL)")?;self.create_app_state()?;},
                1=>self.create_app_state()?,
                2|3=>(),
                4=>(),
                5=>(),
                6=>(),
                7=>return Ok(()),
                _=>return Err(format!("This counter database uses a newer unsupported version ({version}).")),
            }
            if version<4{self.initialize_v4_records()?;}
            if version<5{self.initialize_state("price_overrides",&serde_json::json!({"events":[],"next_id":1}))?;}
            if version<6 {
                self.initialize_state("inventory_directory",&serde_json::json!({"locations":[{"id":1,"name":"Training staging","active":true}],"next_id":2}))?;
                self.initialize_state("inventory_ledger",&serde_json::json!({"events":[],"next_id":1,"next_lot_id":1,"next_shipment_id":1}))?;
            }
            self.initialize_state("receipt_dispatches", &serde_json::json!({"jobs":[],"next_id":1}))?;
            self.initialize_state("printer_settings", &serde_json::json!({"queue":null}))?;
            self.execute("PRAGMA user_version = 7")
        })
    }

    fn create_app_state(&self) -> Result<(), String> {
        self.execute(
            "CREATE TABLE app_state (key TEXT PRIMARY KEY NOT NULL, payload TEXT NOT NULL)",
        )
    }

    fn read_state<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T, String> {
        let mut statement = self.statement("SELECT payload FROM app_state WHERE key = ?1")?;
        statement.text(1, key)?;
        if statement.step()? != ROW {
            return Ok(T::default());
        }
        let record: LogicalRecord<T> = serde_json::from_str(&statement.read_text(0)?)
            .map_err(|error| format!("Could not read saved {key}: {error}"))?;
        if record.version != 1 {
            return Err(format!("Saved {key} uses an unsupported record version."));
        }
        Ok(record.data)
    }

    fn write_state<T: Serialize>(&self, key: &str, data: &T) -> Result<(), String> {
        let payload = serde_json::to_string(&LogicalRecord { version: 1, data })
            .map_err(|error| format!("Could not encode {key}: {error}"))?;
        let mut statement = self.statement("INSERT INTO app_state(key,payload) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET payload=excluded.payload")?;
        statement.text(1, key)?;
        statement.text(2, &payload)?;
        statement.finish()
    }

    /// Migration defaults never replace data already recorded by another build.
    fn initialize_state<T: Serialize>(&self, key: &str, data: &T) -> Result<(), String> {
        let payload = serde_json::to_string(&LogicalRecord { version: 1, data })
            .map_err(|error| format!("Could not encode {key}: {error}"))?;
        let mut statement = self.statement(
            "INSERT INTO app_state(key,payload) VALUES (?1,?2) ON CONFLICT(key) DO NOTHING",
        )?;
        statement.text(1, key)?;
        statement.text(2, &payload)?;
        statement.finish()
    }

    pub fn directory(&self) -> Result<Directory, String> {
        let directory: Directory = self.read_state("directory")?;
        directory.validate()?;
        Ok(directory)
    }

    fn save_directory_unchecked(
        &self,
        previous: &Directory,
        next: &Directory,
    ) -> Result<(), String> {
        next.validate()?;
        self.transaction(|| {
            let current = self.directory()?;
            if &current == next {
                return Ok(());
            }
            if &current != previous {
                return Err("The customer directory changed. Reopen it before saving.".into());
            }
            self.write_state("directory", next)
        })
    }

    pub fn print_log(&self) -> Result<PrintLog, String> {
        let log: PrintLog = self.read_state("print_log")?;
        log.validate()?;
        Ok(log)
    }

    fn save_print_log_unchecked(&self, previous: &PrintLog, next: &PrintLog) -> Result<(), String> {
        next.validate()?;
        self.transaction(|| {
            let current = self.print_log()?;
            if &current == next {
                return Ok(());
            }
            if &current != previous {
                return Err(
                    "The print history changed. Reopen the saved receipt before retrying.".into(),
                );
            }
            self.write_state("print_log", next)
        })
    }

    pub fn catalog(&self) -> Result<crate::catalog::Catalog, String> {
        let catalog: crate::catalog::Catalog = self.read_state("catalog")?;
        catalog.validate()?;
        Ok(catalog)
    }
    #[cfg(any(test, feature = "extended-workflows"))]
    fn save_catalog_unchecked(
        &self,
        previous: &crate::catalog::Catalog,
        next: &crate::catalog::Catalog,
    ) -> Result<(), String> {
        next.validate()?;
        self.transaction(|| {
            let current = self.catalog()?;
            if &current == next {
                return Ok(());
            }
            if &current != previous {
                return Err("Tariffs changed. Reopen settings before saving.".into());
            }
            self.write_state("catalog", next)
        })
    }
    #[cfg(feature = "extended-workflows")]
    pub fn closeouts(&self) -> Result<Vec<crate::closeout::Closeout>, String> {
        let data: Vec<crate::closeout::Closeout> = self.read_state("closeouts")?;
        for record in &data {
            record.validate()?;
        }
        for (index, record) in data.iter().enumerate() {
            if data.iter().skip(index + 1).any(|other| {
                record.report.start < other.report.end && other.report.start < record.report.end
            }) {
                return Err("Saved closeout periods overlap. Review the register database.".into());
            }
        }
        Ok(data)
    }
    #[cfg(feature = "extended-workflows")]
    fn save_closeout_unchecked(&self, record: &crate::closeout::Closeout) -> Result<(), String> {
        record.validate()?;
        self.transaction(|| {
            let mut records = self.closeouts()?;
            if let Some(existing) = records.iter().find(|r| r.id == record.id) {
                return if existing == record {
                    Ok(())
                } else {
                    Err("This shift was already closed with different declarations.".into())
                };
            }
            if records.iter().any(|other| {
                record.report.start < other.report.end && other.report.start < record.report.end
            }) {
                return Err(
                    "This period overlaps a saved closeout. Start after the previous closeout."
                        .into(),
                );
            }
            let current = if record.report.calculation_version == 1 {
                crate::closeout::ShiftReport::from_tickets(
                    &self.list()?,
                    record.report.start,
                    record.report.end,
                )?
            } else {
                crate::closeout::ShiftReport::from_tickets_and_adjustments(
                    &self.list()?,
                    &self.adjustments()?,
                    record.report.start,
                    record.report.end,
                )?
            };
            if current != record.report {
                return Err("Tickets changed while reconciling. Refresh the shift report.".into());
            }
            records.push(record.clone());
            self.write_state("closeouts", &records)
        })
    }

    #[cfg(feature = "extended-workflows")]
    pub fn intake_reviews(&self) -> Result<crate::intake_review::ReviewLedger, String> {
        let ledger: crate::intake_review::ReviewLedger = self.read_state("intake_reviews")?;
        ledger.validate_against(&self.list()?)?;
        Ok(ledger)
    }
    #[cfg(feature = "extended-workflows")]
    fn save_intake_reviews_unchecked(
        &self,
        previous: &crate::intake_review::ReviewLedger,
        next: &crate::intake_review::ReviewLedger,
    ) -> Result<(), String> {
        self.transaction(|| {
            let current=self.intake_reviews()?;
            if &current==next {return Ok(());}
            if &current!=previous {return Err("Source review history changed. Reload before recording another review.".into());}
            if next.events.len()!=current.events.len().checked_add(1).ok_or("Source review limit reached.")?
                || !next.events.starts_with(&current.events) {
                return Err("Source reviews are append-only. Record one new review without changing prior events.".into());
            }
            next.validate_against(&self.list()?)?;
            self.write_state("intake_reviews",next)
        })
    }

    fn save_unchecked(&self, ticket: &Ticket) -> Result<(), String> {
        ticket.validate_intake_records()?;
        // Fixed-width decimal encodings preserve full u64 sort order in SQL TEXT.
        let id = format!("{:020}", ticket.id);
        let created_at = format!("{:020}", ticket.created_at);
        let payload = serde_json::to_string(ticket)
            .map_err(|err| format!("Could not encode the ticket: {err}"))?;
        self.transaction(|| {
            #[cfg(not(feature = "extended-workflows"))]
            let mut preserved_payload = None;
            let mut existing_completed_receipt = false;
            {
                let mut previous = self.statement("SELECT payload FROM tickets WHERE id = ?1")?;
                previous.text(1, &id)?;
                if previous.step()? == ROW {
                    let original_payload = previous.read_text(0)?;
                    let old: Ticket = serde_json::from_str(&original_payload)
                        .map_err(|err| format!("Could not read stored ticket {id}: {err}"))?;
                    if old.id != ticket.id || old.created_at != ticket.created_at {
                        return Err("A different ticket already has this number. Reopen the counter.".into());
                    }
                    if &old == ticket { return Ok(()); }
                    #[cfg(not(feature = "extended-workflows"))]
                    {
                        self.validate_walkthrough_preservation(Some(&old), ticket)?;
                        let original: serde_json::Value = serde_json::from_str(&original_payload).map_err(|_| "Saved ticket JSON is invalid.")?;
                        let known = serde_json::to_value(&old).map_err(|_| "Could not inspect the saved ticket fields.")?;
                        if old.lines != ticket.lines && old.lines.iter().enumerate().any(|(index, line)| {
                            line.id == 0 && original.get("lines").and_then(|lines| lines.get(index)).zip(known.get("lines").and_then(|lines| lines.get(index)))
                                .is_some_and(|(raw, known)| crate::record_compat::has_unknown_fields(raw, known))
                        }) {
                            return Err("This legacy draft contains unmapped line fields. Its material lines are read-only to preserve the saved data.".into());
                        }
                        let mut next: serde_json::Value = serde_json::from_str(&payload).map_err(|_| "Could not inspect the new ticket fields.")?;
                        crate::record_compat::preserve_unknown_fields(&mut next, &original);
                        preserved_payload = Some(serde_json::to_string(&next).map_err(|_| "Could not preserve the saved ticket fields.")?);
                    }
                    for line in ticket.lines.iter().filter(|l|l.id>0) {
                        if !old.lines.iter().any(|previous|previous.id==line.id) && line.id<old.next_line_id {
                            return Err("A deleted material line identity cannot be reused.".into());
                        }
                    }
                    let mut legacy=std::collections::HashMap::<&str,usize>::new();
                    for line in old.lines.iter().filter(|l|l.id==0) {*legacy.entry(&line.code).or_default()+=1;}
                    for line in ticket.lines.iter().filter(|l|l.id==0) {
                        let count=legacy.get_mut(line.code.as_str()).ok_or("New material lines require stable identities.")?;
                        if *count==0 {return Err("New material lines require stable identities.".into());}
                        *count-=1;
                    }
                    if ticket.next_line_id<old.next_line_id {
                        return Err("Material line identifiers cannot be reused. Reopen the ticket.".into());
                    }
                    for line in ticket.lines.iter().filter(|l|l.id>0) {
                        if old.lines.iter().any(|previous|previous.id==line.id && previous.code!=line.code) {
                            return Err("A material line identity cannot be reassigned to another material.".into());
                        }
                    }
                    if old.revision.checked_add(1) != Some(ticket.revision) {
                        return Err("This ticket changed in another operation. Reopen it before editing.".into());
                    }
                    if old.state == State::Completed {
                        existing_completed_receipt = true;
                        let mut reprint = old;
                        reprint.reprint()?;
                        reprint.revision = ticket.revision;
                        if &reprint != ticket {
                            return Err("This completed ticket has changed or is read-only. Reopen the counter.".into());
                        }
                    }
                } else {
                    if ticket.revision!=0 {return Err("A new ticket must begin at revision zero.".into());}
                    if ticket.lines.iter().any(|l|l.id==0) {return Err("New material lines require stable identities.".into());}
                }
            }
            if ticket.state == State::Completed && !existing_completed_receipt {
                let payment = ticket.payment.as_ref().ok_or("A completed ticket requires verified payment or a billing account.")?;
                ticket.validate_completion(payment)?;
                let completed=ticket.completed_at.ok_or("A completed ticket requires its completion timestamp.")?;
                if self.completion_is_closed(completed)? {
                    return Err("This ticket completion falls inside a closed shift. Refresh and reconcile the register before continuing.".into());
                }
            }
            self.reject_reused_card_reference(ticket)?;
            let mut stmt = self.statement(
                "INSERT INTO tickets(id, created_at, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
            )?;
            stmt.text(1, &id)?;
            stmt.text(2, &created_at)?;
            #[cfg(not(feature = "extended-workflows"))]
            let payload = preserved_payload.as_deref().unwrap_or(&payload);
            #[cfg(feature = "extended-workflows")]
            let payload = payload.as_str();
            stmt.text(3, payload)?;
            stmt.finish()
        })
    }

    fn reject_reused_card_reference(&self, ticket: &Ticket) -> Result<(), String> {
        use crate::domain::Payment;
        let references: Vec<&str> = ticket
            .tender_evidence
            .card_attempts
            .iter()
            .filter(|attempt| !attempt.reference.is_empty())
            .map(|attempt| attempt.reference.as_str())
            .collect();
        if references.is_empty() {
            return Ok(());
        }
        for other in self.list()?.iter().filter(|other| other.id != ticket.id) {
            let legacy_reference = match &other.payment {
                Some(Payment::ExternalCard { reference }) => references
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(reference)),
                _ => false,
            };
            if legacy_reference
                || other.tender_evidence.card_attempts.iter().any(|attempt| {
                    references
                        .iter()
                        .any(|reference| reference.eq_ignore_ascii_case(&attempt.reference))
                })
            {
                return Err("That external card reference is already attached to another ticket. Reconcile the original payment.".into());
            }
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Ticket>, String> {
        let stmt = self.statement(
            "SELECT id, created_at, payload FROM tickets ORDER BY created_at DESC, id DESC",
        )?;
        let mut data = Vec::new();
        while stmt.step()? == ROW {
            let id = stmt.read_text(0)?;
            let created_at = stmt.read_text(1)?;
            let payload = stmt.read_text(2)?;
            let ticket: Ticket = serde_json::from_str(&payload)
                .map_err(|err| format!("Could not read stored ticket {id}: {err}"))?;
            if id.parse::<u64>().ok() != Some(ticket.id)
                || created_at.parse::<u64>().ok() != Some(ticket.created_at)
            {
                return Err(format!(
                    "Stored ticket metadata does not match ticket {id}."
                ));
            }
            ticket.validate_intake_records()?;
            data.push(ticket);
        }
        Ok(data)
    }
}

#[derive(Serialize, Deserialize)]
struct LogicalRecord<T> {
    version: u32,
    data: T,
}

impl Drop for Store {
    fn drop(&mut self) {
        // Statement borrows ensure no statement remains active here.
        unsafe { (self.api.close)(self.db.as_ptr()) };
    }
}

struct Statement<'db> {
    store: &'db Store,
    stmt: NonNull<c_void>,
    bound: Vec<CString>,
}

impl Statement<'_> {
    fn text(&mut self, index: c_int, value: &str) -> Result<(), String> {
        let value =
            CString::new(value).map_err(|_| "Invalid NUL character in database parameter.")?;
        // With SQLITE_STATIC (None) the buffer is retained until after finalize.
        let pointer = value.as_ptr();
        self.bound.push(value);
        let status =
            unsafe { (self.store.api.bind_text)(self.stmt.as_ptr(), index, pointer, -1, None) };
        self.store
            .check(status, "Could not bind a database parameter")
    }

    fn step(&self) -> Result<c_int, String> {
        let status = unsafe { (self.store.api.step)(self.stmt.as_ptr()) };
        match status {
            ROW | DONE => Ok(status),
            _ => Err(self
                .store
                .error(status, "Could not run a database statement")),
        }
    }

    fn finish(&self) -> Result<(), String> {
        if self.step()? == DONE {
            Ok(())
        } else {
            Err("A database write unexpectedly returned rows.".into())
        }
    }

    fn read_text(&self, index: c_int) -> Result<String, String> {
        let text = unsafe { (self.store.api.column_text)(self.stmt.as_ptr(), index) };
        if text.is_null() {
            return Err("A required ticket column was missing.".into());
        }
        // SQLite's text pointer is valid until the statement advances or finalizes.
        let len = unsafe { (self.store.api.column_bytes)(self.stmt.as_ptr(), index) };
        let data = unsafe { std::slice::from_raw_parts(text, len as usize) };
        std::str::from_utf8(data)
            .map(str::to_owned)
            .map_err(|_| "A stored ticket field is not valid UTF-8.".into())
    }
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        unsafe { (self.store.api.finalize)(self.stmt.as_ptr()) };
    }
}
#[cfg(test)]
#[path = "../tests/store/store_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/store/store_metadata_tests.rs"]
mod metadata_tests;

#[path = "recovery.rs"]
mod recovery;
pub use recovery::restore_copy;

#[cfg(all(test, feature = "extended-workflows"))]
#[path = "../tests/extended/intake/store_intake_tests.rs"]
mod intake_tests;

#[cfg(all(test, feature = "extended-workflows"))]
#[path = "../tests/extended/auth/store_security_tests.rs"]
mod security_tests;

#[cfg(feature = "extended-workflows")]
#[path = "../extended/auth/store_auth.rs"]
mod authentication;
#[path = "permissions.rs"]
mod permissions;
#[path = "records.rs"]
mod records;

#[path = "receipt_delivery.rs"]
mod receipt_delivery;
#[cfg(any(test, not(feature = "extended-workflows")))]
pub use receipt_delivery::{PrepareDispatch, PreparedDispatch};

#[cfg(test)]
#[path = "../tests/store/store_receipt_dispatch_compat_tests.rs"]
mod receipt_delivery_compat_tests;
#[cfg(test)]
#[path = "../tests/store/store_receipt_dispatch_tests.rs"]
mod receipt_delivery_tests;
#[cfg(test)]
#[path = "../tests/store/store_test_helpers.rs"]
mod test_helpers;

#[cfg(feature = "extended-workflows")]
#[path = "../extended/finance/store_adjustments.rs"]
mod adjustments;

#[cfg(feature = "extended-workflows")]
#[path = "../extended/pricing/store_price_overrides.rs"]
mod price_approvals;

#[cfg(feature = "extended-workflows")]
#[path = "../extended/inventory/store_inventory.rs"]
mod stock;

#[cfg(all(test, feature = "extended-workflows"))]
#[path = "../tests/extended/finance/store_adjustments_tests.rs"]
mod adjustment_tests;

#[cfg(all(test, feature = "extended-workflows"))]
#[path = "../tests/extended/finance/store_closeout_security_tests.rs"]
mod closeout_security_tests;

#[cfg(all(test, feature = "extended-workflows"))]
#[path = "../tests/extended/pricing/store_price_override_tests.rs"]
mod price_override_tests;

#[cfg(all(test, feature = "extended-workflows"))]
#[path = "../tests/extended/inventory/store_inventory_tests.rs"]
mod inventory_tests;

#[cfg(all(test, not(feature = "extended-workflows")))]
#[path = "../tests/store/compatibility_tests.rs"]
mod compatibility_tests;

#[cfg(not(feature = "extended-workflows"))]
#[path = "compatibility.rs"]
mod compatibility;
