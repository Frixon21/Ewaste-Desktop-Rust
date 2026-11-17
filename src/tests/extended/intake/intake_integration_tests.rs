use crate::{
    customers::Customer,
    domain::{Payment, State, Ticket},
    intake::*,
};

#[test]
fn line_ids_survive_price_intake_edits_and_are_never_reused_after_deletion() {
    let mut t = Ticket::new(1);
    t.add_weight_line("803", "4", "0").unwrap();
    t.add_line("801", "1", "").unwrap();
    let first = t.lines[0].id;
    let second = t.lines[1].id;
    assert!(first > 0 && second > first);
    t.remove_line(0).unwrap();
    t.add_weight_line("803", "20", "0").unwrap();
    assert!(t.lines[1].id > second);
    t.line_edit(0, "2", "").unwrap();
    assert_eq!(t.lines[0].id, second);
    let details = MaterialIntake {
        class: MaterialClass::NonCrtDisplay,
        reporting_units: Some(3),
        ..Default::default()
    };
    t = t.with_material_intake(0, &details).unwrap();
    t.line_edit(0, "1", "").unwrap();
    assert_eq!(t.lines[0].material_intake, details);
    let details = MaterialIntake {
        class: MaterialClass::OtherElectronics,
        reporting_units: Some(5),
        actual_weight_thousandths: Some(17000),
        ..Default::default()
    };
    t = t.with_material_intake(1, &details).unwrap();
    let id = t.lines[1].id;
    t.edit_weight_line(1, "30", "1").unwrap();
    assert_eq!(t.lines[1].id, id);
    assert_eq!(t.lines[1].material_intake, details);
    assert_eq!(t.lines[1].quantity, "29");
}

#[test]
fn source_draft_is_separate_from_customer_tender_and_receipt() {
    let mut t = Ticket::new(7);
    t.add_weight_line("803", "4", "0").unwrap();
    let customer = Customer {
        id: 1,
        number: "C1".into(),
        name: "Synthetic person".into(),
        address: "1 Example street".into(),
        phone: "555-0100".into(),
        ..Default::default()
    };
    t.select_customer(&customer).unwrap();
    let source = SourceIntake {
        category: SourceCategory::Residential,
        delivery: DeliveryMode::DropOff,
        reason: IntakeReason::Faulty,
        circumstances: "Broken household device".into(),
        ..SourceIntake::from_customer(&customer)
    };
    let before = t.clone();
    let updated = t.with_source_intake(&source).unwrap();
    assert_eq!(t, before);
    assert_eq!(updated.total(), t.total());
    assert_eq!(updated.lines, t.lines);
    t = updated;
    let source_before = t.source_intake.clone();
    t.select_customer(&Customer {
        id: 2,
        number: "C2".into(),
        name: "Another person".into(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(t.source_intake, source_before);
    assert!(!t.intake_issues().is_empty());
    t.confirm_cash("5").unwrap();
    assert!(t.with_source_intake(&SourceIntake::default()).is_err());
    t.complete(Payment::Cash).unwrap();
    let receipt = t.receipt();
    assert_eq!(t.state, State::Completed);
    assert!(!receipt.contains("Example street"));
    assert!(!receipt.contains("Broken household device"));
    assert!(!receipt.contains("555-0100"));
    assert!(
        t.with_material_intake(0, &MaterialIntake::default())
            .is_err()
    );
}

#[test]
fn old_payloads_load_with_unknown_source_and_no_reporting_measurements() {
    let mut t = Ticket::new(1);
    t.add_weight_line("803", "4", "0").unwrap();
    let mut json = serde_json::to_value(&t).unwrap();
    json.as_object_mut().unwrap().remove("source_intake");
    json.as_object_mut().unwrap().remove("next_line_id");
    let line = json["lines"][0].as_object_mut().unwrap();
    line.remove("material_intake");
    line.remove("id");
    let legacy: Ticket = serde_json::from_value(json).unwrap();
    assert_eq!(legacy.source_intake, SourceIntake::default());
    assert_eq!(legacy.lines[0].material_intake, MaterialIntake::default());
    assert_eq!(legacy.lines[0].id, 0);
    assert!(!legacy.intake_issues().is_empty());
}
