//! Font / glyph invariants, stated from the contract of "render and extract the
//! characters the caller asked for", not from a bug.
//!
//! Invariants:
//!   1. WIDTH IS CHARACTER-BASED, ADDITIVE, AND SIZE-PROPORTIONAL. `measure_text`
//!      measures glyphs (chars), never UTF-8 bytes, so a single accented Latin-1
//!      character measures as one glyph — not two (#309). Width is additive over
//!      concatenation (no kerning in the base-14 metrics) and scales linearly
//!      with font size.
//!   2. NO SILENT GLYPH SUBSTITUTION. A character in the font's supported set,
//!      drawn through a WinAnsi standard-14 font, extracts back as itself — never
//!      as '?' or a dropped/garbled glyph (#272, #287, #392).
//!   3. A DEFINED MAPPING IS HONOURED, AND AN UNDEFINED ONE IS DECLARED. Two
//!      halves of one contract, over every way a document can define what a code
//!      means (`/Encoding`, `/ToUnicode`, `/Identity-H` + CID):
//!      (a) no character that has a mapping is silently extracted as something
//!      else — not '?', not `.notdef`, not the literal ASCII of its code, not
//!      dropped; and (b) when the font genuinely cannot render a character, the
//!      coverage report (`Document::font_missing_glyphs`) lists it. Reporting
//!      `[]` while the glyph does not survive the round-trip is the aggravating
//!      half of #392: the check that should warn gives a green light.
//!
//!      Guards #272 (CFF/ToUnicode garbage), #287 (.notdef), #392 ('?' plus false
//!      all-clear) and the CMap paths those left unguarded.
//!
//! Written from the contract up front so the whole "font mangles characters"
//! class is guarded, not just the reported instances.

use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::text::{measure_text, ExtractionOptions, Font, TextEncoding, TextExtractor};
use proptest::prelude::*;
use std::io::{Cursor, Write};

/// Accented Latin-1 letters, all representable in WinAnsi — the characters the
/// #309/#392 class mangled. Plus ASCII letters as controls.
const LATIN1: &[char] = &[
    'a', 'e', 'i', 'o', 'u', 'n', 'c', 'A', 'E', 'O', 'U', 'à', 'á', 'â', 'ä', 'è', 'é', 'ê', 'ë',
    'ì', 'í', 'î', 'ï', 'ò', 'ó', 'ô', 'ö', 'ù', 'ú', 'û', 'ü', 'ñ', 'ç', 'Á', 'É', 'Ñ', 'Ü',
];

// ---------- Invariant 1: measurement ----------

/// Deterministic pin for #309: `í` (iacute) has a true Helvetica advance of
/// 278/1000 em. The bug returned the 556-unit default width (over-measuring a
/// non-ASCII WinAnsi glyph whose metric wasn't looked up correctly). This nails
/// the exact reported value: 278, not the 556 fallback.
#[test]
fn issue_309_accented_char_uses_true_metric_not_fallback() {
    let wia = measure_text("í", &Font::Helvetica, 1000.0);
    assert_eq!(
        wia, 278.0,
        "í must measure its true metric (278), not the 556 fallback (#309)"
    );
}

fn latin1_string() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(LATIN1), 1..24)
        .prop_map(|cs| cs.into_iter().collect())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Width scales linearly with font size.
    #[test]
    fn width_is_size_proportional(s in latin1_string(), size in 1.0f64..200.0) {
        let h = Font::Helvetica;
        let base = measure_text(&s, &h, size);
        let doubled = measure_text(&s, &h, size * 2.0);
        prop_assert!((doubled - 2.0 * base).abs() < 1e-6, "width must scale with size");
    }

    /// Width is additive over concatenation (base-14 has no kerning), which also
    /// means it is counted per character, not per byte.
    #[test]
    fn width_is_additive_over_chars(a in latin1_string(), b in latin1_string()) {
        let h = Font::Helvetica;
        let whole = measure_text(&format!("{a}{b}"), &h, 12.0);
        let parts = measure_text(&a, &h, 12.0) + measure_text(&b, &h, 12.0);
        prop_assert!((whole - parts).abs() < 1e-6, "width must be additive over chars");
    }

    /// A single Latin-1 glyph never measures wider than the widest base-14 glyph.
    /// A byte-counting measurement of a 2-byte char can exceed this bound.
    #[test]
    fn single_glyph_within_one_em(c in prop::sample::select(LATIN1)) {
        let w = measure_text(&c.to_string(), &Font::Helvetica, 1000.0);
        prop_assert!(w > 0.0 && w <= 1000.0, "one glyph must be <= 1 em, got {w}");
    }
}

