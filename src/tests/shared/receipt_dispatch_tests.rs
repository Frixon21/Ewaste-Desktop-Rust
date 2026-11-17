use super::*;
use crate::{domain::Payment, operator_snapshot::Role};

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "cashier".into(),
        display_name: "Receipt operator".into(),
        role: Role::Cashier,
        revision: 1,
    }
}
fn ticket(id: u64) -> Ticket {
    let mut ticket = Ticket::new(id);
    ticket.created_at = 10;
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.completed_at = Some(20);
    ticket
}
fn start(
    log: &ReceiptDispatchLedger,
    ticket: &Ticket,
    key: &str,
) -> (ReceiptDispatchLedger, DispatchJob, bool) {
    log.prepare(
        ticket,
        CopyKind::Customer,
        key,
        "Selected printer",
        &actor(),
        30,
    )
    .unwrap()
}

#[test]
fn intent_precedes_submission_and_exact_request_never_resubmits() {
    let ticket = ticket(1);
    let original = ticket.clone();
    let empty = ReceiptDispatchLedger::default();
    let (log, job, submit) = start(&empty, &ticket, "customer:1");
    assert!(submit);
    assert!(empty.jobs.is_empty());
    assert_eq!(job.state(), &DispatchState::Dispatching);
    assert_eq!(
        job.receipt,
        format!("CUSTOMER RECEIPT\n{}", ticket.receipt())
    );
    assert_eq!(ticket, original);
    let (same, saved, submit) = start(&log, &ticket, "customer:1");
    assert!(!submit);
    assert_eq!(same, log);
    assert_eq!(saved, job);
    assert!(start_result(&log, &ticket, "different-key").is_err());
}
fn start_result(
    log: &ReceiptDispatchLedger,
    ticket: &Ticket,
    key: &str,
) -> Result<(ReceiptDispatchLedger, DispatchJob, bool), String> {
    log.prepare(
        ticket,
        CopyKind::Customer,
        key,
        "Selected printer",
        &actor(),
        30,
    )
}

#[test]
fn accepted_is_spooler_acceptance_and_explicit_another_copy_is_separate() {
    let ticket = ticket(1);
    let (log, job, _) = start(&ReceiptDispatchLedger::default(), &ticket, "first");
    let outcome = DispatchOutcome::Accepted { windows_job_id: 41 };
    let (done, accepted) = log.record(job.id, 1, &outcome, 31).unwrap();
    assert_eq!(
        accepted.state(),
        &DispatchState::Accepted { windows_job_id: 41 }
    );
    assert_eq!(done.record(job.id, 1, &outcome, 99).unwrap().0, done);
    assert!(
        done.record(
            job.id,
            1,
            &DispatchOutcome::Unknown {
                detail: "Different result".into()
            },
            31
        )
        .is_err()
    );
    let (_, old, submit) = start(&done, &ticket, "first");
    assert!(!submit);
    assert_eq!(old, accepted);
    let (copies, second, submit) = done
        .prepare(
            &ticket,
            CopyKind::Customer,
            "deliberate-second",
            "Selected printer",
            &actor(),
            32,
        )
        .unwrap();
    assert!(submit);
    assert_ne!(second.id, job.id);
    assert_eq!(copies.jobs.len(), 2);
    assert!(
        done.retry(job.id, "never-resubmit-accepted", &actor(), 32)
            .is_err()
    );
}

#[test]
fn retry_requires_proven_not_submitted_and_retains_receipt_queue_and_prior_attempt() {
    let ticket = ticket(1);
    let (log, job, _) = start(&ReceiptDispatchLedger::default(), &ticket, "first");
    assert!(log.retry(job.id, "retry", &actor(), 31).is_err());
    let (failed, _) = log
        .record(
            job.id,
            1,
            &DispatchOutcome::NotSubmitted {
                detail: "Printer could not be opened".into(),
            },
            31,
        )
        .unwrap();
    assert!(start_result(&failed, &ticket, "bypass-retry").is_err());
    let (retry, retried, submit) = failed.retry(job.id, "retry", &actor(), 32).unwrap();
    assert!(submit);
    assert_eq!(retried.id, job.id);
    assert_eq!(retried.queue, job.queue);
    assert_eq!(retried.receipt, job.receipt);
    assert_eq!(retried.attempts.len(), 2);
    assert_eq!(retried.attempts[0], failed.jobs[0].attempts[0]);
    assert_eq!(retry.retry(job.id, "retry", &actor(), 33).unwrap().0, retry);
    assert!(!retry.retry(job.id, "retry", &actor(), 33).unwrap().2);
    assert!(
        retry
            .record(
                job.id,
                1,
                &DispatchOutcome::Accepted { windows_job_id: 8 },
                33
            )
            .is_err()
    );
}

