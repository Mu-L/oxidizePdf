use crate::pipeline::graph::ElementGraph;
use crate::pipeline::token_counter::{TokenCounter, WordProxyCounter};
use crate::pipeline::{Element, ElementData, ElementMetadata};
use std::sync::Arc;

/// Policy for which adjacent element types can be merged into a single chunk.
///
/// - `SameTypeOnly`: strict boundaries — only paragraphs merge with paragraphs,
///   list items with list items. Produces more, smaller chunks. Use for legal text
///   or documents where semantic type boundaries matter.
///
/// - `AnyInlineContent` (default): merges any inline content (paragraphs, list items,
///   key-values) into a single chunk up to `max_tokens`. Reduces fragmentation.
///   Use for general RAG workloads.
///
/// # Example
///
/// ```rust
/// use oxidize_pdf::pipeline::{HybridChunkConfig, MergePolicy};
///
/// let strict = HybridChunkConfig {
///     merge_policy: MergePolicy::SameTypeOnly,
///     ..HybridChunkConfig::default()
/// };
/// assert_eq!(strict.merge_policy, MergePolicy::SameTypeOnly);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MergePolicy {
    /// Only merge Paragraph+Paragraph and ListItem+ListItem (legacy behavior).
    SameTypeOnly,
    /// Merge any adjacent non-structural elements (Paragraph, ListItem, KeyValue).
    /// Titles, Tables, Images, and CodeBlocks always start a new chunk.
    AnyInlineContent,
}

/// Configuration for hybrid chunking.
///
/// # Example
///
/// ```rust
/// use oxidize_pdf::pipeline::{HybridChunkConfig, MergePolicy};
///
/// let config = HybridChunkConfig {
///     max_tokens: 256,
///     overlap_tokens: 30,
///     merge_adjacent: true,
///     propagate_headings: true,
///     merge_policy: MergePolicy::AnyInlineContent,
///     ..Default::default()
/// };
/// assert_eq!(config.max_tokens, 256);
/// ```
#[derive(Debug, Clone)]
pub struct HybridChunkConfig {
    /// Maximum tokens per chunk (approximate — uses word count as proxy).
    pub max_tokens: usize,
    /// Reserved for future text-level overlap. Currently ignored.
    ///
    /// Prior versions used this to copy elements from the end of a flushed
    /// chunk back into the working buffer, which produced chunks with
    /// overlapping element sets and violated the disjointness invariant
    /// required by RAG ingestion (see
    /// `tests/hybrid_chunker_disjoint_test.rs`). The field is preserved for
    /// API compatibility; if a text-level overlap is reintroduced later it
    /// will honor this value.
    pub overlap_tokens: usize,
    /// Whether to merge adjacent elements of the same type (Paragraph+Paragraph, ListItem+ListItem).
    pub merge_adjacent: bool,
    /// Whether to propagate heading context from `parent_heading` metadata.
    pub propagate_headings: bool,
    /// Merge policy for adjacent elements. Default: `MergePolicy::AnyInlineContent`.
    pub merge_policy: MergePolicy,
    /// How each chunk's `full_text` is contextualized for RAG embedding
    /// (issue #376). Default [`ContextMode::Heading`] — byte-identical to the
    /// pre-#376 behavior. Set [`ContextMode::Contextual`] for deterministic,
    /// no-ML document+section context prefixes.
    pub context_mode: ContextMode,
}

impl Default for HybridChunkConfig {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            overlap_tokens: 50,
            merge_adjacent: true,
            propagate_headings: true,
            merge_policy: MergePolicy::AnyInlineContent,
            context_mode: ContextMode::Heading,
        }
    }
}

/// How each chunk's `full_text` (the text used for RAG embedding) is
/// contextualized (issue #376). Contextualization is deterministic and no-ML —
/// the no-ML analogue of Contextual Retrieval / Late Chunking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextMode {
    /// No context prefix — `full_text == text`.
    None,
    /// Prepend only the leaf heading context (the pre-#376 behavior, and the
    /// default so existing callers see byte-identical output).
    #[default]
    Heading,
    /// Prepend a deterministic document + section context snippet in the given
    /// [`ContextFormat`], situating the chunk in its document (title/author or
    /// filename, full heading breadcrumb, optional page span).
    Contextual(ContextFormat),
}

