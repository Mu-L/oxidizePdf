//! Regression tests for issue #386.
//!
//! The pen-position tracking used for space/newline decisions in the flat
//! extraction path recorded `last_x = origin_x + text_width`, which ignores
//! both the horizontal text scaling operator `Tz` (`state.horizontal_scale`)
//! and the CTM's x-scale. When either differs from the identity, `last_x`
//! underestimates the real pen advance, so the next text-showing operator
//! measures `dx = x - last_x` against a stale position and inserts a spurious
//! space between text that is actually flush.
//!
//! Both tests use the *natural* pen advance (two consecutive `Tj` with no
//! intervening `Tm`), so the second operator's origin is exactly the pen
//! position after the first — no hard-coded glyph widths needed. Correct
//! tracking yields `dx == 0` (no space); the bug yields `dx == text_width`
//! (a space).

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::text::TextExtractor;

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

    let mut ex = TextExtractor::new();
    ex.extract_from_page(&doc, 0)
        .expect("extraction should succeed")
        .text
}

#[test]
fn tz_scaled_flush_text_gets_no_spurious_space() {
    // `200 Tz` doubles the horizontal advance. Two consecutive `Tj` (no `Tm`
    // between them) sit flush. Correct tracking => "HiThere". The bug records
    // last_x = origin + text_width (Tz ignored), so dx = text_width > 0 and a
    // space is wrongly inserted.
    let content = concat!(
        "BT\n/F1 10 Tf\n200 Tz\n",
        "1 0 0 1 50 700 Tm\n(Hi) Tj\n(There) Tj\nET"
    );
    let text = extract_flat(content);
    assert!(
        text.contains("HiThere"),
        "Tz-scaled flush text got a spurious space: {text:?}"
    );
}

#[test]
fn ctm_scaled_flush_text_gets_no_spurious_space() {
    // A 2x scaling CTM (`2 0 0 2 0 0 cm`) doubles the user-space advance while
    // `Tz` stays at 100. `text_width` (computed from font_size, not the CTM)
    // underestimates the real advance, so the buggy last_x again trails the
    // real pen and a spurious space appears.
    let content = concat!(
        "2 0 0 2 0 0 cm\n",
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 25 350 Tm\n(Hi) Tj\n(There) Tj\nET"
    );
    let text = extract_flat(content);
    assert!(
        text.contains("HiThere"),
        "CTM-scaled flush text got a spurious space: {text:?}"
    );
}

#[test]
fn unscaled_flush_text_still_joins() {
    // Baseline: with Tz=100 and identity CTM, flush text stays joined (guards
    // against the fix over-correcting and dropping legitimate advances).
    let content = concat!(
        "BT\n/F1 10 Tf\n",
        "1 0 0 1 50 700 Tm\n(Hi) Tj\n(There) Tj\nET"
    );
    let text = extract_flat(content);
    assert!(
        text.contains("HiThere"),
        "unscaled flush text was split: {text:?}"
    );
}

#[test]
fn tz_scaled_wide_gap_still_spaces() {
    // Sanity: a genuinely wide gap under Tz must still produce a space. Here
    // the second Tm is placed far to the right, so dx is unambiguously large
    // regardless of the tracking fix.
    let content = concat!(
        "BT\n/F1 10 Tf\n200 Tz\n",
        "1 0 0 1 50 700 Tm\n(Hi) Tj\n",
        "1 0 0 1 400 700 Tm\n(There) Tj\nET"
    );
    let text = extract_flat(content);
    assert!(
        text.contains("Hi There"),
        "a wide gap under Tz should still insert a space: {text:?}"
    );
}
