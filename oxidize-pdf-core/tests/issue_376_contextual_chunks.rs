//! Feature tests for issue #376 — first-class deterministic contextual RAG
//! chunks (no-ML Contextual Retrieval).
//!
//! `ContextMode` on `HybridChunkConfig` selects how each chunk's `full_text`
//! (the embedding text) is contextualized: `None` (== text), `Heading` (the
//! pre-#376 leaf-heading behavior, default), or `Contextual(ContextFormat)`
//! which prepends a deterministic document + section snippet. The display
//! `text` field always stays context-free.

use oxidize_pdf::pipeline::{
    ContextFormat, ContextMode, DocumentSource, Element, ElementData, ElementMetadata, HybridChunk,
    HybridChunkConfig, HybridChunker, RagChunk,
};

/// A paragraph on page 3 under the breadcrumb `1 Intro › 1.2 Scope`.
fn para(text: &str) -> Element {
    let mut e = Element::Paragraph(ElementData {
        text: text.to_string(),
        metadata: ElementMetadata::default(),
    });
    e.metadata_mut().page = 3;
    e.set_parent_heading(Some("1.2 Scope".to_string()));
    e.set_heading_path(vec!["1 Intro".to_string(), "1.2 Scope".to_string()]);
    e
}

fn one_chunk() -> Vec<HybridChunk> {
    let chunker = HybridChunker::new(HybridChunkConfig::default());
    chunker.chunk(&[para("Alpha beta gamma."), para("Delta epsilon.")])
}

fn source() -> DocumentSource {
    let mut s = DocumentSource::with_file(Some("annual.pdf".to_string()), Some("h123".to_string()));
    s.title = Some("Annual Report".to_string());
    s.author = Some("Acme".to_string());
    s
}

#[test]
fn mode_none_full_text_equals_text() {
    let hcs = one_chunk();
    let c =
        RagChunk::from_hybrid_chunk_with_source_and_mode(0, &hcs[0], &source(), ContextMode::None);
    assert_eq!(c.full_text, c.text, "None mode: full_text is the bare text");
}

#[test]
fn mode_heading_matches_legacy_behavior() {
    let hcs = one_chunk();
    let legacy = RagChunk::from_hybrid_chunk_with_source(0, &hcs[0], &source());
    let heading = RagChunk::from_hybrid_chunk_with_source_and_mode(
        0,
        &hcs[0],
        &source(),
        ContextMode::Heading,
    );
    assert_eq!(
        heading.full_text, legacy.full_text,
        "Heading mode must be byte-identical to the pre-#376 constructor"
    );
}

#[test]
fn mode_contextual_labeled_prefix_and_clean_text() {
    let hcs = one_chunk();
    let c = RagChunk::from_hybrid_chunk_with_source_and_mode(
        0,
        &hcs[0],
        &source(),
        ContextMode::Contextual(ContextFormat::Labeled),
    );
    assert!(
        c.full_text
            .starts_with("Document: Annual Report — Acme\nSection: 1 Intro › 1.2 Scope (p. 3)\n\n"),
        "labeled prefix missing/wrong: {:?}",
        c.full_text
    );
    assert!(
        c.full_text.ends_with(&c.text),
        "full_text must end with the bare chunk text"
    );
    assert!(
        !c.text.contains("Document:") && !c.text.contains("Section:"),
        "display text must stay context-free: {:?}",
        c.text
    );
}

#[test]
fn mode_contextual_prose_prefix() {
    let hcs = one_chunk();
    let c = RagChunk::from_hybrid_chunk_with_source_and_mode(
        0,
        &hcs[0],
        &source(),
        ContextMode::Contextual(ContextFormat::Prose),
    );
    assert!(
        c.full_text.starts_with(
            "This chunk is from \"Annual Report\" by Acme, section \"1 Intro › 1.2 Scope\" (p. 3).\n\n"
        ),
        "prose prefix missing/wrong: {:?}",
        c.full_text
    );
    assert!(c.full_text.ends_with(&c.text));
}

#[test]
fn contextual_is_deterministic() {
    let hcs = one_chunk();
    let mk = || {
        RagChunk::from_hybrid_chunk_with_source_and_mode(
            0,
            &hcs[0],
            &source(),
            ContextMode::Contextual(ContextFormat::Labeled),
        )
    };
    let a = mk();
    let b = mk();
    assert_eq!(a.full_text, b.full_text, "prefix is a pure function");
    assert_eq!(
        a.metadata.chunk_id, b.metadata.chunk_id,
        "chunk_id reproducible"
    );
}

#[test]
fn contextual_without_source_is_section_only() {
    let hcs = one_chunk();
    // No `DocumentSource`: the prefix carries the section breadcrumb only.
    let labeled = RagChunk::from_hybrid_chunk_with_mode(
        0,
        &hcs[0],
        ContextMode::Contextual(ContextFormat::Labeled),
    );
    assert!(
        labeled
            .full_text
            .starts_with("Section: 1 Intro › 1.2 Scope (p. 3)\n\n"),
        "no-source labeled prefix should be section-only: {:?}",
        labeled.full_text
    );
    assert!(
        !labeled.full_text.contains("Document:"),
        "no source → no Document line"
    );

    let prose = RagChunk::from_hybrid_chunk_with_mode(
        0,
        &hcs[0],
        ContextMode::Contextual(ContextFormat::Prose),
    );
    assert!(
        prose
            .full_text
            .starts_with("This chunk is from section \"1 Intro › 1.2 Scope\" (p. 3).\n\n"),
        "no-source prose prefix should be section-only: {:?}",
        prose.full_text
    );
}

#[test]
fn chunk_id_is_mode_independent_when_doc_hash_set() {
    let hcs = one_chunk();
    let heading = RagChunk::from_hybrid_chunk_with_source_and_mode(
        0,
        &hcs[0],
        &source(),
        ContextMode::Heading,
    );
    let contextual = RagChunk::from_hybrid_chunk_with_source_and_mode(
        0,
        &hcs[0],
        &source(),
        ContextMode::Contextual(ContextFormat::Labeled),
    );
    assert_eq!(
        heading.metadata.chunk_id, contextual.metadata.chunk_id,
        "with doc_hash set, chunk_id is derived from the hash, not full_text"
    );
}
