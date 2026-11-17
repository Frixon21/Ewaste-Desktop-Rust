use super::*;
#[test]
fn ticket_history_distinguishes_numbers_names_and_accounts() {
    let mut t = Ticket::new(42);
    t.customer = Some("Example Person".into());
    t.account_record = Some(crate::customers::BusinessAccount {
        id: 1,
        number: "A000001".into(),
        name: "Example business".into(),
        active: true,
    });
    for query in ["", "#000042", "example person", "A000001", "BUSINESS"] {
        assert!(history_matches(&t, query));
    }
    assert!(!history_matches(&t, "000043"));
}
