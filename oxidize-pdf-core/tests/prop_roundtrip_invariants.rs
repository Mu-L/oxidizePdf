//! Round-trip invariant, stated from the write→read fidelity contract, not a
//! bug: a document built with the writer, serialized, and re-parsed preserves
//! what was put in it. Guards #364 (write-then-read empty/garbage), #395
//! (preserved-font collision), #156 (SMask dropped). One property per
//! dimension + a deterministic issue_N pin per class.

use oxidize_pdf::parser::{PdfDocument, PdfReader};
use oxidize_pdf::{Document, Font, Page};
use proptest::prelude::*;
use std::io::Cursor;

/// Serialize `doc` and re-parse it back into a navigable document.
fn reparse(bytes: Vec<u8>) -> PdfDocument<Cursor<Vec<u8>>> {
    let reader = PdfReader::new(Cursor::new(bytes)).expect("re-parse written PDF");
    PdfDocument::new(reader)
}

/// Append one A4 page carrying a single text marker at a fixed position.
fn write_marker_page(doc: &mut Document, marker: &str) {
    let mut page = Page::a4();
    page.text()
        .set_font(Font::Helvetica, 12.0)
        .at(72.0, 700.0)
        .write(marker)
        .expect("write marker");
    doc.add_page(page);
}

// Printable, non-space ASCII marker: survives a WinAnsi write() and extracts as
// one contiguous run. Space and PDF delimiters excluded on purpose.
#[allow(dead_code)] // used from Task 2 onward
fn marker() -> impl Strategy<Value = String> {
    "[A-Za-z0-9]{4,24}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Page count survives the round-trip for any 1..=8 pages.
    #[test]
    fn page_count_preserved(k in 1usize..=8) {
        let mut doc = Document::new();
        for i in 0..k {
            write_marker_page(&mut doc, &format!("PAGE{i}"));
        }
        let bytes = doc.to_bytes().expect("serialize");
        let document = reparse(bytes);
        prop_assert_eq!(document.page_count().expect("page_count"), k as u32);
    }
}