// ---------- Invariant 2: no silent glyph substitution (round-trip) ----------

fn escape_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    out
}

/// Build a one-page PDF that draws `text` through a WinAnsi standard-14 font,
/// with the string bytes WinAnsi-encoded exactly as the writer would emit them.
fn winansi_pdf(text: &str) -> Vec<u8> {
    let encoded = TextEncoding::WinAnsiEncoding.encode(text);
    let mut content = Vec::new();
    content.extend_from_slice(b"BT\n/F1 12 Tf\n72 700 Td\n(");
    content.extend_from_slice(&escape_bytes(&encoded));
    content.extend_from_slice(b") Tj\nET\n");

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> /MediaBox [0 0 612 792] /Contents 4 0 R >>\nendobj\n");
    offsets.push(pdf.len());
    let mut obj4 = Vec::new();
    write!(obj4, "4 0 obj\n<< /Length {} >>\nstream\n", content.len()).unwrap();
    obj4.extend_from_slice(&content);
    obj4.extend_from_slice(b"\nendstream\nendobj\n");
    pdf.extend_from_slice(&obj4);
    offsets.push(pdf.len());
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
    );
    let xref_pos = pdf.len();
    write!(pdf, "xref\n0 6\n0000000000 65535 f \n").unwrap();
    for off in &offsets {
        writeln!(pdf, "{off:010} 00000 n ").unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n"
    )
    .unwrap();
    pdf
}

fn extract(pdf: &[u8]) -> String {
    let reader = PdfReader::new_with_options(Cursor::new(pdf.to_vec()), ParseOptions::lenient())
        .expect("parse");
    let document = reader.into_document();
    let mut extractor = TextExtractor::with_options(ExtractionOptions::default());
    extractor
        .extract_from_page(&document, 0)
        .expect("extract")
        .text
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// A WinAnsi-drawn Latin-1 string extracts back as itself — no '?' or
    /// dropped/garbled glyph (#272, #287, #392).
    #[test]
    fn winansi_latin1_round_trips_through_extraction(text in latin1_string()) {
        let extracted = extract(&winansi_pdf(&text));
        let got: String = extracted.chars().filter(|c| !c.is_whitespace()).collect();
        prop_assert_eq!(
            &got, &text,
            "WinAnsi Latin-1 must extract unchanged; got {:?} for {:?}", got, text
        );
    }
}

// ---------- Invariant 3a: a defined mapping is honoured, on every path ----------
//
// A PDF can define what a character code means in more than one way, and the
// extraction has to honour whichever the document used. The generator's axis is
// therefore the *mapping mechanism*, with the drawn string ranging over the
// mapped character set:
//
//   * `/Encoding /WinAnsiEncoding` — the code is a WinAnsi byte;
//   * `/ToUnicode` on a subsetted simple font with NO `/Encoding` — the codes are
//     font-internal, so the CMap is the only thing that can decode them;
//   * `/Identity-H` + a CID descendant with `/ToUnicode` — two-byte codes.
//
// The last two are the paths where guessing is not merely inaccurate but
// meaningless: the code values carry no information outside their CMap.

/// Characters the generated documents map codes to. Latin, accented, Cyrillic
/// (outside any single-byte encoding) and the space — the space matters because
/// a subset font's first glyph is routinely U+0020 and content streams show it
/// in a text-showing operator of its own.
const MAPPED_ALPHABET: &[char] = &['A', 'B', 'C', 'z', 'á', 'ñ', 'ü', 'Э', 'ч', 'ф', '€', ' '];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mapping {
    /// Simple font, `/ToUnicode` only, no `/Encoding` (Word-style subset).
    ToUnicodeSubset,
    /// Type0 `/Identity-H` with a CID descendant and a two-byte `/ToUnicode`.
    IdentityH,
    /// Type0 `/Identity-H` with NO `/ToUnicode` anywhere: the meaning of a code
    /// comes from the descendant's `/CIDSystemInfo /Ordering` and the built-in
    /// CID→Unicode table for that collection. A separate decoder with its own
    /// acceptance rule, so a property that never reaches it does not guard it.
    CidTable,
}

