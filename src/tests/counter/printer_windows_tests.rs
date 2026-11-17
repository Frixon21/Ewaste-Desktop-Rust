use super::*;

fn measure(text: &str) -> Result<i32, String> {
    Ok(text.chars().map(|c| if c == '界' { 2 } else { 1 }).sum())
}

#[test]
fn wraps_using_measured_width_without_losing_unicode_or_amounts() {
    let text = "Customer copy\nZoë 界🙂  $123.45  ";
    let pages = layout(text, 9, 3, measure).unwrap();
    let lines: Vec<_> = pages.iter().flatten().collect();
    assert_eq!(
        lines.iter().map(|s| s.as_str()).collect::<String>(),
        text.replace('\n', "")
    );
    assert!(lines.iter().all(|s| measure(s).unwrap() <= 9));
    assert!(pages.iter().all(|page| page.len() <= 3));
    assert!(pages.len() > 1);
}

#[test]
fn wraps_at_whitespace_to_keep_fitting_words_and_money_tokens_whole() {
    let text = "Customer: Synthetic customer name $123.45 paid";
    let pages = layout(text, 18, 20, measure).unwrap();
    let lines: Vec<_> = pages.iter().flatten().map(String::as_str).collect();
    assert_eq!(lines.concat(), text);
    assert!(lines.contains(&"Customer: "));
    assert!(lines.iter().any(|line| line.contains("$123.45")));
    assert!(lines.iter().all(|line| measure(line).unwrap() <= 18));
    assert!(!lines.iter().any(|line| line.ends_with("custome")));
}

#[test]
fn oversized_words_still_hard_wrap_losslessly_with_unicode_and_whitespace() {
    let text = "  超長🙂unbrokenword  $123.45";
    let pages = layout(text, 8, 20, measure).unwrap();
    let lines: Vec<_> = pages.iter().flatten().map(String::as_str).collect();
    assert_eq!(lines.concat(), text);
    assert!(lines.iter().all(|line| measure(line).unwrap() <= 8));
    assert!(lines.iter().any(|line| line.contains("$123.45")));
    assert!(lines.len() > 3);
}

#[test]
fn preserves_blank_lines_and_normalizes_only_line_endings_and_tabs() {
    let pages = layout("A\r\n\rB\tC\n", 80, 20, measure).unwrap();
    assert_eq!(pages, vec![vec!["A", "", "B       C", ""]]);
}

#[test]
fn rejects_impossible_geometry_measurement_and_page_explosion() {
    assert!(layout("x", 0, 1, measure).is_err());
    assert!(layout("x", 1, 0, measure).is_err());
    assert!(layout("界", 1, 1, measure).is_err());
    assert!(layout("x", 10, 1, |_| Err("measurement failed".into())).is_err());
    assert!(layout(&"x\n".repeat(MAX_PAGES + 1), 1, 1, measure).is_err());
}

#[test]
fn validates_before_ffi_without_trimming_or_lossy_unicode_conversion() {
    assert_eq!(
        validated_utf16(" Zoë 🙂 ", 30, false).unwrap(),
        " Zoë 🙂 \0".encode_utf16().collect::<Vec<_>>()
    );
    for value in ["", "  ", "x\0y", "x\ny", "x\u{7f}y"] {
        assert!(validated_utf16(value, 30, false).is_err());
    }
    assert!(validated_utf16("🙂", 1, false).is_err());
    assert!(validated_utf16("receipt\nline\t2\r\n", 40, true).is_ok());
    assert!(validated_utf16("bad\u{1b}command", 40, true).is_err());
}

#[derive(Default)]
struct FakeJob {
    fail: &'static str,
    calls: Vec<&'static str>,
}

impl PrintJob for FakeJob {
    fn start_document(&mut self) -> Result<u32, String> {
        self.calls.push("start_document");
        if self.fail == "start_document" {
            Err("start_document".into())
        } else {
            Ok(37)
        }
    }
    fn start_page(&mut self) -> Result<(), String> {
        self.step("start_page")
    }
    fn draw_line(&mut self, _: usize, _: &str) -> Result<(), String> {
        self.step("draw_line")
    }
    fn end_page(&mut self) -> Result<(), String> {
        self.step("end_page")
    }
    fn end_document(&mut self) -> Result<(), String> {
        self.step("end_document")
    }
    fn abort(&mut self) {
        self.calls.push("abort");
    }
}

