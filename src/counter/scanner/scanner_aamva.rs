//! Narrow keyboard profile, based on AAMVA 2026 Annex D.6/D.12 tables D.1–D.4.
//! https://www.aamva.org/getmedia/feab821e-baa5-4524-b865-3eb172b51acd/2026-DLID-Card-Design-Standard.pdf
//!
//! Accepted transport normalization is explicit: the header RS may be dropped;
//! the header CR may become LF; body CRLF becomes LF; a declared subfile's final
//! CR may become LF. Original offsets always address reconstructed ISO-8859-1
//! bytes, including the full canonical 21-byte header, type bytes and terminators.
//! Other prefixes, keyboard layouts, tab separators and extra payloads are rejected.
//! This is text extraction for human review, never card/identity/signature validation.
use super::{MAX_CAPTURE_BYTES, ScanCandidate, ScanError};
use zeroize::Zeroizing;

const PREFIX: &[u8] = b"@\n\x1e\rANSI ";

/// Extracts only name and address from one bounded, declared AAMVA keyboard scan.
/// The caller must not persist `raw`; CaptureSession wipes its owned capture after review.
pub fn decode_aamva_keyboard(raw: &str) -> Result<ScanCandidate, ScanError> {
    if raw.is_empty() {
        return Err(ScanError::Empty);
    }
    if raw.len() > MAX_CAPTURE_BYTES {
        return Err(ScanError::TooLong);
    }
    let canonical = canonicalize(raw)?;
    let subfiles = subfiles(&canonical)?;
    let selected = subfiles
        .iter()
        .find(|entry| matches!(&entry.kind, b"DL" | b"ID"))
        .ok_or(ScanError::InvalidSubfiles)?;
    extract_fields(&canonical[selected.offset + 2..selected.end - 1])
}

fn decimal(bytes: &[u8]) -> Result<usize, ScanError> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(ScanError::InvalidHeader);
    }
    Ok(bytes
        .iter()
        .fold(0, |value, digit| value * 10 + usize::from(digit - b'0')))
}

fn header_length(bytes: &[u8]) -> Result<usize, ScanError> {
    if bytes.len() < 21 || !bytes.starts_with(PREFIX) {
        return Err(ScanError::InvalidHeader);
    }
    if decimal(&bytes[9..15])? == 0 {
        return Err(ScanError::InvalidHeader);
    }
    if !(9..=12).contains(&decimal(&bytes[15..17])?) {
        return Err(ScanError::UnsupportedVersion);
    }
    decimal(&bytes[17..19])?;
    let count = decimal(&bytes[19..21])?;
    if count == 0 {
        return Err(ScanError::InvalidHeader);
    }
    let length = 21 + count * 10;
    if bytes.len() < length {
        return Err(ScanError::InvalidHeader);
    }
    Ok(length)
}

fn canonicalize(raw: &str) -> Result<Zeroizing<Vec<u8>>, ScanError> {
    let mut latin1 = Zeroizing::new(Vec::with_capacity(MAX_CAPTURE_BYTES));
    for character in raw.chars() {
        let byte = u8::try_from(u32::from(character)).map_err(|_| ScanError::UnsupportedProfile)?;
        latin1.push(byte);
    }
    let prefixes: &[&[u8]] = &[PREFIX, b"@\n\x1e\nANSI ", b"@\n\rANSI ", b"@\n\nANSI "];
    let observed = prefixes
        .iter()
        .find(|prefix| latin1.starts_with(prefix))
        .ok_or(ScanError::UnsupportedProfile)?;
    if latin1.len() + PREFIX.len() - observed.len() > MAX_CAPTURE_BYTES {
        return Err(ScanError::TooLong);
    }
    let mut header = Zeroizing::new(Vec::with_capacity(MAX_CAPTURE_BYTES));
    header.extend_from_slice(PREFIX);
    header.extend_from_slice(&latin1[observed.len()..]);
    let length = header_length(&header)?;
    let mut canonical = Zeroizing::new(Vec::with_capacity(MAX_CAPTURE_BYTES));
    canonical.extend_from_slice(&header[..length]);
    let mut index = length;
    while index < header.len() {
        if header[index..].starts_with(b"\r\n") {
            canonical.push(b'\n');
            index += 2;
        } else {
            canonical.push(header[index]);
            index += 1;
        }
    }
    // Only declared terminal positions may translate Enter/LF back to CR. A wrong
    // length cannot be repaired by searching for a nearby field or segment boundary.
    for entry in header[21..length].as_chunks::<10>().0 {
        let end = decimal(&entry[2..6])?
            .checked_add(decimal(&entry[6..10])?)
            .ok_or(ScanError::InvalidSubfiles)?;
        if end == 0 || end > canonical.len() {
            return Err(ScanError::InvalidSubfiles);
        }
        if canonical[end - 1] == b'\n' {
            canonical[end - 1] = b'\r';
        }
    }
    Ok(canonical)
}

struct Subfile {
    kind: [u8; 2],
    offset: usize,
    end: usize,
}

