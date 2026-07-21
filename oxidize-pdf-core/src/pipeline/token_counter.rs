//! Pluggable token counting for chunk-size decisions and reported estimates.

/// Counts tokens in a string. Implementations back chunk-size decisions and the
/// reported `token_estimate` with a real tokenizer.
pub trait TokenCounter: Send + Sync {
    /// Number of tokens in `text` under this counter.
    fn count(&self, text: &str) -> usize;
    /// Stable provenance identifier, e.g. `"word-proxy"` or `"cl100k_base"`.
    fn name(&self) -> &'static str;

    /// Whether this counter is additive across a whitespace join, i.e. whether
    /// `count(a) + count(b) == count(&format!("{a}{sep}{b}"))` for every `a`,
    /// `b` and every single whitespace character `sep`.
    ///
    /// Chunking has to know the cost of the text it is about to emit, and it
    /// builds that text by joining pieces with one whitespace character: `"\n"`
    /// between elements, `" "` between sentences inside an oversized element. A
    /// counter that is additive lets that cost be computed by accumulation; one
    /// that is not forces a re-count of the joined text on every candidate,
    /// which is correct but costs a re-tokenization of everything buffered so
    /// far, each time.
    ///
    /// The promise deliberately covers ANY whitespace separator rather than one
    /// named character: a counter answering for `"\n"` alone would leave the
    /// sentence-join path with no contract to stand on, and using the newline
    /// answer there anyway would be precisely the unmeasured assumption that
    /// #435 was.
    ///
    /// Defaults to `false`, the safe answer: an over-claim here silently
    /// restores that budget bug, where a sum approved a chunk whose real cost
    /// was never measured. Whitespace counting is additive because the
    /// separator cannot fuse two words into one; subword (BPE) counting is not,
    /// because the join boundary re-tokenizes.
    ///
    /// Override only with a proof or a test. `prop_token_counter_invariants.rs`
    /// checks every counter in this crate against its own answer, over every
    /// separator the promise covers.
    fn is_additive_over_whitespace_join(&self) -> bool {
        false
    }
}

/// Zero-dependency default: whitespace-separated word count. Reproduces the
/// historical `estimate_tokens` behaviour exactly.
#[derive(Debug, Default, Clone)]
pub struct WordProxyCounter;

impl TokenCounter for WordProxyCounter {
    fn count(&self, text: &str) -> usize {
        text.split_whitespace().count()
    }
    fn name(&self) -> &'static str {
        "word-proxy"
    }
    /// Additive: the count is `split_whitespace().count()`, so a separator that
    /// is itself whitespace cannot merge the last word of `a` with the first of
    /// `b`, nor create a word that was in neither — whichever whitespace
    /// character it is.
    fn is_additive_over_whitespace_join(&self) -> bool {
        true
    }
}

/// Real token counter backed by tiktoken's cl100k_base BPE (GPT-3.5/4 family).
#[cfg(feature = "tiktoken")]
pub struct TiktokenCounter {
    bpe: tiktoken_rs::CoreBPE,
}

#[cfg(feature = "tiktoken")]
impl TiktokenCounter {
    /// Load the cl100k_base BPE rank tables (embedded in the crate; infallible
    /// in practice — the tables ship with `tiktoken-rs`).
    pub fn cl100k_base() -> Self {
        Self {
            bpe: tiktoken_rs::cl100k_base().expect("cl100k_base tokenizer tables"),
        }
    }
}

#[cfg(feature = "tiktoken")]
impl TokenCounter for TiktokenCounter {
    fn count(&self, text: &str) -> usize {
        self.bpe.encode_ordinary(text).len()
    }
    fn name(&self) -> &'static str {
        "cl100k_base"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_proxy_matches_split_whitespace() {
        let c = WordProxyCounter;
        assert_eq!(c.count(""), 0);
        assert_eq!(c.count("hello world"), 2);
        assert_eq!(c.count("  multiple   spaces  "), 2);
        assert_eq!(c.count("punct, and! more?"), 3);
        assert_eq!(c.name(), "word-proxy");

        // Property: the default counter must stay identical to the historical
        // `split_whitespace().count()` word-proxy — this is the byte-identity
        // invariant the whole feature rests on (#377).
        for s in ["", "a b c", "  x  y ", "line1\nline2\tthree", "único café"] {
            assert_eq!(c.count(s), s.split_whitespace().count());
        }
    }
}

#[cfg(all(test, feature = "tiktoken"))]
mod tiktoken_tests {
    use super::*;

    #[test]
    fn cl100k_exact_reference_counts() {
        let c = TiktokenCounter::cl100k_base();
        // Pinned cl100k_base values (OpenAI reference tokenizer).
        assert_eq!(c.count(""), 0);
        assert_eq!(c.count("hello world"), 2);
        assert_eq!(c.count("The quick brown fox"), 4);
        assert_eq!(c.name(), "cl100k_base");
    }

    #[test]
    fn cl100k_diverges_from_word_proxy_on_subword() {
        let tk = TiktokenCounter::cl100k_base();
        let wp = WordProxyCounter;
        // A URL is one whitespace "word" but many BPE tokens.
        let url = "https://example.com/verify";
        assert_eq!(wp.count(url), 1);
        assert!(
            tk.count(url) > 1,
            "tiktoken should split the URL: {}",
            tk.count(url)
        );
    }
}
