//! Regression tests for issue #379 — unlock of R4 (V4) PDFs with
//! `/EncryptMetadata false` (cleartext metadata).
//!
//! ISO 32000-1 Algorithm 2, step (f): when R >= 4 and EncryptMetadata is false,
//! four bytes `0xFFFFFFFF` must be appended to the key MD5. Skipping that append
//! derives the wrong file key, so the computed `/U` verifier never matches and
//! even the *empty* user password fails to authenticate — the document stays
//! locked and no content can be read. Real-world producers commonly leave
//! metadata in cleartext, which is why this reproduced on external files while
//! oxidize-pdf's own output (metadata encrypted) unlocked fine.
//!
//! Fixtures are produced by qpdf (`--cleartext-metadata`), which upgrades these
//! to V4/R4. See `tests/fixtures/generate_encryption_interop.sh`. Each carries
//! the marker below, so these tests assert real recovered content, not merely
//! that the file opened.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use std::io::Cursor;

const MARKER: &str = "OXIDIZE_INTEROP_FIXTURE_MARKER_V1";

fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/{name}"))
        .unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

fn document_text<R: std::io::Read + std::io::Seek>(
    doc: &oxidize_pdf::parser::PdfDocument<R>,
) -> String {
    doc.extract_text()
        .expect("extract_text")
        .iter()
        .map(|e| e.text.as_str())
        .collect()
}

/// Empty user password must auto-unlock an R4 EncryptMetadata=false file and
/// recover its real text.
fn assert_empty_user_unlocks_and_reads(fixture: &str) {
    let bytes = read_fixture(fixture);
    let reader = PdfReader::new_with_options(Cursor::new(bytes), ParseOptions::lenient())
        .unwrap_or_else(|e| panic!("open {fixture}: {e}"));
    assert!(reader.is_encrypted(), "{fixture} should be encrypted");
    assert!(
        reader.is_unlocked(),
        "{fixture}: empty-user R4 with EncryptMetadata=false should auto-unlock"
    );
    let doc = reader.into_document();
    let text = document_text(&doc);
    assert!(
        text.contains(MARKER),
        "{fixture}: expected marker in extracted text, got {text:?}"
    );
}

/// Non-empty user password must unlock the same construction and recover text.
fn assert_user_password_unlocks_and_reads(fixture: &str, password: &str) {
    let bytes = read_fixture(fixture);
    let mut reader = PdfReader::new_with_options(Cursor::new(bytes), ParseOptions::lenient())
        .unwrap_or_else(|e| panic!("open {fixture}: {e}"));
    assert!(reader.is_encrypted(), "{fixture} should be encrypted");
    reader
        .unlock(password)
        .unwrap_or_else(|e| panic!("{fixture}: unlock with '{password}' failed: {e}"));
    let doc = reader.into_document();
    let text = document_text(&doc);
    assert!(
        text.contains(MARKER),
        "{fixture}: expected marker in extracted text, got {text:?}"
    );
}

#[test]
fn aes128_cleartext_metadata_empty_user_unlocks() {
    assert_empty_user_unlocks_and_reads("interop_qpdf_aes128_ctm_empty.pdf");
}

#[test]
fn rc4_128_cleartext_metadata_empty_user_unlocks() {
    assert_empty_user_unlocks_and_reads("interop_qpdf_rc4-128_ctm_empty.pdf");
}

#[test]
fn aes128_cleartext_metadata_user_password_unlocks() {
    assert_user_password_unlocks_and_reads("interop_qpdf_aes128_ctm_user.pdf", "userpw");
}

#[test]
fn rc4_128_cleartext_metadata_user_password_unlocks() {
    assert_user_password_unlocks_and_reads("interop_qpdf_rc4-128_ctm_user.pdf", "userpw");
}
