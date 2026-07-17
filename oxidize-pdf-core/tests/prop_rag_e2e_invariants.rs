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
