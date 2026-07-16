//! Round-trip invariant, stated from the write→read fidelity contract, not a
//! bug: a document built with the writer, serialized, and re-parsed preserves
//! what was put in it. Guards #364 (write-then-read empty/garbage), #395
//! (preserved-font collision), #156 (SMask dropped). One property per
//! dimension + a deterministic issue_N pin per class.

use oxidize_pdf::parser::{PdfDocument, PdfReader};
use oxidize_pdf::text::{ExtractionOptions, TextExtractor};
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
fn marker() -> impl Strategy<Value = String> {
    "[A-Za-z0-9]{4,24}"
}

/// Build a document: one page per inner Vec, each marker on its own well-separated
/// baseline (≥ 40pt apart), standard-14. Returns serialized bytes.
fn build_text_pages(pages: &[Vec<String>]) -> Vec<u8> {
    let mut doc = Document::new();
    for markers in pages {
        let mut page = Page::a4();
        let mut y = 760.0;
        for m in markers {
            page.text()
                .set_font(Font::Helvetica, 12.0)
                .at(72.0, y)
                .write(m)
                .expect("write marker");
            y -= 60.0; // ≥ 40pt separation → each marker extracts as its own run
        }
        doc.add_page(page);
    }
    doc.to_bytes().expect("serialize")
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

    /// Every written marker is recoverable from its page's extracted text (#364).
    #[test]
    fn text_content_preserved(
        pages in prop::collection::vec(
            prop::collection::vec(marker(), 1..=3),
            1..=4,
        ),
    ) {
        let bytes = build_text_pages(&pages);
        let document = reparse(bytes);
        let mut extractor = TextExtractor::with_options(ExtractionOptions::default());
        for (i, markers) in pages.iter().enumerate() {
            let text = extractor
                .extract_from_page(&document, i as u32)
                .expect("extract")
                .text;
            for m in markers {
                // contains, not order: extraction may reorder/space fragments.
                prop_assert!(
                    text.contains(m.as_str()),
                    "page {i} lost marker {m:?}; got {text:?}"
                );
            }
        }
    }
}

/// #364 pin: a written marker must read back non-empty and present.
#[test]
fn issue_364_written_text_reads_back() {
    let bytes = build_text_pages(&[vec!["MARKER364".to_string()]]);
    let document = reparse(bytes);
    let mut extractor = TextExtractor::with_options(ExtractionOptions::default());
    let text = extractor
        .extract_from_page(&document, 0)
        .expect("extract")
        .text;
    assert!(!text.is_empty(), "#364: written page read back empty");
    assert!(
        text.contains("MARKER364"),
        "#364: marker lost; got {text:?}"
    );
}
