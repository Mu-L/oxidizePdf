//! Round-trip invariant, stated from the write→read fidelity contract, not a
//! bug: a document built with the writer, serialized, and re-parsed preserves
//! what was put in it. Guards #364 (write-then-read empty/garbage), #395
//! (preserved-font collision), #156 (SMask dropped). One property per
//! dimension + a deterministic issue_N pin per class.

use oxidize_pdf::graphics::Image;
use oxidize_pdf::parser::objects::PdfObject;
use oxidize_pdf::parser::{PdfDocument, PdfReader};
use oxidize_pdf::text::{ExtractionOptions, TextExtractor};
use oxidize_pdf::{Document, Font, Page};
use proptest::prelude::*;
use std::io::Cursor;

const ROBOTO_PATH: &str = "../test-pdfs/Roboto-Regular.ttf";
const SOURCE_SANS_PATH: &str = "../test-pdfs/SourceSans3-Regular.otf";

fn roboto() -> Vec<u8> {
    std::fs::read(ROBOTO_PATH).expect("read Roboto fixture")
}

fn source_sans() -> Vec<u8> {
    std::fs::read(SOURCE_SANS_PATH).expect("read SourceSans fixture")
}

/// Serialize `doc` and re-parse it back into a navigable document.
fn reparse(bytes: Vec<u8>) -> PdfDocument<Cursor<Vec<u8>>> {
    let reader = PdfReader::new(Cursor::new(bytes)).expect("re-parse written PDF");
    PdfDocument::new(reader)
}

/// Walk the page's /Font resource dict and return each resolved font's BaseFont
/// (empty string if a font dict has no BaseFont). Mirrors the walk in
/// issue_395_font_collision_test.rs.
fn page_base_fonts(page_index: u32, doc: &PdfDocument<Cursor<Vec<u8>>>) -> Vec<String> {
    let page = doc.get_page(page_index).expect("get_page");
    let resources = match page.get_resources() {
        Some(r) => r,
        None => return Vec::new(),
    };
    let font_dict = match resources.get("Font").and_then(|f| f.as_dict()) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for (_name, obj) in font_dict.0.iter() {
        let resolved = doc.resolve(obj).expect("resolve font");
        if let Some(fd) = resolved.as_dict() {
            let base = fd
                .get("BaseFont")
                .and_then(|b| b.as_name())
                .map(|n| n.0.clone())
                .unwrap_or_default();
            out.push(base);
        }
    }
    out
}

/// Build a one-page doc with an RGBA image drawn on it. Returns None if the
/// image reports no transparency — a defensive guard, since `from_rgba_data`
/// always attaches an alpha channel today, so this branch does not fire for the
/// current generator; it keeps the invariant honest if that ever changes.
fn build_rgba_image_doc(w: u32, h: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let image = Image::from_rgba_data(rgba, w, h).ok()?;
    if !image.has_transparency() {
        return None;
    }
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_image("Img", image);
    page.draw_image("Img", 100.0, 100.0, w as f64, h as f64)
        .expect("draw");
    doc.add_page(page);
    Some(doc.to_bytes().expect("serialize"))
}

/// For each image XObject on the page, whether its /SMask resolves to a stream.
/// Mirrors the walk in overlay_smask_test.rs.
fn page_xobject_smask_flags(page_index: u32, doc: &PdfDocument<Cursor<Vec<u8>>>) -> Vec<bool> {
    let page = doc.get_page(page_index).expect("get_page");
    let resources = match page.get_resources() {
        Some(r) => r,
        None => return Vec::new(),
    };
    let xobj = match resources
        .get("XObject")
        .map(|o| doc.resolve(o).expect("resolve xobj"))
    {
        Some(PdfObject::Dictionary(d)) => d,
        _ => return Vec::new(),
    };
    let mut flags = Vec::new();
    for (_name, obj) in xobj.0.iter() {
        if let PdfObject::Stream(stream) = doc.resolve(obj).expect("resolve stream") {
            // Only image XObjects carry /SMask.
            let is_image = stream
                .dict
                .get("Subtype")
                .and_then(|s| s.as_name())
                .map(|n| n.0 == "Image")
                .unwrap_or(false);
            if !is_image {
                continue;
            }
            let smask_is_stream = match stream.dict.get("SMask") {
                Some(sm) => matches!(
                    doc.resolve(sm).expect("resolve smask"),
                    PdfObject::Stream(_)
                ),
                None => false,
            };
            flags.push(smask_is_stream);
        }
    }
    flags
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
        raw in prop::collection::vec(
            prop::collection::vec(marker(), 1..=3),
            1..=4,
        ),
    ) {
        // Prefix each marker with its per-page index (`M{j}_`). The generated
        // markers never contain '_', so `M{j}_...` matches exactly one marker
        // and no marker can be a substring of another on the same page — a
        // `contains` check would otherwise pass for a dropped marker that another
        // one happens to embed (e.g. "Ab12" inside "XAb12Y").
        let pages: Vec<Vec<String>> = raw
            .iter()
            .map(|markers| {
                markers
                    .iter()
                    .enumerate()
                    .map(|(j, m)| format!("M{j}_{m}"))
                    .collect()
            })
            .collect();
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

    /// An image with transparency keeps a resolvable /SMask through the
    /// round-trip (#156).
    #[test]
    fn image_smask_preserved(w in 2u32..=16, h in 2u32..=16, seed in any::<u64>()) {
        // Deterministic RGBA from the seed. `from_rgba_data` always attaches an
        // alpha channel (so the writer always emits an /SMask); pixel 0 is forced
        // to alpha=0 only to keep the data visibly non-opaque for a reader.
        let n = (w * h) as usize;
        let mut rgba = Vec::with_capacity(n * 4);
        let mut s = seed | 1;
        for i in 0..n {
            for _ in 0..3 {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                rgba.push((s >> 33) as u8);
            }
            rgba.push(if i == 0 { 0 } else { ((s >> 40) as u8) | 1 });
        }
        let Some(bytes) = build_rgba_image_doc(w, h, rgba) else {
            return Ok(()); // no transparency (shouldn't happen given alpha=0 above)
        };
        let document = reparse(bytes);
        let flags = page_xobject_smask_flags(0, &document);
        prop_assert!(!flags.is_empty(), "no image XObject found after round-trip");
        prop_assert!(flags.iter().all(|&f| f), "an image lost its /SMask: {flags:?}");
    }
}

