use super::*;
use crate::{
    customers::{BusinessAccount, Customer},
    domain::{Payment, State},
    intake::MaterialClass,
    workflow::CustomerChoice,
};
fn draft(code: &str, quantity: &str, manual: &str) -> Ticket {
    let mut t = Ticket::new(1);
    t.created_at = 1;
    t.add_line(code, quantity, manual).unwrap();
    t
}
fn validate(previous: Option<&Ticket>, candidate: &Ticket) -> Result<(), String> {
    validate_counter_candidate(
        previous,
        candidate,
        &Catalog::default(),
        &Directory::default(),
        1,
    )
}
#[test]
fn fixed_prices_and_saved_rules_cannot_be_forged_by_whole_ticket_candidates() {
    let original = draft("801", "1", "");
    validate(None, &original).unwrap();
    for mutate in 0..5 {
        let mut changed = original.clone();
        let line = &mut changed.lines[0];
        match mutate {
            0 => {
                line.total_cents = Some(1);
                line.base_cents = Some(1);
            }
            1 => line.rate_cents = Some(1),
            2 => line.minimum_cents = Some(100),
            3 => line.unit = Some("lb".into()),
            _ => line.rule_snapshot.as_mut().unwrap().rate_cents = Some(1),
        }
        assert!(validate(Some(&original), &changed).is_err());
        assert!(validate(None, &changed).is_err());
    }
    let mut corrected = original.clone();
    corrected.line_edit(0, "2", "").unwrap();
    validate(Some(&original), &corrected).unwrap();
    assert_eq!(corrected.total(), Some(1000));
}
#[test]
fn manual_unknown_and_explicit_zero_prices_follow_the_saved_rule() {
    let pending = draft("901", "", "");
    validate(None, &pending).unwrap();
    let mut priced = pending.clone();
    priced.line_edit(0, "", "0").unwrap();
    validate(Some(&pending), &priced).unwrap();
    priced.line_edit(0, "", "2.75").unwrap();
    validate(Some(&pending), &priced).unwrap();
    let mut invalid = priced.clone();
    invalid.lines[0].total_cents = Some(-1);
    assert!(validate(Some(&priced), &invalid).is_err());
    let mut bad_rule = priced.clone();
    bad_rule.lines[0].rule_snapshot = None;
    assert!(validate(Some(&priced), &bad_rule).is_err());
}
#[test]
fn weight_evidence_and_normalized_billing_measurements_must_agree() {
    let mut t = Ticket::new(1);
    t.add_weight_line("803", "21", "1").unwrap();
    validate(None, &t).unwrap();
    let mut corrected = t.clone();
    corrected.edit_weight_line(0, "31", "1").unwrap();
    validate(Some(&t), &corrected).unwrap();
    for mutate in 0..4 {
        let mut bad = t.clone();
        match mutate {
            0 => bad.lines[0].tare_thousandths = None,
            1 => bad.lines[0].gross_thousandths = Some(999),
            2 => bad.lines[0].gross_thousandths = Some(-1),
            _ => bad.lines[0].quantity = ".5".into(),
        };
        assert!(validate(Some(&t), &bad).is_err());
    }
    let mut count = draft("801", "1", "");
    count.lines[0].gross_thousandths = Some(1000);
    count.lines[0].tare_thousandths = Some(0);
    assert!(validate(None, &count).is_err());
}
#[test]
fn metadata_only_changes_preserve_money_but_taken_tender_and_holds_lock_intake() {
    let t = draft("803", "4", "");
    let mut source = t.clone();
    source.source_intake.circumstances = "Synthetic source review".into();
    source.lines[0].classification = "Household".into();
    source.lines[0].piece_count = Some(7);
    source.lines[0].material_intake.class = MaterialClass::LooseBattery;
    validate(Some(&t), &source).unwrap();
    let mut held = t.clone();
    held.hold().unwrap();
    validate(Some(&t), &held).unwrap();
    let mut resume = held.clone();
    resume.resume().unwrap();
    validate(Some(&held), &resume).unwrap();
    resume.lines[0].notes = "Cannot edit while simultaneously resuming".into();
    assert!(validate(Some(&held), &resume).is_err());
    let mut paid = t.clone();
    paid.choose_blank().unwrap();
    paid.confirm_cash("5").unwrap();
    validate(Some(&t), &paid).unwrap();
    let mut changed = paid.clone();
    changed.source_intake.notes = "Changed after cash".into();
    assert!(validate(Some(&paid), &changed).is_err());
}
#[test]
fn new_line_must_use_effective_catalog_but_existing_draft_keeps_old_price() {
    let old = draft("803", "4", "");
    let mut catalog = Catalog::default();
    let mut revised = catalog.rule_for("803", 1).unwrap().clone();
    revised.rate_cents = Some(200);
    revised.minimum_cents = Some(0);
    catalog = catalog.save_rule(&revised).unwrap();
    validate_counter_candidate(Some(&old), &old, &catalog, &Directory::default(), 1).unwrap();
    assert!(validate_counter_candidate(None, &old, &catalog, &Directory::default(), 1).is_err());
    let mut next = old.clone();
    next.add_catalog_line(&catalog, "803", "4", "0", "", 1)
        .unwrap();
    validate_counter_candidate(Some(&old), &next, &catalog, &Directory::default(), 1).unwrap();
    let mut empty = catalog.clone();
    empty.rules.clear();
    assert!(validate_counter_candidate(None, &next, &empty, &Directory::default(), 1).is_err());
}
#[test]
fn customers_and_business_accounts_are_real_saved_separate_records() {
    let base = draft("803", "4", "");
    let directory = Directory::default();
    let (directory, customer) = directory
        .save_customer(&Customer {
            name: "Synthetic person".into(),
            ..Default::default()
        })
        .unwrap();
    let (directory, account) = directory
        .save_account(&BusinessAccount {
            name: "Synthetic business".into(),
            active: true,
            ..Default::default()
        })
        .unwrap();
    let mut selected = base.clone();
    selected.select_customer(&customer).unwrap();
    selected.select_account(&account).unwrap();
    validate_counter_candidate(Some(&base), &selected, &Catalog::default(), &directory, 1).unwrap();
    let mut forged = selected.clone();
    forged.customer_record.as_mut().unwrap().phone = "Unknown saved phone".into();
    assert!(
        validate_counter_candidate(Some(&base), &forged, &Catalog::default(), &directory, 1)
            .is_err()
    );
    forged = selected.clone();
    forged.customer = Some("Wrong person".into());
    assert!(
        validate_counter_candidate(Some(&selected), &forged, &Catalog::default(), &directory, 1)
            .is_err()
    );
    let mut blank = selected.clone();
    blank.customer_choice = CustomerChoice::Blank;
    assert!(
        validate_counter_candidate(Some(&selected), &blank, &Catalog::default(), &directory, 1)
            .is_err()
    );
    let mut complete = selected.clone();
    complete
        .complete(Payment::ChargeTo {
            account: account.number.clone(),
        })
        .unwrap();
    let mut disabled = directory.clone();
    disabled.accounts[0].active = false;
    assert!(
        validate_counter_candidate(
            Some(&selected),
            &complete,
            &Catalog::default(),
            &disabled,
            1
        )
        .is_err()
    );
    let mut renamed = directory.clone();
    renamed.accounts[0].name = "Renamed business".into();
    validate_counter_candidate(Some(&selected), &complete, &Catalog::default(), &renamed, 1)
        .unwrap();
    let mut historical = complete.clone();
    historical.reprint().unwrap();
    validate_counter_candidate(
        Some(&complete),
        &historical,
        &Catalog::default(),
        &disabled,
        1,
    )
    .unwrap();
}
#[test]
fn unchanged_legacy_headers_and_unsnapshotted_lines_remain_readable_but_safe_edits_recompute() {
    let mut legacy = draft("803", "4", "");
    legacy.lines[0].id = 0;
    legacy.lines[0].rule_snapshot = None;
    legacy.next_line_id = 1;
    legacy.customer = Some("Legacy name".into());
    legacy.customer_choice = CustomerChoice::Unset;
    validate(Some(&legacy), &legacy).unwrap();
    let mut changed = legacy.clone();
    changed.line_edit(0, "20", "").unwrap();
    validate(Some(&legacy), &changed).unwrap();
    assert_eq!(changed.total(), Some(800));
    changed.lines[0].total_cents = Some(1);
    assert!(validate(Some(&legacy), &changed).is_err());
    let mut missing = legacy.clone();
    missing.lines[0].rule_snapshot = Catalog::default().rule_for("803", 1).cloned();
    assert!(validate(Some(&legacy), &missing).is_err());
    let mut new_header = legacy.clone();
    new_header.customer = Some("Another unsaved name".into());
    assert!(validate(Some(&legacy), &new_header).is_err());
}
#[test]
fn invalid_metadata_and_negative_unchanged_legacy_money_are_rejected() {
    let mut legacy = draft("901", "", "2");
    legacy.lines[0].rule_snapshot = None;
    legacy.lines[0].rate_cents = Some(-1);
    assert!(validate(Some(&legacy), &legacy).is_err());
    let original = draft("803", "4", "");
    let mut bad = original.clone();
    bad.lines[0].classification = "Bad\nclassification".into();
    assert!(validate(Some(&original), &bad).is_err());
    bad = original.clone();
    bad.lines[0].piece_count = Some(0);
    assert!(validate(Some(&original), &bad).is_err());
    bad = original.clone();
    bad.lines[0].notes = "bad\0note".into();
    assert!(validate(Some(&original), &bad).is_err());
    let mut held = original.clone();
    held.state = State::Held;
    validate(Some(&original), &held).unwrap();
}
#[test]
fn pending_or_unknown_card_history_cannot_be_erased_for_cash_fallback() {
    use crate::tender::CardOutcome;
    let mut pending = draft("803", "4", "");
    pending.choose_blank().unwrap();
    pending.start_card("5", "TEST-ORIGINAL").unwrap();
    let mut erased = pending.clone();
    erased.tender_evidence = Default::default();
    erased.confirm_cash("5").unwrap();
    assert!(validate(Some(&pending), &erased).is_err());
    let mut unknown = pending.clone();
    unknown
        .card_result(CardOutcome::Unknown, false, false)
        .unwrap();
    validate(Some(&pending), &unknown).unwrap();
    let mut forged = unknown.clone();
    forged.tender_evidence.card_attempts.clear();
    assert!(validate(Some(&unknown), &forged).is_err());
    let mut approved = unknown.clone();
    approved
        .card_result(CardOutcome::Approved, true, true)
        .unwrap();
    validate(Some(&unknown), &approved).unwrap();
    let mut changed = approved.clone();
    changed.tender_evidence.card_attempts[0].reference = "TEST-OTHER".into();
    assert!(validate(Some(&approved), &changed).is_err());
    changed = approved.clone();
    changed.tender_evidence.card_attempts[0].outcome = CardOutcome::Declined;
    assert!(validate(Some(&approved), &changed).is_err());
}
#[test]
fn tender_transitions_replay_normal_commands_and_reject_extra_attempts_or_changed_cash() {
    use crate::tender::CardOutcome;
    let mut original = draft("803", "4", "");
    original.choose_blank().unwrap();
    let mut pending = original.clone();
    pending.start_card("5", "TEST-ONE").unwrap();
    validate(Some(&original), &pending).unwrap();
    let mut declined = pending.clone();
    declined
        .card_result(CardOutcome::Declined, false, false)
        .unwrap();
    validate(Some(&pending), &declined).unwrap();
    let mut retried = declined.clone();
    retried.start_card("5", "TEST-TWO").unwrap();
    validate(Some(&declined), &retried).unwrap();
    assert!(validate(Some(&original), &retried).is_err());
    let mut cash = declined.clone();
    cash.confirm_cash("10").unwrap();
    validate(Some(&declined), &cash).unwrap();
    let mut erased = cash.clone();
    erased.tender_evidence.cash = None;
    assert!(validate(Some(&cash), &erased).is_err());
    let mut changed = cash.clone();
    changed
        .tender_evidence
        .cash
        .as_mut()
        .unwrap()
        .received_cents = 500;
    changed.tender_evidence.cash.as_mut().unwrap().change_cents = 0;
    assert!(validate(Some(&cash), &changed).is_err());
    let mut wrong = pending.clone();
    wrong.tender_evidence.card_attempts[0].amount_cents = 1;
    assert!(validate(Some(&original), &wrong).is_err());
}

#[test]
fn held_tickets_require_resume_before_tender_or_completion() {
    let mut draft = draft("803", "4", "");
    draft.choose_blank().unwrap();
    let mut held = draft.clone();
    held.hold().unwrap();
    let mut cash = draft.clone();
    cash.confirm_cash("5").unwrap();
    cash.state = State::Held;
    assert!(validate(Some(&held), &cash).is_err());
    let mut held_cash = cash.clone();
    let mut completed = cash.clone();
    completed.state = State::Draft;
    completed.complete(Payment::Cash).unwrap();
    assert!(validate(Some(&held_cash), &completed).is_err());
    held_cash.resume().unwrap();
    validate(Some(&cash), &held_cash).unwrap();
}
