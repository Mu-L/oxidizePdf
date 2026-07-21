//! Token-counter invariants, stated from the trait's contract.
//!
//! `TokenCounter::is_additive_over_whitespace_join` is a promise the chunker
//! acts on: it is what lets the budget decision accumulate a sum instead of
//! re-counting the joined text. An over-claim is not a slow path, it is a
//! silently wrong budget — exactly the shape of #435, where a sum approved a
//! chunk whose real cost was never measured.
//!
//! So the promise is verified, not trusted:
//!
//!   1. ADDITIVITY IS TRUE WHEN CLAIMED. For a counter that answers `true`,
//!      `count(a) + count(b) == count("a{sep}b")` for generated `a`, `b` and
//!      EVERY separator the promise covers, not just the newline the chunker
//!      happens to use between elements. The sentence-split path joins with a
//!      space, and a property that only ever generated `"\n"` would leave that
//!      caller's fast path resting on an unverified claim.
//!   2. COUNTING IS SANE. Counts are deterministic, and empty text costs
//!      nothing — a counter that charges for `""` would make the chunker's
//!      empty-element accounting drift.
//!
//! The BPE counter is expected to answer `false`; a test pins that, since a
//! future "optimization" flipping it would reintroduce #435 wholesale.

use oxidize_pdf::pipeline::{TokenCounter, WordProxyCounter};
use proptest::prelude::*;
use std::sync::OnceLock;

/// Every separator the additivity promise covers: a single whitespace
/// character. `"\n"` is what the chunker joins elements with, `" "` what
/// `split_by_sentences` joins sentences with; the rest are in the contract, so
/// they are generated too.
fn separator() -> impl Strategy<Value = char> {
    prop_oneof![
        Just('\n'),
        Just(' '),
        Just('\t'),
        Just('\r'),
        // U+000B LINE TABULATION and U+00A0 NO-BREAK SPACE: `char::is_whitespace`
        // is wider than ASCII, and so is the promise.
        Just('\u{000B}'),
        Just('\u{00A0}'),
    ]
}

/// Text fragments that stress the join boundary: words, punctuation, leading and
/// trailing whitespace, empty strings, and multi-byte characters.
fn fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just(" ".to_string()),
        Just("\n".to_string()),
        "[a-z]{1,8}",
        "[a-z]{1,5} [a-z]{1,5}",
        "[a-z]{1,5}\\.",
        " [a-z]{1,5} ",
        "[áéñü]{1,4}",
        "[0-9]{1,6}",
        "[a-z]{1,4}-[a-z]{1,4}",
    ]
}

/// Every counter this crate ships, as trait objects, so the properties run over
/// all of them without naming each one twice.
///
/// Built once for the whole suite: cl100k_base loads multi-MB rank tables, and
/// rebuilding it per generated case costs orders of magnitude more than the
/// property itself.
fn counters() -> &'static [Box<dyn TokenCounter>] {
    static COUNTERS: OnceLock<Vec<Box<dyn TokenCounter>>> = OnceLock::new();
    COUNTERS.get_or_init(|| {
        #[allow(unused_mut)]
        let mut v: Vec<Box<dyn TokenCounter>> = vec![Box::new(WordProxyCounter)];
        #[cfg(feature = "tiktoken")]
        v.push(Box::new(
            oxidize_pdf::pipeline::TiktokenCounter::cl100k_base(),
        ));
        v
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A counter that claims additivity must actually be additive, over every
    /// separator the promise covers: the chunker skips measuring the joined text
    /// on that promise alone, and it joins with more than one character.
    #[test]
    fn claimed_additivity_holds(a in fragment(), b in fragment(), sep in separator()) {
        for c in counters() {
            if !c.is_additive_over_whitespace_join() {
                continue;
            }
            let joined = c.count(&format!("{a}{sep}{b}"));
            let summed = c.count(&a) + c.count(&b);
            prop_assert_eq!(
                joined,
                summed,
                "{} claims additivity but count({:?}) = {} != {} + {}",
                c.name(),
                format!("{a}{sep}{b}"),
                joined,
                c.count(&a),
                c.count(&b)
            );
        }
    }

    /// Counting is a pure function of the text.
    #[test]
    fn counting_is_deterministic(a in fragment()) {
        for c in counters() {
            prop_assert_eq!(c.count(&a), c.count(&a), "{} is not deterministic", c.name());
        }
    }

}

/// Empty text costs nothing. The chunker accounts for elements with no display
/// text; a non-zero cost for `""` would drift its budget.
///
/// A plain test, not a property: there is exactly one input to check, and
/// generating 256 cases to assert the same equality would only hide that.
#[test]
fn empty_text_costs_nothing() {
    for c in counters() {
        assert_eq!(c.count(""), 0, "{} charges for empty text", c.name());
    }
}

/// The word proxy is the default counter and the one whose additivity the fast
/// path depends on. Pinned explicitly so the answer cannot be flipped silently.
#[test]
fn word_proxy_declares_additivity() {
    assert!(
        WordProxyCounter.is_additive_over_whitespace_join(),
        "the default counter must stay additive: the chunker's fast path is \
         conditioned on this answer"
    );
}

/// BPE is not additive across a join — the join boundary re-tokenizes. This is
/// the whole reason #435 existed, so the answer is pinned: flipping it to `true`
/// as an optimization would restore the bug.
#[cfg(feature = "tiktoken")]
#[test]
fn bpe_does_not_claim_additivity() {
    let c = oxidize_pdf::pipeline::TiktokenCounter::cl100k_base();
    assert!(
        !c.is_additive_over_whitespace_join(),
        "cl100k_base must not claim additivity: count(a) + count(b) != \
         count(a\\nb) at the join boundary (#435)"
    );
}

/// A concrete witness that BPE really is non-additive, so the pin above guards
/// something real rather than a hypothetical.
#[cfg(feature = "tiktoken")]
#[test]
fn bpe_non_additivity_has_a_witness() {
    let c = oxidize_pdf::pipeline::TiktokenCounter::cl100k_base();
    let found = (0..200).find_map(|i| {
        let a = format!("token{i}");
        let b = format!("{i}suffix");
        (c.count(&a) + c.count(&b) != c.count(&format!("{a}\n{b}"))).then_some((a, b))
    });
    assert!(
        found.is_some(),
        "expected at least one pair where BPE is non-additive across the join"
    );
}
