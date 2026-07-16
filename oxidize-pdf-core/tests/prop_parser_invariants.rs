//! Core invariants of the parser, stated from ISO 32000-1, not from a bug.
//!
//! INV — LATEST REVISION WINS (ISO 32000-1 §7.5.6, Incremental Updates):
//!   when a PDF is amended by appending an incremental update that redefines an
//!   object, every indirect reference must resolve to the *most recent*
//!   definition. A document whose `/Pages` root is redefined by an update to
//!   hold M pages must report M pages — never the stale original count — and
//!   this must hold whether the cross-reference tables are well-formed or
//!   corrupted (forcing the lenient recovery scan).
//!
//! This is a spec invariant: it is written from the standard, so it guards the
//! whole "incremental update resolves to a stale revision" class up front. It
//! is the property that exposes #426.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use proptest::prelude::*;
use std::io::Cursor;

/// Build a PDF with `base_n` pages, then an incremental update that redefines
/// the `/Pages` root (object 2) to hold `upd_n` pages. When `corrupt_xref` is
/// set, both xref sections use an unparseable keyword so the lenient parser
/// falls into its whole-file recovery scan (the #426 path). The correct page
/// count for the resulting document is always `upd_n` (the latest revision).
fn build_incremental_pdf(base_n: usize, upd_n: usize, corrupt_xref: bool) -> Vec<u8> {
    assert!(base_n >= 1 && upd_n >= base_n);
    let xref_kw = if corrupt_xref { "xrfx" } else { "xref" };
    let mut buf: Vec<u8> = Vec::new();
    let push = |b: &mut Vec<u8>, s: &str| b.extend_from_slice(s.as_bytes());

    push(&mut buf, "%PDF-1.7\n");

    // Object numbering: 1=Catalog, 2=Pages, page objects start at 3.
    // Base pages: objects 3 .. 3+base_n. Update pages: the next `upd_n-base_n`.
    let first_page_obj = 3usize;
    let mut offsets: Vec<(usize, usize)> = Vec::new(); // (obj_num, offset)

    // --- Base revision ---
    offsets.push((1, buf.len()));
    push(&mut buf, "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n");

    let base_kids: String = (0..base_n)
        .map(|i| format!("{} 0 R", first_page_obj + i))
        .collect::<Vec<_>>()
        .join(" ");
    offsets.push((2, buf.len()));
    push(
        &mut buf,
        &format!("2 0 obj\n<</Type/Pages/Count {base_n}/Kids[{base_kids}]>>\nendobj\n"),
    );

    for i in 0..base_n {
        offsets.push((first_page_obj + i, buf.len()));
        push(
            &mut buf,
            &format!(
                "{} 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>\nendobj\n",
                first_page_obj + i
            ),
        );
    }

    let xref1_offset = buf.len();
    write_xref(&mut buf, xref_kw, &offsets, true);
    push(
        &mut buf,
        &format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref1_offset}\n%%EOF\n",
            offsets.len() + 1
        ),
    );

    // --- Incremental update: redefine object 2 with upd_n pages ---
    let mut upd_offsets: Vec<(usize, usize)> = Vec::new();

    let upd_kids: String = (0..upd_n)
        .map(|i| format!("{} 0 R", first_page_obj + i))
        .collect::<Vec<_>>()
        .join(" ");
    upd_offsets.push((2, buf.len()));
    push(
        &mut buf,
        &format!("2 0 obj\n<</Type/Pages/Count {upd_n}/Kids[{upd_kids}]>>\nendobj\n"),
    );

    for i in base_n..upd_n {
        upd_offsets.push((first_page_obj + i, buf.len()));
        push(
            &mut buf,
            &format!(
                "{} 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>\nendobj\n",
                first_page_obj + i
            ),
        );
    }

    let xref2_offset = buf.len();
    write_xref(&mut buf, xref_kw, &upd_offsets, false);
    push(
        &mut buf,
        &format!(
            "trailer\n<</Size {}/Root 1 0 R/Prev {xref1_offset}>>\nstartxref\n{xref2_offset}\n%%EOF\n",
            first_page_obj + upd_n
        ),
    );

    buf
}

/// Write a cross-reference section listing exactly `objs` (plus the free head
/// object 0 when `free_head` is set, for the base section). Objects are grouped
/// into contiguous subsections per ISO 32000-1 §7.5.4 — an incremental update's
/// section lists ONLY the objects it changed, so unlisted objects keep resolving
/// through the `/Prev` chain rather than being marked free (deleted).
fn write_xref(buf: &mut Vec<u8>, keyword: &str, objs: &[(usize, usize)], free_head: bool) {
    // (obj_num, entry-text) sorted ascending, grouped into consecutive runs.
    let mut entries: Vec<(usize, String)> = objs
        .iter()
        .map(|&(n, off)| (n, format!("{off:010} 00000 n \n")))
        .collect();
    if free_head {
        entries.push((0, "0000000000 65535 f \n".to_string()));
    }
    entries.sort_by_key(|(n, _)| *n);

    buf.extend_from_slice(format!("{keyword}\n").as_bytes());
    let mut i = 0;
    while i < entries.len() {
        let start = entries[i].0;
        let mut j = i;
        while j + 1 < entries.len() && entries[j + 1].0 == entries[j].0 + 1 {
            j += 1;
        }
        let count = j - i + 1;
        buf.extend_from_slice(format!("{start} {count}\n").as_bytes());
        for (_, entry) in &entries[i..=j] {
            buf.extend_from_slice(entry.as_bytes());
        }
        i = j + 1;
    }
}

fn page_count(pdf: &[u8]) -> Option<u32> {
    let reader =
        PdfReader::new_with_options(Cursor::new(pdf.to_vec()), ParseOptions::lenient()).ok()?;
    reader.into_document().page_count().ok()
}

/// Deterministic pin, mirroring the #426 report: corrupted xref + incremental
/// update redefining /Pages from 1 to 3 pages must resolve to 3.
#[test]
fn issue_426_incremental_update_resolves_to_latest_revision() {
    let pdf = build_incremental_pdf(1, 3, true);
    assert_eq!(
        page_count(&pdf),
        Some(3),
        "recovery resolved a stale /Pages revision (#426)"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// INV — LATEST REVISION WINS, across page counts, growth, and both a
    /// healthy and a corrupted cross-reference table.
    #[test]
    fn incremental_update_resolves_to_latest(
        base_n in 1usize..4,
        extra in 1usize..4,
        corrupt_xref in any::<bool>(),
    ) {
        let upd_n = base_n + extra;
        let pdf = build_incremental_pdf(base_n, upd_n, corrupt_xref);
        prop_assert_eq!(
            page_count(&pdf),
            Some(upd_n as u32),
            "resolved a stale revision (corrupt_xref={})", corrupt_xref
        );
    }
}