#[test]
fn unknown_or_restarted_dispatching_blocks_key_bypass_and_retry() {
    let ticket = ticket(1);
    let (log, job, _) = start(&ReceiptDispatchLedger::default(), &ticket, "first");
    let reopened: ReceiptDispatchLedger =
        serde_json::from_str(&serde_json::to_string(&log).unwrap()).unwrap();
    reopened
        .validate_against(std::slice::from_ref(&ticket))
        .unwrap();
    assert!(reopened.retry(job.id, "retry", &actor(), 32).is_err());
    let (unknown, _) = log
        .record(
            job.id,
            1,
            &DispatchOutcome::Unknown {
                detail: "Write result uncertain".into(),
            },
            31,
        )
        .unwrap();
    assert!(unknown.retry(job.id, "retry", &actor(), 32).is_err());
    assert!(start_result(&unknown, &ticket, "new-key").is_err());
    let mut other_copy = ticket.clone();
    other_copy.reprints = 9;
    assert!(
        unknown
            .prepare(
                &other_copy,
                CopyKind::Internal,
                "internal",
                "Selected printer",
                &actor(),
                32
            )
            .is_ok()
    );
}

#[test]
fn malformed_inputs_drafts_and_changed_snapshots_are_rejected() {
    let ticket = ticket(1);
    let log = ReceiptDispatchLedger::default();
    assert!(
        log.prepare(
            &ticket,
            CopyKind::Customer,
            "bad key",
            "Selected printer",
            &actor(),
            30
        )
        .is_err()
    );
    for queue in ["", " bad ", "Bad\nPrinter"] {
        assert!(
            log.prepare(&ticket, CopyKind::Customer, "key", queue, &actor(), 30)
                .is_err()
        );
    }
    let mut draft = ticket.clone();
    draft.state = State::Draft;
    assert!(start_result(&log, &draft, "draft").is_err());
    let mut unpaid = ticket.clone();
    unpaid.tender_evidence = Default::default();
    assert!(start_result(&log, &unpaid, "historical-no-proof").is_ok());
    let (log, job, _) = start(&log, &ticket, "key");
    let mut changed = ticket.clone();
    changed.receipt_snapshot = Some("Forged snapshot".into());
    assert!(start_result(&log, &changed, "key").is_err());
    assert!(
        log.record(
            job.id,
            1,
            &DispatchOutcome::Accepted { windows_job_id: 0 },
            31
        )
        .is_err()
    );
    assert!(
        log.record(
            job.id,
            1,
            &DispatchOutcome::Unknown {
                detail: "\n".into()
            },
            31
        )
        .is_err()
    );
    assert!(
        log.record(
            job.id,
            1,
            &DispatchOutcome::Accepted { windows_job_id: 1 },
            29
        )
        .is_err()
    );
}

#[test]
fn persisted_history_validation_rejects_orphans_keys_identity_and_snapshot_tampering() {
    let ticket = ticket(1);
    let (log, _, _) = start(&ReceiptDispatchLedger::default(), &ticket, "key");
    assert!(log.validate_against(&[]).is_err());
    for kind in 0..6 {
        let mut corrupt = log.clone();
        match kind {
            0 => corrupt.next_id = 1,
            1 => corrupt.jobs[0].attempts[0].id = 2,
            2 => corrupt.jobs[0].receipt.push_str("changed"),
            3 => corrupt.jobs[0].actor.id = 0,
            4 => corrupt.jobs[0].attempts[0].recorded_at = Some(31),
            _ => corrupt.jobs.push(corrupt.jobs[0].clone()),
        }
        assert!(
            corrupt
                .validate_against(std::slice::from_ref(&ticket))
                .is_err(),
            "tamper case {kind}"
        );
    }
}

