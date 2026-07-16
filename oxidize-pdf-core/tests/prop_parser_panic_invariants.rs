//! Parser robustness invariant, stated from the API contract (fail-safe), not
//! from a bug: parsing must never crash the process on any input.
//!
//! INV — NEVER PANIC: `PdfReader::new_with_options(bytes, *)` over arbitrary or
//! mutated bytes returns `Ok` or `Err`, never panics / aborts / overflows; and
//! navigating whatever document it yields (page_count, get_page, content
//! streams) is equally panic-free. Guards the "malformed input crashes" class
//! (#401 capacity overflow, #82 stack overflow, #427 UTF-8 boundary).
//!
//! The fuzzer (`fuzz/`) already hunts this class, but only under nightly on a
//! cron cadence. This property runs the same contract on **every PR** on stable,
//! over random bytes and byte-mutated valid PDFs, so a panic cannot slip in
//! between fuzz campaigns. A panic here is reported by proptest with the shrunk
//! input; promote it to `fuzz-regressions/` and fix.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use proptest::prelude::*;
use std::io::Cursor;

/// A small but structurally complete PDF used as the mutation seed: mutating a
/// valid document reaches the parser's deep recovery/branch states far faster
/// than random bytes alone.
const SEED_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>\nendobj\n\
4 0 obj\n<< /Length 44 >>\nstream\nBT /F1 12 Tf 72 720 Td (hello world) Tj ET\nendstream\nendobj\n\
xref\n0 5\n0000000000 65535 f \n\
trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n0\n%%EOF\n";

/// Drive parse + document navigation across every strictness mode. Any panic
/// propagates and fails the property (that is the signal we want).
fn drive(bytes: &[u8]) {
    for options in [
        ParseOptions::strict(),
        ParseOptions::tolerant(),
        ParseOptions::lenient(),
        ParseOptions::skip_errors(),
    ] {
        let Ok(reader) = PdfReader::new_with_options(Cursor::new(bytes.to_vec()), options) else {
            continue;
        };
        let document = reader.into_document();
        let Ok(page_count) = document.page_count() else {
            continue;
        };
        // Cap the walk: a malformed tree can claim an absurd count.
        for i in 0..page_count.min(32) {
            if let Ok(page) = document.get_page(i) {
                let _ = document.get_page_content_streams(&page);
            }
        }
    }
}

/// Apply `n` random single-byte mutations (overwrite at a random index) to a
/// copy of `base`. Length-preserving keeps the seed structurally near-valid so
/// mutations probe branch boundaries rather than degenerating to noise.
fn mutate(base: &[u8], sites: &[(usize, u8)]) -> Vec<u8> {
    let mut buf = base.to_vec();
    if buf.is_empty() {
        return buf;
    }
    for &(idx, val) in sites {
        let i = idx % buf.len();
        buf[i] = val;
    }
    buf
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Arbitrary bytes never crash the parser.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        drive(&bytes);
    }

    /// Byte-mutated valid PDFs never crash the parser. Reaches deep recovery
    /// paths (the ones #401/#427 lived in) that random bytes rarely hit.
    #[test]
    fn mutated_pdf_never_panics(
        sites in prop::collection::vec((any::<usize>(), any::<u8>()), 1..64),
    ) {
        let bytes = mutate(SEED_PDF, &sites);
        drive(&bytes);
    }

    /// Truncations of a valid PDF (every prefix length) never crash — a common
    /// real-world corruption (interrupted download / write).
    #[test]
    fn truncated_pdf_never_panics(cut in 0usize..=SEED_PDF.len()) {
        drive(&SEED_PDF[..cut]);
    }
}