/// The CID collection `Mapping::CidTable` documents in `/CIDSystemInfo`.
const CID_ORDERING: &str = "Japan1";

/// CIDs to draw in the CID-table property: taken from the collection itself
/// rather than hardcoded, because the table has gaps (CID 300 is unmapped in
/// Adobe-Japan1) and a hand-picked list silently rots.
///
/// The low CIDs come first, so the sample always includes CID 1 = U+00A0, a
/// no-break space. That is deliberate: it is a whitespace-only decode reached
/// through this decoder — the same shape as the #438 trigger on the other path.
/// (No CID in this range maps to U+0020; verified against the table.)
fn cid_sample() -> Vec<u16> {
    let collection = oxidize_pdf::text::cid_to_unicode::CidCollection::from_ordering(CID_ORDERING)
        .expect("known CID collection");
    (1u16..400)
        .filter(|cid| collection.cid_to_unicode(*cid).is_some())
        .take(24)
        .collect()
}

/// Serialize `bodies` (object 1..=n) into a PDF with a correct xref table.
fn assemble_pdf(bodies: Vec<Vec<u8>>) -> Vec<u8> {
    let mut pdf = Vec::from(&b"%PDF-1.5\n"[..]);
    let mut offsets = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(pdf.len());
        writeln!(pdf, "{} 0 obj", i + 1).unwrap();
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_pos = pdf.len();
    writeln!(pdf, "xref\n0 {}", bodies.len() + 1).unwrap();
    writeln!(pdf, "0000000000 65535 f ").unwrap();
    for off in &offsets {
        writeln!(pdf, "{off:010} 00000 n ").unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n",
        bodies.len() + 1
    )
    .unwrap();
    pdf
}

