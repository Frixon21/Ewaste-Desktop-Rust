//! Transient keyboard scan capture. Only reviewed name/address leave this module.
use zeroize::{Zeroize, Zeroizing};

pub const MAX_CAPTURE_BYTES: usize = 8_192;
pub const KEYBOARD_PROFILE: &str = "AAMVA ANSI v09–12";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanCandidate {
    pub name: String,
    pub address: String,
}

impl ScanCandidate {
    /// Checks the only two fields allowed to leave scanner review.
    pub fn validate(&self) -> Result<(), ScanError> {
        if self.name.trim().is_empty() || self.address.trim().is_empty() {
            return Err(ScanError::MissingFields);
        }
        if self.name.chars().count() > 120
            || self.address.chars().count() > 240
            || self
                .name
                .chars()
                .chain(self.address.chars())
                .any(char::is_control)
        {
            return Err(ScanError::InvalidFields);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanError {
    Empty,
    TooLong,
    UnsupportedProfile,
    InvalidHeader,
    UnsupportedVersion,
    InvalidSubfiles,
    InvalidFields,
    MissingFields,
    AmbiguousFields,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "No scanner input was captured. Enter the customer manually or scan again.",
            Self::TooLong => "Scanner input is too long. Capture was cleared; enter the customer manually.",
            Self::UnsupportedProfile => "Scanner output does not match the named keyboard profile. Enter the customer manually.",
            Self::InvalidHeader => "Scanner header is incomplete or invalid. Enter the customer manually or scan again.",
            Self::UnsupportedVersion => "This scanner data version is unsupported. Enter the customer manually.",
            Self::InvalidSubfiles => "Scanner segment boundaries are invalid or ambiguous. Enter the customer manually.",
            Self::InvalidFields => "Scanner fields are malformed or incomplete. Enter the customer manually.",
            Self::MissingFields => "Scanner name or address is incomplete. Enter the customer manually.",
            Self::AmbiguousFields => "Scanner name or address is duplicated or truncated. Enter the customer manually.",
        })
    }
}
impl std::error::Error for ScanError {}

pub struct CaptureSession {
    raw: Zeroizing<String>,
}
impl Default for CaptureSession {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Debug for CaptureSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CaptureSession")
            .field("raw", &"[redacted]")
            .field("len", &self.len())
            .finish()
    }
}
impl CaptureSession {
    pub fn new() -> Self {
        Self {
            raw: Zeroizing::new(String::with_capacity(MAX_CAPTURE_BYTES)),
        }
    }
    /// Adds one keyboard event. Overflow clears the complete capture, never truncates it.
    pub fn append_text(&mut self, text: &str) -> Result<(), ScanError> {
        if text.len() > MAX_CAPTURE_BYTES.saturating_sub(self.raw.len()) {
            self.clear();
            return Err(ScanError::TooLong);
        }
        // Capacity was reserved once, so appending cannot free an unwiped allocation.
        self.raw.push_str(text);
        Ok(())
    }
    /// Removes one Unicode scalar while wiping the former allocation.
    pub fn backspace(&mut self) {
        if let Some((last, _)) = self.raw.char_indices().next_back() {
            let mut replacement = Zeroizing::new(String::with_capacity(MAX_CAPTURE_BYTES));
            replacement.push_str(&self.raw[..last]);
            self.raw = replacement;
        }
    }
    pub fn clear(&mut self) {
        self.raw.zeroize();
    }
    pub fn len(&self) -> usize {
        self.raw.len()
    }
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
    /// Produces an unverified review candidate and wipes capture on either outcome.
    pub fn review(&mut self) -> Result<ScanCandidate, ScanError> {
        let result = decode_aamva_keyboard(&self.raw);
        self.clear();
        result
    }
}

#[path = "scanner_aamva.rs"]
mod aamva;
pub use aamva::decode_aamva_keyboard;

#[cfg(test)]
#[path = "../../tests/counter/scanner_tests.rs"]
mod tests;