proptest! {
    // Reduced case count: each case embeds and subsets a TTF (and sometimes an
    // OTF), which is costly. 64 is a coverage/cost tradeoff, not a coverage
    // ceiling on the invariant.
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Embedded fonts survive the round-trip, distinct and uncollapsed (#395).
    ///
    /// Note: the writer always emits the full standard-14 set into every page's
    /// /Font dict regardless of use, so a *count* over all fonts is meaningless
    /// here. #395 was a collision of preserved *embedded* fonts, so the guard
    /// embeds one or two distinct custom fonts and asserts each survives by name
    /// — a collision would drop or merge one of them.
    #[test]
    fn embedded_fonts_preserved(
        use_second in any::<bool>(),
        std_marker in marker(),
    ) {
        let mut doc = Document::new();
        doc.add_font_from_bytes("Roboto", roboto()).expect("embed Roboto");
        if use_second {
            doc.add_font_from_bytes("SourceSans", source_sans())
                .expect("embed SourceSans");
        }
        let mut page = Page::a4();
        page.text()
            .set_font(Font::Custom("Roboto".to_string()), 12.0)
            .at(72.0, 760.0)
            .write("Robo")
            .expect("write");
        if use_second {
            page.text()
                .set_font(Font::Custom("SourceSans".to_string()), 12.0)
                .at(72.0, 720.0)
                .write("Sans")
                .expect("write");
        }
        // A standard-14 line too, to mix embedded and builtin on one page.
        page.text()
            .set_font(Font::Helvetica, 12.0)
            .at(72.0, 680.0)
            .write(&std_marker)
            .expect("write");
        doc.add_page(page);

        let document = reparse(doc.to_bytes().expect("serialize"));
        let bases = page_base_fonts(0, &document);
        prop_assert!(
            bases.iter().any(|b| b.contains("Roboto")),
            "embedded Roboto lost: {bases:?}"
        );
        if use_second {
            prop_assert!(
                bases.iter().any(|b| b.contains("SourceSans")),
                "second embedded font collapsed or lost: {bases:?}"
            );
        }
        // Helvetica (builtin) is always present too.
        prop_assert!(
            bases.iter().any(|b| b == "Helvetica"),
            "Helvetica missing: {bases:?}"
        );
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

/// #395 pin: two distinct embedded fonts on one page must both survive the
/// round-trip without collapsing into one.
#[test]
fn issue_395_two_distinct_embedded_fonts_no_collision() {
    let mut doc = Document::new();
    doc.add_font_from_bytes("Roboto", roboto())
        .expect("embed Roboto");
    doc.add_font_from_bytes("SourceSans", source_sans())
        .expect("embed SourceSans");
    let mut page = Page::a4();
    page.text()
        .set_font(Font::Custom("Roboto".to_string()), 12.0)
        .at(72.0, 760.0)
        .write("Robo")
        .expect("write");
    page.text()
        .set_font(Font::Custom("SourceSans".to_string()), 12.0)
        .at(72.0, 720.0)
        .write("Sans")
        .expect("write");
    doc.add_page(page);
    let document = reparse(doc.to_bytes().expect("serialize"));
    let bases = page_base_fonts(0, &document);
    assert!(
        bases.iter().any(|b| b.contains("Roboto")),
        "#395: Roboto lost: {bases:?}"
    );
    assert!(
        bases.iter().any(|b| b.contains("SourceSans")),
        "#395: SourceSans collapsed or lost: {bases:?}"
    );
}

/// #156 pin: an RGBA image with partial alpha keeps a resolvable /SMask stream.
#[test]
fn issue_156_rgba_image_keeps_smask() {
    // 2x2 RGBA, one fully transparent pixel.
    let rgba = vec![
        255, 0, 0, 0, // transparent red
        0, 255, 0, 128, // semi green
        0, 0, 255, 200, // mostly blue
        255, 255, 0, 255, // opaque yellow
    ];
    let bytes = build_rgba_image_doc(2, 2, rgba).expect("image has transparency");
    let document = reparse(bytes);
    let flags = page_xobject_smask_flags(0, &document);
    assert_eq!(
        flags.len(),
        1,
        "#156: expected exactly one image XObject: {flags:?}"
    );
    assert!(flags[0], "#156: image lost its /SMask after round-trip");
}