/// A stream object body with the right `/Length`.
fn stream_body(data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    write!(body, "<< /Length {} >>\nstream\n", data.len()).unwrap();
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

/// `/ToUnicode` CMap mapping each `(code, char)` pair. Codes are `code_bytes`
/// wide (1 for a simple font, 2 for Identity-H).
fn to_unicode_stream(pairs: &[(u32, char)], code_bytes: usize) -> Vec<u8> {
    let hex_code = |c: u32| match code_bytes {
        1 => format!("<{c:02x}>"),
        _ => format!("<{c:04x}>"),
    };
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n",
    );
    cmap.push_str(match code_bytes {
        1 => "<00> <ff>\n",
        _ => "<0000> <ffff>\n",
    });
    cmap.push_str("endcodespacerange\n");
    cmap.push_str(&format!("{} beginbfchar\n", pairs.len()));
    for (code, ch) in pairs {
        // BMP only — every character in MAPPED_ALPHABET is.
        cmap.push_str(&format!("{} <{:04x}>\n", hex_code(*code), *ch as u32));
    }
    cmap.push_str("endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap.into_bytes()
}

/// Codes are assigned from 0x21 so that the LOW BYTE of every code lands in
/// printable ASCII. That is what makes a fallback observable: if the extraction
/// discards the declared mapping and re-reads the bytes through a guessed
/// encoding, the code surfaces as its own literal ASCII instead of vanishing
/// into a control character that sanitization would silently drop — which would
/// make the property blind to the very substitution it exists to catch.
const FIRST_CODE: u32 = 0x21;

/// The `(code, expected character)` sequence a document of this `mapping` draws
/// for `text`: one code per distinct character, in first-seen order.
fn codes_for_text(mapping: Mapping, text: &str) -> Vec<(u32, char)> {
    assert!(
        mapping != Mapping::CidTable,
        "CidTable draws CIDs, not text"
    );
    let mut table: Vec<(u32, char)> = Vec::new();
    for ch in text.chars() {
        if !table.iter().any(|(_, c)| *c == ch) {
            table.push((FIRST_CODE + table.len() as u32, ch));
        }
    }
    text.chars()
        .map(|ch| *table.iter().find(|(_, c)| *c == ch).unwrap())
        .collect()
}

/// The `(CID, expected character)` sequence for `cids`, resolved through the
/// same collection table the document names in `/CIDSystemInfo`. The table is
/// the shared reference: the property asserts the extraction *consults* it
/// rather than discarding the decode and guessing.
fn codes_for_cids(cids: &[u16]) -> Vec<(u32, char)> {
    let collection = oxidize_pdf::text::cid_to_unicode::CidCollection::from_ordering(CID_ORDERING)
        .expect("known CID collection");
    cids.iter()
        .map(|cid| {
            (
                *cid as u32,
                collection.cid_to_unicode(*cid).expect("CID is mapped"),
            )
        })
        .collect()
}

/// One-page PDF drawing `drawn` (one text-showing operator per code) through a
/// font that defines what those codes mean the way `mapping` says.
fn mapped_pdf(mapping: Mapping, drawn: &[(u32, char)]) -> Vec<u8> {
    let mut table: Vec<(u32, char)> = Vec::new();
    for pair in drawn {
        if !table.contains(pair) {
            table.push(*pair);
        }
    }
    let code_bytes = match mapping {
        Mapping::ToUnicodeSubset => 1,
        Mapping::IdentityH | Mapping::CidTable => 2,
    };

    let mut content = Vec::from(&b"BT\n/F1 12 Tf\n50 700 Td\n"[..]);
    for (code, _) in drawn {
        match code_bytes {
            1 => writeln!(content, "<{code:02x}> Tj").unwrap(),
            _ => writeln!(content, "<{code:04x}> Tj").unwrap(),
        }
    }
    content.extend_from_slice(b"ET\n");

    let widths: String = table.iter().map(|_| "500 ").collect();
    let (font_body, extra): (Vec<u8>, Vec<Vec<u8>>) = match mapping {
        Mapping::ToUnicodeSubset => (
            format!(
                "<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAA+Subset \
                 /FirstChar {} /LastChar {} /Widths [{}] /ToUnicode 6 0 R >>",
                FIRST_CODE,
                FIRST_CODE as usize + table.len() - 1,
                widths.trim_end()
            )
            .into_bytes(),
            vec![stream_body(&to_unicode_stream(&table, 1))],
        ),
        Mapping::IdentityH => (
            b"<< /Type /Font /Subtype /Type0 /BaseFont /BBBBBB+CidFont \
              /Encoding /Identity-H /DescendantFonts [7 0 R] /ToUnicode 6 0 R >>"
                .to_vec(),
            vec![
                stream_body(&to_unicode_stream(&table, 2)),
                b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /BBBBBB+CidFont \
                  /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                  /DW 500 >>"
                    .to_vec(),
            ],
        ),
        // No /ToUnicode anywhere: the only thing that explains a code is the
        // named CID collection, which routes the decode through a different
        // decoder (and, before the #438 fix, a second copy of the same
        // "does this decode look like garbage?" rule).
        Mapping::CidTable => (
            b"<< /Type /Font /Subtype /Type0 /BaseFont /CCCCCC+CidFont \
              /Encoding /Identity-H /DescendantFonts [6 0 R] >>"
                .to_vec(),
            vec![format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /CCCCCC+CidFont \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering ({CID_ORDERING}) /Supplement 0 >> \
                 /DW 500 >>"
            )
            .into_bytes()],
        ),
    };

    let mut bodies = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> \
          /MediaBox [0 0 595 842] /Contents 4 0 R >>"
            .to_vec(),
        stream_body(&content),
        font_body,
    ];
    bodies.extend(extra);
    assemble_pdf(bodies)
}