/// Deterministic, no-ML context-prefix format for [`ContextMode::Contextual`].
///
/// `#[non_exhaustive]` so future formats (e.g. structured JSON/YAML) can be
/// added without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContextFormat {
    /// Labeled lines, e.g. `Document: <title> — <author>` / `Section: <breadcrumb> (p. N–M)`.
    Labeled,
    /// A single natural-language sentence, e.g.
    /// `This chunk is from "<title>" by <author>, section "<breadcrumb>" (p. N–M).`
    Prose,
}

/// A hybrid chunk: a group of elements with heading context.
#[derive(Debug, Clone)]
pub struct HybridChunk {
    elements: Vec<Element>,
    /// The heading context for this chunk (from `parent_heading` of its elements).
    pub heading_context: Option<String>,
    oversized: bool,
    /// Token count stamped at construction by the chunker's active counter,
    /// computed once over the whole chunk text. Returned by `token_estimate()`.
    token_estimate: usize,
}

impl HybridChunk {
    /// The elements in this chunk.
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    /// Concatenated text of all elements.
    pub fn text(&self) -> String {
        self.elements
            .iter()
            .map(|e| e.display_text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Text optimized for RAG embedding: heading context prepended (if any) + chunk content.
    /// Use this for embedding generation. Use `text()` for display.
    pub fn full_text(&self) -> String {
        match &self.heading_context {
            Some(heading) => format!("{}\n\n{}", heading, self.text()),
            None => self.text(),
        }
    }

    /// Token count under the counter that produced this chunk (word-proxy by
    /// default; the chunker's injected `TokenCounter` otherwise).
    pub fn token_estimate(&self) -> usize {
        self.token_estimate
    }

    /// Whether this chunk exceeds max_tokens (e.g., a large table).
    pub fn is_oversized(&self) -> bool {
        self.oversized
    }

    /// Convert into a [`ChunkGroup`](crate::pipeline::spi::ChunkGroup),
    /// dropping the derived `oversized` flag (the pipeline recomputes it).
    #[cfg(feature = "unstable-spi")]
    pub(crate) fn into_group(self) -> crate::pipeline::spi::ChunkGroup {
        crate::pipeline::spi::ChunkGroup {
            elements: self.elements,
            heading_context: self.heading_context,
        }
    }

    /// Build a chunk from a [`ChunkGroup`](crate::pipeline::spi::ChunkGroup),
    /// recomputing `oversized` against `max_tokens`. Used by the pipeline when a
    /// custom strategy produced the grouping.
    #[cfg(feature = "unstable-spi")]
    pub(crate) fn from_group(group: crate::pipeline::spi::ChunkGroup, max_tokens: usize) -> Self {
        let text = group
            .elements
            .iter()
            .map(|e| e.display_text())
            .collect::<Vec<_>>()
            .join("\n");
        // The SPI pipeline path is word-proxy only (#377 resolution C): it has no
        // HybridChunker instance to carry an injected counter.
        let token_estimate = WordProxyCounter.count(&text);
        let oversized = token_estimate > max_tokens;
        HybridChunk {
            elements: group.elements,
            heading_context: group.heading_context,
            oversized,
            token_estimate,
        }
    }
}

/// Hybrid chunker that merges adjacent elements and propagates heading context.
///
/// Groups adjacent inline elements (paragraphs, list items, key-values) into
/// chunks bounded by `max_tokens`. Structural elements (titles, tables, images,
/// code blocks) always start a new chunk. Heading context from the nearest
/// parent title is attached to each chunk for RAG embedding via
/// [`HybridChunk::full_text()`].
///
/// For most use cases, prefer [`PdfDocument::rag_chunks()`](crate::parser::PdfDocument::rag_chunks)
/// which wraps this chunker and returns serializable [`RagChunk`](crate::pipeline::RagChunk)s.
///
/// # Example
///
/// ```rust,no_run
/// use oxidize_pdf::pipeline::{HybridChunker, HybridChunkConfig};
/// use oxidize_pdf::parser::PdfDocument;
///
/// let doc = PdfDocument::open("document.pdf")?;
/// let elements = doc.partition()?;
///
/// let config = HybridChunkConfig { max_tokens: 256, ..Default::default() };
/// let chunker = HybridChunker::new(config);
/// let chunks = chunker.chunk(&elements);
///
/// for chunk in &chunks {
///     println!("~{} tokens: {}", chunk.token_estimate(),
///         chunk.full_text().chars().take(50).collect::<String>());
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct HybridChunker {
    config: HybridChunkConfig,
    counter: Arc<dyn TokenCounter>,
}

impl Default for HybridChunker {
    fn default() -> Self {
        Self {
            config: HybridChunkConfig::default(),
            counter: Arc::new(WordProxyCounter),
        }
    }
}

impl HybridChunker {
    pub fn new(config: HybridChunkConfig) -> Self {
        Self {
            config,
            counter: Arc::new(WordProxyCounter),
        }
    }

    /// Inject a token counter governing split/oversize decisions and the stamped
    /// per-chunk `token_estimate`. Default is `WordProxyCounter`.
    pub fn with_token_counter(mut self, counter: Arc<dyn TokenCounter>) -> Self {
        self.counter = counter;
        self
    }

    /// Build a chunk, stamping its token count once over the whole chunk text
    /// with the active counter.
    ///
    /// The separator here and in [`join_texts`] must stay the same: the budget
    /// decision measures the text this function will later emit, and a different
    /// separator would make the two measurements disagree again (#435).
    fn make_chunk(
        &self,
        elements: Vec<Element>,
        heading_context: Option<String>,
        oversized: bool,
    ) -> HybridChunk {
        let text = elements
            .iter()
            .map(|e| e.display_text())
            .collect::<Vec<_>>()
            .join("\n");
        let token_estimate = self.counter.count(&text);
        HybridChunk {
            elements,
            heading_context,
            oversized,
            token_estimate,
        }
    }

    /// Chunk a list of elements into hybrid chunks.
    pub fn chunk(&self, elements: &[Element]) -> Vec<HybridChunk> {
        if elements.is_empty() {
            return Vec::new();
        }

        let additive = self.counter.is_additive_over_whitespace_join();

        let mut chunks = Vec::new();
        let mut buffer: Vec<Element> = Vec::new();
        // Only maintained for non-additive counters, which are the only ones
        // that need the joined string in order to measure it. Keeping it for an
        // additive counter would copy the whole buffer per element for nothing.
        let mut buffer_text = String::new();
        let mut buffer_tokens = 0usize;
        let mut buffer_heading: Option<String> = None;

        for element in elements {
            let elem_text = element.display_text();
            let elem_tokens = self.counter.count(&elem_text);
            let elem_heading = if self.config.propagate_headings {
                element.metadata().parent_heading.clone()
            } else {
                None
            };

            // Cost of the chunk this element would produce if it joined the
            // buffer, measured over the JOINED text — the text that would
            // actually be emitted. Summing per-element counts instead is only
            // valid for a counter that is additive across the join separator;
            // BPE is not, so a sum can approve a chunk whose real cost was never
            // measured (#435).
            //
            // A counter that declares itself additive over the join separator
            // makes the accumulated sum exactly equal to the joined count, so
            // there is no reason to re-tokenize the buffer: the two agree by the
            // counter's own contract, checked against it in
            // `prop_token_counter_invariants.rs`. Everything else pays a
            // re-count of the buffered text per candidate element, which is what
            // correctness costs when `count(a) + count(b) != count(a\nb)`.
            // Built once and reused if the merge goes ahead: it is the exact
            // text `buffer_text` would become, and re-deriving it on the merge
            // path would pay for a second string build and a second full
            // re-tokenization per merged element.
            let joined_text = (!buffer.is_empty() && !additive)
                .then(|| append_element_text(&buffer_text, &elem_text, false));

            let joined_tokens = match &joined_text {
                Some(joined) => self.counter.count(joined),
                None if buffer.is_empty() => elem_tokens,
                None => buffer_tokens + elem_tokens,
            };

            // Check if this element can merge with the buffer
            let can_merge = self.config.merge_adjacent
                && !buffer.is_empty()
                && can_merge_elements(buffer.last().unwrap(), element, &self.config.merge_policy)
                && joined_tokens <= self.config.max_tokens;

            if can_merge {
                if let Some(joined) = joined_text {
                    buffer_text = joined;
                }
                buffer.push(element.clone());
                buffer_tokens = joined_tokens;
                continue;
            }

            // Can't merge — check if buffer needs flushing
            if !buffer.is_empty() {
                // Flush if: adding would overflow, or types differ, or merge disabled
                if joined_tokens > self.config.max_tokens
                    || !can_merge_elements(
                        buffer.last().unwrap(),
                        element,
                        &self.config.merge_policy,
                    )
                    || !self.config.merge_adjacent
                {
                    self.flush_buffer(
                        &mut chunks,
                        &mut buffer,
                        &mut buffer_text,
                        &mut buffer_tokens,
                        &mut buffer_heading,
                    );
                }
            }

            // Handle oversized element
            if elem_tokens > self.config.max_tokens && buffer.is_empty() {
                if is_splittable_element(element) {
                    let text = element.display_text();
                    let fragments =
                        split_by_sentences(&text, self.counter.as_ref(), self.config.max_tokens);
                    for fragment in fragments {
                        let fragment = fragment.trim();
                        // A single sentence longer than the budget cannot be
                        // split any further without cutting mid-sentence, so it
                        // is emitted whole — but it is flagged, not passed off
                        // as within budget. Claiming `oversized: false` for a
                        // fragment that exceeds `max_tokens` is the same lie the
                        // budget invariant exists to catch (#435).
                        let over = self.counter.count(fragment) > self.config.max_tokens;
                        let fragment_element = make_text_fragment_element(element, fragment);
                        chunks.push(self.make_chunk(
                            vec![fragment_element],
                            elem_heading.clone(),
                            over,
                        ));
                    }
                } else {
                    // Table, image, code: atomic oversized chunk
                    chunks.push(self.make_chunk(vec![element.clone()], elem_heading, true));
                }
                continue;
            }

            // Start a new buffer. Reaching here always means the buffer is
            // empty: appending to a non-empty buffer only happens on the
            // `can_merge` path above (which `continue`s), and every other way
            // through with a non-empty buffer flushes it first — the flush
            // condition is the exact negation of the merge condition.
            debug_assert!(buffer.is_empty(), "buffer must be flushed before restart");
            buffer_heading = elem_heading;
            if !additive {
                buffer_text = elem_text;
            }
            buffer_tokens = elem_tokens;
            buffer.push(element.clone());
        }

        // Flush remaining
        if !buffer.is_empty() {
            chunks.push(self.make_chunk(std::mem::take(&mut buffer), buffer_heading, false));
        }

        chunks
    }

    /// Chunk a list of elements using the relationship graph to keep sections together.
    ///
    /// This method uses graph structure to group elements by section (all children of
    /// a title element), then attempts to pack each section into a single chunk.  If
    /// a section exceeds `max_tokens`, it delegates to [`chunk`](Self::chunk) for that
    /// section's elements, ensuring all resulting sub-chunks still carry the section's
    /// heading context.
    ///
    /// Elements that have no parent section (preamble elements before any title) are
    /// chunked with the standard `chunk()` strategy.
    pub fn chunk_with_graph(&self, elements: &[Element], graph: &ElementGraph) -> Vec<HybridChunk> {
        if elements.is_empty() {
            return Vec::new();
        }

        let mut chunks: Vec<HybridChunk> = Vec::new();

        // Collect preamble: indices with no parent AND not a title.
        let top_sections = graph.top_level_sections();

        // Determine the index of the first title so we know the preamble boundary.
        let first_title_idx = top_sections.first().copied().unwrap_or(elements.len());

        // ── Preamble (elements before the first title section) ────────────────
        if first_title_idx > 0 {
            let preamble: Vec<Element> = elements[..first_title_idx].to_vec();
            chunks.extend(self.chunk(&preamble));
        }

        // ── Process each top-level section ────────────────────────────────────
        for &title_idx in &top_sections {
            let title_heading = elements[title_idx]
                .metadata()
                .parent_heading
                .clone()
                .or_else(|| Some(elements[title_idx].text().to_string()));

            let child_indices = graph.elements_in_section(title_idx);

            // Gather section elements: title + all children.
            let mut section_elements: Vec<Element> = Vec::with_capacity(1 + child_indices.len());
            section_elements.push(elements[title_idx].clone());
            for &ci in &child_indices {
                section_elements.push(elements[ci].clone());
            }

            let section_tokens: usize = section_elements
                .iter()
                .map(|e| self.counter.count(&e.display_text()))
                .sum();

            if section_tokens <= self.config.max_tokens {
                // Entire section fits in one chunk.
                chunks.push(self.make_chunk(section_elements, title_heading, false));
            } else {
                // Section is too large — split with standard chunker, then fix heading.
                let mut sub_chunks = self.chunk(&section_elements);
                for sub in &mut sub_chunks {
                    sub.heading_context = title_heading.clone();
                }
                chunks.extend(sub_chunks);
            }
        }

        chunks
    }

    fn flush_buffer(
        &self,
        chunks: &mut Vec<HybridChunk>,
        buffer: &mut Vec<Element>,
        buffer_text: &mut String,
        buffer_tokens: &mut usize,
        buffer_heading: &mut Option<String>,
    ) {
        // Emit the accumulated buffer as a single chunk and reset all
        // accumulation state. The chunker's contract is that its emitted
        // chunks are element-disjoint — every source Element appears in
        // exactly one chunk. Re-injecting flushed elements back into the
        // buffer (as the old "overlap_tokens" branch did) would violate
        // that contract and, under type-boundary flushes, duplicates the
        // whole just-emitted chunk into the next one. See the regression
        // suite in tests/hybrid_chunker_disjoint_test.rs.
        let flushed = std::mem::take(buffer);
        let heading = buffer_heading.take();
        buffer_text.clear();
        *buffer_tokens = 0;

        chunks.push(self.make_chunk(flushed, heading, false));
    }
}

/// Append one element's text to the buffered chunk text, exactly as
/// [`HybridChunker::make_chunk`] will join the elements: `"\n"` between every
/// pair of ELEMENTS.
///
/// `buffer_is_empty` is about elements, not about text, and the distinction is
/// load-bearing. An element whose `display_text()` is empty is legal (the field
/// is a plain `pub String`), so "the buffer holds nothing" and "the buffered
/// text is the empty string" are different questions. Keying the separator on
/// the second makes the budget decision measure zero while the emitted chunk
/// keeps growing one separator at a time — those separators are real tokens
/// under BPE, which is #435 all over again.
///
/// Single definition on purpose: the budget decision and the emitted chunk have
/// to measure the same string, which they can only do by building it the same
/// way.
fn append_element_text(buffered: &str, next: &str, buffer_is_empty: bool) -> String {
    if buffer_is_empty {
        return next.to_string();
    }
    format!("{buffered}\n{next}")
}

/// Whether two adjacent elements can be merged according to the given policy.
fn can_merge_elements(a: &Element, b: &Element, policy: &MergePolicy) -> bool {
    match policy {
        MergePolicy::SameTypeOnly => matches!(
            (a, b),
            (Element::Paragraph(_), Element::Paragraph(_))
                | (Element::ListItem(_), Element::ListItem(_))
        ),
        MergePolicy::AnyInlineContent => is_inline_element(a) && is_inline_element(b),
    }
}

/// Returns true for text-based elements that can be merged with adjacent elements.
/// Structural elements (Title, Table, Image) and code blocks always start a new chunk.
fn is_inline_element(e: &Element) -> bool {
    matches!(
        e,
        Element::Paragraph(_) | Element::ListItem(_) | Element::KeyValue(_)
    )
}

/// Returns true for elements whose text content can be split at sentence boundaries.
fn is_splittable_element(e: &Element) -> bool {
    matches!(e, Element::Paragraph(_) | Element::ListItem(_))
}

/// Split text at sentence boundaries (`. `, `! `, `? `, `\n`) into fragments of at most
/// `max_tokens` tokens under `counter`. Greedily accumulates sentences; if a single
/// sentence still exceeds `max_tokens`, it is emitted as a single fragment (cannot split
/// further without a semantic break). Never returns an empty Vec.
fn split_by_sentences(text: &str, counter: &dyn TokenCounter, max_tokens: usize) -> Vec<String> {
    // Split into sentences preserving the delimiter as part of the sentence.
    let sentences = split_into_sentences(text);

    // Sentences are joined with `' '`, a single whitespace character, so a
    // counter that declares itself additive over a whitespace join promises the
    // sum equals the measured cost of the join — the same promise the element
    // loop in `chunk` rests on, and checked against every counter in
    // `prop_token_counter_invariants.rs`. Without it, every candidate costs a
    // re-tokenization of the whole accumulated fragment.
    let additive = counter.is_additive_over_whitespace_join();

    let mut fragments: Vec<String> = Vec::new();
    let mut current = String::new();
    // Only meaningful while `current` is non-empty, and only maintained for an
    // additive counter — the other path measures the candidate directly.
    let mut current_tokens = 0usize;

    for sentence in sentences {
        let sentence = sentence.trim();
        if sentence.is_empty() {
            continue;
        }

        if current.is_empty() {
            // Starting a new fragment
            current.push_str(sentence);
            current_tokens = if additive { counter.count(sentence) } else { 0 };
            continue;
        }

        // Measure the joined candidate rather than summing the parts plus one
        // for the separator: that arithmetic assumes an additive counter, which
        // BPE is not (#435). For a counter that does declare additivity, the sum
        // IS the measurement, by its own contract.
        let sentence_tokens = counter.count(sentence);
        let (fits, candidate) = if additive {
            (current_tokens + sentence_tokens <= max_tokens, None)
        } else {
            let candidate = format!("{current} {sentence}");
            (counter.count(&candidate) <= max_tokens, Some(candidate))
        };

        if fits {
            match candidate {
                Some(candidate) => current = candidate,
                None => {
                    current.push(' ');
                    current.push_str(sentence);
                    current_tokens += sentence_tokens;
                }
            }
        } else {
            // Current sentence doesn't fit: flush and start new fragment
            fragments.push(std::mem::take(&mut current));
            current = sentence.to_string();
            current_tokens = sentence_tokens;
        }
    }

    if !current.is_empty() {
        fragments.push(current);
    }

    if fragments.is_empty() {
        // Fallback: return the original text as a single fragment
        fragments.push(text.to_string());
    }

    fragments
}

/// Split text into sentence-like segments preserving punctuation.
/// Splits on `. `, `! `, `? `, and `\n`.
pub(crate) fn split_into_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let mut iter = text.chars().peekable();

    while let Some(ch) = iter.next() {
        current.push(ch);

        if matches!(ch, '.' | '!' | '?') {
            if iter.peek() == Some(&' ') {
                iter.next(); // skip the space after delimiter
                sentences.push(current.trim().to_string());
                current = String::new();
                continue;
            }
        } else if ch == '\n' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
            current = String::new();
        }
    }

