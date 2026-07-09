# Design — Issue #375: Table extraction quality + prose-as-empty-table data loss

**Date:** 2026-07-09
**Issue:** #375 (enhancement, RAG)
**Branch:** `feature/issue-375-table-extraction-quality` (off `develop`)
**SemVer:** MINOR (additive; no breaking change to the public table model)

## Summary

Issue #375 bundles three concerns, delivered as a single block:

1. **Data-loss bug (priority):** pages of single-column prose can be misclassified as an
   empty/sparse table, discarding the page text. Silent data loss in the RAG path.
2. **Merged cells** (rowspan/colspan) are not preserved — spans collapse.
3. **Multi-level (nested) headers** are flattened, losing column semantics.

The fix is a correctness fix (concern 1) plus an additive enrichment of the table data
model (concerns 2 & 3), populated only from hard structural signals. Borderless-table
structure inference is explicitly out of scope and documented as a known limitation.

## Background — current architecture

Two independent table detectors feed one shared, flat table representation:

- **Ruling-based detector:** reads the horizontal/vertical vector lines the PDF draws as
  borders, derives a grid from the line crossings, and assigns text to cells.
- **Spatial/borderless detector:** with no drawn borders, clusters text by X/Y position to
  infer an implicit grid.

Both produce the same shape: a flat grid of plain-text strings (rows, each a list of
strings). There is **no representation** for merged cells or header rows today; those
features are structurally unrepresentable, not merely unimplemented.

Detected tables become a canonical pipeline element, flow into RAG chunks (with
row/column-count metadata), and are exported to GitHub-Flavored Markdown (GFM).

### Root cause of the data-loss bug

When a region is accepted as a table, the partitioner marks **every text fragment whose
position falls inside the table's bounding box as "claimed"** — independently of whether
that fragment's text was actually placed into a cell. The prose classifier only processes
*unclaimed* fragments. So text inside the bounding box but not inside any cell (gaps,
border zones, coordinate-normalization misses) is claimed and never re-classified as prose
→ its text is silently dropped. A prose page with as few as two horizontal and two vertical
drawn lines (a frame, a box, header/footer rules) can trip the detector, claim the whole
page, and emit a near-empty table.

The earlier #345 work only exposed a config switch to turn table detection off — a
workaround. The underlying claim-without-capture defect was never fixed.

## Goals

- No page loses text to an empty/sparse-table misclassification. Zero character-count
  regression across the corpus.
- Merged cells and multi-level headers are represented when a hard signal reveals them, and
  round-trip to correct GFM.
- Additive change only: existing consumers of the flat table view keep working unchanged.
- No-ML, deterministic throughout.

## Non-goals

- Inferring merged cells or header hierarchy from borderless (whitespace-only) tables.
  Documented as a known limitation.
- Reading the PDF's deep structure tree to recover a nested-header hierarchy (parent/child
  header grouping). That data does not reach the text/partition layer today — only a flat
  per-fragment tag name does — and plumbing the full structure tree is a much larger change
  reserved for a separate block. Multi-level headers are instead represented as header rows
  containing merged cells (see Design §3).
- Any breaking change to the public table model.

## Design

### 1. Data-loss safety net (concern 1, always on)

- **Claim only what you capture.** A table may claim only the text it actually placed into a
  cell. Text inside the bounding box but not in any cell stays unclaimed and flows to the
  prose classifier as paragraphs. This removes the silent drop at its source.
- **Reject degenerate tables.** A candidate table that captured almost no text decomposes
  back to prose rather than being accepted.
- **Text-conservation invariant.** After partitioning, the union of all element text must
  cover the page's extracted text. This becomes a hard test (absent today) and the safety
  net that catches any future leak.
- Delivered automatically to everyone with detection enabled — a data-loss fix is not
  hidden behind opt-in. The existing "disable detection" switch remains for compatibility
  but is demoted from a data-safety necessity to a plain preference.

### 2. Additive rich table model (concerns 2 & 3, representation)

- The rich structure — cells with their row/column span, and which rows are headers — is
  built internally as the source of truth.
- The existing flat grid stays public and is **derived** from the rich structure, so the two
  never desynchronize.
- Consumers of the flat view are unaffected; consumers wanting structure use the new layer.
- SemVer: MINOR (additive).

### 3. Sources of rich structure (hard signals only)

**Merged cells (the central, highest-risk task).** From the drawn grid. Today the grid
detector keeps only the *positions* of gridlines (the Y of each horizontal rule, the X of
each vertical rule) and then assumes a complete, perfect grid — it does **not** record which
dividing segments are actually drawn. A merged cell is precisely a *missing* internal
divider, which is invisible under that representation. So this task reworks grid construction
to remember which dividers actually exist (the raw line segments are available upstream and
are currently discarded during clustering) and, where an internal divider is absent, merges
the adjacent cells into one spanning cell. This is the hardest part of the block; everything
else is straightforward on top of it.

**Header rows.** Identify which rows are header rows: from the per-fragment structure tag
when the PDF is tagged (a fragment carrying a "header cell" tag), otherwise fall back to the
existing convention that the top row is the header.

**Multi-level headers = header rows + merged cells.** We do not read a nested-header
hierarchy from anywhere; that grouping is not exposed at this layer (see Non-goals). A
multi-level header is represented as header rows that contain merged cells — e.g. a "Region"
header spanning two columns above "Q1"/"Q2". The merged-cell detection above is what makes it
representable. Where there is neither a drawn header span nor a tag, we keep a single header
row.

**Borderless tables:** flat grid, best effort. No span/header inference.

### 4. Outputs — Markdown degradation and RAG metadata

GFM cannot express merged cells or multi-level headers. Deterministic degradation:

- **Merged cell → repeat its value in every covered cell.** A query landing on any covered
  column then sees the value instead of a gap (avoids the "disconnected numbers" failure).
- **Multi-level header → flatten by joining levels** (e.g. `Region › Q1`) so each column
  keeps its full semantic path.

RAG metadata (row/column counts, has-table flag) is computed from the rich model, counting
the real geometry of the spans.

### 5. Documentation of the known limitation

- Public table-detection guide (`docs/TABLE_DETECTION_GUIDE.md`): explanation with an
  example — borderless tables get no rich structure and their detection is intrinsically
  ambiguous; multi-level headers require a tagged PDF.
- Library module-level docs (rustdoc header of the table-detection module): short note
  pointing to the guide. Single source of truth in the guide; the module note just refers to
  it.

### 6. Testing

- **Reproduce first (TDD red):** a real corpus fixture where a prose page is currently eaten
  by an empty table; assert its text survives → then fix.
- **Text-conservation invariant** over the corpus: zero character-count regression.
- **Merged-cell fixture:** round-trips to correct GFM (spanned value repeated).
- **Multi-level-header fixture (tagged PDF):** round-trips to correct flattened GFM header.
- Every test verifies real content, not absence of crash.

## Risks / known limitations

- **Primary risk — merged-cell detection.** Requires reworking grid construction to retain
  per-segment divider presence (§3). Higher effort and risk than the rest of the block; must
  not regress existing full-grid table detection (guarded by the corpus/ruling tests).
- Borderless-table structure and un-tagged multi-level headers remain flat — documented.
- Changing the claim logic shifts some pages from (empty) table output to prose output. This
  is the intended correction; corpus before/after sweep confirms text is preserved, not lost.

## Rollout

- MINOR release once merged to `develop` and validated. Quality-rust pass before PR.