/// Deterministic pin for #438, reproducing the reported shape: a subsetted font
/// whose `FirstChar` glyph maps to U+0020, shown in a text-showing operator of
/// its own between two letters. Reported as `PROSPECTO!PRELIMINAR` where the
/// document said `PROSPECTO PRELIMINAR`; here, `A A` came back as `A"A`.
#[test]
fn issue_438_lone_space_between_letters_survives() {
    let drawn = codes_for_text(Mapping::ToUnicodeSubset, "A A");
    let extracted = extract(&mapped_pdf(Mapping::ToUnicodeSubset, &drawn));
    assert_eq!(
        extracted.split_whitespace().collect::<Vec<_>>(),
        vec!["A", "A"],
        "a lone mapped space must stay a space, got {extracted:?}"
    );
}

fn mapped_string() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(MAPPED_ALPHABET), 1..16)
        .prop_map(|cs| cs.into_iter().collect())
}

/// Every character the document defines a mapping for extracts as itself.
/// Nothing outside the mapped set may appear: a '?', a `.notdef` replacement, or
/// the literal ASCII of a code all mean the extraction overrode the mapping the
/// document declared.
///
/// One property per mapping mechanism rather than one with the mechanism as a
/// generated axis: with a single property, a failure on one path shrinks and
/// stops there, hiding whether the others also break.
fn assert_mapping_honoured(mapping: Mapping, drawn: &[(u32, char)]) -> Result<(), TestCaseError> {
    let expected: String = drawn.iter().map(|(_, c)| *c).collect();
    let extracted = extract(&mapped_pdf(mapping, drawn));

    let defined: std::collections::HashSet<char> = expected.chars().collect();
    for c in extracted.chars() {
        prop_assert!(
            c.is_whitespace() || defined.contains(&c),
            "{mapping:?}: extracted {c:?}, which the document maps no code to \
             (mapped set {defined:?}); drew {expected:?}, got {extracted:?}"
        );
    }

    // Collapse whitespace runs rather than deleting whitespace: a mapped space
    // that disappears entirely must still fail (it is the #438 symptom seen from
    // the other side), while layout adding its own spacing must not.
    let normalize = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    };
    prop_assert_eq!(
        normalize(&extracted),
        normalize(&expected),
        "{:?}: mapped characters must survive in order; drew {:?}, got {:?}",
        mapping,
        expected,
        extracted
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Subsetted simple font whose codes only the `/ToUnicode` CMap explains.
    ///
    /// This is where #438 lived: the property reproduced it from the contract
    /// and shrank it below the reported case — drawing the single mapped
    /// character `" "` came back as `"!"`, because `decode_text` treated a
    /// decode whose `trim()` is empty as garbage and re-read the code through a
    /// guessed encoding. In a subset with no `/Encoding` there is nothing to
    /// guess from.
    #[test]
    fn to_unicode_subset_mapping_is_honoured(text in mapped_string()) {
        let drawn = codes_for_text(Mapping::ToUnicodeSubset, &text);
        assert_mapping_honoured(Mapping::ToUnicodeSubset, &drawn)?;
    }

    /// Type0 `/Identity-H` with a CID descendant and a two-byte `/ToUnicode`.
    ///
    /// Also broken by #438 — verified by reverting the fix with this test in
    /// place. Worth recording how nearly it was missed: a first version of this
    /// property numbered its codes from 1 and compared with all whitespace
    /// stripped from both sides. Under the bug, the fallback then decoded
    /// `[0x00, 0x01]` into control characters that sanitization deletes, and a
    /// space that vanishes entirely is invisible to an assertion that deletes
    /// spaces anyway. It passed 300 cases against broken code. Both details in
    /// `FIRST_CODE` and in the whitespace-collapsing comparison exist to keep
    /// the substitution observable.
    #[test]
    fn identity_h_mapping_is_honoured(text in mapped_string()) {
        let drawn = codes_for_text(Mapping::IdentityH, &text);
        assert_mapping_honoured(Mapping::IdentityH, &drawn)?;
    }

    /// Type0 `/Identity-H` with NO `/ToUnicode`: the code means whatever the
    /// named CID collection says. This is a different decoder from the two
    /// above (which both resolve through the `/ToUnicode` CMap), with its own
    /// copy of the acceptance rule — so it needs its own property or it stays
    /// unguarded no matter how many cases the others run.
    ///
    /// The drawn CIDs include CID 1, which in Adobe-Japan1 is U+00A0: a
    /// whitespace character reached through this decoder, the same shape as the
    /// #438 trigger on the other path.
    ///
    /// FALSIFIABILITY, stated honestly: unlike the two above, this property
    /// passes both with and without the #438 fix — checked by reverting it. This
    /// decoder's own acceptance rule already tolerated U+00A0 (it tested
    /// `is_ascii_control`, which a no-break space is not), so the bug never
    /// reached here. It guards the path going forward — the two rules are now
    /// one shared predicate, and this is what stops it from drifting back — but
    /// it did not catch #438 and is not evidence that it would have.
    #[test]
    fn cid_table_mapping_is_honoured(
        cids in prop::collection::vec(prop::sample::select(cid_sample()), 1..16),
    ) {
        let drawn = codes_for_cids(&cids);
        assert_mapping_honoured(Mapping::CidTable, &drawn)?;
    }
}

