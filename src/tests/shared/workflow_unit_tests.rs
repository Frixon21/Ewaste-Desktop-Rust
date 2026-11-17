use super::*;
#[test]
fn blank_policy_is_independent_of_fee_and_pending_card_blocks_account() {
    let mut t = Ticket::new(1);
    t.add_weight_line("803", "21", "1").unwrap();
    t.choose_blank().unwrap();
    assert_eq!(t.total(), Some(800));
    t.start_card("8", "EXAMPLE-A").unwrap();
    assert!(
        t.select_account(&BusinessAccount {
            id: 1,
            number: "A1".into(),
            name: "Example".into(),
            active: true
        })
        .is_err()
    );
    assert!(t.start_card("0", "EXAMPLE-B").is_err());
    t.card_result(CardOutcome::Declined, false, false).unwrap();
    t.confirm_cash("10").unwrap();
    t.complete(Payment::Cash).unwrap();
}
#[test]
fn reference_without_evidence_never_completes() {
    let mut t = Ticket::new(1);
    t.add_weight_line("803", "4", "0").unwrap();
    t.choose_blank().unwrap();
    assert!(
        t.complete(Payment::ExternalCard {
            reference: "TEST-1".into()
        })
        .is_err()
    );
    assert_eq!(t.state, State::Draft);
    assert_eq!(t.tender_evidence, crate::tender::TenderEvidence::default());
}
