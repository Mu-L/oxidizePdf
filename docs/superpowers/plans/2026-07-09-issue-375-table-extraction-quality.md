# Issue #375 — Table Extraction Quality + Data-Loss Safety Net — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop silent text loss when prose is misdetected as a table, and additively enrich the table model so merged cells and multi-level headers (header rows + merged cells) are represented and exported to GFM.

**Architecture:** Two independent detectors (ruling/vector-grid in `text/table_detection.rs`, spatial/borderless in `text/structured/`) feed one canonical `Element::Table(TableElementData)` in `pipeline/`. We (1) change fragment *claiming* so a table claims only text it actually placed in a cell (leftover flows to prose), guarded by a text-conservation invariant; (2) add an optional rich `TableStructure` alongside the existing flat `rows` (flat derived from rich, no breaking change); (3) rework grid construction to detect absent interior dividers → merged cells; (4) collapse header rows in the Markdown exporter and compute RAG metadata from the rich model.

**Tech Stack:** Rust (edition per workspace), `cargo test`/`clippy`, no new dependencies, no-ML/deterministic.

## Global Constraints

- MSRV **1.88**; verify `cargo +1.88 build --lib --all-features --locked` before PR.
- **Warnings = errors**: every commit must pass `cargo clippy --lib --all-features -- -D warnings` and `cargo fmt --check`.
- **Additive only**: no breaking change to public types. `TableElementData.rows` stays public and unchanged in meaning. New fields are `Option`/defaulted.
- **No-ML, deterministic**: identical input → identical output. No randomness, stable ordering.
- **Tests verify real content**, never just "no crash" or "non-empty".
- **TDD**: write the failing test, watch it fail, implement minimally, watch it pass, commit.
- Branch: `feature/issue-375-table-extraction-quality` (off `develop`). Frequent commits.
- **quality-rust pass is mandatory before the PR** (final task).
- SemVer target: **MINOR**.

## File map

- `oxidize-pdf-core/src/pipeline/element.rs` — add `TableStructure`, `RichCell`, `structure` field + `from_structure` constructor; Markdown/metadata read it.
- `oxidize-pdf-core/src/pipeline/partition.rs` — claim-only-captured (both paths), reject-degenerate, build rich table from ruling detector.
- `oxidize-pdf-core/src/text/table_detection.rs` — `TableCell` spans, `DetectedTable.header_rows`, grid-segment retention, merged-cell detection.
- `oxidize-pdf-core/src/pipeline/export.rs` — header-row collapse (multi-level → joined GFM header).
- `oxidize-pdf-core/src/pipeline/chunk_metadata.rs` — `table_dims` from rich structure.
- `oxidize-pdf-core/docs/../docs/TABLE_DETECTION_GUIDE.md` (repo `docs/TABLE_DETECTION_GUIDE.md`) + module rustdoc — known-limitation docs.
- Tests: `tests/issue_375_text_conservation_test.rs`, `tests/issue_375_merged_cells_test.rs`, `tests/issue_375_multi_level_header_test.rs`, plus additions to `tests/partition_table_detection_test.rs`.

---

## Task 1: Reproduce the data-loss bug with a text-conservation invariant (RED)

**Files:**
- Create: `oxidize-pdf-core/tests/issue_375_text_conservation_test.rs`

**Interfaces:**
- Consumes: `PdfDocument::partition_with(PartitionConfig) -> ParseResult<Vec<Element>>`; `Element::display_text() -> String`; text extraction to get the page's expected text.
- Produces: the invariant helper `assert_text_conserved` reused by later tasks.

- [ ] **Step 1: Write the failing test**

```rust
// tests/issue_375_text_conservation_test.rs
//
// #375 data-loss guard: after partitioning, the union of all element text must
// cover the page's extracted text. A prose page misdetected as an (empty) table
// currently drops its text -> this fails RED until the claim fix lands.

use oxidize_pdf::parser::{PdfDocument, PdfReader};
use oxidize_pdf::pipeline::{Element, PartitionConfig};
use oxidize_pdf::text::TextExtractor;

const FIXTURE: &str = "tests/fixtures/issue_272_boe_sumario_2025_01_15.pdf";

/// Non-whitespace tokens of `s`, lowercased, for order-independent containment.
fn tokens(s: &str) -> Vec<String> {
    s.split_whitespace().map(|t| t.to_lowercase()).collect()
}

#[test]
fn partition_conserves_page_text_default_config() {
    let doc = PdfDocument::new(PdfReader::open(FIXTURE).expect("open fixture"));

    // Expected: raw extracted text of every page.
    let mut extractor = TextExtractor::new();
    let page_count = doc.page_count().expect("page_count");
    let mut expected = String::new();
    for p in 0..page_count {
        let page_text = extractor
            .extract_from_page(&doc, p)
            .expect("extract page")
            .text;
        expected.push('\n');
        expected.push_str(&page_text);
    }

    // Actual: union of all element display text.
    let elements = doc.partition_with(PartitionConfig::default()).expect("partition");
    let actual: String = elements
        .iter()
        .map(|e| e.display_text())
        .collect::<Vec<_>>()
        .join("\n");

    let actual_tokens: std::collections::HashSet<String> = tokens(&actual).into_iter().collect();

    // Every expected token must survive partitioning (allow a small slack for
    // tokenization artifacts at cell boundaries).
    let missing: Vec<String> = tokens(&expected)
        .into_iter()
        .filter(|t| !actual_tokens.contains(t))
        .collect();

    let miss_ratio = missing.len() as f64 / tokens(&expected).len().max(1) as f64;
    assert!(
        miss_ratio < 0.01,
        "partition dropped {:.1}% of page tokens ({} missing), e.g. {:?}",
        miss_ratio * 100.0,
        missing.len(),
        &missing.iter().take(15).collect::<Vec<_>>()
    );
}
```

- [ ] **Step 2: Run it and confirm RED**

