//! Text-extraction fuzz target.
//!
//! Parses arbitrary bytes leniently, then runs the text-extraction pipeline
//! with column reordering both off and on. This is a crash-only guard for the
//! extraction geometry (the #389/#403/#408/#417/#422/#425 family) — it flags
//! panics / OOM / overflow in the reading-order and column-detection code.
//!
//! Note: token-preservation ("reorder must not shred a token") is a *logical*
//! invariant, checked by the stable proptest harness, not here — libFuzzer only
//! catches crashes, not wrong-but-non-crashing output.
#![no_main]

use libfuzzer_sys::fuzz_target;
use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::text::{ExtractionOptions, TextExtractor};
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let reader = match PdfReader::new_with_options(Cursor::new(data), ParseOptions::lenient()) {
        Ok(r) => r,
        Err(_) => return,
    };
    let document = reader.into_document();

    let page_count = match document.page_count() {
        Ok(n) => n,
        Err(_) => return,
    };

    for reorder in [false, true] {
        let mut extractor = TextExtractor::with_options(ExtractionOptions {
            reorder_columns: reorder,
            detect_columns: reorder,
            ..Default::default()
        });
        for i in 0..page_count.min(32) {
            let _ = extractor.extract_from_page(&document, i);
        }
    }
});
