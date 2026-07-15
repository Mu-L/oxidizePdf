//! Stable regression bridge for the fuzzing harness (see `../../fuzz/`).
//!
//! cargo-fuzz / libFuzzer require nightly, so a crash they find would only
//! reproduce under a nightly cron job. This test closes that gap: every
//! minimized crash artifact promoted into `tests/fixtures/fuzz-regressions/`
//! is replayed here through the exact same parse + navigate + extract path the
//! fuzz targets drive — on stable, in the normal `cargo test` run. Once a crash
//! is promoted, it can never silently regress, even without nightly.
//!
//! Workflow when the fuzzer finds a crash:
//!   1. `cargo +nightly fuzz tmin parse_document <artifact>` to minimize it.
//!   2. Copy the minimized input to `tests/fixtures/fuzz-regressions/` with a
//!      descriptive name, e.g. `issue_401_negative_length.pdf`.
//!   3. This test then guards it forever. Fix the parser until it passes.
//!
//! The fixture bytes are deliberately NOT required to be valid PDFs — they are
//! whatever bytes triggered the crash. The pass condition is simply "driving
//! them does not panic / abort"; a clean `Err` is success.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::text::{ExtractionOptions, TextExtractor};
use std::io::Cursor;
use std::path::PathBuf;

fn regressions_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fuzz-regressions")
}

/// Mirrors `fuzz/fuzz_targets/parse_document.rs`: exercise every strictness
/// mode and walk the page tree. Returns without panicking on any outcome.
fn drive_parse(data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let (selector, pdf) = data.split_first().unwrap();
    let options = match selector % 4 {
        0 => ParseOptions::strict(),
        1 => ParseOptions::tolerant(),
        2 => ParseOptions::lenient(),
        _ => ParseOptions::skip_errors(),
    };
    let Ok(reader) = PdfReader::new_with_options(Cursor::new(pdf), options) else {
        return;
    };
    let document = reader.into_document();
    let Ok(page_count) = document.page_count() else {
        return;
    };
    for i in 0..page_count.min(64) {
        if let Ok(page) = document.get_page(i) {
            let _ = document.get_page_content_streams(&page);
        }
    }
}

/// Mirrors `fuzz/fuzz_targets/extract_text.rs`: lenient parse, then extract with
/// column reordering off and on.
fn drive_extract(data: &[u8]) {
    let Ok(reader) = PdfReader::new_with_options(Cursor::new(data), ParseOptions::lenient()) else {
        return;
    };
    let document = reader.into_document();
    let Ok(page_count) = document.page_count() else {
        return;
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
}

#[test]
fn fuzz_regression_corpus_does_not_crash() {
    let dir = regressions_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => {
            // Dir absent means no crashes promoted yet — nothing to guard.
            return;
        }
    };

    let mut checked = 0usize;
    for entry in entries {
        let path = entry.expect("read dir entry").path();
        if !path.is_file() {
            continue;
        }
        // Skip the README and any hidden housekeeping files.
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.') || n.eq_ignore_ascii_case("README.md"))
        {
            continue;
        }
        let data = std::fs::read(&path).expect("read fixture");
        // A panic here fails the test and names the offending fixture via the
        // standard panic backtrace; that is exactly the regression signal.
        drive_parse(&data);
        drive_extract(&data);
        checked += 1;
    }

    println!("fuzz regression corpus: {checked} input(s) replayed without crashing");
}
