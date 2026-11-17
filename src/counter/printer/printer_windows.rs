//! Windows printer discovery and measured Unicode GDI receipt submission.

use crate::receipt_dispatch::DispatchOutcome;

const MAX_PAGES: usize = 1_000;
const MAX_RECEIPT: usize = 64 * 1_024;

fn validated_utf16(text: &str, limit: usize, multiline: bool) -> Result<Vec<u16>, String> {
    if text.len() > limit.saturating_mul(4) {
        return Err("Print text exceeds the supported length.".into());
    }
    if text.trim().is_empty()
        || text
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\r' | '\n' | '\t')))
    {
        return Err("Print text is empty or contains unsupported control characters.".into());
    }
    let mut wide: Vec<_> = text.encode_utf16().take(limit.saturating_add(1)).collect();
    if wide.len() > limit {
        return Err("Print text exceeds the supported length.".into());
    }
    wide.push(0);
    Ok(wide)
}

fn validate_input(printer: &str, title: &str, receipt: &str) -> Result<(), String> {
    validated_utf16(printer, 1_024, false)?;
    validated_utf16(title, 160, false)?;
    if receipt.len() > MAX_RECEIPT {
        return Err("Receipt exceeds the supported length.".into());
    }
    validated_utf16(receipt, MAX_RECEIPT, true)?;
    Ok(())
}

