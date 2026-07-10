use std::collections::HashSet;

use crate::pipeline::chunk_metadata::ChunkMetadata;
use crate::pipeline::element::Element;
use crate::pipeline::hybrid_chunking::{ContextFormat, ContextMode, HybridChunk};
use crate::pipeline::{DocumentSource, ElementBBox};

#[cfg(feature = "semantic")]
use serde::{Deserialize, Serialize};

/// A RAG-ready chunk with full metadata for vector store ingestion.
///
/// Each `RagChunk` carries everything a vector store needs: text for embedding,
/// heading context for retrieval, and structural metadata (pages, bounding boxes,
/// element types) for citation and filtering.
///
/// Construct via [`PdfDocument::rag_chunks()`](crate::parser::PdfDocument::rag_chunks)
/// or [`PdfDocument::rag_chunks_with_profile()`](crate::parser::PdfDocument::rag_chunks_with_profile).
///
/// # Field guide
///
/// - `text`: raw chunk text for display or keyword search
/// - `full_text`: heading context + text — **use this for embedding generation**
/// - `token_estimate`: token count under the chunker's active `TokenCounter`
///   (word-count proxy by default; a real tokenizer such as cl100k_base when a
///   counter is injected via `rag_chunks_with_counter`). `RagChunk` does not
///   store the counter; query the counter you injected via its `name()` for
///   provenance.
/// - `is_oversized`: true when a single element exceeds `max_tokens`
///
/// # Example
///
/// ```rust,no_run
/// use oxidize_pdf::parser::PdfDocument;
/// use oxidize_pdf::pipeline::ExtractionProfile;
///
/// let doc = PdfDocument::open("paper.pdf")?;
/// let chunks = doc.rag_chunks_with_profile(ExtractionProfile::Rag)?;
///
/// for chunk in &chunks {
///     println!(
///         "[chunk {}] pages={:?} tokens~{} types={:?}",
///         chunk.chunk_index, chunk.page_numbers,
///         chunk.token_estimate, chunk.element_types,
///     );
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "semantic", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub struct RagChunk {
    /// Sequential index of this chunk in the document (0-based).
    pub chunk_index: usize,
    /// Chunk text content (elements joined by newlines).
    pub text: String,
    /// Text for embedding generation, contextualized per the chunker's
    /// [`ContextMode`](crate::pipeline::ContextMode): the bare `text`
    /// (`None`), the leaf heading prepended (`Heading`, the default), or a
    /// deterministic document + section prefix (`Contextual`). The display
    /// `text` field is always context-free.
    pub full_text: String,
    /// Page numbers where this chunk's elements appear (deduplicated, sorted numerically).
    pub page_numbers: Vec<u32>,
    /// Bounding boxes of each element in the chunk.
    pub bounding_boxes: Vec<ElementBBox>,
    /// Type names of each element (e.g. "title", "paragraph", "table").
    pub element_types: Vec<String>,
    /// Heading context inherited from the nearest parent heading.
    pub heading_context: Option<String>,
    /// Token count under the chunker's active `TokenCounter` (word-count
    /// proxy by default; a real tokenizer such as cl100k_base when a counter
    /// is injected via `rag_chunks_with_counter`). `RagChunk` does not store
    /// the counter; query the counter you injected via its `name()`.
    pub token_estimate: usize,
    /// Whether the chunk exceeds the configured `max_tokens`.
    pub is_oversized: bool,
    /// Rich per-chunk metadata (heading path, font/style, counts, ids, source).
    pub metadata: ChunkMetadata,
}

impl RagChunk {
    /// Build a `RagChunk` from a [`HybridChunk`], extracting all metadata from its
    /// elements. Uses [`ContextMode::Heading`] for `full_text` (the pre-#376
    /// behavior); use [`from_hybrid_chunk_with_mode`](Self::from_hybrid_chunk_with_mode)
    /// to select a different [`ContextMode`].
    pub fn from_hybrid_chunk(chunk_index: usize, chunk: &HybridChunk) -> Self {
        Self::from_hybrid_chunk_inner(chunk_index, chunk, None, ContextMode::Heading)
    }

    /// Like [`from_hybrid_chunk`](Self::from_hybrid_chunk) but stamping source
    /// metadata and using `source.doc_hash` for the chunk_id prefix when set.
    /// Uses [`ContextMode::Heading`]; see
    /// [`from_hybrid_chunk_with_source_and_mode`](Self::from_hybrid_chunk_with_source_and_mode).
    pub fn from_hybrid_chunk_with_source(
        chunk_index: usize,
        chunk: &HybridChunk,
        source: &DocumentSource,
    ) -> Self {
        Self::from_hybrid_chunk_with_source_and_mode(
            chunk_index,
            chunk,
            source,
            ContextMode::Heading,
        )
    }

    /// Build a `RagChunk` selecting how `full_text` is contextualized (issue #376).
    /// No source is stamped, so `Contextual` prefixes carry section context only.
    pub fn from_hybrid_chunk_with_mode(
        chunk_index: usize,
        chunk: &HybridChunk,
        context_mode: ContextMode,
    ) -> Self {
        Self::from_hybrid_chunk_inner(chunk_index, chunk, None, context_mode)
    }

