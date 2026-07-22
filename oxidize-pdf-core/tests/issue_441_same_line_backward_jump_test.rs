//! Regression tests for issue #441.
//!
//! The flat-text line-wrap heuristic added for issue #390 fires on a large
//! backward horizontal jump (`dx < -(newline_threshold * 2)`) regardless of
//! Δy. Real-world PDFs reposition glyphs backward *on the same line*
//! (justification, kerned overlays, out-of-order emission); with Δy exactly 0
//! there is no wrap, yet the heuristic inserted a spurious newline mid-word.
//!
//! Fix: a backward jump only counts as a wrap when the pen also moved
//! vertically (`dy > 0`). A strictly same-line backward jump can never be a
//! wrap — a wrapped line always lands on a different baseline. Tight-leading
//! wraps (small but nonzero Δy, the #390 class) still break.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::text::TextExtractor;

/// Build a minimal, valid PDF whose single page has `content` as its content
/// stream. `/F1` maps to Helvetica (Type1) so decoding is trivial.
fn build_pdf(content: &str) -> Vec<u8> {
    let clen = content.len();
    let o1 = "<< /Type /Catalog /Pages 3 0 R >>";
    let o2 = "<< /Type /Page /Parent 3 0 R /MediaBox [0 0 595 842] \
              /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>";
    let o3 = "<< /Type /Pages /Kids [2 0 R] /Count 1 >>";
    let o4 = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

    let mut buf = Vec::<u8>::new();
    buf.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = [0usize; 6];
    let mut push = |buf: &mut Vec<u8>, n: usize, body: &str| {
        offsets[n] = buf.len();
        buf.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    };
    push(&mut buf, 1, o1);
    push(&mut buf, 2, o2);
    push(&mut buf, 3, o3);
    push(&mut buf, 4, o4);

    offsets[5] = buf.len();
    buf.extend_from_slice(
        format!("5 0 obj\n<< /Length {clen} >>\nstream\n{content}\nendstream\nendobj\n").as_bytes(),
    );

    let xref_pos = buf.len();
    buf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n").as_bytes(),
    );
    buf
}

fn extract_flat(content: &str) -> String {
    let doc = PdfReader::new_with_options(
        std::io::Cursor::new(build_pdf(content)),
        ParseOptions::lenient(),
    )
    .expect("PDF should parse")
    .into_document();

    // Default options => preserve_layout = false (the affected flat path).
    let mut ex = TextExtractor::new();
    ex.extract_from_page(&doc, 0)
        .expect("extraction should succeed")
        .text
}

#[test]
fn tj_operator_same_line_backward_jump_gets_no_newline() {
    // The exact signature from issue #441: three `Tj` calls all at y=562.50.
    // "o" jumps backward by ~-35pt (beyond -newline_threshold*2 = -20), then
    // "X" continues forward. Δy is exactly 0 for all three, so nothing
    // wrapped; before the fix the output was "AGR\no X".
    let content = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 356.17 562.50 Tm\n(AGR) Tj\n",
        "1 0 0 1 335.83 562.50 Tm\n(o) Tj\n",
        "1 0 0 1 400.00 562.50 Tm\n(X) Tj\nET"
    );
    let text = extract_flat(content);
    assert!(
        !text.contains('\n'),
        "same-line backward jump must not insert a newline (issue #441): {text:?}"
    );
    let glyphs: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(
        glyphs, "AGRoX",
        "all glyphs must survive in draw order: {text:?}"
    );
}

#[test]
fn tj_array_same_line_backward_jump_gets_no_newline() {
    // Same signature through the `TJ` (ShowTextArray) handler, which
    // duplicates the line-wrap heuristic.
    let content = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 356.17 562.50 Tm\n[(AGR)] TJ\n",
        "1 0 0 1 335.83 562.50 Tm\n[(o)] TJ\n",
        "1 0 0 1 400.00 562.50 Tm\n[(X)] TJ\nET"
    );
    let text = extract_flat(content);
    assert!(
        !text.contains('\n'),
        "TJ path must not insert a newline on a same-line backward jump: {text:?}"
    );
    let glyphs: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(
        glyphs, "AGRoX",
        "all glyphs must survive in draw order: {text:?}"
    );
}

#[test]
fn backward_jump_with_small_nonzero_dy_still_breaks_line() {
    // Boundary pin: the #390 class (tight leading, Δy = 2pt nonzero but far
    // below newline_threshold = 10, plus a big backward jump) must STILL be
    // treated as a wrap. The #441 fix only excludes Δy == 0.
    let content = concat!(
        "BT\n/F1 8 Tf\n",
        "1 0 0 1 300 700 Tm\n(tail) Tj\n",
        "1 0 0 1 50 698 Tm\n(head) Tj\nET"
    );
    let text = extract_flat(content);
    assert!(
        text.contains("tail\nhead"),
        "nonzero-dy backward wrap must still break (the #390 guarantee): {text:?}"
    );
}
