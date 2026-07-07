//! Regression tests for issue #374.
//!
//! `poppler-85140-0.pdf` is a poppler fuzzing fixture that has NO `xref`
//! table and NO `startxref` marker — the trailer directly follows the last
//! `endobj`. A robust PDF reader must reconstruct the cross-reference table by
//! scanning object headers (as poppler/pdf.js/qpdf do), instead of failing at
//! parse stage with `Invalid xref table`.
//!
//! Before the fix, `PdfReader::new` (strict-by-default, `lenient_syntax=false`)
//! propagated `ParseError::InvalidXRef`; only the explicitly-lenient paths
//! (`PdfReader::open`, `ParseOptions::tolerant()`) recovered. This coupled
//! xref reconstruction to `lenient_syntax`, which is a syntax-tolerance knob,
//! not a recovery knob. The fix gates reconstruction on
//! `max_recovery_attempts > 0` instead, so `new()`/`default()` recover while
//! `strict()` (attempts = 0) still fails loudly.

use oxidize_pdf::parser::{ParseError, ParseOptions, PdfName, PdfObject, PdfReader};
use std::io::Cursor;

const FIXTURE: &str = "tests/fixtures/poppler-85140-0.pdf";

/// `PdfReader::new` (default options, in-memory reader — the path used on
/// wasm32) must reconstruct the missing xref and resolve the real catalog
/// object recovered by the object-header scan.
#[test]
fn pdfreader_new_reconstructs_missing_xref_and_resolves_catalog() {
    let bytes = std::fs::read(FIXTURE).expect("read fixture");
    let mut reader =
        PdfReader::new(Cursor::new(bytes)).expect("new() must reconstruct the missing xref table");

    // Not a smoke check: prove the reconstruction found the real objects by
    // resolving the catalog (object 1) and confirming its /Type is /Catalog.
    let catalog = reader
        .catalog()
        .expect("catalog must resolve after reconstruction");
    let ty = catalog.get("Type").expect("catalog has /Type");
    assert_eq!(
        ty,
        &PdfObject::Name(PdfName("Catalog".to_string())),
        "recovered catalog must be /Type /Catalog"
    );

    // The catalog references /Pages 2 0 R; the scan must have indexed object 2
    // as well, and it must resolve to a /Pages node.
    let pages = reader
        .get_object(2, 0)
        .expect("object 2 indexed by scan")
        .clone();
    let pages_dict = pages.as_dict().expect("object 2 is a dictionary");
    assert_eq!(
        pages_dict.get("Type"),
        Some(&PdfObject::Name(PdfName("Pages".to_string()))),
        "recovered object 2 must be /Type /Pages"
    );
}

/// Security guard (fail-safe): an ENCRYPTED PDF whose xref must be
/// reconstructed must NOT be silently opened as plaintext. The recovered
/// synthetic trailer must preserve `/Encrypt` (and `/ID`) so the encryption
/// flow still runs. Before the fix the recovery trailer carried only
/// `/Size`+`/Root`, dropping `/Encrypt`, so a recovered encrypted document
/// looked unencrypted — a fail-open regression once `new()` gained recovery.
#[test]
fn recovered_encrypted_pdf_is_not_opened_as_plaintext() {
    // Standard-security encrypted PDF (V1/R2) with NO xref and NO startxref,
    // forcing xref reconstruction. The trailer declares /Encrypt and /ID.
    let pdf = b"%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>
endobj
5 0 obj
<< /Filter /Standard /V 1 /R 2 /O (0123456789abcdef0123456789abcdef) /U (0123456789abcdef0123456789abcdef) /P -4 >>
endobj
trailer
<< /Size 6 /Root 1 0 R /Encrypt 5 0 R /ID [(0123456789abcdef) (0123456789abcdef)] >>
%%EOF"
        .to_vec();

    match PdfReader::new(Cursor::new(pdf)) {
        // Rejecting the encrypted document is safe.
        Err(_) => {}
        // Opening it is only acceptable if encryption is still recognized and
        // the document stays locked; being opened as plaintext/unlocked is a
        // fail-open bug.
        Ok(reader) => assert!(
            reader.is_encrypted() && !reader.is_unlocked(),
            "encrypted PDF recovered via xref reconstruction must stay \
             encrypted+locked, not be opened as plaintext"
        ),
    }
}

