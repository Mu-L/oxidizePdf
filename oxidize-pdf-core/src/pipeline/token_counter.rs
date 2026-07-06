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
