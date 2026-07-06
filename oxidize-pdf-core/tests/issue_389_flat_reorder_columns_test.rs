//! Regression tests for issue #389 — flat-text column reordering (opt-in).
use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::text::{ExtractionOptions, TextExtractor};

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

fn extract(content: &str, opts: ExtractionOptions) -> oxidize_pdf::text::ExtractedText {
    let doc = PdfReader::new_with_options(
        std::io::Cursor::new(build_pdf(content)),
        ParseOptions::lenient(),
    )
    .expect("PDF should parse")
    .into_document();
    TextExtractor::with_options(opts)
        .extract_from_page(&doc, 0)
        .expect("extraction should succeed")
}

// Two-column table row: col1 email `user@example`+`.com` (x=50,x=100),
// col2 email `tel@example`+`.com` (x=200,x=250), interleaved in stream order.
const TWO_COL_EMAILS: &str = concat!(
    "BT\n/F1 10 Tf\n",
    "1 0 0 1 50 680 Tm\n[(user@example)] TJ\n",
    "1 0 0 1 200 680 Tm\n[(tel@example)] TJ\n",
    "1 0 0 1 100 680 Tm\n[(.com)] TJ\n",
    "1 0 0 1 250 680 Tm\n[(.com)] TJ\nET"
);

#[test]
fn reorder_columns_keeps_column_tokens_adjacent() {
    let opts = ExtractionOptions {
        reorder_columns: true,
        ..Default::default()
    };
    let text = extract(TWO_COL_EMAILS, opts).text;
    assert!(
        text.contains("user@example.com"),
        "col1 email must be intact under reorder_columns: {text:?}"
    );
    assert!(
        text.contains("tel@example.com"),
        "col2 email must be intact under reorder_columns: {text:?}"
    );
}