    /// Build a source-stamped `RagChunk` selecting how `full_text` is
    /// contextualized (issue #376). `Contextual` prefixes draw the document
    /// name/author from `source` and the section breadcrumb from the chunk's
    /// elements.
    pub fn from_hybrid_chunk_with_source_and_mode(
        chunk_index: usize,
        chunk: &HybridChunk,
        source: &DocumentSource,
        context_mode: ContextMode,
    ) -> Self {
        let mut c = Self::from_hybrid_chunk_inner(chunk_index, chunk, Some(source), context_mode);
        c.metadata.source = Some(source.clone());
        c
    }

    /// Shared constructor. `source` (when `Some`) supplies the `doc_hash` used
    /// as the chunk_id prefix, so the id is computed exactly once — callers that
    /// also want the full source stamped do that themselves. `context_mode`
    /// selects how `full_text` is built (issue #376).
    fn from_hybrid_chunk_inner(
        chunk_index: usize,
        chunk: &HybridChunk,
        source: Option<&DocumentSource>,
        context_mode: ContextMode,
    ) -> Self {
        let elements = chunk.elements();
        let page_numbers = collect_pages(elements);
        let bounding_boxes = elements.iter().map(|e| *e.bbox()).collect();
        let element_types: Vec<String> =
            elements.iter().map(|e| e.type_name().to_string()).collect();
        let text = chunk.text();
        // `full_text` (the embedding text) is contextualized per `context_mode`.
        // It is built before metadata because `ChunkMetadata::from_elements`
        // hashes it for the content-derived `chunk_id`.
        let full_text = match context_mode {
            ContextMode::None => text.clone(),
            ContextMode::Heading => chunk.full_text(),
            ContextMode::Contextual(format) => {
                let heading_path = elements
                    .first()
                    .map(|e| e.metadata().heading_path.clone())
                    .unwrap_or_default();
                let page_span = (!page_numbers.is_empty())
                    .then(|| (page_numbers[0], page_numbers[page_numbers.len() - 1]));
                match build_context_prefix(format, source, &heading_path, page_span) {
                    Some(prefix) => format!("{prefix}\n\n{text}"),
                    None => text.clone(),
                }
            }
        };
        let doc_hash = source.and_then(|s| s.doc_hash.as_deref());
        let metadata =
            ChunkMetadata::from_elements(elements, &text, &full_text, chunk_index, doc_hash);

        Self {
            chunk_index,
            text,
            full_text,
            page_numbers,
            bounding_boxes,
            element_types,
            heading_context: chunk.heading_context.clone(),
            token_estimate: chunk.token_estimate(),
            is_oversized: chunk.is_oversized(),
            metadata,
        }
    }

