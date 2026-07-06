//! Regression tests for issue #390.
//!
//! Flat-text extraction (`preserve_layout = false`) glued the last word of one
//! visual line to the first word of the next when the line height was smaller
//! than `newline_threshold` (default 10pt) AND the next line wrapped back to
//! the left. The newline gate keyed only on `dy > newline_threshold`, so a
//! Δy of ~9pt produced no newline; and because the pen jumped backward
//! (negative dx, a line wrap) the space branch didn't fire either — nothing
//! was inserted.
//!
//! Fix: treat a large backward horizontal jump (`dx < -(newline_threshold*2)`)
//! as a line wrap and insert a newline regardless of the exact Δy. Applied to
//! both the `Tj` (`ShowText`) and `TJ` (`ShowTextArray`) handlers.

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
fn tj_line_wrap_below_threshold_gets_newline() {
    // Δy = 700 - 691 = 9 pt  <  newline_threshold (10). The second line wraps
    // back to x=50 (a big negative dx). Before the fix the two lines glued into
    // "...linkhttps://...". A '\n' must now separate them.
    let content = concat!(
        "BT\n/F1 8 Tf\n",
        "1 0 0 1 50 700 Tm\n[(Please scan the QR code at the link)] TJ\n",
        "1 0 0 1 50 691 Tm\n[(https://example.com/verify)] TJ\nET"
    );
    let text = extract_flat(content);
    assert!(
        text.contains("link\nhttps://example.com/verify"),
        "expected a newline between the wrapped lines, got: {text:?}"
    );
    assert!(
        !text.contains("linkhttps"),
        "last word of line 1 must not touch first word of line 2: {text:?}"
    );
}

#[test]
fn tj_operator_line_wrap_below_threshold_gets_newline() {
    // Same wrap, but drawn with the `Tj` (ShowText) operator instead of `TJ`.
    let content = concat!(
        "BT\n/F1 8 Tf\n",
        "1 0 0 1 50 700 Tm\n(Please scan the QR code at the link) Tj\n",
        "1 0 0 1 50 691 Tm\n(https://example.com/verify) Tj\nET"
    );
    let text = extract_flat(content);
    assert!(
        text.contains("link\nhttps://example.com/verify"),
        "Tj path must also break the wrapped line, got: {text:?}"
    );
    assert!(
        !text.contains("linkhttps"),
        "Tj path glued the wrapped lines: {text:?}"
    );
}

#[test]
fn same_line_forward_pieces_do_not_get_spurious_newline() {
    // Two TJ pieces on the SAME line (Δy = 0), the second advancing forward
    // (x=90 > x=50). The line-wrap rule keys on a large *backward* dx, so a
    // forward advance must never insert a newline — a word drawn in pieces
    // stays on one line.
    let content = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 50 700 Tm\n[(Hello)] TJ\n",
        "1 0 0 1 90 700 Tm\n[(World)] TJ\nET"
    );
    let text = extract_flat(content);
    assert!(
        !text.contains('\n'),
        "forward same-line pieces must not gain a newline: {text:?}"
    );
    assert!(
        text.contains("Hello") && text.contains("World"),
        "both pieces must be present: {text:?}"
    );
}
