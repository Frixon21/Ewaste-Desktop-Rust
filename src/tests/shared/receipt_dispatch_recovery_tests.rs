use super::*;
use crate::{domain::Payment, operator_snapshot::Role};

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "cashier".into(),
        display_name: "Receipt recovery operator".into(),
        role: Role::Cashier,
        revision: 1,
    }
}
fn ticket() -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.created_at = 1;
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.completed_at = Some(10);
    ticket
}
fn pending() -> (Ticket, ReceiptDispatchLedger, DispatchJob) {
    let ticket = ticket();
    let (log, job, _) = ReceiptDispatchLedger::default()
        .prepare(
            &ticket,
            CopyKind::Customer,
            "first",
            "Printer A",
            &actor(),
            20,
        )
        .unwrap();
    (ticket, log, job)
}

#[test]
fn checked_copy_requires_confirmation_and_links_a_new_queue_without_changing_original() {
    let (ticket, log, original) = pending();
    let (unknown, prior) = log
        .record(
            original.id,
            1,
            &DispatchOutcome::Unknown {
                detail: "Submission uncertain".into(),
            },
            21,
        )
        .unwrap();
    assert!(
        unknown
            .prepare_checked_copy(
                &ticket,
                &CheckedCopyRequest {
                    prior_job_id: prior.id,
                    prior_attempt_id: 1,
                    key: "recovery",
                    queue: "Printer B",
                    checked: false
                },
                &actor(),
                22
            )
            .is_err()
    );
    let (next, child, submit) = unknown
        .prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: prior.id,
                prior_attempt_id: 1,
                key: "recovery",
                queue: "Printer B",
                checked: true,
            },
            &actor(),
            22,
        )
        .unwrap();
    assert!(submit);
    assert_eq!(next.jobs[0], prior);
    assert_ne!(child.id, prior.id);
    assert_eq!(child.receipt, prior.receipt);
    assert_eq!(child.queue, "Printer B");
    let link = child.checked_copy.as_ref().unwrap();
    assert_eq!(link.prior_job_id, prior.id);
    assert_eq!(link.prior_attempt_id, 1);
    assert_eq!(link.checked_by, actor());
    assert_eq!(link.checked_at, 22);
    assert!(link.queue_and_paper_checked);
    let duplicate = next
        .prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: prior.id,
                prior_attempt_id: 1,
                key: "recovery",
                queue: "Printer B",
                checked: true,
            },
            &actor(),
            23,
        )
        .unwrap();
    assert!(!duplicate.2);
    assert_eq!(duplicate.0, next);
    assert_eq!(duplicate.1, child);
    assert!(
        next.prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: prior.id,
                prior_attempt_id: 1,
                key: "another-child",
                queue: "Printer B",
                checked: true
            },
            &actor(),
            23
        )
        .is_err()
    );
    next.validate_against(&[ticket]).unwrap();
}

#[test]
fn late_original_acceptance_preserves_link_and_latest_child_controls_future_copies() {
    let (ticket, log, original) = pending();
    let (recovery, child, _) = log
        .prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: original.id,
                prior_attempt_id: 1,
                key: "recovery",
                queue: "Printer B",
                checked: true,
            },
            &actor(),
            21,
        )
        .unwrap();
    let (late, _) = recovery
        .record(
            original.id,
            1,
            &DispatchOutcome::Accepted { windows_job_id: 11 },
            22,
        )
        .unwrap();
    late.validate_against(std::slice::from_ref(&ticket))
        .unwrap();
    assert_eq!(late.jobs[1], child);
    assert!(
        late.prepare(
            &ticket,
            CopyKind::Customer,
            "bypass",
            "Printer B",
            &actor(),
            23
        )
        .is_err()
    );
    let (accepted, _) = late
        .record(
            child.id,
            1,
            &DispatchOutcome::Accepted { windows_job_id: 12 },
            23,
        )
        .unwrap();
    let (ordinary, copy, submit) = accepted
        .prepare(
            &ticket,
            CopyKind::Customer,
            "deliberate-copy",
            "Printer B",
            &actor(),
            24,
        )
        .unwrap();
    assert!(submit);
    assert!(copy.checked_copy.is_none());
    ordinary.validate_against(&[ticket]).unwrap();
}

