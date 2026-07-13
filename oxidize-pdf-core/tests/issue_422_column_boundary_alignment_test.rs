//! Issue #422 — `detect_and_sort_columns` merged consecutive *columnar* lines
//! into one block whenever each line independently had a wide gap and the lines
//! were row-spaced (the #417 guard). It never checked that the wide gaps ALIGN
//! horizontally. A label/value form with varying label lengths has a wide gap on
//! every line, but at a different X per line — not a table, just unrelated gaps.
//! Merging them pooled boundaries from unaligned gaps and shredded any token
//! straddling one. Fix: require a shared boundary X (within the dedup tolerance)
//! before merging. No smoke tests: assert the token stays intact AND the lines
//! keep reading order.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::text::{ExtractionOptions, TextExtractor};
use std::io::Write;

fn escape(ch: char) -> String {
    match ch {
        '(' => "\\(".to_string(),
        ')' => "\\)".to_string(),
        '\\' => "\\\\".to_string(),
        _ => ch.to_string(),
    }
}

/// One glyph per `Tj`. A `|` inserts a 65pt jump (above the 50pt
/// `column_threshold`) with no glyph — a wide gap whose X depends on how many
/// glyphs preceded it, so varying label lengths put the gap at varying X.
fn emit_line(content: &mut Vec<u8>, text: &str, start_x: f64, y: f64, advance: f64) {
    let mut x = start_x;
    for ch in text.chars() {
        if ch == '|' {
            x += 65.0;
            continue;
        }
        let escaped = escape(ch);
        content.extend_from_slice(
            format!("BT\n/F1 10 Tf\n{x:.2} {y:.2} Td\n({escaped}) Tj\nET\n").as_bytes(),
        );
        x += advance;
    }
}

fn wrap_pdf(content: &[u8]) -> Vec<u8> {
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> /MediaBox [0 0 612 792] /Contents 4 0 R >>\nendobj\n");
    offsets.push(pdf.len());
    let mut obj4 = Vec::new();
    write!(obj4, "4 0 obj\n<< /Length {} >>\nstream\n", content.len()).unwrap();
    obj4.extend_from_slice(content);
    obj4.extend_from_slice(b"\nendstream\nendobj\n");
    pdf.extend_from_slice(&obj4);
    offsets.push(pdf.len());
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );
    let xref_offset = pdf.len();
    pdf.extend_from_slice(b"xref\n");
    writeln!(pdf, "0 {}", offsets.len() + 1).unwrap();
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        writeln!(pdf, "{:010} 00000 n ", off).unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
        offsets.len() + 1,
        xref_offset
    )
    .unwrap();
    pdf
}

/// Five form lines, normal leading (14pt > line height), each `label|value`.
/// Labels differ in length, so the wide gap lands at a different X per line —
/// no shared column corridor. The ID line carries a CNPJ-shaped token.
fn build_form_pdf() -> Vec<u8> {
    let mut content = Vec::new();
    let fields = [
        ("Name", "SomeoneExample"),
        ("AddressLineOneHere", "SomeStreet123"),
        ("ID", "12.345.678/0001-99"),
        ("Department", "Sales"),
        ("Notes", "noneprovidedtoday"),
    ];
    let mut y = 700.0;
    for (label, value) in fields {
        emit_line(&mut content, &format!("{label}|{value}"), 72.0, y, 6.0);
        y -= 14.0;
    }
    wrap_pdf(&content)
}

fn extract_form(reorder: bool) -> String {
    let doc = PdfReader::new_with_options(
        std::io::Cursor::new(build_form_pdf()),
        ParseOptions::lenient(),
    )
    .expect("PDF should parse")
    .into_document();
    TextExtractor::with_options(ExtractionOptions {
        reorder_columns: reorder,
        ..Default::default()
    })
    .extract_from_page(&doc, 0)
    .expect("extraction should succeed")
    .text
}

const TOKEN: &str = "12.345.678/0001-99";

#[test]
fn reorder_columns_keeps_token_intact_on_misaligned_form() {
    let text = extract_form(true);
    assert!(
        text.contains(TOKEN),
        "misaligned form gaps must not be treated as a column block; token \
         `{TOKEN}` was shredded. Got: {text:?}"
    );
}

#[test]
fn reorder_columns_preserves_reading_order_on_misaligned_form() {
    // Unaligned lines stay singletons → left in reading order top-to-bottom.
    let text = extract_form(true);
    let name = text.find("Name").expect("Name present");
    let addr = text.find("AddressLineOneHere").expect("Address present");
    let id = text.find("ID").expect("ID present");
    let dept = text.find("Department").expect("Department present");
    let notes = text.find("Notes").expect("Notes present");
    assert!(
        name < addr && addr < id && id < dept && dept < notes,
        "unaligned form lines must keep reading order. Got: {text:?}"
    );
}
