use super::*;
use crate::{domain::Payment, operator_snapshot::Role};

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "cashier".into(),
        display_name: "History printer operator".into(),
        role: Role::Cashier,
        revision: 1,
    }
}
fn historical(payment: Option<Payment>) -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.created_at = 10;
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.state = State::Completed;
    ticket.payment = payment;
    ticket.completed_at = None;
    ticket.receipt_snapshot = None;
    ticket
}

#[test]
fn historical_receipts_print_without_inventing_modern_tender_or_account_evidence() {
    for payment in [
        Some(Payment::Cash),
        Some(Payment::ExternalCard {
            reference: "LEGACY-REFERENCE".into(),
        }),
        Some(Payment::ChargeTo {
            account: "OLD-ACCOUNT".into(),
        }),
        None,
    ] {
        let ticket = historical(payment);
        let before = ticket.clone();
        let (log, job, submit) = ReceiptDispatchLedger::default()
            .prepare(
                &ticket,
                CopyKind::Internal,
                "historical",
                "Receipt printer",
                &actor(),
                30,
            )
            .unwrap();
        assert!(submit);
        assert_eq!(job.receipt, format!("INTERNAL COPY\n{}", ticket.receipt()));
        assert_eq!(ticket, before);
        assert_eq!(ticket.tender_evidence, Default::default());
        assert!(ticket.account_record.is_none());
        log.validate_against(&[ticket]).unwrap();
    }
}

#[test]
fn frozen_physical_body_from_another_renderer_remains_valid_and_replays_without_regeneration() {
    let mut ticket = historical(Some(Payment::Cash));
    ticket.completed_at = Some(20);
    ticket.tender_evidence = crate::tender::TenderEvidence::default()
        .record_cash(500, 500)
        .unwrap();
    let (mut log, _, _) = ReceiptDispatchLedger::default()
        .prepare(
            &ticket,
            CopyKind::Internal,
            "historical",
            "Receipt printer",
            &actor(),
            30,
        )
        .unwrap();
    let current = ticket.receipt();
    let alternate = if current.contains("TRAINING COPY - synthetic counter workflow") {
        current.replace(
            "TRAINING COPY - synthetic counter workflow",
            "E-waste Counter",
        )
    } else {
        current.replace(
            "E-waste Counter",
            "TRAINING COPY - synthetic counter workflow",
        )
    };
    assert_ne!(alternate, current);
    log.jobs[0].receipt = format!("INTERNAL COPY\n{alternate}");
    // This fixture is the body originally frozen by the other application build.
    log.jobs[0].source_digest = binding::source_digest(&ticket, &log.jobs[0]).unwrap();
    log.validate_against(std::slice::from_ref(&ticket)).unwrap();
    let (same, job, submit) = log
        .prepare(
            &ticket,
            CopyKind::Internal,
            "historical",
            "Receipt printer",
            &actor(),
            31,
        )
        .unwrap();
    assert!(!submit);
    assert_eq!(same, log);
    assert_eq!(job.receipt, format!("INTERNAL COPY\n{alternate}"));
    let (recovery, child, _) = log
        .prepare_checked_copy(
            &ticket,
            &CheckedCopyRequest {
                prior_job_id: job.id,
                prior_attempt_id: 1,
                key: "checked-copy",
                queue: "Replacement printer",
                checked: true,
            },
            &actor(),
            32,
        )
        .unwrap();
    assert_eq!(child.receipt, job.receipt);
    recovery.validate_against(&[ticket]).unwrap();
}

#[test]
fn source_binding_allows_only_revision_and_reprint_changes_and_detects_body_or_metadata_damage() {
    let ticket = historical(Some(Payment::Cash));
    let (log, _, _) = ReceiptDispatchLedger::default()
        .prepare(
            &ticket,
            CopyKind::Internal,
            "historical",
            "Receipt printer",
            &actor(),
            30,
        )
        .unwrap();
    let mut reprinted = ticket.clone();
    reprinted.revision = 5;
    reprinted.reprints = 3;
    log.validate_against(&[reprinted]).unwrap();
    for kind in 0..4 {
        let mut changed = ticket.clone();
        match kind {
            0 => changed.lines[0].classification = "Different saved classification".into(),
            1 => changed.customer = Some("Different customer".into()),
            2 => changed.source_intake.notes = "Changed saved source field".into(),
            _ => changed.completed_by = Some(actor()),
        }
        assert!(
            log.validate_against(&[changed]).is_err(),
            "ticket binding {kind}"
        );
    }
    for kind in 0..5 {
        let mut changed = log.clone();
        match kind {
            0 => changed.jobs[0].receipt.push_str("Body damage"),
            1 => changed.jobs[0].queue = "Different printer".into(),
            2 => changed.jobs[0].actor.display_name = "Different operator".into(),
            3 => changed.jobs[0].source_digest = "0".repeat(64),
            _ => {
                changed.jobs[0].copy = CopyKind::Customer;
                changed.jobs[0].source_digest =
                    binding::source_digest(&ticket, &changed.jobs[0]).unwrap();
            }
        }
        assert!(
            changed
                .validate_against(std::slice::from_ref(&ticket))
                .is_err(),
            "dispatch binding {kind}"
        );
    }
}
