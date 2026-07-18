//! End-to-end RAG invariants: from PDF bytes through `rag_chunks_with`.
//!
//! These state the two contract properties that are tautological one layer down
//! (see spec §4). A property over `HybridChunker::chunk` takes the `Element`s it
//! is handed as ground truth, so it cannot see that partition dropped a
//! paragraph — for it, that paragraph never existed. Only this layer can.
//!
//! The generator is deliberately narrow (documents this crate's writer can
//! build) and the properties assert over injected markers rather than the whole
//! extracted string: the pipeline reflows separators legitimately.

use oxidize_pdf::parser::{PdfDocument, PdfReader};
use oxidize_pdf::pipeline::HybridChunkConfig;
use oxidize_pdf::{Document, Font, Page};
use proptest::prelude::*;
use std::io::Cursor;

/// Printable, non-space ASCII with no `_`: survives a WinAnsi `write()`, extracts
/// as one contiguous run, and keeps the `_`-terminated markers prefix-free.
fn marker() -> impl Strategy<Value = String> {
    "[A-Za-z0-9]{4,24}"
}

fn open(bytes: Vec<u8>) -> PdfDocument<Cursor<Vec<u8>>> {
    let reader = PdfReader::new(Cursor::new(bytes)).expect("re-parse written PDF");
    PdfDocument::new(reader)
}

/// One page per inner Vec; each marker on its own baseline, 60pt apart so each
/// extracts as its own run and classifies as its own element.
fn build_marker_doc(pages: &[Vec<String>]) -> Vec<u8> {
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
            y -= 60.0;
        }
        doc.add_page(page);
    }
    doc.to_bytes().expect("serialize")
}

/// Build a single-page document of `paras_per_section.len()` sections. Each
/// section is a 20pt bold title `T{s}_HEADING` followed by its 10pt paragraphs
/// `S{n}_ ...`, where `n` is a document-global paragraph counter.
///
/// Returns the bytes plus `para_section[n] = s`, the true section of paragraph
/// `n` — the ground truth the property checks the breadcrumb against.
///
/// Geometry: at most 3 sections × 3 paragraphs = 3 × (40 + 3×20) = 300pt from
/// y=760, so the content never runs off an A4 page.
///
/// The 40pt title→body gap (vs. 20pt between body lines) is the plan's
/// original, realistic geometry: more space-before-heading than intra-
/// paragraph line spacing, as in a real document. At this gap,
/// `merge_into_paragraphs` (src/text/extraction.rs) folds the title into its
/// body — see issue #436 — because its merge threshold
/// (`1.5 * median(line_height)`) has no check on font-size/weight change
/// between lines. Left at 40pt deliberately so the property keeps exercising
/// (and failing against) that real bug instead of walking around it.
fn build_titled_doc(paras_per_section: &[usize]) -> (Vec<u8>, Vec<usize>) {
    let mut doc = Document::new();
    let mut page = Page::a4();
    let mut y = 760.0;
    let mut para_section = Vec::new();
    let mut n = 0usize;

    for (s, &count) in paras_per_section.iter().enumerate() {
        page.text()
            .set_font(Font::HelveticaBold, 20.0)
            .at(72.0, y)
            .write(&format!("T{s}_HEADING"))
            .expect("write title");
        y -= 40.0;
        for _ in 0..count {
            page.text()
                .set_font(Font::Helvetica, 10.0)
                .at(72.0, y)
                .write(&format!("S{n}_ body text of this paragraph"))
                .expect("write paragraph");
            para_section.push(s);
            n += 1;
            y -= 20.0;
        }
    }

    doc.add_page(page);
    (doc.to_bytes().expect("serialize"), para_section)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// I1 — CONSERVATION: every text run written to the document lands in the
    /// `text` of exactly one chunk. Not "at least one": duplication at the
    /// partition layer is invisible to the chunking-layer property, which would
    /// see two distinct elements, place each in its own chunk, and pass clean.
    ///
    /// The `P{p}M{j}_` prefix makes every marker unique and prefix-free, so a
    /// `contains` hit is exact.
    #[test]
    fn every_written_run_lands_in_exactly_one_chunk(
        raw in prop::collection::vec(prop::collection::vec(marker(), 1..=3), 1..=3),
    ) {
        let pages: Vec<Vec<String>> = raw
            .iter()
            .enumerate()
            .map(|(p, ms)| {
                ms.iter()
                    .enumerate()
                    .map(|(j, m)| format!("P{p}M{j}_{m}"))
                    .collect()
            })
            .collect();

        let doc = open(build_marker_doc(&pages));
        let config = HybridChunkConfig { max_tokens: 512, ..Default::default() };
        let chunks = doc.rag_chunks_with(config).expect("rag_chunks_with");

        for markers in &pages {
            for m in markers {
                let hits = chunks.iter().filter(|c| c.text.contains(m.as_str())).count();
                prop_assert_eq!(hits, 1, "run {} landed in {} chunks", m, hits);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// I5 — BREADCRUMB FIDELITY: a chunk's `heading_path` never names a heading
    /// that appears after the chunk's own content. `heading_path` is a filter
    /// field in the vector store; a breadcrumb naming the wrong section returns
    /// wrong passages for a section-scoped query and never errors.
    ///
    /// Honest precondition: the property only asserts on chunks that actually
    /// carry a breadcrumb. If the classifier did not promote our 20pt bold runs
    /// to `Title`, the case proves nothing about breadcrumbs and is discarded —
    /// discarded here, in the open, not hidden behind a weaker assertion.
    ///
    /// The plan's `.first()`/`.last()` ablation is structurally unreachable
    /// through `rag_chunks()`: `HybridChunker::chunk()` treats `Title` as a
    /// hard chunk boundary, so no chunk spans two sections. This property's
    /// falsifiability is instead demonstrated by the real bug #436 it catches.
    ///
    /// PINNED: fails today — see issue #436 (text extraction merges a heading
    /// into the following body when they are close, corrupting heading_path).
    /// The property states the contract; extraction does not honor it yet.
    /// Remove `#[ignore]` when #436 ships and this becomes a permanent guard.
    /// Precedent: #430, #434, #435.
    #[test]
    #[ignore = "issue #436: font-blind paragraph merge swallows headings into body"]
    fn breadcrumb_never_names_a_later_heading(
        paras_per_section in prop::collection::vec(1usize..=3usize, 1..=3),
    ) {
        let (bytes, para_section) = build_titled_doc(&paras_per_section);
        let doc = open(bytes);
        let chunks = doc.rag_chunks().expect("rag_chunks");

        prop_assume!(chunks.iter().any(|c| !c.metadata.heading_path.is_empty()));

        for c in &chunks {
            if c.metadata.heading_path.is_empty() {
                continue;
            }
            // Lowest section index among the paragraphs this chunk carries.
            let own_section = (0..para_section.len())
                .filter(|n| c.text.contains(&format!("S{n}_")))
                .map(|n| para_section[n])
                .min();
            let Some(own_section) = own_section else {
                continue; // title-only chunk: no paragraph to anchor against
            };
            for h in &c.metadata.heading_path {
                for s in 0..paras_per_section.len() {
                    if h.contains(&format!("T{s}_")) {
                        prop_assert!(
                            s <= own_section,
                            "chunk in section {} carries breadcrumb {:?} from later section {}",
                            own_section,
                            h,
                            s
                        );
                    }
                }
            }
        }
    }
}
