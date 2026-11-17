use super::*;
use crate::{
    domain::Payment,
    intake::{DeliveryMode, IntakeReason, MaterialClass, MaterialCondition, SourceCategory},
};

fn completed() -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.created_at = 90;
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.completed_at = Some(100);
    ticket
}

fn documented(ticket: &Ticket) -> ReviewSnapshot {
    let mut snapshot = ReviewSnapshot::from_ticket(ticket).unwrap();
    snapshot.source = SourceIntake {
        category: SourceCategory::Residential,
        reason: IntakeReason::EmptyingGarage,
        delivery: DeliveryMode::DropOff,
        source_name: "Synthetic source".into(),
        source_address: "1 Example Lane".into(),
        ..Default::default()
    };
    snapshot.materials[0].details = MaterialIntake {
        class: MaterialClass::NonCrtDisplay,
        condition: MaterialCondition::Intact,
        reporting_units: Some(1),
        ..Default::default()
    };
    snapshot.disposition = ReviewDisposition::Reviewed;
    snapshot
}

#[test]
fn reviewed_snapshot_changes_documentary_state_without_mutating_financial_ticket() {
    let ticket = completed();
    let original = ticket.clone();
    let snapshot = documented(&ticket);
    let ledger = ReviewLedger::default();
    let (saved, event) = ledger
        .record(
            &ticket,
            &snapshot,
            "Operator",
            "Checked synthetic source",
            "review-1",
            110,
        )
        .unwrap();
    assert!(ledger.events.is_empty());
    assert_eq!(saved.latest(1), Some(&event));
    assert_eq!(ticket, original);
    assert_eq!(ticket.total(), Some(500));
    assert_eq!(ticket.receipt(), original.receipt());
    assert_eq!(event.snapshot.materials[0].details.reporting_units, Some(1));
    assert_eq!(saved.events.len(), 1);
}

#[test]
fn repeated_request_is_idempotent_but_conflicting_reuse_is_rejected() {
    let ticket = completed();
    let snapshot = documented(&ticket);
    let (ledger, event) = ReviewLedger::default()
        .record(&ticket, &snapshot, "Operator", "Checked", "r1", 110)
        .unwrap();
    let (same, retry) = ledger
        .record(&ticket, &snapshot, "Operator", "Checked", "r1", 120)
        .unwrap();
    assert_eq!(same, ledger);
    assert_eq!(retry, event);
    assert_eq!(retry.at, 110);
    let changed = ReviewSnapshot {
        disposition: ReviewDisposition::NotForClaim,
        ..snapshot
    };
    assert!(
        ledger
            .record(&ticket, &changed, "Operator", "Checked", "r1", 120)
            .is_err()
    );
    assert!(
        ledger
            .record(
                &ticket,
                &changed,
                "Operator",
                "Not for synthetic claim",
                "r2",
                109
            )
            .is_err()
    );
    let (next, _) = ledger
        .record(
            &ticket,
            &changed,
            "Operator",
            "Not for synthetic claim",
            "r2",
            110,
        )
        .unwrap();
    assert_eq!(next.events.len(), 2);
    assert_eq!(ledger.events.len(), 1);
}

#[test]
fn incomplete_documents_remain_pending_and_do_not_become_legal_eligibility() {
    let ticket = completed();
    let pending = ReviewSnapshot::from_ticket(&ticket).unwrap();
    assert_eq!(pending.disposition, ReviewDisposition::PendingReview);
    let reviewed = ReviewSnapshot {
        disposition: ReviewDisposition::Reviewed,
        ..pending.clone()
    };
    assert!(
        ReviewLedger::default()
            .record(&ticket, &reviewed, "Operator", "Review", "r1", 110)
            .is_err()
    );
    assert!(
        ReviewLedger::default()
            .record(&ticket, &pending, "Operator", "Missing source", "r1", 110)
            .is_ok()
    );
    let excluded = ReviewSnapshot {
        disposition: ReviewDisposition::NotForClaim,
        ..pending
    };
    assert!(
        ReviewLedger::default()
            .record(
                &ticket,
                &excluded,
                "Operator",
                "Documented exclusion",
                "r2",
                110
            )
            .is_ok()
    );
    assert!(
        ReviewLedger::default()
            .record(&ticket, &excluded, "Operator", "", "r2", 110)
            .is_err()
    );
}

