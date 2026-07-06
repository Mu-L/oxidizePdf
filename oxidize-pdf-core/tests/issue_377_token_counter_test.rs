//! #377 — TokenCounter injection into the chunking path.
use oxidize_pdf::pipeline::{
    Element, ElementData, ElementMetadata, HybridChunkConfig, HybridChunker,
};
use std::sync::Arc;

fn para(text: &str) -> Element {
    Element::Paragraph(ElementData {
        text: text.to_string(),
        metadata: ElementMetadata::default(),
    })
}

#[test]
fn default_hybrid_chunker_stamps_word_count() {
    // No injection → WordProxyCounter → token_estimate == whitespace word count.
    let elements = vec![para("alpha beta gamma delta")];
    let chunker = HybridChunker::new(HybridChunkConfig::default());
    let chunks = chunker.chunk(&elements);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].token_estimate(), 4);
}

#[test]
fn with_token_counter_accepts_word_proxy_explicitly() {
    use oxidize_pdf::pipeline::WordProxyCounter;
    let elements = vec![para("alpha beta gamma")];
    let chunker = HybridChunker::new(HybridChunkConfig::default())
        .with_token_counter(Arc::new(WordProxyCounter));
    let chunks = chunker.chunk(&elements);
    assert_eq!(chunks[0].token_estimate(), 3);
}

#[test]
fn default_semantic_chunker_stamps_word_count() {
    use oxidize_pdf::pipeline::{SemanticChunkConfig, SemanticChunker};
    let elements = vec![para("one two three four five")];
    let chunker = SemanticChunker::new(SemanticChunkConfig::default());
    let chunks = chunker.chunk(&elements);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].token_estimate(), 5);
}

#[test]
fn semantic_with_token_counter_builder_exists() {
    use oxidize_pdf::pipeline::{SemanticChunkConfig, SemanticChunker, WordProxyCounter};
    let elements = vec![para("one two three")];
    let chunker = SemanticChunker::new(SemanticChunkConfig::default())
        .with_token_counter(Arc::new(WordProxyCounter));
    let chunks = chunker.chunk(&elements);
    assert_eq!(chunks[0].token_estimate(), 3);
}

/// Build a small in-memory PDF whose body, chunked with a tight token budget,
/// yields several chunks. Pattern mirrors `rag_chunk_metadata_test.rs::build_chunks`.
fn build_test_document() -> oxidize_pdf::parser::PdfDocument<std::io::Cursor<Vec<u8>>> {
    use oxidize_pdf::parser::{PdfDocument, PdfReader};
    use oxidize_pdf::text::Font;
    use oxidize_pdf::{Document, Page};
    use std::io::Cursor;

    let mut doc = Document::new();
    let mut page = Page::a4();

    page.text()
        .set_font(Font::HelveticaBold, 16.0)
        .at(50.0, 760.0)
        .write("SECTION ALPHA HEADING")
        .unwrap();

    let body_lines = [
        (720.0, "Alpha marker paragraph with several words to fill the first token budget bucket completely."),
        (700.0, "Bravo marker paragraph with several words to fill the second token budget bucket completely."),
        (680.0, "Charlie marker paragraph with several words to fill the third token budget bucket completely."),
        (660.0, "Delta marker paragraph with several words to fill the fourth token budget bucket completely."),
        (640.0, "Echo marker paragraph with several words to fill the fifth token budget bucket completely."),
        (620.0, "Foxtrot marker paragraph with several words to fill the sixth token budget bucket completely."),
    ];
    for (y, line) in body_lines {
        page.text()
            .set_font(Font::Helvetica, 11.0)
            .at(50.0, y)
            .write(line)
            .unwrap();
    }

    doc.add_page(page);
    let pdf_bytes = doc.to_bytes().expect("pdf generation should succeed");

    let reader = PdfReader::new(Cursor::new(pdf_bytes)).expect("parse generated PDF");
    PdfDocument::new(reader)
}

#[test]
fn rag_chunks_with_counter_word_proxy_parity() {
    use oxidize_pdf::pipeline::{HybridChunkConfig, MergePolicy, WordProxyCounter};

    let doc = build_test_document();

    // Tight token budget to force multiple chunks; merge adjacent so paragraphs
    // accrete into a bucket until the budget overflows.
    let config = HybridChunkConfig {
        max_tokens: 12,
        overlap_tokens: 0,
        merge_adjacent: true,
        propagate_headings: true,
        merge_policy: MergePolicy::AnyInlineContent,
    };

    let base = doc.rag_chunks_with(config.clone()).unwrap();
    let injected = doc
        .rag_chunks_with_counter(config, Arc::new(WordProxyCounter))
        .unwrap();

    assert!(
        base.len() >= 2,
        "expected at least two chunks to make the parity check meaningful, got {}",
        base.len()
    );

    // Same grouping/metadata; word-proxy injection changes nothing.
    assert_eq!(base.len(), injected.len());
    for (b, i) in base.iter().zip(&injected) {
        assert_eq!(b.text, i.text);
        assert_eq!(b.token_estimate, i.token_estimate);
    }
}
