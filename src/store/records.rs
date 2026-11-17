use super::*;
use crate::{
    operator_snapshot::OperatorSnapshot,
    security_audit::{AuditAction, AuditLedger},
};

impl Store {
    pub(super) fn read_required<T: DeserializeOwned>(&self, key: &str) -> Result<T, String> {
        let mut statement = self.statement("SELECT payload FROM app_state WHERE key=?1")?;
        statement.text(1, key)?;
        if statement.step()? != ROW {
            return Err(format!(
                "Required register metadata '{key}' is missing. Restore a verified backup."
            ));
        }
        let record: LogicalRecord<T> = serde_json::from_str(&statement.read_text(0)?)
            .map_err(|_| format!("Saved {key} is invalid."))?;
        if record.version != 1 {
            return Err(format!("Saved {key} uses an unsupported record version."));
        }
        Ok(record.data)
    }

    pub(super) fn initialize_v4_records(&self) -> Result<(), String> {
        // Keep the schema readable by extended builds. The basic counter uses
        // neither the credential directory nor its setup and throttle records.
        self.initialize_state(
            "auth",
            &serde_json::json!({"operators":[],"next_id":1,"register_id":""}),
        )?;
        self.initialize_state("auth_setup", &"NeedsBootstrap")?;
        self.initialize_state(
            "login_throttle",
            &serde_json::json!({"failures":0,"blocked_until":0}),
        )?;
        self.initialize_state("security_audit", &AuditLedger::default())?;
        self.initialize_state("adjustments", &serde_json::json!({"events":[],"next_id":1}))
    }

    pub fn security_audit(&self) -> Result<AuditLedger, String> {
        let ledger: AuditLedger = self.read_required("security_audit")?;
        ledger.validate()?;
        Ok(ledger)
    }

    pub(super) fn append_audit(
        &self,
        actor: Option<&OperatorSnapshot>,
        action: AuditAction,
        subject: &str,
        detail: &str,
    ) -> Result<(), String> {
        let current = self.security_audit()?;
        let next = current.append(actor, action, subject, detail, crate::domain::now())?;
        self.write_state("security_audit", &next)
    }

    pub fn seed_empty_register(&self) -> Result<Ticket, String> {
        self.transaction(|| {
            if !self.list()?.is_empty() {
                return Err("A register ticket already exists. Reopen the register.".into());
            }
            let ticket = Ticket::new(1);
            self.save_unchecked(&ticket)?;
            Ok(ticket)
        })
    }
}
