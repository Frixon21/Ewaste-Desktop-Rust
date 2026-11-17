//! Stored approved totals remain readable when their optional editing workflow
//! is excluded. This module contains no approval commands or history projector.
use crate::domain::{State, Ticket};
pub use crate::record_compat::PriceApproval;

pub fn validate_ordinary_candidate(
    previous: Option<&Ticket>,
    candidate: &Ticket,
) -> Result<(), String> {
    if let Some(old) = previous {
        if old.state != State::Completed
            && old.lines.iter().any(|line| line.price_approval.is_some())
            && old != candidate
        {
            return Err("This saved draft has an existing price approval and is read-only in the walkthrough build.".into());
        }
        for line in &candidate.lines {
            let original = old
                .lines
                .iter()
                .find(|old| line.id > 0 && old.id == line.id);
            if original.and_then(|line| line.price_approval.as_ref())
                != line.price_approval.as_ref()
            {
                return Err(
                    "Price approval cannot be added, removed or changed in the walkthrough build."
                        .into(),
                );
            }
        }
    } else if candidate
        .lines
        .iter()
        .any(|line| line.price_approval.is_some())
    {
        return Err("New walkthrough records cannot contain price approval authority.".into());
    }
    Ok(())
}
