use super::*;
use crate::scanner::ScanCandidate;

fn candidate(name: &str, address: &str) -> ScanCandidate {
    ScanCandidate {
        name: name.into(),
        address: address.into(),
    }
}
fn customer(name: &str, address: &str, phone: &str) -> Customer {
    Customer {
        name: name.into(),
        address: address.into(),
        phone: phone.into(),
        ..Default::default()
    }
}

#[test]
fn scan_review_is_immutable_and_new_customer_contains_only_approved_name_and_address() {
    let directory = Directory::default();
    let before = serde_json::to_string(&directory).unwrap();
    let review = directory
        .review_scan(&candidate(
            "  Jane Q Doe  ",
            "  10 Example Street, Test City CA  ",
        ))
        .unwrap();
    assert_eq!(
        review.customer,
        customer("Jane Q Doe", "10 Example Street, Test City CA", "")
    );
    assert!(review.matches.is_empty());
    assert!(review.prompt.contains("explicitly"));
    assert_eq!(serde_json::to_string(&directory).unwrap(), before);
}

#[test]
fn normalized_name_order_suggests_stable_ids_without_selecting_even_a_unique_record() {
    let (directory, first) = Directory::default()
        .save_customer(&customer("Jane Q Doe", "10 Example Street", "555-0100"))
        .unwrap();
    let review = directory
        .review_scan(&candidate("DOE,  jane q", "Different approved address"))
        .unwrap();
    assert_eq!(review.matches, vec![first.id]);
    assert_eq!(review.customer.id, 0);
    assert!(review.customer.number.is_empty());
    assert_eq!(directory.customers[0], first);
    assert!(!review.prompt.to_lowercase().contains("verified"));
    let (directory, second) = directory
        .save_customer(&customer("Jane Q Doe", "20 Other Street", "555-0200"))
        .unwrap();
    let duplicate = directory
        .review_scan(&candidate("q Doe Jane", "Different approved address"))
        .unwrap();
    assert_eq!(duplicate.matches, vec![first.id, second.id]);
    assert_eq!(duplicate.customer.id, 0);
    assert_eq!(directory.customers.len(), 2);
    let mut reordered = directory.clone();
    reordered.customers.reverse();
    assert_eq!(
        reordered
            .review_scan(&candidate("Doe Jane Q", "Different approved address"))
            .unwrap()
            .matches,
        vec![first.id, second.id]
    );
}

#[test]
fn same_address_candidates_remain_distinct_and_business_accounts_are_excluded() {
    let (directory, first) = Directory::default()
        .save_customer(&customer(
            "First Person",
            "10 Oak Lane, Test City CA",
            "555-0100",
        ))
        .unwrap();
    let (directory, second) = directory
        .save_customer(&customer(
            "Second Person",
            "10 Oak Lane, Test City CA",
            "555-0200",
        ))
        .unwrap();
    let review = directory
        .review_scan(&candidate("Another Person", "  10 OAK lane  TEST city CA "))
        .unwrap();
    assert_eq!(review.matches, vec![first.id, second.id]);
    let (accounts, _) = Directory::default()
        .save_account(&BusinessAccount {
            number: "BUSINESS-1".into(),
            name: "Another Person".into(),
            active: true,
            ..Default::default()
        })
        .unwrap();
    assert!(
        accounts
            .review_scan(&candidate("Another Person", "10 Oak Lane"))
            .unwrap()
            .matches
            .is_empty()
    );
}

#[test]
fn blank_scanned_phone_never_updates_existing_customer_and_explicit_create_is_separate() {
    let (directory, saved) = Directory::default()
        .save_customer(&customer("Jane Doe", "Original saved address", "555-0100"))
        .unwrap();
    let review = directory
        .review_scan(&candidate("Doe Jane", "Reviewed scanned address"))
        .unwrap();
    assert_eq!(review.matches, vec![saved.id]);
    assert!(review.customer.phone.is_empty());
    assert_eq!(directory.customers[0], saved);
    let (created, new) = directory.save_customer(&review.customer).unwrap();
    assert_ne!(new.id, saved.id);
    assert!(new.phone.is_empty());
    assert_eq!(
        created
            .customers
            .iter()
            .find(|customer| customer.id == saved.id),
        Some(&saved)
    );
    assert_eq!(directory.customers, vec![saved]);
}

#[test]
fn invalid_candidates_and_corrupt_directories_fail_without_partial_review_or_mutation() {
    let (directory, saved) = Directory::default()
        .save_customer(&customer("Jane Doe", "Saved address", "555-0100"))
        .unwrap();
    let before = directory.clone();
    for input in [
        candidate("", "Address"),
        candidate("Jane\nDoe", "Address"),
        candidate(&"N".repeat(121), "Address"),
        candidate("Jane Doe", &"A".repeat(241)),
        candidate("Jane Doe", "Address\0private"),
        candidate("...", "Address"),
        candidate("Jane Doe", "---"),
    ] {
        assert!(directory.review_scan(&input).is_err());
        assert_eq!(directory, before);
    }
    let mut corrupt = directory.clone();
    corrupt.customers.push(saved);
    assert!(
        corrupt
            .review_scan(&candidate("Jane Doe", "Saved address"))
            .is_err()
    );
}

#[test]
fn matching_does_not_concatenate_tokens_or_erase_repeated_name_parts() {
    let (directory, _) = Directory::default()
        .save_customer(&customer("Ann A Bell", "Distinct address", "555-0100"))
        .unwrap();
    assert!(
        directory
            .review_scan(&candidate("Anna Bell", "Another address"))
            .unwrap()
            .matches
            .is_empty()
    );
    assert!(
        directory
            .review_scan(&candidate("Ann A A Bell", "Another address"))
            .unwrap()
            .matches
            .is_empty()
    );
}
