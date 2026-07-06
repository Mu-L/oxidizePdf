//! Regression tests for issue #381.
//!
//! Flat-text extraction (`preserve_layout = false`) must insert a newline
//! between `TJ` (`ShowTextArray`) blocks drawn on different visual lines.
//! Before the fix, the `TextElement::Text` arm of `ShowTextArray` neither
//! checked the pen origin against `newline_threshold` nor updated
//! `last_x`/`last_y`, so text on separate lines was glued together and a
//! following `Tj` measured its gap from a stale position.
//!
//! Nuance (also asserted): the fix adds ONLY the vertical (newline) case.
//! A single word drawn as several positioned pieces inside one `TJ` array on
//! the same line must NOT gain a spurious newline — horizontal spacing stays
//! governed by the existing `TextElement::Spacing` kern logic.

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
fn tj_blocks_on_different_lines_get_a_newline() {
    // Δy = 700 - 680 = 20 > newline_threshold (10) => a '\n' must separate them.
    let content = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 50 700 Tm\n[(user@example.com)] TJ\n",
        "1 0 0 1 50 680 Tm\n[(* footnote text here)] TJ\nET"
    );
    let text = extract_flat(content);

    assert!(
        !text.contains("user@example.com*"),
        "TJ blocks on different lines were glued together: {text:?}"
    );
    assert!(
        text.contains("user@example.com\n* footnote text here"),
        "expected a newline between the two TJ lines, got: {text:?}"
    );
}

#[test]
fn tj_pieces_of_one_word_on_same_line_are_not_split() {
    // A single word "example" drawn as three positioned pieces inside ONE TJ
    // array, all at the same y. Kerning between pieces is expressed as small
    // negative adjustments (below tj_space_threshold), so nothing should break
    // the word — and crucially, no newline may appear.
    let content = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 50 700 Tm\n",
        "[(ex) -5 (am) -5 (ple)] TJ\nET"
    );
    let text = extract_flat(content);

    assert!(
        text.contains("example"),
        "same-line TJ pieces of one word were split: {text:?}"
    );
    assert!(
        !text.contains('\n'),
        "no newline expected within a single visual line, got: {text:?}"
    );
}

#[test]
fn tj_then_tj_on_same_line_stay_joined() {
    // Two TJ blocks at the SAME y (same line) must not get a newline: the fix
    // is vertical-only.
    let content = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 50 700 Tm\n[(Hello)] TJ\n",
        "1 0 0 1 90 700 Tm\n[(World)] TJ\nET"
    );
    let text = extract_flat(content);

    assert!(
        !text.contains('\n'),
        "no newline expected for TJ blocks on the same line, got: {text:?}"
    );
}

#[test]
fn tj_then_bare_tj_on_different_line_gets_newline() {
    // Real-world streams mix `Tj` and `TJ`. The TJ arm must keep `last_y` in
    // sync so a following bare `Tj` (whose newline check reads `last_y`) still
    // sees the line change — and vice versa. Guards the `last_y` write in the
    // TJ arm against the read in the `ShowText` (Tj) arm.
    let tj_then_tj = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 50 700 Tm\n[(TJ line)] TJ\n",
        "1 0 0 1 50 680 Tm\n(Tj line) Tj\nET"
    );
    assert!(
        extract_flat(tj_then_tj).contains("TJ line\nTj line"),
        "TJ→Tj across lines lost the newline: {:?}",
        extract_flat(tj_then_tj)
    );

    let tj_then_ta = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 50 700 Tm\n(Tj line) Tj\n",
        "1 0 0 1 50 680 Tm\n[(TJ line)] TJ\nET"
    );
    assert!(
        extract_flat(tj_then_ta).contains("Tj line\nTJ line"),
        "Tj→TJ across lines lost the newline: {:?}",
        extract_flat(tj_then_ta)
    );
}

#[test]
fn tj_updates_position_so_following_tj_measures_correctly() {
    // TJ on line 1, then TJ on line 2, then TJ back near line 1's y but a full
    // line below the previous. Each vertical jump > threshold must newline.
    // This exercises last_y being kept in sync by the TJ arm (stale last_y
    // would drop the second newline).
    let content = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 50 700 Tm\n[(line one)] TJ\n",
        "1 0 0 1 50 680 Tm\n[(line two)] TJ\n",
        "1 0 0 1 50 660 Tm\n[(line three)] TJ\nET"
    );
    let text = extract_flat(content);

    assert_eq!(
        text.matches('\n').count(),
        2,
        "expected exactly two newlines across three TJ lines, got: {text:?}"
    );
    assert!(
        text.contains("line one\nline two\nline three"),
        "lines were not separated correctly: {text:?}"
    );
}