#[test]
fn request_keys_cannot_change_copy_queue_ticket_or_another_jobs_retry() {
    let ticket = ticket(1);
    let (log, job, _) = start(&ReceiptDispatchLedger::default(), &ticket, "first");
    for (other_ticket, copy, queue) in [
        (ticket.clone(), CopyKind::Internal, "Selected printer"),
        (ticket.clone(), CopyKind::Customer, "Different printer"),
        (self::ticket(2), CopyKind::Customer, "Selected printer"),
    ] {
        assert!(
            log.prepare(&other_ticket, copy, "first", queue, &actor(), 31)
                .is_err()
        );
    }
    let (log, _, _) = log
        .prepare(
            &ticket,
            CopyKind::Internal,
            "internal",
            "印刷 queue",
            &actor(),
            31,
        )
        .unwrap();
    assert!(log.retry(job.id, "internal", &actor(), 32).is_err());
    assert!(log.retry(999, "retry", &actor(), 32).is_err());
    assert!(
        log.record(999, 1, &DispatchOutcome::Accepted { windows_job_id: 1 }, 32)
            .is_err()
    );
    let mut invalid_actor = actor();
    invalid_actor.id = 0;
    assert!(
        log.prepare(
            &self::ticket(2),
            CopyKind::Customer,
            "other",
            "Printer",
            &invalid_actor,
            32
        )
        .is_err()
    );
    assert!(
        log.prepare(
            &self::ticket(2),
            CopyKind::Customer,
            "other",
            "Printer",
            &actor(),
            19
        )
        .is_err()
    );
    assert!(
        log.prepare(
            &self::ticket(2),
            CopyKind::Customer,
            &"x".repeat(129),
            "Printer",
            &actor(),
            32
        )
        .is_err()
    );
    assert!(
        log.prepare(
            &self::ticket(2),
            CopyKind::Customer,
            "other",
            &"x".repeat(241),
            &actor(),
            32
        )
        .is_err()
    );
}

#[test]
fn saved_attempt_history_rejects_duplicate_keys_invalid_times_and_unresolved_retries() {
    let ticket = ticket(1);
    let (log, job, _) = start(&ReceiptDispatchLedger::default(), &ticket, "first");
    let (failed, _) = log
        .record(
            job.id,
            1,
            &DispatchOutcome::NotSubmitted {
                detail: "No submission".into(),
            },
            31,
        )
        .unwrap();
    let (retry, _, _) = failed.retry(job.id, "retry", &actor(), 32).unwrap();
    for kind in 0..7 {
        let mut corrupt = retry.clone();
        match kind {
            0 => corrupt.jobs[0].attempts[1].request_key = "first".into(),
            1 => corrupt.jobs[0].attempts[1].requested_at = 30,
            2 => {
                corrupt.jobs[0].attempts[0].state = DispatchState::Unknown {
                    detail: "Unresolved".into(),
                }
            }
            3 => corrupt.jobs[0].attempts[0].recorded_at = Some(29),
            4 => corrupt.jobs[0].attempts[1].actor.revision = 0,
            5 => corrupt.jobs[0].attempts[0].requested_at = 19,
            _ => corrupt.jobs[0].request_key = "changed".into(),
        }
        assert!(
            corrupt
                .validate_against(std::slice::from_ref(&ticket))
                .is_err(),
            "tamper case {kind}"
        );
    }
    assert!(failed.retry(job.id, "retry", &actor(), 30).is_err());
    assert!(
        log.record(
            job.id,
            1,
            &DispatchOutcome::NotSubmitted {
                detail: "x".repeat(501)
            },
            31
        )
        .is_err()
    );
    let mut no_payment = ticket.clone();
    no_payment.payment = None;
    assert!(
        start_result(
            &ReceiptDispatchLedger::default(),
            &no_payment,
            "missing-payment"
        )
        .is_ok()
    );
    let mut before_created = ticket.clone();
    before_created.completed_at = Some(1);
    assert!(
        start_result(
            &ReceiptDispatchLedger::default(),
            &before_created,
            "bad-time"
        )
        .is_err()
    );
}
