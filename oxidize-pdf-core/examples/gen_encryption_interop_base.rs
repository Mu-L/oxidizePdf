//! Generate the base PDF used by the encryption interop fixture matrix
//! (`tests/fixtures/generate_encryption_interop.sh`).
//!
//! The document is a single page carrying a unique, easy-to-assert marker
//! string. Interop tests decrypt a qpdf-produced encrypted copy and assert the
//! marker is recovered — verifying real decrypted content, not just that the
//! file opens.
//!
//! The *content* (marker text, page structure) is deterministic and the
//! `/CreationDate` is pinned below, but `Document::save()` always stamps
//! `/ModDate` with the current time (`document.rs`), which cannot be pinned via
//! the current public API. So regenerating the base yields a small byte diff in
//! the `/ModDate` field — this is expected, NOT a corruption signal. The
//! committed fixtures are the source of truth; regeneration only documents
//! provenance.
//!
//! Run (writes next to the other fixtures, independent of cwd):
//!
//!   cargo run --example gen_encryption_interop_base
//!
//! Output: tests/fixtures/interop_base.pdf

use chrono::{TimeZone, Utc};
use oxidize_pdf::error::Result;
use oxidize_pdf::{Document, Font, Page};

/// Unique marker asserted by the interop decryption tests.
pub const INTEROP_MARKER: &str = "OXIDIZE_INTEROP_FIXTURE_MARKER_V1";

fn main() -> Result<()> {
    let mut doc = Document::new();
    doc.set_title("oxidize-pdf encryption interop base");
    doc.set_author("oxidize-pdf");
    // Pin CreationDate for stable provenance. ModDate is still stamped by save().
    doc.set_creation_date(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());

    let mut page = Page::a4();
    page.text()
        .set_font(Font::Helvetica, 18.0)
        .at(72.0, 700.0)
        .write(INTEROP_MARKER)?;
    page.text()
        .set_font(Font::Helvetica, 12.0)
        .at(72.0, 660.0)
        .write("Encryption interoperability fixture. Do not edit by hand.")?;
    doc.add_page(page);

    let output_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interop_base.pdf"
    );
    doc.save(output_path)?;
    println!("wrote {output_path}");
    Ok(())
}
