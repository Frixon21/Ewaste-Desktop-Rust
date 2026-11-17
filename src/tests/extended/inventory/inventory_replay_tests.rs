use super::*;

#[test]
fn receiving_requires_deliberate_confirmation_saved_completion_and_supported_stream() {
    let original = ticket(1);
    let directory = InventoryDirectory::default();
    let reviews = ReviewLedger::default();
    let input = ReceiveInput {
        confirmed: false,
        ..receive_input(1)
    };
    assert!(
        InventoryLedger::default()
            .receive(
                std::slice::from_ref(&original),
                &reviews,
                &directory,
                &input,
                &actor(),
                "receive",
                200
            )
            .is_err()
    );
    let mut draft = original.clone();
    draft.state = crate::domain::State::Draft;
    assert!(
        InventoryLedger::default()
            .receive(
                &[draft],
                &reviews,
                &directory,
                &receive_input(1),
                &actor(),
                "receive",
                200
            )
            .is_err()
    );
    for class in [
        MaterialClass::LooseBattery,
        MaterialClass::Oil,
        MaterialClass::OilFilter,
    ] {
        let mut outside = original.clone();
        outside.lines[0].material_intake.class = class;
        assert!(
            InventoryLedger::default()
                .receive(
                    &[outside],
                    &reviews,
                    &directory,
                    &receive_input(1),
                    &actor(),
                    "receive",
                    200
                )
                .is_err()
        );
    }
    let cashier = OperatorSnapshot {
        role: Role::Cashier,
        ..actor()
    };
    assert!(
        InventoryLedger::default()
            .receive(
                &[original],
                &reviews,
                &directory,
                &receive_input(1),
                &cashier,
                "receive",
                200
            )
            .is_err()
    );
}

#[test]
fn bad_dates_values_locations_and_conflicting_request_keys_preserve_the_ledger() {
    let original = ticket(1);
    let (ledger, _) = received(&original);
    let directory = InventoryDirectory::default();
    for input in [
        ReceiveInput {
            received_at: Some(201),
            ..receive_input(1)
        },
        ReceiveInput {
            received_at: Some(100),
            first_accumulation_at: Some(105),
            ..receive_input(1)
        },
        ReceiveInput {
            location_id: 999,
            ..receive_input(1)
        },
        ReceiveInput {
            container: "bad\ncontainer".into(),
            ..receive_input(1)
        },
    ] {
        assert!(
            InventoryLedger::default()
                .receive(
                    std::slice::from_ref(&original),
                    &ReviewLedger::default(),
                    &directory,
                    &input,
                    &actor(),
                    "receive",
                    200
                )
                .is_err()
        );
    }
    let conflict = ReceiveInput {
        container: "CHANGED".into(),
        ..receive_input(1)
    };
    assert!(
        ledger
            .receive(
                &[original],
                &ReviewLedger::default(),
                &directory,
                &conflict,
                &actor(),
                "receive-1",
                210
            )
            .is_err()
    );
    assert!(
        ledger
            .move_lot(&directory, 1, 1, "OTHER", "Moved", &actor(), "bad key", 210)
            .is_err()
    );
    assert!(
        ledger
            .move_lot(&directory, 1, 1, "OTHER", "Moved", &actor(), "move", 199)
            .is_err()
    );
    assert_eq!(ledger.events.len(), 1);
}

#[test]
fn shipment_dates_weights_and_confirmation_are_validated_without_defaulting_unknowns() {
    let original = ticket(1);
    let (ledger, _) = received(&original);
    for input in [
        ShipmentInput {
            confirmed: false,
            ..shipment(vec![1])
        },
        ShipmentInput {
            shipped_at: Some(109),
            ..shipment(vec![1])
        },
        ShipmentInput {
            shipped_at: Some(231),
            ..shipment(vec![1])
        },
        ShipmentInput {
            actual_weight_thousandths: Some(0),
            ..shipment(vec![1])
        },
        ShipmentInput {
            evidence_reference: String::new(),
            ..shipment(vec![1])
        },
        ShipmentInput {
            destination: PartySnapshot::default(),
            ..shipment(vec![1])
        },
    ] {
        assert!(
            ledger
                .ship(
                    &InventoryDirectory::default(),
                    &input,
                    &actor(),
                    "ship",
                    230
                )
                .is_err()
        );
    }
    let unknown = ShipmentInput {
        shipped_at: None,
        ..shipment(vec![1])
    };
    let (next, shipment) = ledger
        .ship(
            &InventoryDirectory::default(),
            &unknown,
            &actor(),
            "ship",
            230,
        )
        .unwrap();
    assert_eq!(shipment.shipped_at, None);
    assert_eq!(shipment.actual_weight_thousandths, None);
    assert_eq!(
        next.project().unwrap().lots[0]
            .origin
            .material
            .actual_weight_thousandths,
        None
    );
}

#[test]
fn replay_rejects_tampered_origin_actor_movement_and_sequences() {
    let original = ticket(1);
    let (ledger, _) = received(&original);
    let directory = InventoryDirectory::default();
    let mut bad_origin = ledger.clone();
    if let InventoryAction::Received { lot } = &mut bad_origin.events[0].action {
        lot.origin.material.reporting_units = Some(99);
    }
    assert!(bad_origin.project().is_ok());
    assert!(
        bad_origin
            .validate_against(
                std::slice::from_ref(&original),
                &ReviewLedger::default(),
                &directory
            )
            .is_err()
    );
    let mut bad_actor = ledger.clone();
    bad_actor.events[0].actor.role = Role::Cashier;
    assert!(bad_actor.project().is_err());
    let mut bad_sequence = ledger.clone();
    bad_sequence.next_lot_id = 99;
    assert!(bad_sequence.project().is_err());
    let (moved, _) = ledger
        .move_lot(
            &directory,
            1,
            1,
            "NEW-CONTAINER",
            "Move",
            &actor(),
            "move",
            210,
        )
        .unwrap();
    let mut bad_move = moved.clone();
    if let InventoryAction::Moved { from_container, .. } = &mut bad_move.events[1].action {
        *from_container = "WRONG".into();
    }
    assert!(bad_move.project().is_err());
    let restored: InventoryLedger =
        serde_json::from_str(&serde_json::to_string(&moved).unwrap()).unwrap();
    assert_eq!(restored, moved);
    restored
        .validate_against(&[original], &ReviewLedger::default(), &directory)
        .unwrap();
}

#[test]
fn an_exact_review_pin_cannot_reference_a_different_ticket_or_future_review() {
    let original = ticket(1);
    let snapshot = ReviewSnapshot::from_ticket(&original).unwrap();
    let (reviews, _) = ReviewLedger::default()
        .record(&original, &snapshot, "Supervisor", "Review", "review", 250)
        .unwrap();
    let input = ReceiveInput {
        review_event_id: Some(1),
        ..receive_input(1)
    };
    assert!(
        InventoryLedger::default()
            .receive(
                std::slice::from_ref(&original),
                &reviews,
                &InventoryDirectory::default(),
                &input,
                &actor(),
                "receive",
                200
            )
            .is_err()
    );
    let originals = [original, ticket(2)];
    let input = ReceiveInput {
        ticket_id: 2,
        review_event_id: Some(1),
        ..receive_input(1)
    };
    assert!(
        InventoryLedger::default()
            .receive(
                &originals,
                &reviews,
                &InventoryDirectory::default(),
                &input,
                &actor(),
                "receive",
                300
            )
            .is_err()
    );
}