// ---------- Invariant 3b: the coverage report does not lie ----------
//
// The other half of the same contract. When a character genuinely has no glyph,
// rendering it as `.notdef` is correct PDF behaviour — what is not acceptable is
// a green light that turns out to be false. `font_missing_glyphs` exists so a
// caller can know beforehand, so a character it does NOT list has to come back
// out of the document. #392 is exactly that disagreement: `[]` reported while
// the glyphs came out as '?'.
//
// SCOPE, stated rather than implied: extraction reads the text, not the paint.
// A character whose glyph is `.notdef` but whose `/ToUnicode` is right still
// extracts fine — verified: Roboto reports 漢/字/✓ missing and all three still
// extract. So this property guards the direction that matters for a silent data
// loss (reported present ⇒ must survive) and CANNOT see the reverse (reported
// missing ⇒ blank box actually painted). Detecting a painted `.notdef` needs the
// rendered page, not the extracted text; that is left unguarded here on purpose.

const ROBOTO_PATH: &str = "../test-pdfs/Roboto-Regular.ttf";

fn roboto() -> Vec<u8> {
    std::fs::read(ROBOTO_PATH).expect("read Roboto fixture")
}

/// Latin (surely covered), Cyrillic and CJK (the ranges an embedded Latin subset
/// may or may not carry). The property does not assume which side each falls on
/// — that is precisely what the report is supposed to tell the caller.
const COVERAGE_ALPHABET: &[char] = &['A', 'z', 'á', 'ñ', 'Э', 'ч', 'ф', '漢', '字', '€', '✓'];

proptest! {
    // Each case embeds and subsets a real TTF, which is costly; 48 is a
    // cost tradeoff, not a coverage ceiling on the invariant.
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// A character the coverage report does not list must survive being drawn:
    /// reporting nothing missing while the glyph comes out as '?' (or not at
    /// all) is the false all-clear of #392.
    #[test]
    fn coverage_report_agrees_with_what_survives(
        text in prop::collection::vec(prop::sample::select(COVERAGE_ALPHABET), 1..12)
            .prop_map(|cs| cs.into_iter().collect::<String>()),
    ) {
        let mut doc = oxidize_pdf::Document::new();
        doc.add_font_from_bytes("F", roboto()).expect("embed font");
        let missing: std::collections::HashSet<char> =
            doc.font_missing_glyphs("F", &text).into_iter().collect();

        let mut page = oxidize_pdf::Page::a4();
        page.text()
            .set_font(Font::Custom("F".to_string()), 24.0)
            .at(50.0, 700.0)
            .write(&text)
            .expect("write text");
        doc.add_page(page);
        let extracted = extract(&doc.to_bytes().expect("serialize"));

        let survived: std::collections::HashSet<char> = extracted.chars().collect();
        for ch in text.chars().filter(|c| !c.is_whitespace()) {
            if missing.contains(&ch) {
                continue; // declared unrenderable up front — honest.
            }
            prop_assert!(
                survived.contains(&ch),
                "{ch:?} was not reported missing (report: {missing:?}) yet did not \
                 survive drawing; drew {text:?}, extracted {extracted:?}"
            );
        }
    }
}