/// Fail-safe for PDF 1.5+ encrypted files that use a cross-reference STREAM
/// (no classic `trailer` keyword — `/Encrypt` lives in the xref-stream dict).
/// When the xref stream can't be used and recovery kicks in, `/Encrypt` must
/// still be carried into the synthetic trailer, or the document would open as
/// plaintext. Modern (AES) encrypted PDFs commonly use xref streams.
#[test]
fn recovered_encrypted_xref_stream_pdf_is_not_opened_as_plaintext() {
    // Encrypted document whose only cross-reference is a stream object
    // (object 6, /Type /XRef ... /Encrypt 5 0 R). No `startxref`, forcing
    // reconstruction; the xref-stream dict is the only place /Encrypt appears.
    let pdf = b"%PDF-1.5
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>
endobj
5 0 obj
<< /Filter /Standard /V 1 /R 2 /O (0123456789abcdef0123456789abcdef) /U (0123456789abcdef0123456789abcdef) /P -4 >>
endobj
6 0 obj
<< /Type /XRef /Size 7 /Root 1 0 R /Encrypt 5 0 R /ID [(0123456789abcdef) (0123456789abcdef)] /W [1 1 1] /Length 4 >>
stream
\x00\x00\x00\x00
endstream
endobj
%%EOF"
        .to_vec();

    match PdfReader::new(Cursor::new(pdf)) {
        Err(_) => {}
        Ok(reader) => assert!(
            reader.is_encrypted() && !reader.is_unlocked(),
            "encrypted xref-stream PDF recovered via reconstruction must stay \
             encrypted+locked, not be opened as plaintext"
        ),
    }
}

/// Fail-safe regression guard: a value BEFORE `/Encrypt` in the xref-stream
/// dict that contains the ASCII bytes `stream` (e.g. `/Producer (streamlined)`)
/// must not truncate the dict and cause `/Encrypt` to be dropped. The dict must
/// be parsed with a real lexer that respects string boundaries, not truncated
/// at a raw `stream` substring.
#[test]
fn recovered_encrypted_xref_stream_with_stream_substring_before_encrypt() {
    let pdf = b"%PDF-1.5
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>
endobj
5 0 obj
<< /Filter /Standard /V 1 /R 2 /O (0123456789abcdef0123456789abcdef) /U (0123456789abcdef0123456789abcdef) /P -4 >>
endobj
6 0 obj
<< /Type /XRef /Producer (a streamlined pdf tool) /Size 7 /Root 1 0 R /Encrypt 5 0 R /ID [(0123456789abcdef) (0123456789abcdef)] /W [1 1 1] /Length 4 >>
stream
\x00\x00\x00\x00
endstream
endobj
%%EOF"
        .to_vec();

    match PdfReader::new(Cursor::new(pdf)) {
        Err(_) => {}
        Ok(reader) => assert!(
            reader.is_encrypted() && !reader.is_unlocked(),
            "a `stream` substring in a value before /Encrypt must not drop /Encrypt (fail-open)"
        ),
    }
}

/// Fail-safe under lenient/tolerant options: the recovery trailer parser must
/// honour the caller's ParseOptions. A stray byte between `trailer` and its
/// `<<` (plausible in exactly the corrupted files recovery exists for) must be
/// tolerated under `tolerant()` so `/Encrypt` is still recovered — not dropped
/// because an internal lexer silently reverted to strict tokenizing.
#[test]
fn recovered_encrypted_pdf_with_stray_trailer_byte_preserves_encrypt_under_tolerant() {
    // Classic trailer preceded by a stray '`' before `<<`; no xref/startxref
    // forces recovery.
    let pdf = b"%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>
endobj
5 0 obj
<< /Filter /Standard /V 1 /R 2 /O (0123456789abcdef0123456789abcdef) /U (0123456789abcdef0123456789abcdef) /P -4 >>
endobj
trailer
` << /Size 6 /Root 1 0 R /Encrypt 5 0 R /ID [(0123456789abcdef) (0123456789abcdef)] >>
%%EOF"
        .to_vec();

    match PdfReader::new_with_options(Cursor::new(pdf), ParseOptions::tolerant()) {
        Err(_) => {}
        Ok(reader) => assert!(
            reader.is_encrypted() && !reader.is_unlocked(),
            "under tolerant(), a stray byte before the trailer dict must not \
             cause /Encrypt to be dropped (fail-open)"
        ),
    }
}

/// Guard: `strict()` disables recovery (`max_recovery_attempts == 0`), so the
/// missing xref must still fail loudly with `InvalidXRef`. This preserves the
/// project's fail-loud-by-default contract for callers that opt into strict
/// parsing.
#[test]
fn strict_mode_still_fails_loudly_on_missing_xref() {
    let bytes = std::fs::read(FIXTURE).expect("read fixture");
    // PdfReader does not implement Debug, so match on the Result directly
    // instead of using expect_err.
    match PdfReader::new_with_options(Cursor::new(bytes), ParseOptions::strict()) {
        Ok(_) => panic!("strict() must NOT reconstruct a missing xref"),
        Err(ParseError::InvalidXRef) => {}
        Err(other) => panic!("strict() must fail with InvalidXRef, got: {other:?}"),
    }
}
