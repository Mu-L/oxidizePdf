//! Regression guard for issue #392 — the Unicode-keyed font path
//! (`add_font*` + `Font::Custom` + write) must render/extract non-Latin text
//! (Cyrillic here) instead of silently degrading to a single-byte encoding that
//! substitutes `?`.
//!
//! The bug was real on the Python wheel 0.14.0 (a core predating #240): the text
//! path emitted single-byte WinAnsi codes, so any character outside that range
//! became `?`. Since #240 the `Font::Custom` path emits UTF-16BE hex strings and
//! the writer emits a Type0/CIDFontType2 font with Identity-H and a CIDToGIDMap
//! that maps Unicode code points to glyph IDs via the font's own cmap. These
//! tests lock that contract so it cannot silently regress.
//!
//! Fixture: `../test-pdfs/Roboto-Regular.ttf` (has full Cyrillic coverage).
//! Skips gracefully if the fixture is missing.
use oxidize_pdf::parser::{PdfDocument, PdfReader};
use oxidize_pdf::{Document, Font, Page};
use std::io::Cursor;

const ROBOTO_PATH: &str = "../test-pdfs/Roboto-Regular.ttf";

// Cyrillic string outside the single-byte Latin range (U+044D, U+0447, U+0433,
// U+0444, U+0433). Exactly the failing input class from the issue.
const CYRILLIC: &str = "эчгфг";

fn load_fixture() -> Option<Vec<u8>> {
    std::fs::read(ROBOTO_PATH)
        .map_err(|_| eprintln!("SKIPPED: {ROBOTO_PATH} not found"))
        .ok()
}

/// Build a one-page PDF that writes `CYRILLIC` via the Unicode-keyed path and
/// return the serialized bytes.
fn build_pdf_with_cyrillic(font_data: Vec<u8>) -> Vec<u8> {
    let mut doc = Document::new();
    doc.add_font_from_bytes("Roboto", font_data)
        .expect("add_font_from_bytes should succeed");

    let mut page = Page::a4();
    page.text()
        .set_font(Font::Custom("Roboto".to_string()), 40.0)
        .at(60.0, 700.0)
        .write(CYRILLIC)
        .expect("writing Cyrillic via Unicode-keyed path should succeed");
    doc.add_page(page);

    doc.to_bytes().expect("PDF generation should succeed")
}

#[test]
fn unicode_keyed_font_roundtrips_cyrillic() {
    let font_data = match load_fixture() {
        Some(d) => d,
        None => return,
    };
    let pdf_bytes = build_pdf_with_cyrillic(font_data);

    let reader =
        PdfReader::new(Cursor::new(&pdf_bytes)).expect("generated PDF must be re-parseable");
    let extracted = PdfDocument::new(reader)
        .extract_text_from_page(0)
        .expect("text extraction must succeed");

    assert!(
        extracted.text.contains(CYRILLIC),
        "Unicode-keyed path must round-trip Cyrillic, not degrade to '?': {:?}",
        extracted.text
    );
    assert!(
        !extracted.text.contains('?'),
        "no character may degrade to '?': {:?}",
        extracted.text
    );
}

#[test]
fn unicode_keyed_font_emits_type0_not_single_byte() {
    // The distinguishing artifact of the fix: a Type0 composite font with
    // Identity-H, not a simple single-byte font. The font dictionaries are not
    // compressed, so a raw-byte scan is a reliable check.
    let font_data = match load_fixture() {
        Some(d) => d,
        None => return,
    };
    let pdf_bytes = build_pdf_with_cyrillic(font_data);
    let haystack = String::from_utf8_lossy(&pdf_bytes);

    for marker in ["/Type0", "/CIDFontType2", "Identity-H", "CIDToGIDMap"] {
        assert!(
            haystack.contains(marker),
            "Unicode-keyed Cyrillic font must be emitted as a Type0/CIDFontType2 \
             composite font (missing marker {marker:?})"
        );
    }
}

#[test]
fn font_missing_glyphs_matches_renderability() {
    // Issue request #3: `font_missing_glyphs` must not give a false all-clear
    // that contradicts what the font actually paints. Roboto covers the
    // Cyrillic, so it reports none missing AND the text round-trips — consistent.
    let font_data = match load_fixture() {
        Some(d) => d,
        None => return,
    };
    let mut doc = Document::new();
    doc.add_font_from_bytes("Roboto", font_data)
        .expect("add_font_from_bytes should succeed");

    assert_eq!(
        doc.font_missing_glyphs("Roboto", CYRILLIC),
        Vec::<char>::new(),
        "Roboto covers this Cyrillic, so no glyph is missing"
    );
}
