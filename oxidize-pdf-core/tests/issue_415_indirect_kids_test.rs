//! Issue #415 Bug 1: indirect `/Kids` (and `/Count`) references break the page
//! tree, so `page_count()` returns 0 and `extract_text()` yields empty text —
//! with no error — for spec-legal PDFs (ISO 32000-1 §7.3.10: any object may be
//! an indirect reference). Observed on iText-produced documents.
//!
//! The `/Pages` node stores `/Count N G R` and `/Kids N G R` instead of an
//! inline integer / array; the array itself lives in a separate object.
//! No smoke tests: we assert the real page count and the real page text.

use oxidize_pdf::parser::{PdfDocument, PdfReader};
use oxidize_pdf::pdfa::{PdfALevel, PdfAValidator};
use std::io::Cursor;

/// Builds a one-page PDF whose `/Pages` node references `/Count` and `/Kids`
/// **indirectly** (objects 6 and 7), mirroring iText 5.5.9 output.
fn build_pdf_with_indirect_kids(content_stream: &[u8]) -> Vec<u8> {
    let stream_len = content_stream.len();
    let bodies: Vec<Vec<u8>> = vec![
        // 1: Catalog
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        // 2: Pages — Count and Kids are indirect references, not inline
        b"<< /Type /Pages /Count 6 0 R /Kids 7 0 R >>".to_vec(),
        // 3: Page
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        // 4: Content stream
        {
            let mut s = format!("<< /Length {stream_len} >>\nstream\n").into_bytes();
            s.extend_from_slice(content_stream);
            s.extend_from_slice(b"\nendstream");
            s
        },
        // 5: Font
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
        // 6: the /Count value, as a standalone indirect integer object
        b"1".to_vec(),
        // 7: the /Kids array, as a standalone indirect array object
        b"[ 3 0 R ]".to_vec(),
    ];

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::with_capacity(bodies.len());
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(pdf.len() as u64);
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_pos = pdf.len() as u64;
    let n = bodies.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {n}\n").as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size {n} /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n").as_bytes(),
    );
    pdf
}

#[test]
fn indirect_kids_yields_correct_page_count() {
    let content = b"BT /F1 12 Tf 72 700 Td (Hello Indirect Kids) Tj ET";
    let pdf = build_pdf_with_indirect_kids(content);

    let reader = PdfReader::new(Cursor::new(pdf)).expect("parse PDF");
    let document = PdfDocument::new(reader);

    assert_eq!(
        document.page_count().expect("page_count"),
        1,
        "indirect /Kids and /Count must resolve to one page, not zero"
    );
}

#[test]
fn indirect_kids_extracts_document_text() {
    // Faithful reproduction of the reporter's symptom: the whole-document
    // `extract_text()` iterates `0..page_count()`. An empty page-tree flat
    // index makes it silently return an empty Vec.
    let content = b"BT /F1 12 Tf 72 700 Td (Hello Indirect Kids) Tj ET";
    let pdf = build_pdf_with_indirect_kids(content);

    let reader = PdfReader::new(Cursor::new(pdf)).expect("parse PDF");
    let document = PdfDocument::new(reader);

    let pages = document.extract_text().expect("extract_text");
    assert_eq!(pages.len(), 1, "extract_text must yield one page, not zero");
    assert!(
        pages[0].text.contains("Hello Indirect Kids"),
        "page text lost — the page tree was not traversed. Got: {:?}",
        pages[0].text
    );
}

#[test]
fn indirect_kids_resolves_in_get_page() {
    // `PdfDocument::get_page` must reach the page via the (now correct) flat
    // index for an indirect-/Kids document — not silently fall through.
    let content = b"BT /F1 12 Tf 72 700 Td (Hello Indirect Kids) Tj ET";
    let pdf = build_pdf_with_indirect_kids(content);

    let reader = PdfReader::new(Cursor::new(pdf)).expect("parse PDF");
    let document = PdfDocument::new(reader);

    let page = document
        .get_page(0)
        .expect("get_page(0) must resolve indirect /Kids");
    assert!(
        page.dict.get("Contents").is_some(),
        "resolved page must carry its content stream"
    );
}

#[test]
fn indirect_kids_does_not_break_pdfa_page_iteration() {
    // The PDF/A validator reads `/Kids` independently of the page-tree flat
    // index (pdfa::validator::get_page_dict). With an indirect `/Kids` it used
    // to hard-error "Pages missing Kids array", aborting validation. After the
    // fix, validation completes and returns a result (with whatever
    // conformance errors), never a parse failure over the page tree.
    let content = b"BT /F1 12 Tf 72 700 Td (Hello Indirect Kids) Tj ET";
    let pdf = build_pdf_with_indirect_kids(content);
    let mut reader = PdfReader::new(Cursor::new(pdf)).expect("parse PDF");

    let result = PdfAValidator::new(PdfALevel::A1b).validate(&mut reader);
    match result {
        Ok(_) => {}
        Err(e) => {
            panic!("indirect /Kids must resolve in the PDF/A page walk, got parse error: {e}")
        }
    }
}

#[test]
fn indirect_count_resolves_in_reader_page_count() {
    // `PdfReader::page_count()` (public, lower-level API) reads `/Count` via
    // `as_integer()` and falls back to `/Kids` via `as_array()`; both return
    // None for an indirect reference. Issue #415 cites reader.rs directly.
    let content = b"BT /F1 12 Tf 72 700 Td (Hello Indirect Kids) Tj ET";
    let pdf = build_pdf_with_indirect_kids(content);

    let mut reader = PdfReader::new(Cursor::new(pdf)).expect("parse PDF");
    assert_eq!(
        reader.page_count().expect("page_count"),
        1,
        "indirect /Count must resolve in PdfReader::page_count"
    );
}
