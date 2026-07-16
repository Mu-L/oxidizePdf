//! Font / glyph invariants, stated from the contract of "render and extract the
//! characters the caller asked for", not from a bug.
//!
//! Invariants:
//!   1. WIDTH IS CHARACTER-BASED, ADDITIVE, AND SIZE-PROPORTIONAL. `measure_text`
//!      measures glyphs (chars), never UTF-8 bytes, so a single accented Latin-1
//!      character measures as one glyph — not two (#309). Width is additive over
//!      concatenation (no kerning in the base-14 metrics) and scales linearly
//!      with font size.
//!   2. NO SILENT GLYPH SUBSTITUTION. A character in the font's supported set,
//!      drawn through a WinAnsi standard-14 font, extracts back as itself — never
//!      as '?' or a dropped/garbled glyph (#272, #287, #392).
//!
//! Written from the contract up front so the whole "font mangles characters"
//! class is guarded, not just the reported instances.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::text::{measure_text, ExtractionOptions, Font, TextEncoding, TextExtractor};
use proptest::prelude::*;
use std::io::{Cursor, Write};

/// Accented Latin-1 letters, all representable in WinAnsi — the characters the
/// #309/#392 class mangled. Plus ASCII letters as controls.
const LATIN1: &[char] = &[
    'a', 'e', 'i', 'o', 'u', 'n', 'c', 'A', 'E', 'O', 'U', 'à', 'á', 'â', 'ä', 'è', 'é', 'ê', 'ë',
    'ì', 'í', 'î', 'ï', 'ò', 'ó', 'ô', 'ö', 'ù', 'ú', 'û', 'ü', 'ñ', 'ç', 'Á', 'É', 'Ñ', 'Ü',
];

// ---------- Invariant 1: measurement ----------

/// Deterministic pin for #309: `í` (iacute) has a true Helvetica advance of
/// 278/1000 em. The bug returned the 556-unit default width (over-measuring a
/// non-ASCII WinAnsi glyph whose metric wasn't looked up correctly). This nails
/// the exact reported value: 278, not the 556 fallback.
#[test]
fn issue_309_accented_char_uses_true_metric_not_fallback() {
    let wia = measure_text("í", &Font::Helvetica, 1000.0);
    assert_eq!(
        wia, 278.0,
        "í must measure its true metric (278), not the 556 fallback (#309)"
    );
}

fn latin1_string() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(LATIN1), 1..24)
        .prop_map(|cs| cs.into_iter().collect())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Width scales linearly with font size.
    #[test]
    fn width_is_size_proportional(s in latin1_string(), size in 1.0f64..200.0) {
        let h = Font::Helvetica;
        let base = measure_text(&s, &h, size);
        let doubled = measure_text(&s, &h, size * 2.0);
        prop_assert!((doubled - 2.0 * base).abs() < 1e-6, "width must scale with size");
    }

    /// Width is additive over concatenation (base-14 has no kerning), which also
    /// means it is counted per character, not per byte.
    #[test]
    fn width_is_additive_over_chars(a in latin1_string(), b in latin1_string()) {
        let h = Font::Helvetica;
        let whole = measure_text(&format!("{a}{b}"), &h, 12.0);
        let parts = measure_text(&a, &h, 12.0) + measure_text(&b, &h, 12.0);
        prop_assert!((whole - parts).abs() < 1e-6, "width must be additive over chars");
    }

    /// A single Latin-1 glyph never measures wider than the widest base-14 glyph.
    /// A byte-counting measurement of a 2-byte char can exceed this bound.
    #[test]
    fn single_glyph_within_one_em(c in prop::sample::select(LATIN1)) {
        let w = measure_text(&c.to_string(), &Font::Helvetica, 1000.0);
        prop_assert!(w > 0.0 && w <= 1000.0, "one glyph must be <= 1 em, got {w}");
    }
}

// ---------- Invariant 2: no silent glyph substitution (round-trip) ----------

fn escape_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    out
}

/// Build a one-page PDF that draws `text` through a WinAnsi standard-14 font,
/// with the string bytes WinAnsi-encoded exactly as the writer would emit them.
fn winansi_pdf(text: &str) -> Vec<u8> {
    let encoded = TextEncoding::WinAnsiEncoding.encode(text);
    let mut content = Vec::new();
    content.extend_from_slice(b"BT\n/F1 12 Tf\n72 700 Td\n(");
    content.extend_from_slice(&escape_bytes(&encoded));
    content.extend_from_slice(b") Tj\nET\n");

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
    obj4.extend_from_slice(&content);
    obj4.extend_from_slice(b"\nendstream\nendobj\n");
    pdf.extend_from_slice(&obj4);
    offsets.push(pdf.len());
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );
    let xref_pos = pdf.len();
    write!(pdf, "xref\n0 6\n0000000000 65535 f \n").unwrap();
    for off in &offsets {
        write!(pdf, "{off:010} 00000 n \n").unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n"
    )
    .unwrap();
    pdf
}

fn extract(pdf: &[u8]) -> String {
    let reader = PdfReader::new_with_options(Cursor::new(pdf.to_vec()), ParseOptions::lenient())
        .expect("parse");
    let document = reader.into_document();
    let mut extractor = TextExtractor::with_options(ExtractionOptions::default());
    extractor
        .extract_from_page(&document, 0)
        .expect("extract")
        .text
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// A WinAnsi-drawn Latin-1 string extracts back as itself — no '?' or
    /// dropped/garbled glyph (#272, #287, #392).
    #[test]
    fn winansi_latin1_round_trips_through_extraction(text in latin1_string()) {
        let extracted = extract(&winansi_pdf(&text));
        let got: String = extracted.chars().filter(|c| !c.is_whitespace()).collect();
        prop_assert_eq!(
            &got, &text,
            "WinAnsi Latin-1 must extract unchanged; got {:?} for {:?}", got, text
        );
    }
}