impl FakeJob {
    fn step(&mut self, name: &'static str) -> Result<(), String> {
        self.calls.push(name);
        if self.fail == name {
            Err(name.into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn accepts_only_after_every_page_and_end_document_succeed() {
    let mut job = FakeJob::default();
    assert!(matches!(
        spool(&mut job, &[vec!["one".into()], vec!["two".into()]]),
        DispatchOutcome::Accepted { windows_job_id: 37 }
    ));
    assert_eq!(
        job.calls,
        [
            "start_document",
            "start_page",
            "draw_line",
            "end_page",
            "start_page",
            "draw_line",
            "end_page",
            "end_document"
        ]
    );
}

#[test]
fn any_failure_after_start_document_is_uncertain_and_never_retries() {
    for fail in [
        "start_document",
        "start_page",
        "draw_line",
        "end_page",
        "end_document",
    ] {
        let mut job = FakeJob {
            fail,
            ..Default::default()
        };
        assert!(
            matches!(
                spool(&mut job, &[vec!["x".into()]]),
                DispatchOutcome::Unknown { .. }
            ),
            "{fail}"
        );
        assert_eq!(job.calls.last(), Some(&"abort"));
        assert_eq!(
            job.calls
                .iter()
                .filter(|&&call| call == "start_document")
                .count(),
            1
        );
    }
}

#[test]
fn empty_layout_is_never_submitted() {
    let mut job = FakeJob::default();
    assert!(matches!(
        spool(&mut job, &[]),
        DispatchOutcome::NotSubmitted { .. }
    ));
    assert!(job.calls.is_empty());
}

#[test]
fn invalid_user_input_is_rejected_without_contacting_any_printer() {
    for (printer, title, body) in [
        ("", "title", "body"),
        ("printer", "", "body"),
        ("printer", "title", ""),
        ("x\0y", "title", "body"),
        ("printer", "title", "x\u{1b}y"),
    ] {
        assert!(matches!(
            submit(printer, title, body),
            DispatchOutcome::NotSubmitted { .. }
        ));
    }
    assert!(validate_input(&"p".repeat(1_025), "title", "body").is_err());
    assert!(validate_input("printer", &"t".repeat(161), "body").is_err());
    assert!(validate_input("printer", "title", &"x".repeat(MAX_RECEIPT + 1)).is_err());
}

#[cfg(windows)]
fn printer_buffer(names: &[&[u16]]) -> Vec<usize> {
    use windows_sys::Win32::Graphics::Printing::PRINTER_INFO_4W;
    let record_bytes = names.len() * std::mem::size_of::<PRINTER_INFO_4W>();
    let text_bytes = names.iter().map(|name| name.len() * 2).sum::<usize>();
    let mut buffer =
        vec![0usize; (record_bytes + text_bytes).div_ceil(std::mem::size_of::<usize>())];
    let mut offset = record_bytes;
    for (index, name) in names.iter().enumerate() {
        unsafe {
            let text = buffer.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>();
            std::ptr::copy_nonoverlapping(name.as_ptr(), text, name.len());
            buffer
                .as_mut_ptr()
                .cast::<PRINTER_INFO_4W>()
                .add(index)
                .write(PRINTER_INFO_4W {
                    pPrinterName: text,
                    pServerName: std::ptr::null_mut(),
                    Attributes: 0,
                });
        }
        offset += name.len() * 2;
    }
    buffer
}

#[cfg(windows)]
#[test]
fn discovery_parser_preserves_unicode_sorts_and_deduplicates_names() {
    let first: Vec<_> = "Zoë 🙂\0".encode_utf16().collect();
    let second: Vec<_> = "Printer 1\0".encode_utf16().collect();
    let buffer = printer_buffer(&[&first, &second, &first]);
    assert_eq!(
        native::names_from_buffer(&buffer, 3).unwrap(),
        ["Printer 1", "Zoë 🙂"]
    );
    assert_eq!(
        native::names_from_buffer(&[], 0).unwrap(),
        Vec::<String>::new()
    );
}

#[cfg(windows)]
#[test]
fn discovery_parser_rejects_invalid_counts_pointers_unicode_and_lengths() {
    use windows_sys::Win32::Graphics::Printing::PRINTER_INFO_4W;
    assert!(native::names_from_buffer(&[0], 1).is_err());
    let mut buffer = printer_buffer(&[&[b'X' as u16, 0]]);
    unsafe {
        (*buffer.as_mut_ptr().cast::<PRINTER_INFO_4W>()).pPrinterName = std::ptr::null_mut();
    }
    assert!(native::names_from_buffer(&buffer, 1).is_err());
    let buffer = printer_buffer(&[&[0xD800, 0]]);
    assert!(native::names_from_buffer(&buffer, 1).is_err());
    let buffer = printer_buffer(&[&vec![b'X' as u16; 1_026]]);
    assert!(native::names_from_buffer(&buffer, 1).is_err());
    let buffer = printer_buffer(&[&[0]]);
    assert!(native::names_from_buffer(&buffer, 1).is_err());
}

#[cfg(windows)]
#[test]
#[ignore = "Opt-in PDF driver check; explicit workspace output only; never a physical printer"]
fn microsoft_pdf_only_to_new_workspace_verification_file() {
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".verification")
        .join("printer")
        .join(format!(
            "receipt-gdi-{}-{}.pdf",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    assert!(!output.exists());
    assert!(
        installed_printers()
            .unwrap()
            .iter()
            .any(|name| name == "Microsoft Print to PDF"),
        "Install Microsoft's PDF printer before opting into this test."
    );
    let body = format!(
        "CUSTOMER COPY — SYNTHETIC VERIFICATION\nZoë 界🙂\nCustomer: {}\nItem 801    Qty 2    $10.00\nPaid cash $10.00\n{}SYNTHETIC RECEIPT END — $123.45 — 界🙂\n",
        "Synthetic long customer name Zoë 界🙂 ".repeat(8),
        "Pagination verification $123.45\n".repeat(150)
    );
    let result = native::submit_to_output(
        "Microsoft Print to PDF",
        "Synthetic receipt verification",
        &body,
        Some(&output),
    );
    assert!(
        matches!(result, DispatchOutcome::Accepted { .. }),
        "{result:?}"
    );
    for _ in 0..40 {
        if output.metadata().is_ok_and(|metadata| metadata.len() > 100) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let bytes = std::fs::read(&output).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    eprintln!("Synthetic PDF verification output: {}", output.display());
}
