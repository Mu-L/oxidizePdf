//! Acceptance tests for issue #395: a preserved embedded font must not be
//! silently dropped or de-referenced when its `/Font` resource key collides
//! with a writer-injected base-14 font, and the preserved content that
//! references it must keep resolving to the embedded font.
//!
//! Root cause (see the issue): `write_page_with_fonts` injects all base-14
//! fonts into every page `/Font` dict keyed by their PostScript names
//! (`Helvetica`, `Times-Roman`, …). Preserved resources are merged
//! overlay-wins-on-collision, so a preserved font keyed `/Helvetica` is
//! discarded in favour of the non-embedded injected stub.
//!
//! The pre-fix mitigation renamed EVERY preserved font unconditionally
//! (`/F1` → `/OrigF1`) and rewrote the content. That is wrong two ways:
//!   * `rewrite_font_references` is line-based, so a font operator split
//!     across a newline (`/Helvetica\n12 Tf`) is NOT rewritten — the dict key
//!     becomes `/OrigHelvetica` but the content still says `/Helvetica`, which
//!     then resolves to the injected stub (or, for a non-base-14 key, to
//!     nothing). This is the class of failure reported on `testi.pdf`.
//!   * renaming non-colliding fonts is pure risk with no benefit.
//!
//! The fix is collision-only disambiguation (rename a preserved font only when
//! its key collides with the injected/reserved set, rewriting only those
//! references) plus a whitespace-robust content rewrite.

use oxidize_pdf::parser::objects::{PdfDictionary, PdfObject};
use oxidize_pdf::parser::{PdfDocument, PdfReader};
use oxidize_pdf::{Document, Page};
use std::io::Cursor;

/// The distinctive BaseFont of the preserved embedded font. It differs from any
/// injected stub's BaseFont so we can find it unambiguously in the output.
const EMBEDDED_BASE_FONT: &str = "ABCDEF+CustomEmbed";

/// Build a minimal, valid PDF whose page `/Resources /Font /<font_key>` maps to
/// an embedded TrueType font (distinctive BaseFont, FontDescriptor, FontFile2),
/// and whose content selects it with a `<size> Tf` operator. When `cross_line`
/// is true the font name and size are placed on separate content lines — a
/// layout the pre-fix line-based rewriter fails to handle.
fn build_pdf(font_key: &str, cross_line: bool) -> Vec<u8> {
    let content = if cross_line {
        format!("BT\n/{font_key}\n12 Tf 72 720 Td (Hi) Tj ET")
    } else {
        format!("BT /{font_key} 12 Tf 72 720 Td (Hi) Tj ET")
    };
    let obj4 = format!(
        "<< /Length {} >>\nstream\n{}\nendstream",
        content.len(),
        content
    );

    // A dummy embedded font program. `from_parsed_with_content` copies the
    // stream bytes verbatim (it does not parse the TTF), so arbitrary bytes work
    // for a preservation round-trip.
    let fontfile = "\x00\x01\x00\x00dummy-truetype-program";
    let obj7 = format!(
        "<< /Length {} /Length1 {} >>\nstream\n{}\nendstream",
        fontfile.len(),
        fontfile.len(),
        fontfile
    );

    let objects: Vec<String> = vec![
        // 1: Catalog.
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        // 2: Page tree root.
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        // 3: Page — the /Font key is `font_key`.
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /{font_key} 5 0 R >> >> /Contents 4 0 R >>"
        ),
        // 4: Content stream.
        obj4,
        // 5: The embedded font dictionary (distinctive BaseFont).
        format!(
            "<< /Type /Font /Subtype /TrueType /BaseFont /{EMBEDDED_BASE_FONT} \
             /FirstChar 32 /LastChar 32 /Widths [500] /Encoding /WinAnsiEncoding \
             /FontDescriptor 6 0 R >>"
        ),
        // 6: FontDescriptor referencing the embedded program.
        format!(
            "<< /Type /FontDescriptor /FontName /{EMBEDDED_BASE_FONT} /Flags 4 \
             /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 700 /Descent -200 \
             /CapHeight 700 /StemV 80 /FontFile2 7 0 R >>"
        ),
        // 7: FontFile2 embedded font program stream.
        obj7,
    ];

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");

    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
    }

    let xref_offset = pdf.len();
    let size = objects.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {}\n", size).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
            size, xref_offset
        )
        .as_bytes(),
    );

    pdf
}

