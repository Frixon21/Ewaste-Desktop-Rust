use super::*;
use crate::{auth::Role, domain::Payment};

fn actor() -> OperatorSnapshot {
    OperatorSnapshot {
        id: 1,
        login: "supervisor".into(),
        display_name: "Synthetic Supervisor".into(),
        role: Role::Supervisor,
        revision: 1,
    }
}
fn ticket() -> Ticket {
    let mut ticket = Ticket::new(1);
    ticket.created_at = 100;
    ticket.add_weight_line("803", "4", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket
}

#[test]
fn approval_keeps_original_price_basis_and_only_changes_effective_amount() {
    let ticket = ticket();
    let line = ticket.lines[0].clone();
    let (ledger, approved) = PriceOverrideLedger::default()
        .approve(
            &ticket,
            line.id,
            300,
            &actor(),
            "Synthetic concession",
            "approve-1",
            110,
        )
        .unwrap();
    assert_eq!(ticket.lines[0], line);
    assert_eq!(approved.lines[0].total_cents, Some(500));
    assert_eq!(approved.lines[0].effective_total_cents(), Some(300));
    assert_eq!(approved.total(), Some(300));
    assert_eq!(approved.lines[0].base_cents, Some(160));
    assert_eq!(approved.lines[0].minimum_cents, Some(340));
    assert_eq!(approved.lines[0].rule_snapshot, line.rule_snapshot);
    assert_eq!(approved.revision, ticket.revision + 1);
    ledger.validate_against(&[approved]).unwrap();
}

#[test]
fn clear_is_explicit_and_deletion_preserves_authorized_history() {
    let ticket = ticket();
    let id = ticket.lines[0].id;
    let (ledger, approved) = PriceOverrideLedger::default()
        .approve(
            &ticket,
            id,
            0,
            &actor(),
            "Training free route",
            "approve-free",
            110,
        )
        .unwrap();
    let (cleared_ledger, cleared) = ledger
        .clear(
            &approved,
            id,
            &actor(),
            "Return to original tariff",
            "clear-free",
            111,
        )
        .unwrap();
    assert!(cleared.lines[0].price_approval.is_none());
    assert_eq!(cleared.total(), Some(500));
    assert_eq!(cleared_ledger.events.len(), 2);
    let mut deleted = approved.clone();
    deleted.remove_line(0).unwrap();
    deleted.revision += 1;
    validate_ordinary_candidate(Some(&approved), &deleted).unwrap();
    ledger.validate_against(&[deleted]).unwrap();
    assert_eq!(ledger.events.len(), 1);
}

#[test]
fn ordinary_candidates_cannot_forge_strip_or_rebind_approval_authority() {
    let ticket = ticket();
    let id = ticket.lines[0].id;
    let (ledger, approved) = PriceOverrideLedger::default()
        .approve(&ticket, id, 300, &actor(), "Reason", "approve", 110)
        .unwrap();
    assert!(validate_ordinary_candidate(Some(&ticket), &approved).is_err());
    let mut stripped = approved.clone();
    stripped.lines[0].price_approval = None;
    assert!(validate_ordinary_candidate(Some(&approved), &stripped).is_err());
    assert!(ledger.validate_against(&[stripped]).is_err());
    let mut changed = approved.clone();
    changed.lines[0].quantity = "5".into();
    assert!(validate_ordinary_candidate(Some(&approved), &changed).is_err());
    assert!(ledger.validate_against(&[changed]).is_err());
    let mut source_edit = approved.clone();
    source_edit.source_intake.notes = "Internal source correction".into();
    source_edit.revision += 1;
    validate_ordinary_candidate(Some(&approved), &source_edit).unwrap();
    ledger.validate_against(&[source_edit]).unwrap();
}

#[test]
fn identical_requests_are_safe_after_completion_or_deletion_and_conflicts_fail() {
    let ticket = ticket();
    let id = ticket.lines[0].id;
    let (ledger, mut approved) = PriceOverrideLedger::default()
        .approve(&ticket, id, 300, &actor(), "Reason", "same-request", 110)
        .unwrap();
    let (same, same_ticket) = ledger
        .approve(&approved, id, 300, &actor(), "Reason", "same-request", 111)
        .unwrap();
    assert_eq!(same, ledger);
    assert_eq!(same_ticket, approved);
    assert!(
        ledger
            .approve(&approved, id, 301, &actor(), "Reason", "same-request", 111)
            .is_err()
    );
    assert!(
        ledger
            .clear(&approved, id, &actor(), "Reason", "same-request", 111)
            .is_err()
    );
    approved.confirm_cash("3").unwrap();
    approved.complete(Payment::Cash).unwrap();
    approved.revision += 1;
    let (same, completed) = ledger
        .approve(&approved, id, 300, &actor(), "Reason", "same-request", 999)
        .unwrap();
    assert_eq!(same, ledger);
    assert_eq!(completed, approved);
    assert!(
        ledger
            .clear(&approved, id, &actor(), "Clear", "clear-after-sale", 112)
            .is_err()
    );
    let mut deleted = same_ticket;
    deleted.remove_line(0).unwrap();
    deleted.revision += 1;
    assert_eq!(
        ledger
            .approve(&deleted, id, 300, &actor(), "Reason", "same-request", 112)
            .unwrap()
            .1,
        deleted
    );
}

#[test]
fn price_affecting_edits_require_clear_but_cleared_lines_can_be_reapproved() {
    let ticket = ticket();
    let id = ticket.lines[0].id;
    let (ledger, approved) = PriceOverrideLedger::default()
        .approve(&ticket, id, 300, &actor(), "Reason", "approve", 110)
        .unwrap();
    let (ledger, mut cleared) = ledger
        .clear(&approved, id, &actor(), "Correct weight", "clear", 111)
        .unwrap();
    cleared.edit_weight_line(0, "20", "0").unwrap();
    cleared.revision += 1;
    assert_eq!(cleared.lines[0].total_cents, Some(800));
    ledger
        .validate_against(std::slice::from_ref(&cleared))
        .unwrap();
    let (ledger, revised) = ledger
        .approve(
            &cleared,
            id,
            600,
            &actor(),
            "Revised concession",
            "approve-new-weight",
            112,
        )
        .unwrap();
    assert_eq!(revised.lines[0].total_cents, Some(800));
    assert_eq!(revised.total(), Some(600));
    assert_ne!(ledger.events[0].before.basis, ledger.events[2].before.basis);
    assert_eq!(ledger.events[2].ticket_revision, cleared.revision);
    ledger.validate_against(&[revised]).unwrap();
}

#[test]
fn every_tender_history_and_held_state_block_new_authority() {
    use crate::tender::CardOutcome;
    for outcome in [
        CardOutcome::Pending,
        CardOutcome::Declined,
        CardOutcome::Cancelled,
        CardOutcome::Unknown,
        CardOutcome::Approved,
    ] {
        let mut ticket = ticket();
        let id = ticket.lines[0].id;
        ticket.start_card("5", "SALE-1").unwrap();
        if outcome != CardOutcome::Pending {
            ticket
                .card_result(outcome, outcome == CardOutcome::Approved, false)
                .unwrap();
        }
        assert!(
            PriceOverrideLedger::default()
                .approve(&ticket, id, 300, &actor(), "Reason", "blocked", 110)
                .is_err()
        );
    }
    let mut cash = ticket();
    let id = cash.lines[0].id;
    cash.confirm_cash("5").unwrap();
    assert!(
        PriceOverrideLedger::default()
            .approve(&cash, id, 300, &actor(), "Reason", "cash-blocked", 110)
            .is_err()
    );
    let mut held = ticket();
    held.hold().unwrap();
    assert!(
        PriceOverrideLedger::default()
            .approve(&held, id, 300, &actor(), "Reason", "held-blocked", 110)
            .is_err()
    );
}

#[test]
fn legacy_unpriced_negative_and_unprivileged_requests_leave_no_history() {
    let ticket = ticket();
    let id = ticket.lines[0].id;
    let empty = PriceOverrideLedger::default();
    for amount in [-1, 100_000_000] {
        assert!(
            empty
                .approve(
                    &ticket,
                    id,
                    amount,
                    &actor(),
                    "Reason",
                    "invalid-amount",
                    110
                )
                .is_err()
        );
    }
    assert!(
        empty
            .approve(
                &ticket,
                id,
                300,
                &OperatorSnapshot {
                    role: Role::Cashier,
                    ..actor()
                },
                "Reason",
                "cashier",
                110
            )
            .is_err()
    );
    for explanation in ["", "Bad\nreason", &"A".repeat(501)] {
        assert!(
            empty
                .approve(
                    &ticket,
                    id,
                    300,
                    &actor(),
                    explanation,
                    "invalid-reason",
                    110
                )
                .is_err()
        );
    }
    assert!(
        empty
            .approve(&ticket, id, 300, &actor(), "Reason", "bad key", 110)
            .is_err()
    );
    let mut legacy = ticket.clone();
    legacy.lines[0].id = 0;
    assert!(
        empty
            .approve(&legacy, 0, 300, &actor(), "Reason", "legacy", 110)
            .is_err()
    );
    let mut unpriced = ticket.clone();
    unpriced.lines[0].total_cents = None;
    assert!(
        empty
            .approve(&unpriced, id, 300, &actor(), "Reason", "unpriced", 110)
            .is_err()
    );
    assert!(
        empty
            .approve(&ticket, id, 300, &actor(), "Reason", "before-created", 99)
            .is_err()
    );
    assert!(
        empty
            .clear(&ticket, id, &actor(), "No approval", "clear-empty", 110)
            .is_err()
    );
    assert!(empty.events.is_empty());
}

#[test]
fn replay_rejects_removed_old_or_cross_line_authority_and_corrupt_history() {
    let ticket = ticket();
    let id = ticket.lines[0].id;
    let (first, approved) = PriceOverrideLedger::default()
        .approve(&ticket, id, 300, &actor(), "Reason", "approve-1", 110)
        .unwrap();
    let (latest, current) = first
        .approve(
            &approved,
            id,
            400,
            &actor(),
            "Revised reason",
            "approve-2",
            111,
        )
        .unwrap();
    let mut old_ref = current.clone();
    old_ref.lines[0].price_approval = approved.lines[0].price_approval.clone();
    assert!(latest.validate_against(&[old_ref]).is_err());
    let mut copied = current.clone();
    copied.add_weight_line("803", "4", "0").unwrap();
    copied.lines[1].price_approval = copied.lines[0].price_approval.clone();
    copied.revision += 1;
    assert!(latest.validate_against(&[copied]).is_err());
    let mut bad = latest.clone();
    bad.next_id = 1;
    assert!(
        bad.validate_against(std::slice::from_ref(&current))
            .is_err()
    );
    bad = latest.clone();
    bad.events[1].at = 109;
    assert!(
        bad.validate_against(std::slice::from_ref(&current))
            .is_err()
    );
    bad = latest.clone();
    bad.events[1].actor.role = Role::Cashier;
    assert!(
        bad.validate_against(std::slice::from_ref(&current))
            .is_err()
    );
    bad = latest.clone();
    bad.events[1]
        .after
        .approval
        .as_mut()
        .unwrap()
        .approved_total_cents = 301;
    assert!(
        bad.validate_against(std::slice::from_ref(&current))
            .is_err()
    );
    assert!(latest.validate_against(&[]).is_err());
    assert!(
        latest
            .validate_against(&[current.clone(), current.clone()])
            .is_err()
    );
    let encoded = serde_json::to_string(&latest).unwrap();
    assert_eq!(
        serde_json::from_str::<PriceOverrideLedger>(&encoded).unwrap(),
        latest
    );
}

#[test]
fn zero_price_never_waives_quantity_customer_or_classification_rules() {
    let mut ticket = ticket();
    ticket.lines.clear();
    ticket.add_line("801", "1", "").unwrap();
    ticket.customer_choice = crate::workflow::CustomerChoice::Unset;
    let id = ticket.lines[0].id;
    let (_, mut free) = PriceOverrideLedger::default()
        .approve(
            &ticket,
            id,
            0,
            &actor(),
            "Training concession",
            "free-tv",
            110,
        )
        .unwrap();
    assert_eq!(free.lines[0].total_cents, Some(500));
    assert_eq!(free.total(), Some(0));
    assert!(free.choose_blank().is_err());
    assert!(free.confirm_cash("0").is_err());
    free.select_customer(&crate::customers::Customer {
        id: 1,
        number: "C1".into(),
        name: "Example Customer".into(),
        ..Default::default()
    })
    .unwrap();
    assert!(free.confirm_cash("0").is_err());
    free.line_intake(0, "Flat screen", "1", "").unwrap();
    free.confirm_cash("0").unwrap();
    free.complete(Payment::Cash).unwrap();
    assert_eq!(free.state, State::Completed);
}

#[test]
fn deleted_approved_line_ids_cannot_be_reused_on_new_materials() {
    let ticket = ticket();
    let id = ticket.lines[0].id;
    let (ledger, approved) = PriceOverrideLedger::default()
        .approve(&ticket, id, 300, &actor(), "Reason", "approve", 110)
        .unwrap();
    let mut deleted = approved.clone();
    deleted.remove_line(0).unwrap();
    deleted.revision += 1;
    let mut new = deleted.clone();
    new.add_weight_line("803", "4", "0").unwrap();
    new.revision += 1;
    assert_ne!(new.lines[0].id, id);
    assert!(new.lines[0].price_approval.is_none());
    validate_ordinary_candidate(Some(&deleted), &new).unwrap();
    ledger.validate_against(std::slice::from_ref(&new)).unwrap();
    new.lines[0].id = id;
    assert!(validate_ordinary_candidate(Some(&deleted), &new).is_err());
    assert!(ledger.validate_against(&[new]).is_err());
}

#[test]
fn cross_ticket_approval_time_cannot_move_backwards_or_be_rewritten_on_restore() {
    let first = ticket();
    let second = Ticket { id: 2, ..ticket() };
    let id = first.lines[0].id;
    let (ledger, first) = PriceOverrideLedger::default()
        .approve(&first, id, 300, &actor(), "First concession", "first", 120)
        .unwrap();
    assert!(
        ledger
            .approve(
                &second,
                id,
                300,
                &actor(),
                "Second concession",
                "second-backwards",
                119
            )
            .is_err()
    );
    let (mut ledger, second) = ledger
        .approve(
            &second,
            id,
            300,
            &actor(),
            "Second concession",
            "second",
            121,
        )
        .unwrap();
    ledger.events[1].at = 119;
    assert!(ledger.validate_against(&[first, second]).is_err());
}