    let remaining = current.trim().to_string();
    if !remaining.is_empty() {
        sentences.push(remaining);
    }

    sentences
}

/// Create a new Paragraph element from an existing element's metadata, replacing the text.
/// Preserves provenance (page, bbox, parent_heading, heading_path).
fn make_text_fragment_element(source: &Element, fragment_text: &str) -> Element {
    let metadata = source.metadata().clone();
    Element::Paragraph(ElementData {
        text: fragment_text.to_string(),
        metadata: ElementMetadata {
            page: metadata.page,
            bbox: metadata.bbox,
            parent_heading: metadata.parent_heading,
            heading_path: metadata.heading_path,
            ..Default::default()
        },
    })
}

#[cfg(feature = "unstable-spi")]
impl crate::pipeline::spi::ChunkingStrategy for HybridChunker {
    fn chunk(&self, elements: &[Element]) -> Vec<crate::pipeline::spi::ChunkGroup> {
        // Call the inherent method explicitly to avoid recursing into this impl.
        HybridChunker::chunk(self, elements)
            .into_iter()
            .map(HybridChunk::into_group)
            .collect()
    }
}

#[cfg(all(test, feature = "unstable-spi"))]
mod tests {
    use super::*;

    #[test]
    fn from_group_recomputes_oversized_and_preserves_content() {
        use crate::pipeline::spi::ChunkGroup;

        let big = Element::Paragraph(ElementData {
            text: "one two three four five six seven eight".to_string(),
            metadata: ElementMetadata::default(),
        });
        // Budget far below the token count → oversized.
        let group = ChunkGroup::new(vec![big.clone()], Some("H".to_string()));
        let hc = HybridChunk::from_group(group, 2);
        assert!(
            hc.is_oversized(),
            "8-word chunk over a 2-token budget is oversized"
        );
        assert_eq!(hc.heading_context.as_deref(), Some("H"));
        assert_eq!(hc.elements().len(), 1);

        // Generous budget → not oversized.
        let group2 = ChunkGroup::new(vec![big], None);
        let hc2 = HybridChunk::from_group(group2, 100);
        assert!(!hc2.is_oversized());
    }
}
