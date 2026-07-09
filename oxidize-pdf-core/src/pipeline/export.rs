use crate::pipeline::{Element, TableElementData};

/// Configuration for element-aware markdown export.
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Include Header and Footer elements in output (default: false).
    pub include_headers_footers: bool,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            include_headers_footers: false,
        }
    }
}

/// Exports a slice of [`Element`]s to Markdown format.
#[derive(Debug, Clone, Default)]
pub struct ElementMarkdownExporter {
    pub config: ExportConfig,
}

impl ElementMarkdownExporter {
    pub fn new(config: ExportConfig) -> Self {
        Self { config }
    }

    /// Export elements to a Markdown string.
    pub fn export(&self, elements: &[Element]) -> String {
        if elements.is_empty() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        for element in elements {
            if let Some(md) = self.element_to_markdown(element) {
                parts.push(md);
            }
        }
        parts.join("\n\n")
    }

    fn element_to_markdown(&self, element: &Element) -> Option<String> {
        match element {
            Element::Title(d) => Some(format!("# {}", d.text.trim())),
            Element::Paragraph(d) => Some(d.text.trim().to_string()),
            Element::ListItem(d) => Some(format!("- {}", d.text.trim())),
            Element::KeyValue(kv) => Some(format!("**{}**: {}", kv.key.trim(), kv.value.trim())),
            Element::CodeBlock(d) => Some(format!("```\n{}\n```", d.text.trim())),
            Element::Image(img) => {
                let alt = img.alt_text.as_deref().unwrap_or("");
                Some(format!("![{}]()", alt))
            }
            Element::Table(t) => Some(table_to_markdown_data(t)),
            Element::Header(_) | Element::Footer(_) => {
                if self.config.include_headers_footers {
                    Some(element.display_text())
                } else {
                    None
                }
            }
        }
    }
}

fn table_to_markdown(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push(format!("| {} |", rows[0].join(" | ")));
    let sep: Vec<&str> = vec!["---"; rows[0].len()];
    lines.push(format!("| {} |", sep.join(" | ")));
    for row in &rows[1..] {
        lines.push(format!("| {} |", row.join(" | ")));
    }
    lines.join("\n")
}

/// Structure-aware table export: when `data.structure` reveals a multi-level
/// header (`header_rows > 1`), collapse the first `header_rows` rows into a
/// single GFM header row, joining each column's header texts top-to-bottom
/// with " › " (skipping empties and consecutive duplicates so a vertically-
/// or horizontally-merged header cell that is already repeated across the
/// flat `rows` view doesn't render as "X › X"). Body rows start right after
/// the header rows; merged body cells are already repeated in `rows`, so
/// their rendering is unchanged.
///
/// When `structure` is absent or `header_rows <= 1`, delegates to
/// [`table_to_markdown`] — behavior for single/no-header tables is unchanged.
fn table_to_markdown_data(data: &TableElementData) -> String {
    match &data.structure {
        Some(st) if st.header_rows > 1 && !data.rows.is_empty() => {
            let ncols = st.num_cols;
            let header_rows = st.header_rows.min(data.rows.len());
            let mut header = Vec::with_capacity(ncols);
            for c in 0..ncols {
                let mut parts: Vec<&str> = Vec::new();
                for row in &data.rows[..header_rows] {
                    let cell = row.get(c).map(|s| s.as_str()).unwrap_or("");
                    if !cell.is_empty() && parts.last() != Some(&cell) {
                        parts.push(cell);
                    }
                }
                header.push(parts.join(" › "));
            }
            let mut lines = vec![
                format!("| {} |", header.join(" | ")),
                format!("| {} |", vec!["---"; ncols].join(" | ")),
            ];
            for row in &data.rows[header_rows..] {
                lines.push(format!("| {} |", row.join(" | ")));
            }
            lines.join("\n")
        }
        _ => table_to_markdown(&data.rows),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{ElementMetadata, RichCell, TableStructure};

    #[test]
    fn multi_level_header_flattens_with_separator() {
        // "Region" spans both columns over row 0; row 1 sub-headers "Q1"/"Q2".
        // Dedup must prevent "Region › Region" leaking through.
        let structure = TableStructure {
            num_rows: 3,
            num_cols: 2,
            header_rows: 2,
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
                    is_header: true,
                },
                RichCell {
                    row: 1,
                    col: 1,
                    row_span: 1,
                    col_span: 1,
                    text: "Q2".into(),
                    is_header: true,
                },
                RichCell {
                    row: 2,
                    col: 0,
                    row_span: 1,
                    col_span: 1,
                    text: "10".into(),
                    is_header: false,
                },
                RichCell {
                    row: 2,
                    col: 1,
                    row_span: 1,
                    col_span: 1,
                    text: "20".into(),
                    is_header: false,
                },
            ],
        };
        let data = TableElementData::from_structure(structure, ElementMetadata::default());
        let md = table_to_markdown_data(&data);
        assert_eq!(
            md,
            "| Region › Q1 | Region › Q2 |\n| --- | --- |\n| 10 | 20 |"
        );
    }

    #[test]
    fn no_structure_table_unchanged() {
        let metadata = ElementMetadata::default();
        let data = TableElementData {
            rows: vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["1".to_string(), "2".to_string()],
            ],
            structure: None,
            metadata,
        };
        let expected = table_to_markdown(&data.rows);
        assert_eq!(table_to_markdown_data(&data), expected);
        assert_eq!(expected, "| a | b |\n| --- | --- |\n| 1 | 2 |");
    }

    #[test]
    fn single_header_row_structure_unchanged() {
        // header_rows == 1: same behavior as the plain no-structure path.
        let structure = TableStructure {
            num_rows: 2,
            num_cols: 2,
            header_rows: 1,
            cells: vec![
                RichCell {
                    row: 0,
                    col: 0,
                    row_span: 1,
                    col_span: 1,
                    text: "a".into(),
                    is_header: true,
                },
                RichCell {
                    row: 0,
                    col: 1,
                    row_span: 1,
                    col_span: 1,
                    text: "b".into(),
                    is_header: true,
                },
                RichCell {
                    row: 1,
                    col: 0,
                    row_span: 1,
                    col_span: 1,
                    text: "1".into(),
                    is_header: false,
                },
                RichCell {
                    row: 1,
                    col: 1,
                    row_span: 1,
                    col_span: 1,
                    text: "2".into(),
                    is_header: false,
                },
            ],
        };
        let data = TableElementData::from_structure(structure, ElementMetadata::default());
        assert_eq!(
            table_to_markdown_data(&data),
            "| a | b |\n| --- | --- |\n| 1 | 2 |"
        );
    }
}
