use super::*;
impl Store {
    #[cfg(not(feature = "extended-workflows"))]
    pub(crate) fn test_receipt_result_audit_failure(&self, enabled: bool) -> Result<(), String> {
        if enabled {
            self.execute("CREATE TRIGGER deny_receipt_result_audit BEFORE UPDATE ON app_state WHEN NEW.key='security_audit' BEGIN SELECT RAISE(ABORT,'synthetic receipt result audit failure'); END")
        } else {
            self.execute("DROP TRIGGER deny_receipt_result_audit")
        }
    }
    pub fn save(&self, ticket: &Ticket) -> Result<(), String> {
        self.save_unchecked(ticket)
    }
    pub fn save_directory(&self, previous: &Directory, next: &Directory) -> Result<(), String> {
        self.save_directory_unchecked(previous, next)
    }
    pub fn save_print_log(&self, previous: &PrintLog, next: &PrintLog) -> Result<(), String> {
        self.save_print_log_unchecked(previous, next)
    }
    pub fn save_catalog(
        &self,
        previous: &crate::catalog::Catalog,
        next: &crate::catalog::Catalog,
    ) -> Result<(), String> {
        self.save_catalog_unchecked(previous, next)
    }
    #[cfg(feature = "extended-workflows")]
    pub fn save_closeout(&self, record: &crate::closeout::Closeout) -> Result<(), String> {
        self.save_closeout_unchecked(record)
    }
    #[cfg(feature = "extended-workflows")]
    pub fn save_intake_reviews(
        &self,
        previous: &crate::intake_review::ReviewLedger,
        next: &crate::intake_review::ReviewLedger,
    ) -> Result<(), String> {
        self.save_intake_reviews_unchecked(previous, next)
    }
}
