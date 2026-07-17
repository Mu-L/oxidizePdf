//! Chunking-layer invariants, stated from the RAG contract (`the chunk set is a
//! faithful partition of the input text, and every chunk is fit to be
//! vectorized`), not from a reported bug. `HybridChunker::chunk` is a pure
//! function of (elements, config), so the generator is cheap and the input space
//! is large: element types × lengths × headings × five config dimensions × two
//! token counters.
//!
//! Layer rule (see spec §4): an invariant lives at the cheapest layer where it
//! is NOT tautological. `heading_path` is supplied by the caller at this layer,
//! so asserting it here would test the test; it is guarded end-to-end by the
//! breadcrumb property in `prop_rag_e2e_invariants.rs`. `chunk_index` is not
//! separately guarded: production assigns it by `enumerate()`, so a
//! contiguity assertion at any layer is structurally trivial (there is no input
//! that makes it non-sequential) — not worth a property.
//!
//! Known gap, accepted: `HybridChunker::chunk_with_graph` is a second public
//! grouping path with its own section logic and is NOT covered here (it needs an
//! `ElementGraph`). Example guards live in `hybrid_chunking_graph_test.rs`.

#[cfg(feature = "tiktoken")]
use oxidize_pdf::pipeline::TiktokenCounter;
use oxidize_pdf::pipeline::{
    ContextFormat, ContextMode, Element, ElementData, ElementMetadata, HybridChunk,
    HybridChunkConfig, HybridChunker, KeyValueElementData, MergePolicy, RagChunk, TableElementData,
    TokenCounter, WordProxyCounter,
};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "tiktoken")]
use std::sync::{Arc, OnceLock};

/// A generated element, before it is given its position-derived marker.
#[derive(Debug, Clone)]
struct ElemSpec {
    /// 0=Paragraph 1=ListItem 2=KeyValue (inline, mergeable);
    /// 3=Title 4=Table 5=CodeBlock (structural, always start a new chunk).
    kind: u8,
    words: usize,
    page: u32,
    has_heading: bool,
}

fn elem_spec() -> impl Strategy<Value = ElemSpec> {
    (0u8..6u8, 1usize..=60usize, 0u32..=3u32, any::<bool>()).prop_map(
        |(kind, words, page, has_heading)| ElemSpec {
            kind,
            words,
            page,
            has_heading,
        },
    )
}

