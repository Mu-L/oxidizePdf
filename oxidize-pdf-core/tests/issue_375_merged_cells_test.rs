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
            RichCell {
                row: 0,
                col: 0,
                row_span: 1,
                col_span: 2,
                text: "Region".into(),
                is_header: true,
            },
            RichCell {
                row: 1,
                col: 0,
                row_span: 1,
                col_span: 1,
                text: "Q1".into(),
                is_header: false,
            },
            RichCell {
                row: 1,
                col: 1,
                row_span: 1,
                col_span: 1,
                text: "Q2".into(),
                is_header: false,
            },
        ],
    };
    let data = TableElementData::from_structure(structure, ElementMetadata::default());
    // Flat view repeats the spanned header value across both covered columns.
    assert_eq!(
        data.rows,
        vec![
            vec!["Region".to_string(), "Region".to_string()],
            vec!["Q1".to_string(), "Q2".to_string()],
        ]
    );
    assert_eq!(data.structure.as_ref().unwrap().header_rows, 1);
}