fn subfiles(bytes: &[u8]) -> Result<Vec<Subfile>, ScanError> {
    let header = header_length(bytes)?;
    let mut entries = Vec::new();
    let mut identity_count = 0;
    for entry in bytes[21..header].as_chunks::<10>().0 {
        let kind = [entry[0], entry[1]];
        let identity = matches!(&kind, b"DL" | b"ID");
        if !identity && !(kind[0] == b'Z' && kind[1].is_ascii_uppercase()) {
            return Err(ScanError::InvalidSubfiles);
        }
        if entries
            .iter()
            .any(|previous: &Subfile| previous.kind == kind)
        {
            return Err(ScanError::InvalidSubfiles);
        }
        identity_count += usize::from(identity);
        let offset = decimal(&entry[2..6])?;
        let length = decimal(&entry[6..10])?;
        let end = offset
            .checked_add(length)
            .ok_or(ScanError::InvalidSubfiles)?;
        if length < 3
            || offset < header
            || end > bytes.len()
            || bytes[offset..offset + 2] != kind
            || bytes[end - 1] != b'\r'
        {
            return Err(ScanError::InvalidSubfiles);
        }
        if bytes[offset + 2..end - 1]
            .iter()
            .any(|&byte| byte != b'\n' && !(0x20..=0x7e).contains(&byte) && byte < 0xa0)
        {
            return Err(ScanError::InvalidFields);
        }
        entries.push(Subfile { kind, offset, end });
    }
    if identity_count != 1 {
        return Err(ScanError::InvalidSubfiles);
    }
    // The named profile accepts one fully covered payload, not gaps or appended scans.
    entries.sort_by_key(|entry| entry.offset);
    let mut expected = header;
    for entry in &entries {
        if entry.offset != expected {
            return Err(ScanError::InvalidSubfiles);
        }
        expected = entry.end;
    }
    if expected != bytes.len() {
        return Err(ScanError::InvalidSubfiles);
    }
    Ok(entries)
}

const FIELDS: [&[u8]; 13] = [
    b"DCS", b"DAC", b"DAD", b"DCU", b"DAG", b"DAH", b"DAI", b"DAJ", b"DAK", b"DCG", b"DDE", b"DDF",
    b"DDG",
];
const LIMITS: [usize; 13] = [40, 40, 40, 5, 35, 35, 20, 2, 9, 3, 1, 1, 1];

fn extract_fields(data: &[u8]) -> Result<ScanCandidate, ScanError> {
    let mut selected: [Option<&[u8]>; 13] = [None; 13];
    let mut lines = data.split(|&byte| byte == b'\n').peekable();
    while let Some(line) = lines.next() {
        if line.is_empty() && lines.peek().is_none() {
            continue;
        }
        if line.len() < 3 || !line[..3].iter().all(u8::is_ascii_uppercase) {
            return Err(ScanError::InvalidFields);
        }
        // Borrow selected fields only; ignored identifiers, DOB, signatures and
        // jurisdiction values are never copied into a candidate, log or error.
        if let Some(index) = FIELDS.iter().position(|&id| id == &line[..3]) {
            if selected[index].is_some() {
                return Err(ScanError::AmbiguousFields);
            }
            let value = trim_ascii(&line[3..]);
            if value.len() > LIMITS[index] {
                return Err(ScanError::InvalidFields);
            }
            selected[index] = Some(value);
        }
    }
    for value in &selected[10..] {
        if *value != Some(b"N".as_slice()) {
            return Err(ScanError::AmbiguousFields);
        }
    }
    for index in [0, 1, 4, 6, 7, 8] {
        if selected[index].is_none_or(unavailable) {
            return Err(ScanError::MissingFields);
        }
    }
    if selected[7].is_none_or(|value| value.len() != 2 || !value.iter().all(u8::is_ascii_uppercase))
    {
        return Err(ScanError::InvalidFields);
    }
    if let Some(country) = selected[9]
        && !matches!(country, b"USA" | b"CAN")
    {
        return Err(ScanError::InvalidFields);
    }
    let name = join_fields(&selected, &[1, 2, 0, 3], " ");
    let street = join_fields(&selected, &[4, 5], ", ");
    let city = join_fields(&selected, &[6], "");
    let region = join_fields(&selected, &[7, 8], " ");
    let country = join_fields(&selected, &[9], "");
    let address = [street, city, region, country]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let candidate = ScanCandidate { name, address };
    candidate.validate()?;
    Ok(candidate)
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first() == Some(&b' ') {
        value = &value[1..];
    }
    while value.last() == Some(&b' ') {
        value = &value[..value.len() - 1];
    }
    value
}

fn unavailable(value: &[u8]) -> bool {
    value.is_empty() || value.eq_ignore_ascii_case(b"NONE") || value.eq_ignore_ascii_case(b"unavl")
}

fn join_fields(fields: &[Option<&[u8]>; 13], indices: &[usize], separator: &str) -> String {
    indices
        .iter()
        .filter_map(|&index| fields[index])
        .filter(|value| !unavailable(value))
        .map(|value| {
            value
                .iter()
                .map(|&byte| char::from(byte))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
#[path = "../../tests/counter/scanner_aamva_tests.rs"]
pub(crate) mod tests;