#[test]
fn superseded_not_submitted_attempt_cannot_retry_and_child_can_recover_again_only_after_check() {
    let (ticket, log, original) = pending();
    let (failed, _) = log
        .record(
            original.id,
            1,
            &DispatchOutcome::NotSubmitted {
                detail: "Old queue gone".into(),
            },
            21,
        )
        .unwrap();
    let (recovery, child, _) = failed
        .prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: original.id,
                prior_attempt_id: 1,
                key: "recovery",
                queue: "Printer B",
                checked: true,
            },
            &actor(),
            22,
        )
        .unwrap();
    assert!(
        recovery
            .retry(original.id, "unsafe-original-retry", &actor(), 23)
            .is_err()
    );
    assert!(
        !recovery
            .retry(original.id, "first", &actor(), 23)
            .unwrap()
            .2
    );
    assert!(
        recovery
            .prepare_checked_copy(
                &ticket,
                &CheckedCopyRequest {
                    prior_job_id: original.id,
                    prior_attempt_id: 2,
                    key: "wrong-attempt",
                    queue: "Printer C",
                    checked: true
                },
                &actor(),
                23
            )
            .is_err()
    );
    let (unknown, _) = recovery
        .record(
            child.id,
            1,
            &DispatchOutcome::Unknown {
                detail: "Unknown child".into(),
            },
            23,
        )
        .unwrap();
    let (chain, new_child, _) = unknown
        .prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: child.id,
                prior_attempt_id: 1,
                key: "checked-again",
                queue: "Printer C",
                checked: true,
            },
            &actor(),
            24,
        )
        .unwrap();
    assert_eq!(new_child.checked_copy.unwrap().prior_job_id, child.id);
    chain.validate_against(&[ticket]).unwrap();
}

#[test]
fn recovery_rejects_accepted_prior_stale_identity_payload_conflicts_and_tampered_links() {
    let (ticket, log, original) = pending();
    let (accepted, _) = log
        .record(
            original.id,
            1,
            &DispatchOutcome::Accepted { windows_job_id: 10 },
            21,
        )
        .unwrap();
    assert!(
        accepted
            .prepare_checked_copy(
                &ticket,
                &CheckedCopyRequest {
                    prior_job_id: original.id,
                    prior_attempt_id: 1,
                    key: "unnecessary",
                    queue: "Printer B",
                    checked: true
                },
                &actor(),
                22
            )
            .is_err()
    );
    assert!(
        log.prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: original.id,
                prior_attempt_id: 2,
                key: "wrong",
                queue: "Printer B",
                checked: true
            },
            &actor(),
            21
        )
        .is_err()
    );
    assert!(
        log.prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: original.id,
                prior_attempt_id: 1,
                key: "early",
                queue: "Printer B",
                checked: true
            },
            &actor(),
            19
        )
        .is_err()
    );
    let (recovery, _, _) = log
        .prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: original.id,
                prior_attempt_id: 1,
                key: "recovery",
                queue: "Printer B",
                checked: true,
            },
            &actor(),
            21,
        )
        .unwrap();
    assert!(
        recovery
            .prepare_checked_copy(
                &ticket,
                &CheckedCopyRequest {
                    prior_job_id: original.id,
                    prior_attempt_id: 1,
                    key: "recovery",
                    queue: "Printer C",
                    checked: true
                },
                &actor(),
                22
            )
            .is_err()
    );
    for kind in 0..7 {
        let mut corrupt = recovery.clone();
        let link = corrupt.jobs[1].checked_copy.as_mut().unwrap();
        match kind {
            0 => link.queue_and_paper_checked = false,
            1 => link.prior_job_id = 999,
            2 => link.prior_attempt_id = 2,
            3 => link.checked_at = 19,
            4 => link.checked_by.id = 2,
            5 => corrupt.jobs[1].checked_copy = None,
            _ => {
                corrupt.jobs[0].checked_copy = Some(link.clone());
            }
        }
        assert!(
            corrupt
                .validate_against(std::slice::from_ref(&ticket))
                .is_err(),
            "tamper {kind}"
        );
    }
}
