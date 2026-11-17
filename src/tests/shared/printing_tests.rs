use super::*;

const RECEIPT: &str = "E-WASTE COUNTER\nTicket #000001\nTotal $5.00\n";

#[test]
fn failure_retry_keeps_one_job_and_saved_receipt() {
    let original = PrintLog::default();
    let (failed, job) = original
        .request(1, CopyKind::Customer, RECEIPT, "ticket-1-customer", true)
        .unwrap();
    assert!(original.jobs.is_empty());
    assert_eq!(job.state, PrintState::Failed);
    assert_eq!(failed.successful_internal_copies(1), 0);
    let (printed, retry) = failed.retry(job.id, false).unwrap();
    assert_eq!(printed.jobs.len(), 1);
    assert_eq!(retry.id, job.id);
    assert_eq!(retry.receipt, RECEIPT);
    assert_eq!(retry.attempts, 2);
    assert_eq!(retry.state, PrintState::Succeeded);
    let (same, same_job) = printed.retry(job.id, true).unwrap();
    assert_eq!(same, printed);
    assert_eq!(same_job, retry);
}

#[test]
fn request_key_is_idempotent_but_explicit_new_copy_is_a_new_job() {
    let (log, job) = PrintLog::default()
        .request(1, CopyKind::Internal, RECEIPT, "internal-1", false)
        .unwrap();
    let (same, duplicate) = log
        .request(1, CopyKind::Internal, RECEIPT, "internal-1", false)
        .unwrap();
    assert_eq!(same, log);
    assert_eq!(duplicate, job);
    assert_eq!(log.successful_internal_copies(1), 1);
    let (new, second) = log
        .request(1, CopyKind::Internal, RECEIPT, "internal-2", false)
        .unwrap();
    assert_ne!(job.id, second.id);
    assert_eq!(new.successful_internal_copies(1), 2);
    assert_eq!(new.successful_internal_copies(2), 0);
}

#[test]
fn failed_repeated_request_retries_same_job_and_view_is_read_only() {
    let (log, first) = PrintLog::default()
        .request(1, CopyKind::Internal, RECEIPT, "internal-1", true)
        .unwrap();
    assert_eq!(log.successful_internal_copies(1), 0);
    let viewed = log.job(first.id).unwrap();
    assert_eq!(viewed.attempts, 1);
    assert_eq!(log.jobs.len(), 1);
    let (retried, job) = log
        .request(1, CopyKind::Internal, RECEIPT, "internal-1", false)
        .unwrap();
    assert_eq!(job.id, first.id);
    assert_eq!(retried.successful_internal_copies(1), 1);
    assert_eq!(retried.jobs.len(), 1);
}

#[test]
fn changed_ticket_copy_or_snapshot_cannot_reuse_request_key() {
    let (log, _) = PrintLog::default()
        .request(1, CopyKind::Customer, RECEIPT, "copy-1", true)
        .unwrap();
    assert!(
        log.request(2, CopyKind::Customer, RECEIPT, "copy-1", false)
            .is_err()
    );
    assert!(
        log.request(1, CopyKind::Internal, RECEIPT, "copy-1", false)
            .is_err()
    );
    assert!(
        log.request(1, CopyKind::Customer, "Different receipt", "copy-1", false)
            .is_err()
    );
    assert!(log.retry(999, false).is_err());
}

#[test]
fn input_validation_and_sequence_exhaustion_are_safe() {
    let log = PrintLog::default();
    assert!(
        log.request(0, CopyKind::Customer, RECEIPT, "copy-1", false)
            .is_err()
    );
    assert!(
        log.request(1, CopyKind::Customer, "", "copy-1", false)
            .is_err()
    );
    assert!(
        log.request(1, CopyKind::Customer, RECEIPT, "", false)
            .is_err()
    );
    assert!(
        log.request(1, CopyKind::Customer, RECEIPT, "bad\nkey", false)
            .is_err()
    );
    let full = PrintLog {
        next_id: u64::MAX,
        ..log
    };
    assert!(
        full.request(1, CopyKind::Customer, RECEIPT, "copy-1", false)
            .is_err()
    );
    assert!(
        PrintLog {
            next_id: 0,
            jobs: vec![]
        }
        .validate()
        .is_err()
    );
    let (valid, _) = PrintLog::default()
        .request(1, CopyKind::Customer, RECEIPT, "copy-1", false)
        .unwrap();
    let mut duplicate = valid.clone();
    duplicate.jobs.push(valid.jobs[0].clone());
    assert!(duplicate.validate().is_err());
    let mut bad_attempt = valid;
    bad_attempt.jobs[0].attempts = 0;
    assert!(bad_attempt.validate().is_err());
}

#[test]
fn logs_survive_serialization_and_failed_retry_attempts_are_audited() {
    let (log, job) = PrintLog::default()
        .request(1, CopyKind::Customer, RECEIPT, "copy-1", true)
        .unwrap();
    let json = serde_json::to_string(&log).unwrap();
    let loaded: PrintLog = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded, log);
    let (retry, job) = loaded.retry(job.id, true).unwrap();
    assert_eq!(job.attempts, 2);
    assert_eq!(job.state, PrintState::Failed);
    assert_eq!(retry.jobs.len(), 1);
    assert_eq!(
        serde_json::from_str::<PrintLog>("{}").unwrap(),
        PrintLog::default()
    );
}
