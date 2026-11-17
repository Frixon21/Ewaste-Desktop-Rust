use super::*;

#[test]
fn all_materials_complete() {
    let mut ticket = Ticket::new(1);
    #[cfg(not(feature = "extended-workflows"))]
    {
        ticket.source_intake.category = crate::intake::SourceCategory::Residential;
    }
    for item in CATALOG {
        ticket
            .add_line(
                item.code,
                if item.unit == Some("lb") { "4" } else { "" },
                if item.unit.is_none() { "2.75" } else { "" },
            )
            .unwrap();
    }
    assert_eq!(ticket.total(), Some(2925));
    ticket
        .select_customer(&crate::customers::Customer {
            id: 1,
            number: "C1".into(),
            name: "Example".into(),
            ..Default::default()
        })
        .unwrap();
    for index in 0..ticket.lines.len() {
        ticket
            .line_intake(index, "Training classification", "", "")
            .unwrap();
    }
    ticket
        .confirm_cash(&format!(
            "{}.{:02}",
            ticket.total().unwrap() / 100,
            ticket.total().unwrap() % 100
        ))
        .unwrap();
    ticket.complete(Payment::Cash).unwrap();
    assert!(ticket.receipt().contains("$29.25"));
    assert!(ticket.remove_line(0).is_err());
}

#[test]
fn price_edit_hold_and_charge() {
    let mut ticket = Ticket::new(15);
    ticket.add_line("901", "", "").unwrap();
    assert!(ticket.complete(Payment::Cash).is_err());
    ticket.line_edit(0, "", "0").unwrap();
    ticket.line_intake(0, "", "4", "Internal").unwrap();
    ticket.hold().unwrap();
    assert!(ticket.add_line("801", "", "").is_err());
    ticket.resume().unwrap();
    ticket
        .select_customer(&crate::customers::Customer {
            id: 1,
            number: "C1".into(),
            name: "Example".into(),
            ..Default::default()
        })
        .unwrap();
    ticket
        .select_account(&crate::customers::BusinessAccount {
            id: 1,
            number: "EXAMPLE-1".into(),
            name: "Example business".into(),
            active: true,
        })
        .unwrap();
    ticket
        .complete(Payment::ChargeTo {
            account: "EXAMPLE-1".into(),
        })
        .unwrap();
    ticket.reprint().unwrap();
    assert!(ticket.receipt().contains("Pending billing"));
    assert!(!ticket.receipt().contains("Internal"));
}

#[test]
fn external_validation_and_weight_rounding() {
    let mut ticket = Ticket::new(1);
    assert!(ticket.add_line("803", "0", "").is_err());
    ticket.add_line("803", ".5", "").unwrap();
    ticket.line_edit(0, "12.52", "").unwrap();
    assert_eq!(ticket.total(), Some(501));
    assert!(
        ticket
            .complete(Payment::ExternalCard {
                reference: "".into()
            })
            .is_err()
    );
    ticket.choose_blank().unwrap();
    ticket.start_card("5.01", "TEST-1").unwrap();
    ticket
        .card_result(crate::tender::CardOutcome::Approved, true, false)
        .unwrap();
    ticket
        .complete(Payment::ExternalCard {
            reference: "TEST-1".into(),
        })
        .unwrap();
    assert!(ticket.receipt().contains("TEST-1"));
}