Run: `cargo test --test issue_375_text_conservation_test -- --nocapture`
Expected: FAIL — a meaningful fraction of tokens missing (the #345 empty-table drop). If it unexpectedly PASSES, the fixture doesn't trigger the bug; switch to the alternate reproduction in Step 3 before proceeding.

- [ ] **Step 3 (only if Step 2 passed): synthetic spatial reproduction**

If the fixture no longer triggers it, add this second test that drives the spatial path directly with prose fragments that clear the 0.5 confidence gate yet leave bbox-interior fragments uncelled:

```rust
use oxidize_pdf::pipeline::Partitioner;
use oxidize_pdf::text::extraction::TextFragment;

fn frag(text: &str, x: f64, y: f64) -> TextFragment {
    TextFragment {
        text: text.to_string(), x, y,
        width: text.len() as f64 * 6.0, height: 12.0, font_size: 12.0,
        font_name: None, is_bold: false, is_italic: false, color: None,
        space_decisions: Vec::new(), mcid: None, struct_tag: None,
    }
}

#[test]
fn spatial_table_does_not_drop_interior_prose() {
    // A justified prose block: 3 X-columns x 4 Y-rows, but one stray token sits
    // in a gap between columns (inside the table bbox, outside every cell).
    let mut frags = Vec::new();
    for (r, y) in [780.0, 760.0, 740.0, 720.0].iter().enumerate() {
        for (c, x) in [72.0, 200.0, 330.0].iter().enumerate() {
            frags.push(frag(&format!("w{r}{c}"), *x, *y));
        }
    }
    frags.push(frag("ORPHAN", 150.0, 730.0)); // in-bbox, between columns

    let elements = Partitioner::new(PartitionConfig::default())
        .partition_fragments(&frags, 0, 842.0);
    let all: String = elements.iter().map(|e| e.display_text()).collect::<Vec<_>>().join(" ");
    assert!(all.contains("ORPHAN"), "interior prose token was dropped: {all}");
}
```

Run: `cargo test --test issue_375_text_conservation_test -- --nocapture`
Expected: FAIL on `spatial_table_does_not_drop_interior_prose` (ORPHAN claimed by bbox, never celled, never re-classified).

- [ ] **Step 4: Commit the RED test**

```bash
git add oxidize-pdf-core/tests/issue_375_text_conservation_test.rs
git commit -m "test(#375): failing text-conservation invariant reproduces table data loss"
```

---

## Task 2: Claim only captured text — ruling path

**Files:**
- Modify: `oxidize-pdf-core/src/pipeline/partition.rs:348-357` (ruling claim loop)

**Interfaces:**
- Consumes: `DetectedTable { cells: Vec<TableCell>, .. }` where each `TableCell` has a `bbox` with `contains_point(x, y)`.
- Produces: unchanged public surface; only claiming behavior changes.

- [ ] **Step 1: Replace bbox-membership claiming with cell-membership claiming**

Current (lines 348–357) claims every fragment inside the table bbox. Replace the inner loop body so a fragment is claimed only if its center lands inside an actual cell of this table:

```rust
                                // #375: claim only fragments actually placed in a
                                // cell. Fragments inside the table bbox but in no
                                // cell (gaps, borders, normalization misses) stay
                                // unclaimed and fall through to prose classification.
                                for (i, f) in fragments.iter().enumerate() {
                                    if claimed[i] {
                                        continue;
                                    }
                                    let cx = f.x + f.width / 2.0;
                                    let cy = f.y + f.height / 2.0;
                                    if table.cells.iter().any(|cell| cell.bbox.contains_point(cx, cy)) {
                                        claimed[i] = true;
                                    }
                                }
```

(Delete the old `let (rx, ry) = ...; let (rr, rt) = ...;` bbox-corner computation above it — it is now unused; clippy will flag it.)

- [ ] **Step 2: Build and lint**

Run: `cargo clippy --lib --all-features -- -D warnings`
Expected: clean (no unused-variable warning for the removed bbox corners).

- [ ] **Step 3: Run the ruling regression tests**

Run: `cargo test --test ruling_table_partition_test`
Expected: PASS — genuine bordered tables still capture their cells (fragments are inside cells, so still claimed).

- [ ] **Step 4: Commit**

```bash
git add oxidize-pdf-core/src/pipeline/partition.rs
git commit -m "fix(#375): ruling tables claim only celled fragments, not whole bbox"
```

---

## Task 3: Claim only captured text — spatial path

**Files:**
- Modify: `oxidize-pdf-core/src/pipeline/partition.rs:419-428` (spatial claim loop)

**Interfaces:**
- Consumes: spatial `Table { rows: Vec<Row>, .. }`, `Row { cells: Vec<Cell>, .. }`, `Cell { text, bounding_box, .. }` where `bounding_box` has `contains(x, y)` (structured `BoundingBox`).

- [ ] **Step 1: Replace the spatial claim loop**

Replace lines 419–428 so a fragment is claimed only if its center is inside a non-empty cell of the detected table:

```rust
                            // #375: claim only fragments inside a populated cell.
                            for (i, f) in fragments.iter().enumerate() {
                                if claimed[i] {
                                    continue;
                                }
                                let cx = f.x + f.width / 2.0;
                                let cy = f.y + f.height / 2.0;
                                let in_cell = table.rows.iter().flat_map(|r| &r.cells).any(|c| {
                                    !c.is_empty() && c.bounding_box.contains(cx, cy)
                                });
                                if in_cell {
                                    claimed[i] = true;
                                }
                            }
```

If structured `BoundingBox` exposes containment under a different name, confirm with:
Run: `grep -n "fn contains" oxidize-pdf-core/src/text/structured/types.rs`
and use that method name.

- [ ] **Step 2: Lint + spatial regression**

Run: `cargo clippy --lib --all-features -- -D warnings && cargo test --test partition_table_detection_test`
Expected: PASS.

- [ ] **Step 3: Run the conservation invariant from Task 1**

Run: `cargo test --test issue_375_text_conservation_test -- --nocapture`
Expected: the `spatial_...` synthetic test now PASSES (ORPHAN survives). The full-fixture test may still fail if the ruling path or degenerate tables contribute — proceed to Task 4.

- [ ] **Step 4: Commit**

```bash
git add oxidize-pdf-core/src/pipeline/partition.rs
git commit -m "fix(#375): spatial tables claim only populated-cell fragments"
```

---

## Task 4: Reject degenerate tables (decompose to prose)

**Files:**
- Modify: `oxidize-pdf-core/src/pipeline/partition.rs` (ruling accept gate ~324; spatial accept gate ~392)

**Interfaces:**
- A table that placed almost no text is not emitted at all; its fragments remain unclaimed → prose.

- [ ] **Step 1: Add a degeneracy guard next to each confidence gate**

Ruling path — after the `if table.confidence < ... { continue; }` at ~324, add:

```rust
                                // #375: a table with fewer than 2 populated cells is not a
                                // real table — skip it so its fragments become prose.
                                // (Softened from `populated < rows` after Task 4 review found
                                // that threshold over-rejected legitimately sparse narrow tables;
                                // human-approved 2026-07-09.)
                                let populated = table.cells.iter().filter(|c| !c.text.is_empty()).count();
                                if populated < 2 {
                                    continue;
                                }
```

Spatial path — after its confidence gate at ~392, add:

```rust
                            // #375: same near-empty guard for the spatial path.
                            let populated = table.rows.iter().flat_map(|r| &r.cells)
                                .filter(|c| !c.is_empty()).count();
                            if populated < 2 {
                                continue;
                            }
```

- [ ] **Step 2: Run the full conservation invariant**

Run: `cargo test --test issue_375_text_conservation_test -- --nocapture`
Expected: PASS (both tests). If the full-fixture test still shows missing tokens, inspect which elements swallowed them (`--nocapture` prints the sample) and tighten the guard; do not raise the slack above 1%.

- [ ] **Step 3: Corpus sanity — no regression on real tables**

Run: `cargo test --test table_integration_test --test ruling_table_partition_test --test advanced_tables_tests`
Expected: PASS — real tables still detected.

- [ ] **Step 4: Commit**

```bash
git add oxidize-pdf-core/src/pipeline/partition.rs
git commit -m "fix(#375): degenerate tables decompose to prose instead of eating text"
```

---

## Task 5: Additive rich table model (`TableStructure`, `RichCell`, `structure` field)

**Files:**
- Modify: `oxidize-pdf-core/src/pipeline/element.rs` (add types near `TableElementData`, line 215; add field; add constructor)
- Modify: `oxidize-pdf-core/src/pipeline/partition.rs` (two `TableElementData { rows, metadata }` literals at ~334 and ~409 → add `structure: None`)

**Interfaces:**
- Produces:
  - `struct RichCell { pub row: usize, pub col: usize, pub row_span: usize, pub col_span: usize, pub text: String, pub is_header: bool }`
  - `struct TableStructure { pub cells: Vec<RichCell>, pub num_rows: usize, pub num_cols: usize, pub header_rows: usize }`
  - `TableElementData { pub rows, pub structure: Option<TableStructure>, pub metadata }`
  - `TableElementData::from_structure(structure: TableStructure, metadata: ElementMetadata) -> Self` — fills `rows` from the structure by expanding spans (repeating each spanning cell's text across every covered `(row, col)`), so the flat view is derived from the rich one.

- [ ] **Step 1: Write the failing test**

```rust
// tests/issue_375_merged_cells_test.rs  (create; more tests added in Task 7/9)
use oxidize_pdf::pipeline::{ElementMetadata, RichCell, TableElementData, TableStructure};

#[test]
fn from_structure_expands_spans_into_flat_rows() {
    // 2x2 grid; top row is a single cell spanning both columns (a merged header).
    let structure = TableStructure {
        num_rows: 2,
        num_cols: 2,
        header_rows: 1,
        cells: vec![
            RichCell { row: 0, col: 0, row_span: 1, col_span: 2, text: "Region".into(), is_header: true },
            RichCell { row: 1, col: 0, row_span: 1, col_span: 1, text: "Q1".into(), is_header: false },
            RichCell { row: 1, col: 1, row_span: 1, col_span: 1, text: "Q2".into(), is_header: false },
        ],
    };
    let data = TableElementData::from_structure(structure, ElementMetadata::default());
    // Flat view repeats the spanned header value across both covered columns.
    assert_eq!(data.rows, vec![
        vec!["Region".to_string(), "Region".to_string()],
        vec!["Q1".to_string(), "Q2".to_string()],
    ]);
    assert_eq!(data.structure.as_ref().unwrap().header_rows, 1);
}
```

- [ ] **Step 2: Run it — FAIL to compile (types don't exist)**

Run: `cargo test --test issue_375_merged_cells_test`
Expected: FAIL — unresolved `RichCell`/`TableStructure`/`from_structure`.

- [ ] **Step 3: Add the types, field, and constructor**

In `src/pipeline/element.rs`, above `TableElementData` (line 215):

```rust
/// One cell of a table's rich structure. `row`/`col` are the cell's top-left
/// position in the base grid; `row_span`/`col_span` are >= 1.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "semantic", derive(Serialize, Deserialize))]
pub struct RichCell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub text: String,
    pub is_header: bool,
}

/// Rich table structure: merged cells and header rows. Present only when a hard
/// signal (drawn grid / structure tags) revealed it; borderless tables leave it
/// `None` and use the flat `rows` view only.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "semantic", derive(Serialize, Deserialize))]
pub struct TableStructure {
    pub cells: Vec<RichCell>,
    pub num_rows: usize,
    pub num_cols: usize,
    /// Number of leading rows that are headers (0 = none, 1 = single header row,
    /// >1 = multi-level header expressed as header rows + merged cells).
    pub header_rows: usize,
}
```

Change `TableElementData` (line 218) to add the field:

```rust
pub struct TableElementData {
    /// Row-major flat cell data. Each inner Vec is one row. When `structure` is
    /// present this is its expanded (span-repeated) view; otherwise it is the
    /// primary representation.
    pub rows: Vec<Vec<String>>,
    /// Rich structure (merged cells / header rows) when a hard signal revealed it.
    pub structure: Option<TableStructure>,
    pub metadata: ElementMetadata,
}
```

Add the constructor in an `impl TableElementData` block (create one right after the struct):

```rust
impl TableElementData {
    /// Build from rich structure, deriving the flat `rows` view by expanding each
    /// spanning cell's text across every covered (row, col).
    pub fn from_structure(structure: TableStructure, metadata: ElementMetadata) -> Self {
        let mut rows = vec![vec![String::new(); structure.num_cols]; structure.num_rows];
        for cell in &structure.cells {
            for r in cell.row..(cell.row + cell.row_span).min(structure.num_rows) {
                for c in cell.col..(cell.col + cell.col_span).min(structure.num_cols) {
                    rows[r][c] = cell.text.clone();
                }
            }
        }
        Self { rows, structure: Some(structure), metadata }
    }
}
```

- [ ] **Step 4: Export the new types**

In the module's public re-exports (same place `TableElementData` is exported — check `src/pipeline/mod.rs`), add `RichCell, TableStructure`:
Run: `grep -n "TableElementData" oxidize-pdf-core/src/pipeline/mod.rs`
Then add `RichCell, TableStructure` to that `pub use` list.

- [ ] **Step 5: Fix the two struct literals in partition.rs**

At `partition.rs` ~334 and ~409, the `Element::Table(TableElementData { rows, metadata: ... })` literals must add `structure: None,`:

```rust
                                elements.push(Element::Table(TableElementData {
                                    rows,
                                    structure: None,
                                    metadata: ElementMetadata { /* unchanged */ ..Default::default() },
                                }));
```

Also fix any other construction sites the compiler reports:
Run: `cargo build --lib 2>&1 | grep -n "TableElementData"`
Add `structure: None,` to each.

- [ ] **Step 6: Run test + build**

Run: `cargo test --test issue_375_merged_cells_test && cargo clippy --lib --all-features -- -D warnings`
Expected: PASS + clean.

- [ ] **Step 7: Commit**

```bash
git add oxidize-pdf-core/src/pipeline/element.rs oxidize-pdf-core/src/pipeline/mod.rs oxidize-pdf-core/src/pipeline/partition.rs oxidize-pdf-core/tests/issue_375_merged_cells_test.rs
git commit -m "feat(#375): additive rich table model (TableStructure/RichCell), flat rows derived"
```

---

## Task 6: Add span fields to the ruling detector's cell (additive, no behavior change)

**Files:**
- Modify: `oxidize-pdf-core/src/text/table_detection.rs:161-184` (`TableCell` struct + `new`)
- Modify: `oxidize-pdf-core/src/text/table_detection.rs:93-119` (`DetectedTable` gains `header_rows`)

**Interfaces:**
- Produces: `TableCell { .., pub row_span: usize, pub col_span: usize }` (default 1); `DetectedTable { .., pub header_rows: usize }` (default 0). Existing `DetectedTable::new` keeps its signature and sets `header_rows: 0`.

- [ ] **Step 1: Add fields, defaulting to the current (no-span) behavior**

`TableCell` — add fields and default them in `new`:

```rust
pub struct TableCell {
    pub row: usize,
    pub column: usize,
    pub bbox: BoundingBox,
    pub text: String,
    pub has_borders: bool,
    /// Number of base rows this cell spans (>= 1).
    pub row_span: usize,
    /// Number of base columns this cell spans (>= 1).
    pub col_span: usize,
}
```
```rust
    pub fn new(row: usize, column: usize, bbox: BoundingBox) -> Self {
        Self { row, column, bbox, text: String::new(), has_borders: false, row_span: 1, col_span: 1 }
    }
```

`DetectedTable` — add `pub header_rows: usize` and set it to `0` in `new` (keep `new`'s signature unchanged):

```rust
pub struct DetectedTable {
    pub bbox: BoundingBox,
    pub cells: Vec<TableCell>,
    pub rows: usize,
    pub columns: usize,
    pub confidence: f64,
    /// Leading header rows (0 until header detection runs; Task 8).
    pub header_rows: usize,
}
```
```rust
    pub fn new(bbox: BoundingBox, cells: Vec<TableCell>, rows: usize, columns: usize) -> Self {
        let confidence = Self::calculate_confidence(&cells, rows, columns);
        Self { bbox, cells, rows, columns, confidence, header_rows: 0 }
    }
```

- [ ] **Step 2: Fix any other `TableCell`/`DetectedTable` literals**

Run: `cargo build --lib 2>&1 | grep -nE "TableCell|DetectedTable"`
Update each literal to include the new fields (spans `1`, `header_rows` `0`).

- [ ] **Step 3: Existing detector tests unchanged**

Run: `cargo test --lib text::table_detection && cargo test --test table_integration_test`
Expected: PASS, byte-identical behavior (spans all 1).

- [ ] **Step 4: Commit**

```bash
git add oxidize-pdf-core/src/text/table_detection.rs
git commit -m "feat(#375): additive span + header_rows fields on ruling detector (default no-op)"
```

---

## Task 7: Detect merged cells from drawn grid (retain divider segments) — PRIMARY RISK

**Files:**
- Modify: `oxidize-pdf-core/src/text/table_detection.rs` — `GridPattern` (line 514) retains segments; new `merge_cells_across_absent_dividers`; call it inside `detect_bordered_table` (line 318) after `create_cells_from_grid`.
- Create: `oxidize-pdf-core/tests/issue_375_merged_cells_detect_test.rs`

**Interfaces:**
- Consumes: `VectorLine { x1, y1, x2, y2, .. }` from `graphics.horizontal_lines()/vertical_lines()`; grid `rows: Vec<f64>` (Y gridlines), `columns: Vec<f64>` (X gridlines).
- Produces: `TableCell`s where merged regions appear once at their top-left with `row_span`/`col_span` > 1, and covered interior positions are omitted. `get_cell` still indexes the base grid; callers that need spans read `cells` directly (Task 9 does).

**Algorithm (deterministic):** two base cells are in the same merged region when the divider segment on their shared edge is *absent*. (a) A vertical divider between base columns `c|c+1` across base row `r` is present if some vertical `VectorLine` at `x ≈ columns[c+1]` covers the row's Y-range `[rows[r], rows[r+1]]` (within `alignment_tolerance`). (b) A horizontal divider between base rows `r|r+1` across base column `c` is present if some horizontal `VectorLine` at `y ≈ rows[r+1]` covers `[columns[c], columns[c+1]]`. Build merged regions by union-find over base cells connected by *absent* dividers; each region becomes one `TableCell` at its min-row/min-col with spans = extent, `has_borders: true`.

- [ ] **Step 1: Write the failing test (fixture with one merged header cell)**

```rust
// tests/issue_375_merged_cells_detect_test.rs
//
// Build a bordered 2-col table whose top row is a single cell spanning both
// columns (the vertical divider is omitted only in row 0). Detection must yield
// a top-left cell with col_span == 2.

use oxidize_pdf::graphics::extraction::{ExtractedGraphics, VectorLine};
use oxidize_pdf::text::extraction::TextFragment;
use oxidize_pdf::text::table_detection::TableDetector;

fn hline(x1: f64, x2: f64, y: f64) -> VectorLine { VectorLine { x1, y1: y, x2, y2: y } }
fn vline(x: f64, y1: f64, y2: f64) -> VectorLine { VectorLine { x1: x, y1, x2: x, y2 } }
fn frag(t: &str, x: f64, y: f64) -> TextFragment {
    TextFragment { text: t.into(), x, y, width: 30.0, height: 10.0, font_size: 10.0,
        font_name: None, is_bold: false, is_italic: false, color: None,
        space_decisions: Vec::new(), mcid: None, struct_tag: None }
}

#[test]
fn merged_header_cell_detected_with_col_span_2() {
    // Grid X at 100,200,300 ; Y at 100 (bottom),150 (mid),200 (top).
    // Row 0 (top band, y 150..200) has NO middle vertical => one spanning cell.
    // Row 1 (y 100..150) HAS the middle vertical => two cells.
    let h = vec![hline(100.0, 300.0, 100.0), hline(100.0, 300.0, 150.0), hline(100.0, 300.0, 200.0)];
    let v = vec![
        vline(100.0, 100.0, 200.0),           // left border (full height)
        vline(300.0, 100.0, 200.0),           // right border (full height)
        vline(200.0, 100.0, 150.0),           // MIDDLE divider only in row 1
    ];
    let graphics = ExtractedGraphics::from_lines(h, v); // see Step 3 if constructor differs
    let frags = vec![
        frag("Header", 150.0, 170.0),
        frag("A", 130.0, 120.0), frag("B", 230.0, 120.0),
    ];

    let det = TableDetector::default();
    let tables = det.detect(&graphics, &frags).expect("detect");
    let table = tables.first().expect("one table");
    let top_left = table.cells.iter().find(|c| c.row == 0 && c.column == 0).expect("cell 0,0");
    assert_eq!(top_left.col_span, 2, "merged header should span 2 columns");
    // The non-merged row keeps two single cells.
    assert!(table.cells.iter().any(|c| c.row == 1 && c.column == 0 && c.col_span == 1));
    assert!(table.cells.iter().any(|c| c.row == 1 && c.column == 1 && c.col_span == 1));
}
```

- [ ] **Step 2: Run — FAIL**

Run: `cargo test --test issue_375_merged_cells_detect_test`
Expected: FAIL — either `from_lines` missing (fix in Step 3) or `col_span == 1` (merge not yet implemented).

- [ ] **Step 3: Confirm the graphics test constructor**

Run: `grep -nE "pub fn |pub struct (ExtractedGraphics|VectorLine)|struct VectorLine" oxidize-pdf-core/src/graphics/extraction.rs | head -40`
- If there is no public `ExtractedGraphics::from_lines`, add a small test-only constructor next to `ExtractedGraphics` that stores the given H/V lines and reports `has_table_structure()` true when both have ≥2 (mirroring the real fields). If `VectorLine` has fields beyond `x1,y1,x2,y2`, extend the helpers to fill them (defaults). Adjust the test's helpers accordingly. Keep the constructor `#[cfg(any(test, feature = "test-helpers"))]` if the codebase gates test helpers, else `pub`.

- [ ] **Step 4: Implement grid-segment retention**

Change `GridPattern` to also hold the raw segments, and populate it in `detect_grid_pattern`:

```rust
struct GridPattern {
    rows: Vec<f64>,
    columns: Vec<f64>,
    h_segments: Vec<(f64, f64, f64)>, // (y, x_start, x_end)
    v_segments: Vec<(f64, f64, f64)>, // (x, y_start, y_end)
}
```
In `detect_grid_pattern`, before returning, collect normalized segments from `h_lines`/`v_lines` (`(line.y1, line.x1.min(line.x2), line.x1.max(line.x2))` for horizontals; symmetric for verticals).

- [ ] **Step 5: Implement `merge_cells_across_absent_dividers`**

Add the method and call it in `detect_bordered_table` right after `create_cells_from_grid` (before `assign_text_to_cells`, so text lands in merged cells):

```rust
fn divider_present_vertical(&self, grid: &GridPattern, x: f64, y0: f64, y1: f64) -> bool {
    let (lo, hi) = (y0.min(y1), y0.max(y1));
    grid.v_segments.iter().any(|&(sx, s0, s1)| {
        (sx - x).abs() <= self.config.alignment_tolerance
            && s0 <= lo + self.config.alignment_tolerance
            && s1 >= hi - self.config.alignment_tolerance
    })
}
// symmetric divider_present_horizontal(grid, y, x0, x1)

fn merge_cells_across_absent_dividers(&self, grid: &GridPattern, cells: Vec<TableCell>) -> Vec<TableCell> {
    let num_rows = grid.rows.len().saturating_sub(1);
    let num_cols = grid.columns.len().saturating_sub(1);
    if num_rows == 0 || num_cols == 0 { return cells; }

    // Union-find over base cells (index = r*num_cols + c).
    let mut parent: Vec<usize> = (0..num_rows * num_cols).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        if p[i] != i { let r = find(p, p[i]); p[i] = r; } p[i]
    }
    for r in 0..num_rows {
        for c in 0..num_cols {
            // merge right if the vertical divider at columns[c+1] is absent over this row band
            if c + 1 < num_cols {
                let x = grid.columns[c + 1];
                if !self.divider_present_vertical(grid, x, grid.rows[r], grid.rows[r + 1]) {
                    let (a, b) = (find(&mut parent, r * num_cols + c), find(&mut parent, r * num_cols + c + 1));
                    parent[a] = b;
                }
            }
            // merge down if the horizontal divider at rows[r+1] is absent over this column band
            if r + 1 < num_rows {
                let y = grid.rows[r + 1];
                if !self.divider_present_horizontal(grid, y, grid.columns[c], grid.columns[c + 1]) {
                    let (a, b) = (find(&mut parent, r * num_cols + c), find(&mut parent, (r + 1) * num_cols + c));
                    parent[a] = b;
                }
            }
        }
    }

    // Group base cells by root; each group -> one merged TableCell at its min row/col.
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<usize, Vec<&TableCell>> = BTreeMap::new();
    for cell in &cells {
        let root = find(&mut parent, cell.row * num_cols + cell.column);
        groups.entry(root).or_default().push(cell);
    }
    let mut merged = Vec::new();
    for (_root, group) in groups {
        let min_r = group.iter().map(|c| c.row).min().unwrap();
        let min_c = group.iter().map(|c| c.column).min().unwrap();
        let max_r = group.iter().map(|c| c.row).max().unwrap();
        let max_c = group.iter().map(|c| c.column).max().unwrap();
        // Merged bbox = union of member bboxes (use grid extents for determinism).
        let x = grid.columns[min_c];
        let y = grid.rows[min_r].min(grid.rows[max_r + 1]);
        let w = (grid.columns[max_c + 1] - grid.columns[min_c]).abs();
        let hgt = (grid.rows[max_r + 1] - grid.rows[min_r]).abs();
        let mut cell = TableCell::new(min_r, min_c, BoundingBox::new(x, y, w, hgt));
        cell.has_borders = true;
        cell.row_span = max_r - min_r + 1;
        cell.col_span = max_c - min_c + 1;
        merged.push(cell);
    }
    merged.sort_by(|a, b| (a.row, a.column).cmp(&(b.row, b.column)));
    merged
}
```

Wire into `detect_bordered_table`:

```rust
        let cells = self.create_cells_from_grid(&grid);
        let cells = self.merge_cells_across_absent_dividers(&grid, cells); // #375
        let cells_with_text = self.assign_text_to_cells(cells, text_fragments);
```

Note: `DetectedTable.rows`/`columns` remain the **base** grid dimensions; merged cells carry spans. `get_cell`'s flat index no longer matches merged cells — that's acceptable because Task 9 reads `cells` directly; leave `get_cell` as-is for base-grid callers.

- [ ] **Step 6: Iterate against the test**

Run: `cargo test --test issue_375_merged_cells_detect_test -- --nocapture`
Expected: PASS. Adjust tolerance handling until the merged region is exactly col_span 2 in row 0 and single cells in row 1. Then confirm no regression on full grids:
Run: `cargo test --lib text::table_detection && cargo test --test table_integration_test --test ruling_table_partition_test`
Expected: PASS (a fully-ruled table has all dividers present → every region is 1×1 → identical to before).

- [ ] **Step 7: Commit**

```bash
git add oxidize-pdf-core/src/text/table_detection.rs oxidize-pdf-core/tests/issue_375_merged_cells_detect_test.rs
git commit -m "feat(#375): detect merged cells from absent grid dividers (union-find over base cells)"
```

---

## Task 8: Identify header rows (from tags, else top row)

**Files:**
- Modify: `oxidize-pdf-core/src/text/table_detection.rs` — set `DetectedTable.header_rows` in `detect_bordered_table` after text assignment.

**Interfaces:**
- Consumes: `TextFragment.struct_tag: Option<String>` (a marked-content/structure tag name; header cells commonly tagged `"TH"` or containing `"Header"`).
- Produces: `header_rows` = count of leading rows whose fragments are header-tagged; falls back to `1` (top row) when the table is non-trivial and no tags are present.

- [ ] **Step 1: Write the failing test**

```rust
// append to tests/issue_375_merged_cells_detect_test.rs
#[test]
fn header_row_detected_from_th_tag() {
    let h = vec![hline(100.0,300.0,100.0), hline(100.0,300.0,150.0), hline(100.0,300.0,200.0)];
    let v = vec![vline(100.0,100.0,200.0), vline(200.0,100.0,200.0), vline(300.0,100.0,200.0)];
    let graphics = ExtractedGraphics::from_lines(h, v);
    let mut th = |t: &str, x: f64, y: f64| {
        let mut f = frag(t, x, y); f.struct_tag = Some("TH".to_string()); f
    };
    let frags = vec![
        th("H1", 130.0, 170.0), th("H2", 230.0, 170.0), // header row
        frag("a", 130.0, 120.0), frag("b", 230.0, 120.0),
    ];
    let tables = TableDetector::default().detect(&graphics, &frags).expect("detect");
    assert_eq!(tables.first().unwrap().header_rows, 1);
}
```

- [ ] **Step 2: Run — FAIL** (`header_rows` is 0)

Run: `cargo test --test issue_375_merged_cells_detect_test header_row_detected_from_th_tag`
Expected: FAIL.

- [ ] **Step 3: Implement header-row counting**

After `assign_text_to_cells` in `detect_bordered_table`, before building `DetectedTable`, compute header rows and store it. Since `DetectedTable::new` sets `header_rows: 0`, set the field after construction:

```rust
        let mut table = DetectedTable::new(bbox, cells_with_text, num_rows, num_cols);
        table.header_rows = Self::count_header_rows(&table, text_fragments);
        Ok(Some(table))
```

Add helper — a row is a header row if any fragment inside its cells is header-tagged; count only the leading contiguous header rows; if none tagged and the table has ≥2 rows, default to 1:

```rust
fn count_header_rows(table: &DetectedTable, fragments: &[TextFragment]) -> usize {
    fn is_header_tag(tag: &str) -> bool {
        let t = tag.to_ascii_uppercase();
        t == "TH" || t.contains("HEADER")
    }
    let mut tagged_leading = 0usize;
    for r in 0..table.rows {
        let row_cells: Vec<&TableCell> = table.cells.iter()
            .filter(|c| c.row <= r && r < c.row + c.row_span).collect();
        let has_header = fragments.iter().any(|f| {
            f.struct_tag.as_deref().map(is_header_tag).unwrap_or(false)
                && row_cells.iter().any(|c| c.bbox.contains_point(f.x + f.width/2.0, f.y + f.height/2.0))
        });
        if has_header { tagged_leading = r + 1; } else { break; }
    }
    if tagged_leading > 0 { tagged_leading }
    else if table.rows >= 2 { 1 } else { 0 }
}
```

- [ ] **Step 4: Run — PASS + regression**

Run: `cargo test --test issue_375_merged_cells_detect_test && cargo test --lib text::table_detection`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxidize-pdf-core/src/text/table_detection.rs oxidize-pdf-core/tests/issue_375_merged_cells_detect_test.rs
git commit -m "feat(#375): count header rows from structure tags, default to top row"
```

---

## Task 9: Build rich `TableElementData` in the ruling partition path

**Files:**
- Modify: `oxidize-pdf-core/src/pipeline/partition.rs` — replace `ruling_table_to_rows(table)` usage at ~327/334 with a rich builder that produces `TableStructure`; spatial path keeps `structure: None`.

**Interfaces:**
- Consumes: `DetectedTable { cells (with spans), rows, columns, header_rows, .. }`.
- Produces: `Element::Table(TableElementData::from_structure(structure, metadata))` where `structure.cells` mirror the detector's merged cells.

- [ ] **Step 1: Write the failing test**

```rust
// tests/issue_375_multi_level_header_test.rs
use oxidize_pdf::graphics::extraction::{ExtractedGraphics, VectorLine};
use oxidize_pdf::pipeline::{Element, PartitionConfig, Partitioner};
use oxidize_pdf::text::extraction::TextFragment;

// (reuse hline/vline/frag helpers — copy the small helpers from the detect test)

#[test]
fn ruling_partition_emits_rich_structure_with_merged_header() {
    let h = vec![/* same 3 h-lines as the merged-header fixture */];
    let v = vec![/* left, right full; middle only in row 1 */];
    let graphics = ExtractedGraphics::from_lines(h, v);
    let frags = vec![/* "Region" spanning header row, "Q1"/"Q2" below */];

    let elements = Partitioner::new(PartitionConfig::default())
        .partition_fragments_with_graphics(&frags, Some(&graphics), 0, 842.0);
    let table = elements.iter().find_map(|e| match e { Element::Table(t) => Some(t), _ => None })
        .expect("a table element");
    let st = table.structure.as_ref().expect("rich structure present");
    assert!(st.cells.iter().any(|c| c.row == 0 && c.col == 0 && c.col_span == 2 && c.is_header));
    assert_eq!(st.header_rows, 1);
    // Flat view still complete (spanned value repeated).
    assert_eq!(table.rows[0].len(), 2);
}
```

- [ ] **Step 2: Run — FAIL** (`structure` is None today)

Run: `cargo test --test issue_375_multi_level_header_test`
Expected: FAIL.

- [ ] **Step 3: Add the rich builder and use it in the ruling path**

Replace `ruling_table_to_rows` (partition.rs:761) with a structure builder (keep the name or add a new fn; update the call site):

```rust
/// Convert a ruling-detected table (with merged cells + header rows) into a
/// rich `TableStructure`. The flat `rows` view is derived by `from_structure`.
fn ruling_table_to_structure(table: &crate::text::table_detection::DetectedTable) -> TableStructure {
    let header_rows = table.header_rows;
    let cells = table.cells.iter().map(|c| RichCell {
        row: c.row,
        col: c.column,
        row_span: c.row_span.max(1),
        col_span: c.col_span.max(1),
        text: c.text.clone(),
        is_header: c.row < header_rows,
    }).collect();
    TableStructure { cells, num_rows: table.rows, num_cols: table.columns, header_rows }
}
```

At the ruling emit site (~327–342), replace:

```rust
                                let structure = ruling_table_to_structure(table);
                                let bbox = ElementBBox::new(
                                    table.bbox.x, table.bbox.y, table.bbox.width, table.bbox.height,
                                );
                                let mut data = TableElementData::from_structure(
                                    structure,
                                    ElementMetadata {
                                        page,
                                        bbox,
                                        confidence: table.confidence,
                                        ..Default::default()
                                    },
                                );
                                // keep metadata bbox already set above
                                elements.push(Element::Table(std::mem::take(&mut data).into_table()));
```

If wrapping helpers feel awkward, simpler: build `data` then `elements.push(Element::Table(data));` — `from_structure` already returns `TableElementData`. Use:

```rust
                                elements.push(Element::Table(TableElementData::from_structure(
                                    ruling_table_to_structure(table),
                                    ElementMetadata { page, bbox, confidence: table.confidence, ..Default::default() },
                                )));
```

Import `RichCell, TableStructure` at the top of partition.rs (extend the `use crate::pipeline::{...}` list on line 3–5). Remove the now-unused `ruling_table_to_rows` if nothing else calls it (clippy will flag).

Also update the Task 2 ruling claim loop: it references `table.cells` bboxes — still correct (merged cell bboxes cover their region), no change needed.

- [ ] **Step 4: Run — PASS + conservation still green**

Run: `cargo test --test issue_375_multi_level_header_test --test issue_375_text_conservation_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxidize-pdf-core/src/pipeline/partition.rs oxidize-pdf-core/tests/issue_375_multi_level_header_test.rs
git commit -m "feat(#375): ruling path emits rich TableStructure (merged cells + header rows)"
```

---

## Task 10: Markdown degradation — collapse multi-level headers, keep repeated spans

**Files:**
- Modify: `oxidize-pdf-core/src/pipeline/export.rs:54,66-78` — `table_to_markdown` becomes structure-aware.

**Interfaces:**
- Consumes: `TableElementData { rows, structure }`.
- Rendering: when `structure.header_rows > 1`, collapse the first `header_rows` rows into ONE GFM header, joining each column's header texts top-to-bottom with `" › "` (dedup consecutive equal). Body = remaining rows. Merged-cell values are already repeated in `rows`, so body rendering is unchanged. When `structure` absent or `header_rows <= 1`, keep current behavior.

- [ ] **Step 1: Write the failing test**

```rust
// tests/issue_375_multi_level_header_test.rs  (append)
use oxidize_pdf::pipeline::{ElementMetadata, RichCell, TableElementData, TableStructure};
use oxidize_pdf::pipeline::export::element_to_markdown_for_test; // see Step 3 for exact entry point

#[test]
fn multi_level_header_flattens_with_separator() {
    // 3 header-ish rows collapsed: "Region" (span2) over ["Q1","Q2"].
    let structure = TableStructure {
        num_rows: 3, num_cols: 2, header_rows: 2,
        cells: vec![
            RichCell{row:0,col:0,row_span:1,col_span:2,text:"Region".into(),is_header:true},
            RichCell{row:1,col:0,row_span:1,col_span:1,text:"Q1".into(),is_header:true},
            RichCell{row:1,col:1,row_span:1,col_span:1,text:"Q2".into(),is_header:true},
            RichCell{row:2,col:0,row_span:1,col_span:1,text:"10".into(),is_header:false},
            RichCell{row:2,col:1,row_span:1,col_span:1,text:"20".into(),is_header:false},
        ],
    };
    let data = TableElementData::from_structure(structure, ElementMetadata::default());
    let md = oxidize_pdf::pipeline::export::table_to_markdown_data(&data); // exact name per Step 3
    assert_eq!(md, "| Region › Q1 | Region › Q2 |\n| --- | --- |\n| 10 | 20 |");
}
```

- [ ] **Step 2: Run — FAIL**

Run: `cargo test --test issue_375_multi_level_header_test multi_level_header_flattens_with_separator`
Expected: FAIL (function missing / current flat behavior).

- [ ] **Step 3: Implement structure-aware export**

Add a `pub(crate)` (or `pub`) `table_to_markdown_data(&TableElementData) -> String` and route `Element::Table` through it; keep `table_to_markdown(rows)` for the no-structure case:

```rust
pub fn table_to_markdown_data(data: &TableElementData) -> String {
    match &data.structure {
        Some(st) if st.header_rows > 1 && !data.rows.is_empty() => {
            let ncols = data.rows[0].len();
            let mut header = Vec::with_capacity(ncols);
            for c in 0..ncols {
                let mut parts: Vec<&str> = Vec::new();
                for r in 0..st.header_rows.min(data.rows.len()) {
                    let cell = data.rows[r].get(c).map(|s| s.as_str()).unwrap_or("");
                    if parts.last() != Some(&cell) && !cell.is_empty() {
                        parts.push(cell);
                    }
                }
                header.push(parts.join(" › "));
            }
            let mut lines = vec![
                format!("| {} |", header.join(" | ")),
                format!("| {} |", vec!["---"; ncols].join(" | ")),
            ];
            for row in &data.rows[st.header_rows.min(data.rows.len())..] {
                lines.push(format!("| {} |", row.join(" | ")));
            }
            lines.join("\n")
        }
        _ => table_to_markdown(&data.rows), // single/no header: unchanged
    }
}
```

Change the arm at line 54:
```rust
            Element::Table(t) => Some(table_to_markdown_data(t)),
```

- [ ] **Step 4: Run — PASS + existing export tests**

Run: `cargo test --test issue_375_multi_level_header_test && cargo test --lib pipeline::export && cargo test --test '*export*' 2>/dev/null; cargo test --lib export`
Expected: PASS; single-header tables unchanged.

- [ ] **Step 5: Commit**

```bash
git add oxidize-pdf-core/src/pipeline/export.rs oxidize-pdf-core/tests/issue_375_multi_level_header_test.rs
git commit -m "feat(#375): GFM export collapses multi-level headers, repeats merged spans"
```

---

## Task 11: RAG metadata from the rich model

**Files:**
- Modify: `oxidize-pdf-core/src/pipeline/chunk_metadata.rs:262-277` (`table_dims`)

**Interfaces:**
- `table_dims` reports the rich `(num_rows, num_cols)` when `structure` is present (true geometry), else the flat `rows` dims.

- [ ] **Step 1: Write the failing test**

```rust
// tests/issue_375_multi_level_header_test.rs  (append) — or a chunk_metadata unit test
#[test]
fn table_dims_prefer_rich_structure() {
    use oxidize_pdf::pipeline::chunk_metadata::ChunkMetadata; // if table_dims is private, test via ChunkMetadata
    // Build a chunk whose only element is the rich merged table; assert table_cols == 2, table_rows == 3.
    // (Use the same TableElementData as Task 10.)
}
```

If `table_dims` is private (`fn table_dims`), assert through the public `ChunkMetadata` that exposes `table_rows`/`table_cols`. Confirm the constructor path:
Run: `grep -n "pub fn.*ChunkMetadata\|table_dims\|pub struct ChunkMetadata" oxidize-pdf-core/src/pipeline/chunk_metadata.rs`

- [ ] **Step 2: Run — FAIL** (counts come from flat rows, not structure)

- [ ] **Step 3: Implement**

```rust
fn table_dims(elements: &[Element]) -> (Option<usize>, Option<usize>) {
    elements
        .iter()
        .filter_map(|e| match e {
            Element::Table(t) => Some(match &t.structure {
                Some(st) => (st.num_rows, st.num_cols),
                None => (t.rows.len(), t.rows.iter().map(|r| r.len()).max().unwrap_or(0)),
            }),
            _ => None,
        })
        .max_by_key(|(rows, _)| *rows)
        .map(|(r, c)| (Some(r), Some(c)))
        .unwrap_or((None, None))
}
```

- [ ] **Step 4: Run — PASS + metadata regression**

Run: `cargo test --lib pipeline::chunk_metadata && cargo test --test issue_375_multi_level_header_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add oxidize-pdf-core/src/pipeline/chunk_metadata.rs oxidize-pdf-core/tests/issue_375_multi_level_header_test.rs
git commit -m "feat(#375): RAG table metadata counts rich structure geometry"
```

---

## Task 12: Document the known limitation

**Files:**
- Modify: `docs/TABLE_DETECTION_GUIDE.md` (add a "Known limitations" section)
- Modify: `oxidize-pdf-core/src/text/table_detection.rs` (module `//!` header — short note pointing to the guide)

- [ ] **Step 1: Add the guide section**

Append to `docs/TABLE_DETECTION_GUIDE.md`:

```markdown
## Known limitations (as of #375)

Rich table structure — merged cells and multi-level headers — is only produced when a
**hard signal** reveals it:

- **Merged cells** are detected from the drawn grid: where an internal dividing line is
  absent, the adjacent cells are merged. Tables without drawn borders ("borderless") are
  returned as a flat grid with no merged-cell information.
- **Multi-level headers** are represented as header rows containing merged cells (e.g. a
  "Region" header spanning two columns above "Q1"/"Q2"). Header rows are identified from the
  PDF's structure tags when the document is tagged, otherwise the top row is assumed to be
  the header. A nested header *hierarchy* is not read from the PDF's internal structure tree.
- **Borderless-table detection is intrinsically ambiguous** and best-effort; when in doubt
  the content is preserved as prose rather than forced into a grid (no text is dropped).

For GitHub-Flavored Markdown export, merged-cell values are repeated across every column
they cover, and multi-level headers are flattened into a single header row joining the
levels with " › ".
```

- [ ] **Step 2: Add the module note**

Extend the `//!` header of `table_detection.rs` with one line:

```rust
//! Merged cells are detected from absent grid dividers; borderless tables and un-tagged
//! multi-level headers stay flat. See `docs/TABLE_DETECTION_GUIDE.md` for the full limits.
```

- [ ] **Step 3: Build (rustdoc compiles) + commit**

Run: `cargo build --lib`
```bash
git add docs/TABLE_DETECTION_GUIDE.md oxidize-pdf-core/src/text/table_detection.rs
git commit -m "docs(#375): document borderless + un-tagged-header limitations"
```

---

## Task 13: Integration sweep, MSRV, and quality-rust

**Files:** none (verification only) — fixes land in the relevant prior task's files if issues surface.

- [ ] **Step 1: Full library + integration suite**

Run: `cargo test --lib && cargo test --test issue_375_text_conservation_test --test issue_375_merged_cells_test --test issue_375_merged_cells_detect_test --test issue_375_multi_level_header_test --test partition_table_detection_test --test ruling_table_partition_test --test table_integration_test --test advanced_tables_tests`
Expected: all PASS.

- [ ] **Step 2: Corpus text-conservation (real PDFs, no regression)**

Run: `cargo test --test table_extraction_real_pdfs` and the corpus tests `t1`/`t2` per repo convention (see `.private`/CI). Confirm zero character-count regressions vs `develop` on a before/after sweep of a handful of table-bearing fixtures.

- [ ] **Step 3: Lint, format, MSRV**

Run:
```bash
cargo clippy --lib --all-features -- -D warnings
cargo fmt --check
cargo +1.88 build --lib --all-features --locked
```
Expected: all clean.

- [ ] **Step 4: quality-rust pass (mandatory)**

Dispatch the `quality-rust` agent over the #375 diff. Apply verified findings (correctness/security first). Re-run Steps 1 & 3 after any fix.

- [ ] **Step 5: Final commit (if quality-rust produced fixes)**

```bash
git add -A
git commit -m "chore(#375): quality-rust findings applied"
```

---

## Self-review notes

- **Spec coverage:** data-loss (Tasks 1–4), additive rich model (Task 5), merged cells from grid (Tasks 6–7), header rows / multi-level (Tasks 8–9), Markdown degradation (Task 10), RAG metadata (Task 11), documentation (Task 12), verification + MSRV + quality-rust (Task 13). All spec sections covered.
- **Uncertain anchors flagged for the implementer to confirm before coding:** (a) `ExtractedGraphics`/`VectorLine` test constructor (Task 7 Step 3); (b) structured `BoundingBox` containment method name (Task 3 Step 1); (c) the exact export entry-point name and `chunk_metadata` visibility (Tasks 10–11); (d) the header tag string(s) real tagged PDFs use (Task 8 — we match `TH`/`*HEADER*`, widen if corpus shows another). These are grounded to a `grep` step, not left as prose placeholders.
- **Type consistency:** `RichCell`/`TableStructure` field names identical across Tasks 5, 9, 10, 11; `TableCell.row_span/col_span` and `DetectedTable.header_rows` identical across Tasks 6–9.
