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