#[test]
fn stable_and_legacy_identities_must_cover_frozen_lines_exactly() {
    let mut ticket = completed();
    ticket.lines[0].id = 0;
    let snapshot = documented(&ticket);
    assert_eq!(
        snapshot.materials[0].identity,
        LineIdentity::LegacyOrdinal(0)
    );
    assert!(
        ReviewLedger::default()
            .record(&ticket, &snapshot, "Operator", "Legacy review", "r1", 110)
            .is_ok()
    );
    let mut wrong = snapshot.clone();
    wrong.materials[0].identity = LineIdentity::Stable(999);
    assert!(
        ReviewLedger::default()
            .record(&ticket, &wrong, "Operator", "Review", "r1", 110)
            .is_err()
    );
    let mut duplicate = snapshot.clone();
    duplicate.materials.push(snapshot.materials[0].clone());
    assert!(
        ReviewLedger::default()
            .record(&ticket, &duplicate, "Operator", "Review", "r1", 110)
            .is_err()
    );
    let missing = ReviewSnapshot {
        materials: vec![],
        ..snapshot.clone()
    };
    assert!(
        ReviewLedger::default()
            .record(&ticket, &missing, "Operator", "Review", "r1", 110)
            .is_err()
    );
    ticket.state = State::Draft;
    ticket.lines.reverse();
    assert!(
        ReviewLedger::default()
            .record(&ticket, &snapshot, "Operator", "Review", "r1", 110)
            .is_err()
    );
}

#[test]
fn source_correction_appends_new_snapshot_and_normalizes_fields() {
    let ticket = completed();
    let first = documented(&ticket);
    let (ledger, original) = ReviewLedger::default()
        .record(&ticket, &first, " Operator ", " Initial review ", "r1", 110)
        .unwrap();
    let mut corrected = first;
    corrected.source.source_name = " Corrected synthetic source ".into();
    let (next, event) = ledger
        .record(
            &ticket,
            &corrected,
            "Operator",
            "Corrected source",
            "r2",
            120,
        )
        .unwrap();
    assert_eq!(next.events[0], original);
    assert_eq!(
        event.snapshot.source.source_name,
        "Corrected synthetic source"
    );
    assert_eq!(event.operator, "Operator");
    assert_eq!(next.latest(1), Some(&event));
    assert_eq!(
        serde_json::from_str::<ReviewLedger>(&serde_json::to_string(&next).unwrap()).unwrap(),
        next
    );
}

#[test]
fn invalid_input_and_corrupt_saved_history_are_rejected() {
    let ticket = completed();
    let snapshot = documented(&ticket);
    let empty = ReviewLedger::default();
    for (operator, reason, key) in [
        ("", "Review", "r1"),
        ("Operator", "", "r1"),
        ("Bad\nOperator", "Review", "r1"),
        ("Operator", "Review", "bad key"),
    ] {
        assert!(
            empty
                .record(&ticket, &snapshot, operator, reason, key, 110)
                .is_err()
        );
    }
    assert!(
        empty
            .record(&ticket, &snapshot, "Operator", "Review", "r1", 99)
            .is_err()
    );
    let (ledger, event) = empty
        .record(&ticket, &snapshot, "Operator", "Review", "r1", 110)
        .unwrap();
    let corrupt = ReviewLedger {
        events: vec![event.clone(), event],
        ..ledger.clone()
    };
    assert!(corrupt.validate().is_err());
    let corrupt = ReviewLedger {
        next_id: 0,
        ..ledger.clone()
    };
    assert!(corrupt.validate().is_err());
    let mut corrupt = ledger;
    corrupt.events[0].snapshot.source.source_name = "bad\nname".into();
    assert!(corrupt.validate().is_err());
}

