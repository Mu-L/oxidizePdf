//! Regression test for issue #427.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use std::io::Cursor;

#[test]
fn malformed_catalog_scan_does_not_panic_on_invalid_utf8() {
    let bytes: &[u8] = &[
        37, 80, 68, 70, 45, 49, 53, 255, 255, 32, 48, 32, 111, 98, 106, 10, 60, 60, 47, 84, 121,
        112, 101, 47, 67, 97, 116, 97, 108, 111, 103, 37, 110, 116, 10, 49, 32, 48, 32, 111, 98,
        106, 116,
    ];

    let result = std::panic::catch_unwind(|| {
        PdfReader::new_with_options(Cursor::new(bytes), ParseOptions::lenient())
    });

    assert!(
        result.is_ok(),
        "malformed input must return a parse result instead of panicking"
    );
    assert!(
        result.unwrap().is_err(),
        "the malformed PDF should be rejected"
    );
}
