//! Issue #375 Task 7: detect merged cells from a drawn grid by retaining which
//! divider line-segments are actually present.
//!
//! Fixture: a bordered 2-column table whose TOP row omits the middle vertical
//! divider (a single spanning header cell) while the BOTTOM row keeps it (two
//! cells). Detection must yield a top-left cell with `col_span == 2` and two
//! `col_span == 1` cells in the bottom row.

use oxidize_pdf::graphics::extraction::{ExtractedGraphics, VectorLine};
use oxidize_pdf::text::extraction::TextFragment;
use oxidize_pdf::text::table_detection::TableDetector;

/// Horizontal line at height `y` from `x1` to `x2`.
fn hline(x1: f64, x2: f64, y: f64) -> VectorLine {
    VectorLine::new(x1, y, x2, y, 1.0, true, None)
}

/// Vertical line at `x` from `y1` to `y2`.
fn vline(x: f64, y1: f64, y2: f64) -> VectorLine {
    VectorLine::new(x, y1, x, y2, 1.0, true, None)
}

fn frag(t: &str, x: f64, y: f64) -> TextFragment {
    TextFragment {
        text: t.into(),
        x,
        y,
        width: 30.0,
        height: 10.0,
        font_size: 10.0,
        font_name: None,
        is_bold: false,
        is_italic: false,
        color: None,
        space_decisions: Vec::new(),
        mcid: None,
        struct_tag: None,
    }
}

fn build_graphics(h: Vec<VectorLine>, v: Vec<VectorLine>) -> ExtractedGraphics {
    let mut g = ExtractedGraphics::new();
    for line in h.into_iter().chain(v.into_iter()) {
        g.add_line(line);
    }
    g
}

#[test]
fn merged_header_cell_detected_with_col_span_2() {
    // Grid X at 100,200,300 ; Y at 100 (bottom),150 (mid),200 (top).
    // Row 0 (top band, y 150..200) has NO middle vertical => one spanning cell.
    // Row 1 (y 100..150) HAS the middle vertical => two cells.
    let h = vec![
        hline(100.0, 300.0, 100.0),
        hline(100.0, 300.0, 150.0),
        hline(100.0, 300.0, 200.0),
    ];
    let v = vec![
        vline(100.0, 100.0, 200.0), // left border (full height)
        vline(300.0, 100.0, 200.0), // right border (full height)
        vline(200.0, 100.0, 150.0), // MIDDLE divider only in row 1
    ];
    let graphics = build_graphics(h, v);
    let frags = vec![
        frag("Header", 150.0, 170.0),
        frag("A", 130.0, 120.0),
        frag("B", 230.0, 120.0),
    ];

    let det = TableDetector::default();
    let tables = det.detect(&graphics, &frags).expect("detect");
    let table = tables.first().expect("one table");

    // Base grid dimensions stay 2x2.
    assert_eq!(table.rows, 2, "base grid rows");
    assert_eq!(table.columns, 2, "base grid columns");

    let top_left = table
        .cells
        .iter()
        .find(|c| c.row == 0 && c.column == 0)
        .expect("cell 0,0");
    assert_eq!(top_left.col_span, 2, "merged header should span 2 columns");
    assert_eq!(top_left.row_span, 1, "merged header spans a single row");

    // The merged header must NOT leave a separate cell at (0,1).
    assert!(
        !table.cells.iter().any(|c| c.row == 0 && c.column == 1),
        "interior position of a merged cell must be omitted"
    );

    // The non-merged bottom row keeps two single cells.
    assert!(
        table
            .cells
            .iter()
            .any(|c| c.row == 1 && c.column == 0 && c.col_span == 1 && c.row_span == 1),
        "bottom-left single cell"
    );
    assert!(
        table
            .cells
            .iter()
            .any(|c| c.row == 1 && c.column == 1 && c.col_span == 1 && c.row_span == 1),
        "bottom-right single cell"
    );
}
