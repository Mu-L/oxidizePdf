//! Parser robustness fuzz target.
//!
//! Feeds arbitrary bytes through the full parse + document-navigation path and
//! lets libFuzzer flag any panic, abort, OOM, or arithmetic overflow. This is
//! the guard for the "malformed input crashes / silently drops content" class:
//! issues #401 (negative /Length capacity-overflow panic), #82 (stack overflow
//! on circular refs), #260 (/Length mismatch), #415, #426 (recovery resolves a
//! stale /Pages root). A clean `Err` is a valid outcome — only a crash is a bug.
#![no_main]

use libfuzzer_sys::fuzz_target;
use oxidize_pdf::parser::{ParseOptions, PdfReader};
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    // First byte selects the parse strictness so one corpus exercises every
    // recovery path. The tolerant/lenient/skip_errors modes drive the
    // whole-file recovery scan where #401/#415/#426 live, so all must be fuzzed.
    let (selector, pdf) = data.split_first().unwrap();
    let options = match selector % 4 {
        0 => ParseOptions::strict(),
        1 => ParseOptions::tolerant(),
        2 => ParseOptions::lenient(),
        _ => ParseOptions::skip_errors(),
    };

    let reader = match PdfReader::new_with_options(Cursor::new(pdf), options) {
        Ok(r) => r,
        // A clean parse error is a valid, non-crashing outcome.
        Err(_) => return,
    };
    let document = reader.into_document();

    let page_count = match document.page_count() {
        Ok(n) => n,
        Err(_) => return,
    };

    // A malformed page tree can report an absurd count; cap the walk so the
    // fuzzer spends its budget finding new crashes rather than iterating a
    // claimed 4-billion pages a real caller would never materialize either.
    for i in 0..page_count.min(64) {
        let page = match document.get_page(i) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let _ = document.get_page_content_streams(&page);
    }
});