    /// Serialize this chunk to a JSON string (requires `semantic` feature).
    #[cfg(feature = "semantic")]
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Collect unique page numbers from elements, deduplicated and sorted numerically.
fn collect_pages(elements: &[Element]) -> Vec<u32> {
    if elements.is_empty() {
        return Vec::new();
    }
    // Fast path: all elements on the same page (most common case)
    let first_page = elements[0].page();
    if elements.iter().all(|e| e.page() == first_page) {
        return vec![first_page];
    }
    // General path: deduplicate and sort
    let mut seen = HashSet::new();
    let mut pages = Vec::new();
    for e in elements {
        let p = e.page();
        if seen.insert(p) {
            pages.push(p);
        }
    }
    pages.sort_unstable();
    pages
}

/// Render a page span as `p. N` (single page) or `p. N–M` (range, en dash).
fn render_page(span: (u32, u32)) -> String {
    if span.0 == span.1 {
        format!("p. {}", span.0)
    } else {
        format!("p. {}\u{2013}{}", span.0, span.1)
    }
}

/// Build the deterministic, no-ML context prefix for a chunk (issue #376).
///
/// Pure function of its inputs (document source fields, heading breadcrumb, page
/// span) → reproducible across runs. Returns `None` when there is neither a
/// document name nor a section to anchor on (a page alone is not enough
/// context); the caller then leaves `full_text == text`.
///
/// `author` is deliberately only rendered alongside a document name (title or
/// filename) — an authored *section* reads oddly and adds no retrieval signal —
/// so a source with only an `author` set and no title/filename contributes
/// nothing to a section-only prefix.
fn build_context_prefix(
    format: ContextFormat,
    source: Option<&DocumentSource>,
    heading_path: &[String],
    page_span: Option<(u32, u32)>,
) -> Option<String> {
    // `title` wins; `filename` is the fallback document name.
    let doc_name = source.and_then(|s| s.title.clone().or_else(|| s.filename.clone()));
    let author = source.and_then(|s| s.author.clone());
    let section = (!heading_path.is_empty()).then(|| heading_path.join(" \u{203A} "));
    let page = page_span.map(render_page);

    // No document or section anchor → no prefix at all.
    if doc_name.is_none() && section.is_none() {
        return None;
    }

    let prefix = match format {
        ContextFormat::Labeled => {
            let mut lines: Vec<String> = Vec::new();
            if let Some(name) = &doc_name {
                lines.push(match &author {
                    Some(a) => format!("Document: {name} — {a}"),
                    None => format!("Document: {name}"),
                });
            }
            if let Some(sec) = &section {
                lines.push(format!("Section: {sec}"));
            }
            // The page span attaches to the last (most specific) context line.
            if let (Some(pg), Some(last)) = (&page, lines.last_mut()) {
                last.push_str(&format!(" ({pg})"));
            }
            lines.join("\n")
        }
        ContextFormat::Prose => {
            let mut s = String::from("This chunk is from ");
            if let Some(name) = &doc_name {
                s.push_str(&format!("\"{name}\""));
                if let Some(a) = &author {
                    s.push_str(&format!(" by {a}"));
                }
                if let Some(sec) = &section {
                    s.push_str(&format!(", section \"{sec}\""));
                }
            } else if let Some(sec) = &section {
                s.push_str(&format!("section \"{sec}\""));
            }
            if let Some(pg) = &page {
                s.push_str(&format!(" ({pg})"));
            }
            s.push('.');
            s
        }
    };
    Some(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{ContextFormat, DocumentSource};

    fn src(title: Option<&str>, author: Option<&str>, filename: Option<&str>) -> DocumentSource {
        DocumentSource {
            title: title.map(str::to_string),
            author: author.map(str::to_string),
            filename: filename.map(str::to_string),
            ..Default::default()
        }
    }

    fn hp(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    // ── issue #376: Labeled prefix ───────────────────────────────────────────

    #[test]
    fn labeled_full_fields() {
        let s = src(Some("Annual Report"), Some("Acme Corp"), None);
        let p = build_context_prefix(
            ContextFormat::Labeled,
            Some(&s),
            &hp(&["1 Intro", "1.2 Scope"]),
            Some((3, 4)),
        );
        assert_eq!(
            p.as_deref(),
            Some("Document: Annual Report — Acme Corp\nSection: 1 Intro › 1.2 Scope (p. 3–4)")
        );
    }

    #[test]
    fn labeled_title_only() {
        let s = src(Some("Doc"), None, None);
        let p = build_context_prefix(ContextFormat::Labeled, Some(&s), &[], None);
        assert_eq!(p.as_deref(), Some("Document: Doc"));
    }

    #[test]
    fn labeled_filename_fallback_when_no_title() {
        let s = src(None, None, Some("report.pdf"));
        let p = build_context_prefix(ContextFormat::Labeled, Some(&s), &[], None);
        assert_eq!(p.as_deref(), Some("Document: report.pdf"));
    }

    #[test]
    fn labeled_section_only_without_source() {
        let p = build_context_prefix(ContextFormat::Labeled, None, &hp(&["A", "B"]), None);
        assert_eq!(p.as_deref(), Some("Section: A › B"));
    }

    #[test]
    fn labeled_single_page() {
        let p = build_context_prefix(ContextFormat::Labeled, None, &hp(&["A"]), Some((7, 7)));
        assert_eq!(p.as_deref(), Some("Section: A (p. 7)"));
    }

    #[test]
    fn labeled_nothing_is_none() {
        let s = src(None, None, None);
        assert!(
            build_context_prefix(ContextFormat::Labeled, Some(&s), &[], Some((3, 3))).is_none()
        );
        assert!(build_context_prefix(ContextFormat::Labeled, None, &[], None).is_none());
    }

    // ── issue #376: Prose prefix ─────────────────────────────────────────────

    #[test]
    fn prose_full_fields() {
        let s = src(Some("Annual Report"), Some("Acme Corp"), None);
        let p = build_context_prefix(
            ContextFormat::Prose,
            Some(&s),
            &hp(&["1 Intro", "1.2 Scope"]),
            Some((3, 4)),
        );
        assert_eq!(
            p.as_deref(),
            Some(
                "This chunk is from \"Annual Report\" by Acme Corp, section \"1 Intro › 1.2 Scope\" (p. 3–4)."
            )
        );
    }

    #[test]
    fn prose_title_only() {
        let s = src(Some("Doc"), None, None);
        let p = build_context_prefix(ContextFormat::Prose, Some(&s), &[], None);
        assert_eq!(p.as_deref(), Some("This chunk is from \"Doc\"."));
    }

    #[test]
    fn prose_section_only_without_source() {
        let p = build_context_prefix(ContextFormat::Prose, None, &hp(&["A", "B"]), Some((5, 5)));
        assert_eq!(
            p.as_deref(),
            Some("This chunk is from section \"A › B\" (p. 5).")
        );
    }

    #[test]
    fn prose_filename_fallback_and_no_section() {
        let s = src(None, Some("Ann"), Some("f.pdf"));
        let p = build_context_prefix(ContextFormat::Prose, Some(&s), &[], None);
        assert_eq!(p.as_deref(), Some("This chunk is from \"f.pdf\" by Ann."));
    }

    #[test]
    fn prose_nothing_is_none() {
        assert!(build_context_prefix(ContextFormat::Prose, None, &[], Some((1, 2))).is_none());
    }
}