// Wrapping only inserts visual line breaks. Unicode scalar boundaries preserve UTF-16
// surrogate pairs, whitespace and amount text. Driver measurements include fallback fonts.
fn layout(
    text: &str,
    width: i32,
    lines_per_page: usize,
    mut measure: impl FnMut(&str) -> Result<i32, String>,
) -> Result<Vec<Vec<String>>, String> {
    if width <= 0 || lines_per_page == 0 {
        return Err("Printer has no usable printable area.".into());
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = Vec::new();
    for logical in normalized.split('\n') {
        let mut expanded = String::new();
        let mut column = 0;
        for c in logical.chars() {
            if c == '\t' {
                let spaces = 8 - column % 8;
                expanded.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            } else {
                expanded.push(c);
                column += 1;
            }
        }
        wrap_line(&expanded, width, &mut measure, &mut lines)?;
        if lines.len().div_ceil(lines_per_page) > MAX_PAGES {
            return Err("Receipt requires too many printed pages.".into());
        }
    }
    Ok(lines
        .chunks(lines_per_page)
        .map(|page| page.to_vec())
        .collect())
}

fn wrap_line(
    line: &str,
    width: i32,
    measure: &mut impl FnMut(&str) -> Result<i32, String>,
    lines: &mut Vec<String>,
) -> Result<(), String> {
    if line.is_empty() {
        lines.push(String::new());
        return Ok(());
    }
    let mut boundaries: Vec<_> = line.char_indices().map(|(index, _)| index).collect();
    boundaries.push(line.len());
    let mut start = 0;
    while start < boundaries.len() - 1 {
        let (mut low, mut high) = (start + 1, boundaries.len());
        while low < high {
            let middle = low + (high - low) / 2;
            let measured = measure(&line[boundaries[start]..boundaries[middle]])?;
            if measured < 0 {
                return Err("Printer returned invalid text measurements.".into());
            }
            if measured <= width {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut end = low - 1;
        if end == start {
            return Err("Printer area is too narrow for a receipt character.".into());
        }
        if end < boundaries.len() - 1 {
            // Prefer a complete word, including its trailing whitespace. Never trim
            // the frozen text; a word wider than the printable area still hard-wraps.
            end = (start + 1..=end)
                .rev()
                .find(|&index| {
                    line[boundaries[index - 1]..boundaries[index]]
                        .chars()
                        .next()
                        .is_some_and(char::is_whitespace)
                })
                .unwrap_or(end);
        }
        lines.push(line[boundaries[start]..boundaries[end]].to_owned());
        start = end;
    }
    Ok(())
}

trait PrintJob {
    fn start_document(&mut self) -> Result<u32, String>;
    fn start_page(&mut self) -> Result<(), String>;
    fn draw_line(&mut self, line: usize, text: &str) -> Result<(), String>;
    fn end_page(&mut self) -> Result<(), String>;
    fn end_document(&mut self) -> Result<(), String>;
    fn abort(&mut self);
}

fn spool(job: &mut impl PrintJob, pages: &[Vec<String>]) -> DispatchOutcome {
    if pages.is_empty() || pages.iter().any(Vec::is_empty) {
        return DispatchOutcome::NotSubmitted {
            detail: "No receipt pages to print.".into(),
        };
    }
    // Once StartDoc is attempted, a communication failure must never invite an
    // automatic retry. Even AbortDoc success cannot prove nothing reached paper.
    let result = (|| {
        let id = job.start_document()?;
        for page in pages {
            job.start_page()?;
            for (line, text) in page.iter().enumerate() {
                job.draw_line(line, text)?;
            }
            job.end_page()?;
        }
        job.end_document()?;
        Ok::<_, String>(id)
    })();
    match result {
        Ok(windows_job_id) => DispatchOutcome::Accepted { windows_job_id },
        Err(detail) => {
            job.abort();
            DispatchOutcome::Unknown { detail }
        }
    }
}

/// Returns installed local printer and connected printer names without changing defaults.
pub fn installed_printers() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        native::installed_printers()
    }
    #[cfg(not(windows))]
    {
        Err("Windows printing is unavailable on this platform.".into())
    }
}

/// Submits the supplied frozen receipt body; queue acceptance does not prove paper output.
/// Call on a worker thread because Windows drivers may block.
pub fn submit(printer: &str, title: &str, receipt: &str) -> DispatchOutcome {
    if let Err(detail) = validate_input(printer, title, receipt) {
        return DispatchOutcome::NotSubmitted { detail };
    }
    #[cfg(windows)]
    {
        native::submit_to_output(printer, title, receipt, None)
    }
    #[cfg(not(windows))]
    {
        DispatchOutcome::NotSubmitted {
            detail: "Windows printing is unavailable on this platform.".into(),
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{mem::size_of, path::Path, ptr};
    use windows_sys::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError, SIZE},
        Graphics::{Gdi::*, Printing::*},
        Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage},
    };

    fn windows_error(stage: &str) -> String {
        // Do not include user receipt content, printer names or file paths in errors.
        format!("{stage} failed (Windows error {}).", unsafe {
            GetLastError()
        })
    }

    pub(super) fn installed_printers() -> Result<Vec<String>, String> {
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        let (mut needed, mut count) = (0, 0);
        let first = unsafe {
            EnumPrintersW(
                flags,
                ptr::null(),
                4,
                ptr::null_mut(),
                0,
                &mut needed,
                &mut count,
            )
        };
        if first != 0 && needed == 0 {
            return Ok(Vec::new());
        }
        if first == 0 && unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
            return Err(windows_error("Printer discovery"));
        }
        for _ in 0..3 {
            if needed == 0 || needed > 16 * 1_024 * 1_024 {
                return Err("Printer discovery returned an invalid buffer size.".into());
            }
            // A word allocation satisfies PRINTER_INFO_4W alignment, unlike Vec<u8>.
            let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
            let capacity = (buffer.len() * size_of::<usize>()) as u32;
            let ok = unsafe {
                EnumPrintersW(
                    flags,
                    ptr::null(),
                    4,
                    buffer.as_mut_ptr().cast(),
                    capacity,
                    &mut needed,
                    &mut count,
                )
            };
            if ok != 0 {
                return names_from_buffer(&buffer, count as usize);
            }
            if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
                return Err(windows_error("Printer discovery"));
            }
        }
        Err("Printer list changed repeatedly; refresh the list.".into())
    }

    pub(super) fn names_from_buffer(buffer: &[usize], count: usize) -> Result<Vec<String>, String> {
        let bytes = std::mem::size_of_val(buffer);
        if count > bytes / size_of::<PRINTER_INFO_4W>() {
            return Err("Printer discovery returned invalid records.".into());
        }
        let start = buffer.as_ptr() as usize;
        let end = start + bytes;
        let records =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<PRINTER_INFO_4W>(), count) };
        let mut names = Vec::new();
        for record in records {
            let pointer = record.pPrinterName as usize;
            if pointer < start || pointer >= end || !pointer.is_multiple_of(2) {
                return Err("Printer discovery returned an invalid name pointer.".into());
            }
            let available = ((end - pointer) / 2).min(1_025);
            let wide = unsafe { std::slice::from_raw_parts(record.pPrinterName, available) };
            let length = wide
                .iter()
                .position(|&unit| unit == 0)
                .ok_or("Printer name is not terminated or is too long.")?;
            let name = String::from_utf16(&wide[..length])
                .map_err(|_| "Printer name is not valid Unicode.")?;
            validated_utf16(&name, 1_024, false)?;
            names.push(name);
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    struct GdiJob {
        dc: HDC,
        font: HFONT,
        previous_font: HGDIOBJ,
        title: Vec<u16>,
        output: Option<Vec<u16>>,
        margin_x: i32,
        margin_y: i32,
        line_height: i32,
        attempted: bool,
    }

    impl Drop for GdiJob {
        fn drop(&mut self) {
            unsafe {
                if self.attempted {
                    AbortDoc(self.dc);
                }
                if !self.previous_font.is_null() {
                    SelectObject(self.dc, self.previous_font);
                }
                if !self.font.is_null() {
                    DeleteObject(self.font);
                }
                DeleteDC(self.dc);
            }
        }
    }

    impl GdiJob {
        fn create(printer: &str, title: &str, output: Option<&Path>) -> Result<Self, String> {
            let printer = validated_utf16(printer, 1_024, false)?;
            let title = validated_utf16(title, 160, false)?;
            let output = output
                .map(|path| {
                    use std::os::windows::ffi::OsStrExt;
                    let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
                    if wide.is_empty() || wide.len() > 32_000 || wide.contains(&0) {
                        return Err("Invalid print output path.".to_owned());
                    }
                    wide.push(0);
                    Ok(wide)
                })
                .transpose()?;
            let driver: Vec<_> = "WINSPOOL\0".encode_utf16().collect();
            let dc =
                unsafe { CreateDCW(driver.as_ptr(), printer.as_ptr(), ptr::null(), ptr::null()) };
            if dc.is_null() {
                return Err(windows_error("Opening printer"));
            }
            let mut job = Self {
                dc,
                font: ptr::null_mut(),
                previous_font: ptr::null_mut(),
                title,
                output,
                margin_x: 0,
                margin_y: 0,
                line_height: 0,
                attempted: false,
            };
            job.configure()?;
            Ok(job)
        }

        fn configure(&mut self) -> Result<(), String> {
            let dpi_x = unsafe { GetDeviceCaps(self.dc, LOGPIXELSX as i32) };
            let dpi_y = unsafe { GetDeviceCaps(self.dc, LOGPIXELSY as i32) };
            if !(36..=9_600).contains(&dpi_x) || !(36..=9_600).contains(&dpi_y) {
                return Err("Printer returned invalid resolution.".into());
            }
            let face: Vec<_> = "Consolas\0".encode_utf16().collect();
            self.font = unsafe {
                CreateFontW(
                    -(dpi_y * 10 / 72),
                    0,
                    0,
                    0,
                    FW_NORMAL as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    DEFAULT_QUALITY as u32,
                    (FIXED_PITCH | FF_MODERN) as u32,
                    face.as_ptr(),
                )
            };
            if self.font.is_null() {
                return Err(windows_error("Creating receipt font"));
            }
            let previous = unsafe { SelectObject(self.dc, self.font) };
            if previous.is_null() || previous as isize == -1 {
                return Err(windows_error("Selecting receipt font"));
            }
            self.previous_font = previous;
            if unsafe { SetBkMode(self.dc, TRANSPARENT as i32) } == 0
                || unsafe { SetTextColor(self.dc, 0) } == u32::MAX
            {
                return Err(windows_error("Configuring receipt text"));
            }
            let mut metrics = TEXTMETRICW::default();
            if unsafe { GetTextMetricsW(self.dc, &mut metrics) } == 0 {
                return Err(windows_error("Measuring receipt font"));
            }
            self.line_height = metrics
                .tmHeight
                .checked_add(metrics.tmExternalLeading)
                .and_then(|height| height.checked_add((dpi_y / 72).max(1)))
                .filter(|height| (1..=100_000).contains(height))
                .ok_or("Printer returned invalid font metrics.")?;
            self.margin_x = (dpi_x / 12).max(1);
            self.margin_y = (dpi_y / 12).max(1);
            Ok(())
        }

        fn pages(&self, receipt: &str) -> Result<Vec<Vec<String>>, String> {
            let width = unsafe { GetDeviceCaps(self.dc, HORZRES as i32) };
            let height = unsafe { GetDeviceCaps(self.dc, VERTRES as i32) };
            if !(1..=1_000_000).contains(&width) || !(1..=1_000_000).contains(&height) {
                return Err("Printer returned invalid printable dimensions.".into());
            }
            let usable_width = width - 2 * self.margin_x;
            let usable_height = height - 2 * self.margin_y;
            if usable_height <= 0 {
                return Err("Printer has no usable printable area.".into());
            }
            layout(
                receipt,
                usable_width,
                (usable_height / self.line_height) as usize,
                |text| {
                    let wide: Vec<_> = text.encode_utf16().collect();
                    let mut size = SIZE::default();
                    if unsafe {
                        GetTextExtentPoint32W(self.dc, wide.as_ptr(), wide.len() as i32, &mut size)
                    } == 0
                    {
                        return Err(windows_error("Measuring receipt text"));
                    }
                    Ok(size.cx)
                },
            )
        }
    }

    impl PrintJob for GdiJob {
        fn start_document(&mut self) -> Result<u32, String> {
            let info = DOCINFOW {
                cbSize: size_of::<DOCINFOW>() as i32,
                lpszDocName: self.title.as_ptr(),
                lpszOutput: self
                    .output
                    .as_ref()
                    .map_or(ptr::null(), |output| output.as_ptr()),
                lpszDatatype: ptr::null(),
                fwType: 0,
            };
            self.attempted = true;
            let id = unsafe { StartDocW(self.dc, &info) };
            if id <= 0 {
                return Err(windows_error("Starting receipt job"));
            }
            Ok(id as u32)
        }
        fn start_page(&mut self) -> Result<(), String> {
            if unsafe { StartPage(self.dc) } <= 0 {
                return Err(windows_error("Starting receipt page"));
            }
            Ok(())
        }
        fn draw_line(&mut self, line: usize, text: &str) -> Result<(), String> {
            if text.is_empty() {
                return Ok(());
            }
            let wide: Vec<_> = text.encode_utf16().collect();
            let y = self.margin_y + line as i32 * self.line_height;
            if unsafe { TextOutW(self.dc, self.margin_x, y, wide.as_ptr(), wide.len() as i32) } == 0
            {
                return Err(windows_error("Drawing receipt text"));
            }
            Ok(())
        }
        fn end_page(&mut self) -> Result<(), String> {
            if unsafe { EndPage(self.dc) } <= 0 {
                return Err(windows_error("Finishing receipt page"));
            }
            Ok(())
        }
        fn end_document(&mut self) -> Result<(), String> {
            if unsafe { EndDoc(self.dc) } <= 0 {
                return Err(windows_error("Finishing receipt job"));
            }
            self.attempted = false;
            Ok(())
        }
        fn abort(&mut self) {
            if self.attempted {
                unsafe {
                    AbortDoc(self.dc);
                }
                self.attempted = false;
            }
        }
    }

    pub(super) fn submit_to_output(
        printer: &str,
        title: &str,
        receipt: &str,
        output: Option<&Path>,
    ) -> DispatchOutcome {
        let preflight = (|| {
            validate_input(printer, title, receipt)?;
            let job = GdiJob::create(printer, title, output)?;
            let pages = job.pages(receipt)?;
            Ok::<_, String>((job, pages))
        })();
        match preflight {
            Ok((mut job, pages)) => spool(&mut job, &pages),
            Err(detail) => DispatchOutcome::NotSubmitted { detail },
        }
    }
}

#[cfg(test)]
#[path = "../../tests/counter/printer_windows_tests.rs"]
mod tests;