/// Resolve `obj` to a dictionary, following one level of indirection.
fn resolve_dict<R: std::io::Read + std::io::Seek>(
    doc: &PdfDocument<R>,
    obj: &PdfObject,
) -> Option<PdfDictionary> {
    match obj {
        PdfObject::Dictionary(d) => Some(d.clone()),
        PdfObject::Reference(num, gen) => match doc.get_object(*num, *gen) {
            Ok(PdfObject::Dictionary(d)) => Some(d),
            Ok(PdfObject::Stream(s)) => Some(s.dict.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Whitespace-tolerant check: does the content select `key` via a
/// `/key <size> Tf` operator anywhere (across any whitespace, incl. newlines)?
fn content_selects_font(content: &[u8], key: &str) -> bool {
    let s = String::from_utf8_lossy(content);
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let want = format!("/{key}");
    tokens.windows(3).any(|w| w[0] == want && w[2] == "Tf")
}

/// Round-trip the source PDF through `from_parsed_with_content` → write →
/// re-parse, then assert the embedded font survives in the output `/Font` dict
/// AND the preserved content still selects it.
fn assert_embedded_font_survives(font_key: &str, cross_line: bool) {
    let pdf_bytes = build_pdf(font_key, cross_line);

    let reader =
        PdfReader::new(Cursor::new(&pdf_bytes)).expect("hand-built PDF must be re-parseable");
    let document = PdfDocument::new(reader);
    let parsed_page = document.get_page(0).expect("page 0 must parse");
    let page = Page::from_parsed_with_content(&parsed_page, &document)
        .expect("from_parsed_with_content must succeed");

    let mut out_doc = Document::new();
    out_doc.add_page(page);
    let out_bytes = out_doc.to_bytes().expect("writing output must succeed");

    let out_reader =
        PdfReader::new(Cursor::new(&out_bytes)).expect("output PDF must be re-parseable");
    let out_document = PdfDocument::new(out_reader);
    let out_page = out_document.get_page(0).expect("output page 0 must parse");

    let resources = out_page
        .get_resources()
        .expect("output page must have /Resources");
    let fonts = resources
        .get("Font")
        .and_then(|o| o.as_dict())
        .expect("output /Font must be an inline dictionary");

    // (1) The embedded font must survive under SOME key.
    let embedded_key = fonts
        .0
        .iter()
        .find_map(|(name, value)| {
            let dict = resolve_dict(&out_document, value)?;
            let base = dict.get("BaseFont").and_then(|o| o.as_name())?;
            (base.as_str() == EMBEDDED_BASE_FONT).then(|| name.as_str().to_string())
        })
        .unwrap_or_else(|| {
            let keys: Vec<_> = fonts.0.keys().map(|k| k.as_str().to_string()).collect();
            panic!(
                "embedded font (BaseFont /{EMBEDDED_BASE_FONT}) was dropped from the output \
                 /Font dict; keys present: {keys:?}"
            )
        });

    // (2) The preserved content must select the embedded font's key, so the
    // text resolves to the embedded font — not the injected stub, and not a
    // dangling key.
    let streams = out_page
        .content_streams_with_document(&out_document)
        .expect("output content streams must resolve");
    let content: Vec<u8> = streams.into_iter().flatten().collect();
    assert!(
        content_selects_font(&content, &embedded_key),
        "preserved content must select the embedded font via `/{embedded_key} <size> Tf`; \
         got content: {:?}",
        String::from_utf8_lossy(&content)
    );
}

/// Simple base-14 collision: `/Helvetica` on a single content line. This case
/// already works with the pre-fix unconditional rename; it is a guard against
/// the #391 regression (removing the mechanism drops the embedded font).
#[test]
fn base14_key_collision_simple_content() {
    assert_embedded_font_survives("Helvetica", false);
}

/// Base-14 collision with the font operator split across a newline. The pre-fix
/// line-based rewrite leaves the content pointing at `/Helvetica` (the injected
/// stub) while the embedded font sits under `/OrigHelvetica`. Requires both the
/// collision-only rename and a whitespace-robust content rewrite.
#[test]
fn base14_key_collision_cross_line_content() {
    assert_embedded_font_survives("Helvetica", true);
}

/// Non-base-14 key with the font operator split across a newline — the
/// `testi.pdf` failure class. The pre-fix code renames `/DistinctFont` →
/// `/OrigDistinctFont` but fails to rewrite the cross-line reference, leaving
/// `/DistinctFont` dangling (no injected stub to fall back on). Collision-only
/// disambiguation fixes it by not renaming a non-colliding font at all.
#[test]
fn non_base14_key_cross_line_content_not_corrupted() {
    assert_embedded_font_survives("DistinctFont", true);
}