/// Build the element for `s` at input position `idx`.
///
/// The text opens with the unique marker `E{idx}_`. Markers are prefix-free
/// thanks to the trailing `_` and a body that never contains `_`, so
/// `text.contains("E1_")` cannot be satisfied by `E11_`.
///
/// Every fifth word ends in `.` so the text carries real sentence boundaries —
/// without them `split_by_sentences` returns the whole text as one fragment and
/// the oversized-split path (`hybrid_chunking.rs:337-356`) is never exercised.
fn make_element(s: &ElemSpec, idx: usize) -> Element {
    let body = (0..s.words)
        .map(|w| {
            if w % 5 == 4 {
                format!("w{w}.")
            } else {
                format!("w{w}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let text = format!("E{idx}_ {body}");
    let metadata = ElementMetadata {
        page: s.page,
        parent_heading: s.has_heading.then(|| format!("H{idx}")),
        ..Default::default()
    };
    match s.kind {
        0 => Element::Paragraph(ElementData { text, metadata }),
        1 => Element::ListItem(ElementData { text, metadata }),
        2 => Element::KeyValue(KeyValueElementData {
            key: text,
            value: String::from("v"),
            metadata,
        }),
        3 => Element::Title(ElementData { text, metadata }),
        4 => Element::Table(TableElementData::new(vec![vec![text]], metadata)),
        _ => Element::CodeBlock(ElementData { text, metadata }),
    }
}

fn element_seq() -> impl Strategy<Value = Vec<Element>> {
    prop::collection::vec(elem_spec(), 1..=12).prop_map(|specs| {
        specs
            .iter()
            .enumerate()
            .map(|(i, s)| make_element(s, i))
            .collect()
    })
}

/// `max_tokens` in 4..=64 against elements of 1..=60 words: the budget is
/// routinely smaller than a single element, so the flush, the type-boundary and
/// the oversized-split paths all fire.
fn chunk_config() -> impl Strategy<Value = HybridChunkConfig> {
    (
        4usize..=64usize,
        any::<bool>(),
        any::<bool>(),
        prop_oneof![
            Just(MergePolicy::SameTypeOnly),
            Just(MergePolicy::AnyInlineContent)
        ],
        prop_oneof![
            Just(ContextMode::None),
            Just(ContextMode::Heading),
            Just(ContextMode::Contextual(ContextFormat::Labeled)),
            Just(ContextMode::Contextual(ContextFormat::Prose)),
        ],
    )
        .prop_map(
            |(max_tokens, merge_adjacent, propagate_headings, merge_policy, context_mode)| {
                HybridChunkConfig {
                    max_tokens,
                    overlap_tokens: 0,
                    merge_adjacent,
                    propagate_headings,
                    merge_policy,
                    context_mode,
                }
            },
        )
}

fn chunk(elements: &[Element], config: HybridChunkConfig) -> Vec<HybridChunk> {
    HybridChunker::new(config).chunk(elements)
}

/// Word -> occurrence count. Whitespace-split, so it is robust to the chunker's
/// legitimate reflow of separators but still catches a dropped or duplicated word.
fn word_multiset(s: &str) -> BTreeMap<&str, usize> {
    let mut m = BTreeMap::new();
    for w in s.split_whitespace() {
        *m.entry(w).or_insert(0) += 1;
    }
    m
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// I2a — IDENTITY: each input element's marker lands in exactly one chunk.
    /// When an oversized element is split, its marker rides the first fragment,
    /// so the count stays 1 either way.
    #[test]
    fn every_element_marker_lands_in_exactly_one_chunk(
        elements in element_seq(),
        config in chunk_config(),
    ) {
        let chunks = chunk(&elements, config);
        for idx in 0..elements.len() {
            let marker = format!("E{idx}_");
            let hits = chunks.iter().filter(|c| c.text().contains(&marker)).count();
            prop_assert_eq!(hits, 1, "marker {} landed in {} chunks", marker, hits);
        }
    }

    /// I2b — CONSERVATION: the chunk set carries every input word exactly as
    /// often as the input did. Identity alone (I2a) is satisfiable by dropping
    /// everything but the first word of each element; conservation alone does
    /// not localize. Both are required.
    #[test]
    fn chunk_text_conserves_every_input_word(
        elements in element_seq(),
        config in chunk_config(),
    ) {
        let input = elements
            .iter()
            .map(|e| e.display_text())
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = chunk(&elements, config);
        let output = chunks.iter().map(|c| c.text()).collect::<Vec<_>>().join("\n");
        prop_assert_eq!(word_multiset(&output), word_multiset(&input));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// I3 — HONEST BUDGET: the number stamped as a chunk's cost counts exactly
    /// the text the library designates as embeddable.
    ///
    /// `RagChunk::full_text` is that designated text (`rag.rs:23`:
    /// "use this for embedding generation") and `token_estimate` is documented
    /// as its token count. If they disagree, a consumer sizing against a real
    /// embedding model's hard limit is silently over budget: the provider
    /// truncates the tail, the tail never reaches the vector, and that content
    /// becomes unretrievable while still sitting in the store.
    ///
    /// PINNED: fails today — see issue #434. The property states the contract;
    /// the code does not honor it yet. Remove `#[ignore]` when the fix ships and
    /// this becomes a permanent guard. Precedent: #430.
    #[test]
    #[ignore = "issue #434: token_estimate does not measure full_text"]
    fn stamped_count_measures_the_text_designated_for_embedding(
        elements in element_seq(),
        config in chunk_config(),
    ) {
        let mode = config.context_mode;
        let chunks = chunk(&elements, config);
        for (i, c) in chunks.iter().enumerate() {
            let rag = RagChunk::from_hybrid_chunk_with_mode(i, c, mode);
            let measured = WordProxyCounter.count(&rag.full_text);
            prop_assert_eq!(
                rag.token_estimate,
                measured,
                "chunk {}: stamped {} tokens, full_text measures {}",
                i,
                rag.token_estimate,
                measured
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// I4a — ORDER: chunks come out in input-element order. Consumers rebuild
    /// context from neighbouring chunks, so a reordering silently changes what
    /// an LLM is handed as "the surrounding text".
    ///
    /// Fragments of a split element carry no marker (only the first fragment
    /// does), so they are skipped here; they are contiguous with their marked
    /// head by construction.
    #[test]
    fn chunks_come_out_in_input_element_order(
        elements in element_seq(),
        config in chunk_config(),
    ) {
        let n = elements.len();
        let chunks = chunk(&elements, config);
        let firsts: Vec<usize> = chunks
            .iter()
            .filter_map(|c| {
                let t = c.text();
                (0..n).find(|i| t.contains(&format!("E{i}_")))
            })
            .collect();
        let mut sorted = firsts.clone();
        sorted.sort_unstable();
        prop_assert_eq!(&firsts, &sorted, "chunk order does not follow input order");
    }

    /// I4b — PAGE TRACEABILITY: a chunk's `page_numbers` is exactly the union of
    /// its elements' pages. It is a filter field in the vector store: if it
    /// lies, a page-scoped query returns incomplete results and never errors.
    ///
    /// Scope: because both sides are compared as `BTreeSet`s, this guards
    /// membership — no page dropped, none foreign — but not the dedup/ordering
    /// of the production `Vec` (the set re-sorts and re-dedups both sides). Nor
    /// does it guard whether each element's page was assigned correctly: the
    /// chunker does not assign pages, partition does, so a mis-assigned page is
    /// I1's territory (E2E), not observable here where both sides read
    /// `metadata().page` from the same elements.
    #[test]
    fn chunk_pages_are_the_union_of_its_elements_pages(
        elements in element_seq(),
        config in chunk_config(),
    ) {
        let mode = config.context_mode;
        let chunks = chunk(&elements, config);
        for (i, c) in chunks.iter().enumerate() {
            let expected: BTreeSet<u32> =
                c.elements().iter().map(|e| e.metadata().page).collect();
            let rag = RagChunk::from_hybrid_chunk_with_mode(i, c, mode);
            let actual: BTreeSet<u32> = rag.page_numbers.iter().copied().collect();
            prop_assert_eq!(actual, expected, "chunk {} page set", i);
        }
    }
}

/// cl100k_base ships multi-MB rank tables; load once for the whole suite rather
/// than per generated case.
#[cfg(feature = "tiktoken")]
fn tiktoken() -> Arc<TiktokenCounter> {
    static C: OnceLock<Arc<TiktokenCounter>> = OnceLock::new();
    C.get_or_init(|| Arc::new(TiktokenCounter::cl100k_base()))
        .clone()
}

// Fewer cases than the other blocks: every case runs real BPE over the whole
// chunk set, which is orders of magnitude slower than the word proxy.
#[cfg(feature = "tiktoken")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// I3-BPE — BUDGET UNDER THE INJECTED COUNTER: no chunk the chunker did not
    /// itself flag oversized may exceed `max_tokens` when measured with the very
    /// counter that governed the split.
    ///
    /// The split decision sums per-element counts while the stamp counts the
    /// joined text (`hybrid_chunking.rs:296,308` vs `:271-277`). That identity
    /// holds for whitespace counting and not for BPE.
    ///
    /// PINNED: fails today — see issue #435. The property states the contract;
    /// the code does not honor it yet. Remove `#[ignore]` when the fix ships and
    /// this becomes a permanent guard. Precedent: #430.
    #[test]
    #[ignore = "issue #435: split decision sums per-element counts; BPE is not additive across the join"]
    fn no_chunk_exceeds_its_budget_under_bpe(
        elements in element_seq(),
        config in chunk_config(),
    ) {
        let counter = tiktoken();
        let max_tokens = config.max_tokens;
        let chunks = HybridChunker::new(config)
            .with_token_counter(counter.clone())
            .chunk(&elements);
        for (i, c) in chunks.iter().enumerate() {
            if c.is_oversized() {
                continue;
            }
            let measured = counter.count(&c.text());
            prop_assert!(
                measured <= max_tokens,
                "chunk {}: {} BPE tokens over a {}-token budget",
                i,
                measured,
                max_tokens
            );
        }
    }
}
