use crate::{
    domain::{Payment, Ticket},
    price_overrides::PriceApproval,
};

#[test]
fn approved_total_drives_cash_receipt_and_export_without_erasing_original_tariff() {
    let mut ticket = Ticket::new(1);
    ticket.add_weight_line("803", "20", "0").unwrap();
    ticket.choose_blank().unwrap();
    ticket.lines[0].price_approval = Some(PriceApproval {
        event_id: 1,
        approved_total_cents: 300,
    });
    assert_eq!(ticket.lines[0].total_cents, Some(800));
    assert_eq!(ticket.total(), Some(300));
    ticket.confirm_cash("5").unwrap();
    ticket.complete(Payment::Cash).unwrap();
    assert_eq!(
        ticket.tender_evidence.cash.as_ref().unwrap().change_cents,
        200
    );
    let receipt = ticket.receipt();
    assert!(receipt.contains("803 Other E-waste (20 lb)  $3.00"));
    assert!(receipt.contains("Original line total $8.00 / Authorized line total $3.00"));
    let csv = crate::intake_export::export_intake(&[ticket], &Default::default(), false).unwrap();
    let mut rows = csv.lines();
    let headers: Vec<_> = rows.next().unwrap().split(',').collect();
    let values: Vec<_> = rows.next().unwrap().split(',').collect();
    for (name, want) in [
        ("training_billing_line_total_cents", "800"),
        ("training_approved_line_total_cents", "300"),
        ("training_effective_line_total_cents", "300"),
        ("training_price_approval_event_id", "1"),
    ] {
        let index = headers.iter().position(|header| *header == name).unwrap();
        assert_eq!(values[index], want);
    }
}

#[test]
fn billed_measurement_changes_require_clearing_approval_but_detail_edits_keep_it() {
    let mut ticket = Ticket::new(1);
    ticket.add_weight_line("803", "20", "0").unwrap();
    let approval = PriceApproval {
        event_id: 1,
        approved_total_cents: 300,
    };
    ticket.lines[0].price_approval = Some(approval.clone());
    let original = ticket.clone();
    assert!(ticket.edit_weight_line(0, "21", "0").is_err());
    assert_eq!(ticket, original);
    ticket.edit_weight_line(0, "20", "0").unwrap();
    ticket
        .line_intake(0, "Training class", "2", "Internal note")
        .unwrap();
    assert_eq!(ticket.lines[0].price_approval, Some(approval));
    ticket.hold().unwrap();
    ticket.resume().unwrap();
    assert_eq!(ticket.total(), Some(300));
}
