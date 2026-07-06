//! Pluggable token counting for chunk-size decisions and reported estimates.

/// Counts tokens in a string. Implementations back chunk-size decisions and the
/// reported `token_estimate` with a real tokenizer.
pub trait TokenCounter: Send + Sync {
    /// Number of tokens in `text` under this counter.
    fn count(&self, text: &str) -> usize;
    /// Stable provenance identifier, e.g. `"word-proxy"` or `"cl100k_base"`.
    fn name(&self) -> &'static str;
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