#[test]
fn mixed_stream_reviews_follow_stable_ids_even_when_input_order_differs() {
    let mut ticket = Ticket::new(2);
    ticket.created_at = 90;
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.add_weight_line("803", "20", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.confirm_cash("20").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    ticket.completed_at = Some(100);
    let original = ticket.clone();
    let mut snapshot = documented(&ticket);
    snapshot.materials[1].details = MaterialIntake {
        class: MaterialClass::LooseBattery,
        condition: MaterialCondition::Damaged,
        reporting_units: Some(10),
        actual_weight_thousandths: Some(3500),
        notes: "Separate reporting count".into(),
    };
    snapshot.materials.reverse();
    let (ledger, event) = ReviewLedger::default()
        .record(
            &ticket,
            &snapshot,
            "Operator",
            "Mixed synthetic streams",
            "mixed",
            110,
        )
        .unwrap();
    assert_eq!(
        event.snapshot.materials[0].identity,
        LineIdentity::Stable(ticket.lines[0].id)
    );
    assert_eq!(
        event.snapshot.materials[1].identity,
        LineIdentity::Stable(ticket.lines[1].id)
    );
    assert_eq!(
        event.snapshot.materials[1].details.reporting_units,
        Some(10)
    );
    event.snapshot.normalized_for(&ticket).unwrap();
    assert!(event.snapshot.issues(&ticket).unwrap().is_empty());
    ledger
        .validate_against(std::slice::from_ref(&ticket))
        .unwrap();
    assert!(ledger.validate_against(&[]).is_err());
    assert_eq!(ticket, original);
    assert_eq!(ticket.total(), Some(1300));
}

#[test]
fn copied_source_mismatch_stays_a_review_issue_and_cannot_be_approved() {
    let ticket = completed();
    let mut snapshot = documented(&ticket);
    snapshot.source.linked_customer_id = Some(999);
    assert!(
        snapshot
            .issues(&ticket)
            .unwrap()
            .iter()
            .any(|issue| issue.code == "linked-customer")
    );
    assert!(snapshot.normalized_for(&ticket).is_err());
    snapshot.disposition = ReviewDisposition::PendingReview;
    let (ledger, event) = ReviewLedger::default()
        .record(
            &ticket,
            &snapshot,
            "Operator",
            "Source correction pending",
            "pending",
            110,
        )
        .unwrap();
    ledger
        .validate_against(std::slice::from_ref(&ticket))
        .unwrap();
    let mut corrupt = ledger.clone();
    corrupt.events[0].snapshot.disposition = ReviewDisposition::Reviewed;
    assert!(corrupt.validate_against(&[ticket]).is_err());
    assert_eq!(event.snapshot.source.linked_customer_id, Some(999));
}

#[test]
fn chronology_invalid_mapping_and_id_exhaustion_are_rejected_on_restore() {
    let ticket = completed();
    let snapshot = documented(&ticket);
    let (ledger, event) = ReviewLedger::default()
        .record(&ticket, &snapshot, "Operator", "Review", "r1", 110)
        .unwrap();
    let mut backwards = ledger.clone();
    backwards.events.push(ReviewEvent {
        id: 2,
        at: 109,
        request_key: "r2".into(),
        ..event.clone()
    });
    backwards.next_id = 3;
    assert!(backwards.validate().is_err());
    let mut wrong = ledger.clone();
    wrong.events[0].snapshot.materials[0].identity = LineIdentity::Stable(999);
    assert!(
        wrong
            .validate_against(std::slice::from_ref(&ticket))
            .is_err()
    );
    let mut early = ledger;
    early.events[0].at = 99;
    assert!(
        early
            .validate_against(std::slice::from_ref(&ticket))
            .is_err()
    );
    let full = ReviewLedger {
        next_id: u64::MAX,
        ..Default::default()
    };
    assert!(
        full.record(&ticket, &snapshot, "Operator", "Review", "r1", 110)
            .is_err()
    );
    assert!(ReviewSnapshot::from_ticket(&Ticket::new(3)).is_err());
}
