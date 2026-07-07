/// Utilities for analyzing and modifying PDF content streams
///
/// This module provides functions to extract font references, remap names,
/// and perform other content stream transformations needed for overlay operations.
use std::collections::{HashMap, HashSet};

/// Extract all font references from a content stream
///
/// Searches for "/Fx" patterns where x is a number or name, typically appearing
/// in font selection operators like "BT /F1 12 Tf (Hello) Tj ET"
///
/// # Arguments
/// * `content` - Raw content stream bytes
///
/// # Returns
/// Set of font names referenced in the stream (e.g., ["F1", "F2", "Arial"])
///
/// # Example
/// ```ignore
/// let content = b"BT /F1 12 Tf (Hello) Tj /F2 10 Tf (World) Tj ET";
/// let fonts = extract_font_references(content);
/// assert!(fonts.contains("F1"));
/// assert!(fonts.contains("F2"));
/// ```
#[allow(dead_code)] // Will be used in Phase 2
pub fn extract_font_references(content: &[u8]) -> HashSet<String> {
    let mut font_names = HashSet::new();

    // Convert to string for easier parsing
    let content_str = String::from_utf8_lossy(content);

    // Look for "/FontName" patterns followed by Tf operator
    // Pattern: /FontName <number> Tf
    for line in content_str.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();

        for (i, token) in tokens.iter().enumerate() {
            // Check if this is a font name (starts with /)
            if token.starts_with('/') {
                // Check if followed by number and Tf (font selection operator)
                if i + 2 < tokens.len() {
                    // tokens[i+1] should be number (size)
                    // tokens[i+2] should be "Tf"
                    if tokens[i + 2] == "Tf" {
                        // Extract font name (remove leading /)
                        let font_name = token[1..].to_string();
                        font_names.insert(font_name);
                    }
                }
            }
        }
    }

    font_names
}

/// Rename fonts in a dictionary by adding a prefix
///
/// Takes a font dictionary and renames all font keys by adding "Orig" prefix.
/// This prevents naming conflicts between preserved fonts and overlay fonts.
///
/// # Arguments
/// * `fonts` - Font dictionary from preserved resources
///
/// # Returns
/// New dictionary with renamed fonts (/F1 → /OrigF1, /Arial → /OrigArial)
///
/// # Limitations
/// - Does not detect naming collisions if /OrigF1 already exists (rare but possible)
/// - Clones entire font dictionaries (acceptable for typical PDFs with <50 fonts)
/// - No validation that font names are valid PDF names
///
/// # Example
/// ```ignore
/// use std::collections::HashMap;
/// let mut fonts = HashMap::new();
/// fonts.insert("F1".to_string(), "font_dict_1");
/// fonts.insert("F2".to_string(), "font_dict_2");
///
/// let renamed = rename_preserved_fonts(&fonts);
/// assert!(renamed.contains_key("OrigF1"));
/// assert!(renamed.contains_key("OrigF2"));
/// ```
#[allow(dead_code)] // Will be used in Phase 2.3
pub fn rename_preserved_fonts(fonts: &crate::objects::Dictionary) -> crate::objects::Dictionary {
    let mut renamed = crate::objects::Dictionary::new();

    for (key, value) in fonts.iter() {
        // Add "Orig" prefix to font name
        let new_name = format!("Orig{}", key);
        renamed.set(new_name, value.clone());
    }

    renamed
}

/// Rewrite font references in a content stream using a name mapping
///
/// Searches for font selection operators ("/FontName size Tf") and replaces
/// the font names according to the provided mapping. This is used to update
/// content streams when fonts have been renamed to avoid conflicts.
///
/// # Arguments
/// * `content` - Original content stream bytes
/// * `mappings` - Map from old font names to new font names (e.g., "F1" → "OrigF1")
///
/// # Returns
/// New content stream with updated font references
///
/// # Limitations
/// - **Whitespace normalization**: Original whitespace (multiple spaces, tabs) is
///   normalized to single spaces. PDF remains valid but loses formatting fidelity.
/// - **Binary data risk**: Uses lossy UTF-8 conversion. Safe for text-only content streams,
///   but may corrupt streams with inline images or binary data (rare in practice).
/// - **Performance**: Creates complete copy of content stream. For very large streams
///   (>5MB), consider streaming approach.
/// - **No validation**: Does not verify that resulting PDF operators are valid.
///
/// # Example
/// ```ignore
/// use std::collections::HashMap;
/// let content = b"BT /F1 12 Tf (Hello) Tj ET";
/// let mut mappings = HashMap::new();
/// mappings.insert("F1".to_string(), "OrigF1".to_string());
///
/// let rewritten = rewrite_font_references(content, &mappings);
/// // Result: b"BT /OrigF1 12 Tf (Hello) Tj ET"
/// ```
pub fn rewrite_font_references(content: &[u8], mappings: &HashMap<String, String>) -> Vec<u8> {
    if mappings.is_empty() {
        return content.to_vec();
    }

    // Tokenize into raw byte spans. Only the bytes of a font name that sits in
    // the operand position of a `Tf` operator are replaced; every other byte
    // (whitespace, comments, string/hex literals, inline binary) is copied
    // verbatim. This is robust to layout the previous line-based rewriter
    // failed on — e.g. a font operator split across a newline
    // (`/F1\n12 Tf`) — and never corrupts string/comment content that merely
    // happens to contain a `/name`.
    let tokens = tokenize_content(content);

    // Mark the name tokens to rewrite: a Name whose value is in `mappings` and
    // that is immediately followed (ignoring whitespace/comments) by a Number
    // and the keyword `Tf`.
    let mut rewrite: Vec<Option<&String>> = vec![None; tokens.len()];
    for i in 0..tokens.len() {
        let tok = &tokens[i];
        if tok.kind != TokenKind::Name {
            continue;
        }
        // Name bytes include the leading '/'; strip it for the map lookup.
        let name = &content[tok.start + 1..tok.end];
        let Ok(name) = std::str::from_utf8(name) else {
            continue;
        };
        let Some(new_name) = mappings.get(name) else {
            continue;
        };
        let is_size = tokens
            .get(i + 1)
            .is_some_and(|t| t.kind == TokenKind::Number);
        let is_tf = tokens
            .get(i + 2)
            .is_some_and(|t| t.kind == TokenKind::Keyword && &content[t.start..t.end] == b"Tf");
        if is_size && is_tf {
            rewrite[i] = Some(new_name);
        }
    }

    // Reassemble, preserving all inter-token bytes verbatim.
    let mut out = Vec::with_capacity(content.len());
    let mut pos = 0usize;
    for (i, tok) in tokens.iter().enumerate() {
        out.extend_from_slice(&content[pos..tok.start]);
        match rewrite[i] {
            Some(new_name) => {
                out.push(b'/');
                out.extend_from_slice(new_name.as_bytes());
            }
            None => out.extend_from_slice(&content[tok.start..tok.end]),
        }
        pos = tok.end;
    }
    out.extend_from_slice(&content[pos..]);
    out
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum TokenKind {
    Name,
    Number,
    Keyword,
    /// String/hex literal, array/dict delimiter, or any other token that breaks
    /// a `/name <size> Tf` run.
    Other,
}

struct Token {
    start: usize,
    end: usize,
    kind: TokenKind,
}

/// True for PDF whitespace (ISO 32000-1 Table 1).
fn is_pdf_whitespace(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

/// True for PDF delimiter characters (ISO 32000-1 Table 2).
fn is_pdf_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(b: u8) -> bool {
    !is_pdf_whitespace(b) && !is_pdf_delimiter(b)
}

/// Split a content stream into significant tokens, recording each token's byte
/// span. Whitespace and comments are not emitted as tokens (they are the gaps
/// between tokens). String and hex literals are consumed whole as `Other` so a
/// `/name` inside them is never treated as a font reference.
fn tokenize_content(content: &[u8]) -> Vec<Token> {
    let n = content.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < n {
        let c = content[i];
        if is_pdf_whitespace(c) {
            i += 1;
            continue;
        }
        match c {
            b'%' => {
                // Comment: skip to end of line (gap, not a token).
                while i < n && content[i] != b'\n' && content[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => {
                // Literal string: balanced parens honouring backslash escapes.
                let start = i;
                i += 1;
                let mut depth = 1;
                while i < n && depth > 0 {
                    match content[i] {
                        b'\\' => i += 1, // skip the escaped byte too
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                tokens.push(Token {
                    start,
                    end: i,
                    kind: TokenKind::Other,
                });
            }
            b'<' => {
                if i + 1 < n && content[i + 1] == b'<' {
                    tokens.push(Token {
                        start: i,
                        end: i + 2,
                        kind: TokenKind::Other,
                    });
                    i += 2;
                } else {
                    // Hex string up to '>'.
                    let start = i;
                    i += 1;
                    while i < n && content[i] != b'>' {
                        i += 1;
                    }
                    if i < n {
                        i += 1; // consume '>'
                    }
                    tokens.push(Token {
                        start,
                        end: i,
                        kind: TokenKind::Other,
                    });
                }
            }
            b'>' => {
                let end = if i + 1 < n && content[i + 1] == b'>' {
                    i + 2
                } else {
                    i + 1
                };
                tokens.push(Token {
                    start: i,
                    end,
                    kind: TokenKind::Other,
                });
                i = end;
            }
            b'[' | b']' | b'{' | b'}' | b')' => {
                tokens.push(Token {
                    start: i,
                    end: i + 1,
                    kind: TokenKind::Other,
                });
                i += 1;
            }
            b'/' => {
                let start = i;
                i += 1;
                while i < n && is_regular(content[i]) {
                    i += 1;
                }
                tokens.push(Token {
                    start,
                    end: i,
                    kind: TokenKind::Name,
                });
            }
            _ => {
                let start = i;
                while i < n && is_regular(content[i]) {
                    i += 1;
                }
                let kind = if is_pdf_number(&content[start..i]) {
                    TokenKind::Number
                } else {
                    TokenKind::Keyword
                };
                tokens.push(Token {
                    start,
                    end: i,
                    kind,
                });
            }
        }
    }
    tokens
}

/// True if `bytes` is a PDF numeric token (integer or real, optional sign).
fn is_pdf_number(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut seen_digit = false;
    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'+' | b'-' if idx == 0 => {}
            b'0'..=b'9' => seen_digit = true,
            b'.' => {}
            _ => return false,
        }
    }
    seen_digit
}

/// Resource keys of the standard Type1 fonts that `write_page_with_fonts`
/// injects into every page `/Font` dictionary. A preserved font whose resource
/// key equals one of these is shadowed by the injected stub unless it is
/// disambiguated (issue #395).
pub(crate) const INJECTED_BASE_FONT_KEYS: [&str; 12] = [
    "Helvetica",
    "Helvetica-Bold",
    "Helvetica-Oblique",
    "Helvetica-BoldOblique",
    "Times-Roman",
    "Times-Bold",
    "Times-Italic",
    "Times-BoldItalic",
    "Courier",
    "Courier-Bold",
    "Courier-Oblique",
    "Courier-BoldOblique",
];

/// Build a rename map for preserved fonts, disambiguating ONLY the keys that
/// collide with `reserved` (the keys already present in the destination page
/// `/Font` dictionary: the injected base fonts plus any overlay fonts).
///
/// Non-colliding keys are absent from the map and keep their original name, so
/// no content rewrite is performed for them — which is what makes inputs like
/// `testi.pdf` (all non-base-14 font names) safe. The disambiguated name is
/// guaranteed unique against both `reserved` and every preserved key, so it can
/// never introduce a fresh collision.
pub fn collision_font_mapping<'a>(
    preserved_keys: impl IntoIterator<Item = &'a str>,
    reserved: &HashSet<String>,
) -> HashMap<String, String> {
    let preserved: Vec<String> = preserved_keys.into_iter().map(|s| s.to_string()).collect();
    let preserved_set: HashSet<&str> = preserved.iter().map(|s| s.as_str()).collect();

    let mut map: HashMap<String, String> = HashMap::new();
    for key in &preserved {
        if !reserved.contains(key) {
            continue;
        }
        let mut candidate = format!("Orig{key}");
        let mut suffix = 1u32;
        while reserved.contains(&candidate)
            || preserved_set.contains(candidate.as_str())
            || map.values().any(|v| v == &candidate)
        {
            candidate = format!("Orig{key}_{suffix}");
            suffix += 1;
        }
        map.insert(key.clone(), candidate);
    }
    map
}

/// Apply a font rename `map` to a preserved font dictionary: keys present in the
/// map are renamed to their mapped value; every other key (and all values) are
/// carried over unchanged.
pub fn apply_font_rename_map(
    fonts: &crate::objects::Dictionary,
    map: &HashMap<String, String>,
) -> crate::objects::Dictionary {
    let mut out = crate::objects::Dictionary::new();
    for (key, value) in fonts.iter() {
        let new_key = map.get(key).cloned().unwrap_or_else(|| key.clone());
        out.set(new_key, value.clone());
    }
    out
}

/// Check if a font has embedded font data (FontFile/FontFile2/FontFile3)
///
/// Analyzes a font dictionary to determine if it contains embedded font program data.
/// Embedded fonts have a FontDescriptor that references font stream objects.
///
/// # Font Stream Types (ISO 32000-1):
/// - **FontFile**: Type 1 font program (PostScript)
/// - **FontFile2**: TrueType font program
/// - **FontFile3**: Subtype-specific font (CFF, OpenType, etc.)
///
/// # Arguments
/// * `font_dict` - Font dictionary to analyze
///
/// # Returns
/// `true` if font has embedded data, `false` for standard/base fonts
///
/// # Example
/// ```ignore
/// // Embedded font (e.g., Arial with TTF data)
/// let font_dict = Dictionary::from([
///     ("Type", "Font"),
///     ("FontDescriptor", Reference(10, 0)), // -> has FontFile2
/// ]);
/// assert!(has_embedded_font_data(&font_dict)); // true
///
/// // Standard font (e.g., Helvetica)
/// let standard_font = Dictionary::from([
///     ("Type", "Font"),
///     ("BaseFont", "Helvetica"),
/// ]);
/// assert!(!has_embedded_font_data(&standard_font)); // false
/// ```
#[allow(dead_code)] // Will be used in Phase 3.2
pub fn has_embedded_font_data(font_dict: &crate::objects::Dictionary) -> bool {
    use crate::objects::Object;

    // Check if font has a FontDescriptor
    if let Some(Object::Dictionary(descriptor)) = font_dict.get("FontDescriptor") {
        // Check for any of the three font stream types
        descriptor.contains_key("FontFile")
            || descriptor.contains_key("FontFile2")
            || descriptor.contains_key("FontFile3")
    } else if let Some(Object::Reference(_)) = font_dict.get("FontDescriptor") {
        // FontDescriptor is a reference - we need to resolve it to check
        // For now, assume it MIGHT have embedded data (conservative approach)
        // Phase 3.2 will handle proper resolution
        true
    } else {
        // No FontDescriptor = standard font (Helvetica, Times, etc.)
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_font_references_simple() {
        let content = b"BT /F1 12 Tf (Hello) Tj ET";
        let fonts = extract_font_references(content);

        assert_eq!(fonts.len(), 1);
        assert!(fonts.contains("F1"));
    }

    #[test]
    fn test_extract_font_references_multiple() {
        let content = b"BT /F1 12 Tf (Hello) Tj ET BT /F2 10 Tf (World) Tj ET";
        let fonts = extract_font_references(content);

        assert_eq!(fonts.len(), 2);
        assert!(fonts.contains("F1"));
        assert!(fonts.contains("F2"));
    }

    #[test]
    fn test_extract_font_references_with_named_fonts() {
        let content = b"BT /ArialBold 14 Tf (Test) Tj /Helvetica 10 Tf (More) Tj ET";
        let fonts = extract_font_references(content);

        assert_eq!(fonts.len(), 2);
        assert!(fonts.contains("ArialBold"));
        assert!(fonts.contains("Helvetica"));
    }

    #[test]
    fn test_extract_font_references_multiline() {
        let content = b"BT\n/F1 12 Tf\n(Line 1) Tj\nET\nBT\n/F2 10 Tf\n(Line 2) Tj\nET";
        let fonts = extract_font_references(content);

        assert_eq!(fonts.len(), 2);
        assert!(fonts.contains("F1"));
        assert!(fonts.contains("F2"));
    }

    #[test]
    fn test_extract_font_references_no_fonts() {
        let content = b"100 200 m 300 400 l S";
        let fonts = extract_font_references(content);

        assert_eq!(fonts.len(), 0);
    }

    #[test]
    fn test_extract_font_references_ignore_false_positives() {
        // /Pattern shouldn't be detected as a font (not followed by Tf)
        let content = b"/Pattern cs /P1 scn 100 100 m 200 200 l S";
        let fonts = extract_font_references(content);

        assert_eq!(fonts.len(), 0);
    }

    // Tests for rename_preserved_fonts

    #[test]
    fn test_rename_preserved_fonts_simple() {
        use crate::objects::{Dictionary, Object};

        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Integer(1));
        fonts.set("F2", Object::Integer(2));

        let renamed = rename_preserved_fonts(&fonts);

        assert_eq!(renamed.len(), 2);
        assert!(renamed.contains_key("OrigF1"));
        assert!(renamed.contains_key("OrigF2"));
        assert!(!renamed.contains_key("F1")); // Original keys should not exist
        assert!(!renamed.contains_key("F2"));
    }

    #[test]
    fn test_rename_preserved_fonts_named_fonts() {
        use crate::objects::{Dictionary, Object};

        let mut fonts = Dictionary::new();
        fonts.set("Arial", Object::Integer(10));
        fonts.set("Helvetica", Object::Integer(20));
        fonts.set("TimesNewRoman", Object::Integer(30));

        let renamed = rename_preserved_fonts(&fonts);

        assert_eq!(renamed.len(), 3);
        assert!(renamed.contains_key("OrigArial"));
        assert!(renamed.contains_key("OrigHelvetica"));
        assert!(renamed.contains_key("OrigTimesNewRoman"));
    }

    #[test]
    fn test_rename_preserved_fonts_preserves_values() {
        use crate::objects::{Dictionary, Object};

        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Integer(42));
        fonts.set("Arial", Object::String("test".to_string()));

        let renamed = rename_preserved_fonts(&fonts);

        // Values should be preserved
        assert_eq!(renamed.get("OrigF1"), Some(&Object::Integer(42)));
        assert_eq!(
            renamed.get("OrigArial"),
            Some(&Object::String("test".to_string()))
        );
    }

    #[test]
    fn test_rename_preserved_fonts_empty_dictionary() {
        use crate::objects::Dictionary;

        let fonts = Dictionary::new();
        let renamed = rename_preserved_fonts(&fonts);

        assert_eq!(renamed.len(), 0);
    }

    #[test]
    fn test_rename_preserved_fonts_complex_objects() {
        use crate::objects::{Dictionary, Object};

        let mut fonts = Dictionary::new();

        // Create a complex font dictionary
        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name("Font".to_string()));
        font_dict.set("Subtype", Object::Name("Type1".to_string()));
        font_dict.set("BaseFont", Object::Name("Helvetica".to_string()));

        fonts.set("F1", Object::Dictionary(font_dict.clone()));

        let renamed = rename_preserved_fonts(&fonts);

        assert_eq!(renamed.len(), 1);
        assert!(renamed.contains_key("OrigF1"));

        // Verify the complex object is preserved
        if let Some(Object::Dictionary(dict)) = renamed.get("OrigF1") {
            assert_eq!(dict.get("Type"), Some(&Object::Name("Font".to_string())));
            assert_eq!(
                dict.get("Subtype"),
                Some(&Object::Name("Type1".to_string()))
            );
            assert_eq!(
                dict.get("BaseFont"),
                Some(&Object::Name("Helvetica".to_string()))
            );
        } else {
            panic!("Expected dictionary object");
        }
    }

    #[test]
    fn test_rename_preserved_fonts_all_keys_prefixed() {
        use crate::objects::{Dictionary, Object};

        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Integer(1));
        fonts.set("F2", Object::Integer(2));
        fonts.set("Arial", Object::Integer(3));
        fonts.set("Helvetica", Object::Integer(4));

        let renamed = rename_preserved_fonts(&fonts);

        // Verify ALL keys have "Orig" prefix
        for key in renamed.keys() {
            assert!(
                key.starts_with("Orig"),
                "Key '{}' should start with 'Orig'",
                key
            );
        }
    }

    // Tests for rewrite_font_references

    #[test]
    fn test_rewrite_font_references_simple() {
        let content = b"BT /F1 12 Tf (Hello) Tj ET";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert_eq!(result, "BT /OrigF1 12 Tf (Hello) Tj ET");
    }

    #[test]
    fn test_rewrite_font_references_multiple() {
        let content = b"BT /F1 12 Tf (Hello) Tj ET BT /F2 10 Tf (World) Tj ET";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());
        mappings.insert("F2".to_string(), "OrigF2".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert_eq!(
            result,
            "BT /OrigF1 12 Tf (Hello) Tj ET BT /OrigF2 10 Tf (World) Tj ET"
        );
    }

    #[test]
    fn test_rewrite_font_references_named_fonts() {
        let content = b"BT /Arial 14 Tf (Test) Tj /Helvetica 10 Tf (More) Tj ET";
        let mut mappings = HashMap::new();
        mappings.insert("Arial".to_string(), "OrigArial".to_string());
        mappings.insert("Helvetica".to_string(), "OrigHelvetica".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert_eq!(
            result,
            "BT /OrigArial 14 Tf (Test) Tj /OrigHelvetica 10 Tf (More) Tj ET"
        );
    }

    #[test]
    fn test_rewrite_font_references_multiline() {
        let content = b"BT\n/F1 12 Tf\n(Line 1) Tj\nET\nBT\n/F2 10 Tf\n(Line 2) Tj\nET";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());
        mappings.insert("F2".to_string(), "OrigF2".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert!(result.contains("/OrigF1 12 Tf"));
        assert!(result.contains("/OrigF2 10 Tf"));
        assert!(!result.contains("/F1 12 Tf"));
        assert!(!result.contains("/F2 10 Tf"));
    }

    #[test]
    fn test_rewrite_font_references_partial_mapping() {
        // Only map F1, leave F2 unchanged
        let content = b"BT /F1 12 Tf (Hello) Tj /F2 10 Tf (World) Tj ET";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert!(result.contains("/OrigF1 12 Tf"));
        assert!(result.contains("/F2 10 Tf")); // F2 unchanged
        assert!(!result.contains("/F1 12 Tf"));
    }

    #[test]
    fn test_rewrite_font_references_no_mappings() {
        let content = b"BT /F1 12 Tf (Hello) Tj ET";
        let mappings = HashMap::new();

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        // Should remain unchanged
        assert_eq!(result, "BT /F1 12 Tf (Hello) Tj ET");
    }

    #[test]
    fn test_rewrite_font_references_non_font_operators() {
        // Content with /Pattern (not a font)
        let content = b"/Pattern cs /P1 scn 100 100 m 200 200 l S";
        let mut mappings = HashMap::new();
        mappings.insert("Pattern".to_string(), "OrigPattern".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        // /Pattern should NOT be rewritten (not followed by Tf)
        assert!(result.contains("/Pattern cs"));
        assert!(!result.contains("/OrigPattern"));
    }

    #[test]
    fn test_rewrite_font_references_preserves_other_content() {
        let content = b"100 200 m 300 400 l S BT /F1 12 Tf (Text) Tj ET q Q";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        // Font should be rewritten
        assert!(result.contains("/OrigF1 12 Tf"));
        // Other operators preserved
        assert!(result.contains("100 200 m"));
        assert!(result.contains("300 400 l"));
        assert!(result.contains("(Text) Tj"));
    }

    // Edge case tests

    #[test]
    fn test_rewrite_font_references_preserves_whitespace() {
        // The structure-preserving rewriter (issue #395) only replaces the font
        // name bytes; all surrounding whitespace is kept verbatim.
        let content = b"BT  /F1   12  Tf  (Text)  Tj  ET"; // Multiple spaces
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        // Only the name changed; the original spacing is preserved exactly.
        assert_eq!(result, "BT  /OrigF1   12  Tf  (Text)  Tj  ET");
    }

    #[test]
    fn test_rewrite_font_references_with_indentation() {
        // Indentation and newlines are preserved (only the name is rewritten).
        let content = b"BT\n  /F1 12 Tf\n  100 700 Td\n  (Text) Tj\nET";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert_eq!(result, "BT\n  /OrigF1 12 Tf\n  100 700 Td\n  (Text) Tj\nET");
    }

    #[test]
    fn test_rewrite_font_references_cross_line_operator() {
        // Regression for issue #395: the font name and its size on separate
        // lines. The old line-based rewriter missed this; the tokenizer-based
        // one rewrites it and keeps the newline.
        let content = b"BT\n/F1\n12 Tf (Text) Tj ET";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert_eq!(result, "BT\n/OrigF1\n12 Tf (Text) Tj ET");
    }

    #[test]
    fn test_rewrite_font_references_ignores_name_inside_string() {
        // A `/F1 12 Tf` sequence appearing INSIDE a literal string must not be
        // rewritten — it is text content, not an operator.
        let content = b"BT /F1 12 Tf (/F1 12 Tf literal) Tj ET";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        // The operator is rewritten; the string literal is untouched.
        assert_eq!(result, "BT /OrigF1 12 Tf (/F1 12 Tf literal) Tj ET");
    }

    #[test]
    fn test_rewrite_font_references_real_size_and_negative() {
        // Font sizes may be reals or signed; both are valid Number operands.
        let content = b"/F1 12.5 Tf /F2 -8 Tf";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());
        mappings.insert("F2".to_string(), "OrigF2".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert_eq!(result, "/OrigF1 12.5 Tf /OrigF2 -8 Tf");
    }

    #[test]
    fn test_rename_preserved_fonts_no_collision_detection() {
        // DOCUMENTED LIMITATION: No collision detection
        use crate::objects::{Dictionary, Object};

        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Integer(1));
        fonts.set("OrigF1", Object::Integer(2)); // Already has "Orig" prefix!

        let renamed = rename_preserved_fonts(&fonts);

        // Both get renamed (collision not detected)
        assert!(renamed.contains_key("OrigF1")); // From original "OrigF1"
        assert!(renamed.contains_key("OrigOrigF1")); // From "F1"

        // This is acceptable - naming collisions are extremely rare in real PDFs
        // If needed, integration code can detect and handle this
    }

    #[test]
    fn test_rewrite_font_references_with_tabs() {
        // Tab separators are valid PDF whitespace and are preserved verbatim.
        let content = b"BT\t/F1\t12\tTf\t(Text)\tTj\tET";
        let mut mappings = HashMap::new();
        mappings.insert("F1".to_string(), "OrigF1".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        // Font renamed correctly; tabs kept.
        assert_eq!(result, "BT\t/OrigF1\t12\tTf\t(Text)\tTj\tET");
    }

    // Tests for collision_font_mapping (issue #395)

    fn reserved_set(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_collision_mapping_renames_only_colliding_keys() {
        // /Helvetica collides with the injected base font; /F1 and /Arial do not.
        let reserved = reserved_set(&INJECTED_BASE_FONT_KEYS);
        let map = collision_font_mapping(["Helvetica", "F1", "Arial"], &reserved);

        assert_eq!(
            map.get("Helvetica").map(String::as_str),
            Some("OrigHelvetica")
        );
        assert!(
            !map.contains_key("F1"),
            "non-colliding key must not be renamed"
        );
        assert!(
            !map.contains_key("Arial"),
            "non-colliding key must not be renamed"
        );
    }

    #[test]
    fn test_collision_mapping_empty_when_no_collision() {
        // The `testi.pdf` class: all font names are non-base-14 → no rename.
        let reserved = reserved_set(&INJECTED_BASE_FONT_KEYS);
        let map = collision_font_mapping(["ArialMT", "Arial-BoldMT", "TT0"], &reserved);
        assert!(map.is_empty());
    }

    #[test]
    fn test_collision_mapping_disambiguates_against_reserved_and_preserved() {
        // Reserved already contains the naive candidate `OrigHelvetica`, and the
        // preserved set contains `OrigHelvetica_1`; the mapping must skip both.
        let mut reserved = reserved_set(&["Helvetica", "OrigHelvetica"]);
        reserved.insert("Helvetica".to_string());
        let map = collision_font_mapping(["Helvetica", "OrigHelvetica_1"], &reserved);

        let new_name = map.get("Helvetica").expect("colliding key must be renamed");
        assert_ne!(new_name, "OrigHelvetica");
        assert_ne!(new_name, "OrigHelvetica_1");
        assert!(!reserved.contains(new_name));
        // `OrigHelvetica_1` is not reserved, so it is not renamed.
        assert!(!map.contains_key("OrigHelvetica_1"));
    }

    #[test]
    fn test_collision_mapping_overlay_font_key() {
        // A preserved key colliding with an overlay (non-base-14) key is renamed.
        let reserved = reserved_set(&["CustomOverlay"]);
        let map = collision_font_mapping(["CustomOverlay"], &reserved);
        assert_eq!(
            map.get("CustomOverlay").map(String::as_str),
            Some("OrigCustomOverlay")
        );
    }

    #[test]
    fn test_apply_font_rename_map_renames_only_mapped_keys() {
        use crate::objects::{Dictionary, Object};

        let mut fonts = Dictionary::new();
        fonts.set("Helvetica", Object::Integer(1));
        fonts.set("F1", Object::Integer(2));

        let mut map = HashMap::new();
        map.insert("Helvetica".to_string(), "OrigHelvetica".to_string());

        let out = apply_font_rename_map(&fonts, &map);

        assert!(out.contains_key("OrigHelvetica"));
        assert!(!out.contains_key("Helvetica"));
        assert!(out.contains_key("F1"), "unmapped key must be kept as-is");
        assert_eq!(out.get("OrigHelvetica"), Some(&Object::Integer(1)));
        assert_eq!(out.get("F1"), Some(&Object::Integer(2)));
    }

    #[test]
    fn test_rewrite_font_references_hyphenated_font_names() {
        // Font names with hyphens (common in real PDFs: Arial-Bold, etc.)
        let content = b"BT /Arial-Bold 14 Tf (Text) Tj /Times-Italic 12 Tf (More) Tj ET";
        let mut mappings = HashMap::new();
        mappings.insert("Arial-Bold".to_string(), "OrigArial-Bold".to_string());
        mappings.insert("Times-Italic".to_string(), "OrigTimes-Italic".to_string());

        let rewritten = rewrite_font_references(content, &mappings);
        let result = String::from_utf8(rewritten).unwrap();

        assert!(result.contains("/OrigArial-Bold 14 Tf"));
        assert!(result.contains("/OrigTimes-Italic 12 Tf"));
    }

    // Tests for has_embedded_font_data

    #[test]
    fn test_has_embedded_font_data_with_fontfile() {
        use crate::objects::{Dictionary, Object, ObjectId};

        let mut descriptor = Dictionary::new();
        descriptor.set("Type", Object::Name("FontDescriptor".to_string()));
        descriptor.set("FontFile", Object::Reference(ObjectId::new(10, 0))); // Type 1 font

        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name("Font".to_string()));
        font_dict.set("FontDescriptor", Object::Dictionary(descriptor));

        assert!(has_embedded_font_data(&font_dict));
    }

    #[test]
    fn test_has_embedded_font_data_with_fontfile2() {
        use crate::objects::{Dictionary, Object, ObjectId};

        let mut descriptor = Dictionary::new();
        descriptor.set("Type", Object::Name("FontDescriptor".to_string()));
        descriptor.set("FontFile2", Object::Reference(ObjectId::new(20, 0))); // TrueType

        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name("Font".to_string()));
        font_dict.set("FontDescriptor", Object::Dictionary(descriptor));

        assert!(has_embedded_font_data(&font_dict));
    }

    #[test]
    fn test_has_embedded_font_data_with_fontfile3() {
        use crate::objects::{Dictionary, Object, ObjectId};

        let mut descriptor = Dictionary::new();
        descriptor.set("Type", Object::Name("FontDescriptor".to_string()));
        descriptor.set("FontFile3", Object::Reference(ObjectId::new(30, 0))); // CFF/OpenType

        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name("Font".to_string()));
        font_dict.set("FontDescriptor", Object::Dictionary(descriptor));

        assert!(has_embedded_font_data(&font_dict));
    }

    #[test]
    fn test_has_embedded_font_data_descriptor_without_streams() {
        use crate::objects::{Dictionary, Object};

        // FontDescriptor exists but no font streams (unusual but possible)
        let mut descriptor = Dictionary::new();
        descriptor.set("Type", Object::Name("FontDescriptor".to_string()));
        descriptor.set("FontName", Object::Name("Arial".to_string()));
        // NO FontFile/FontFile2/FontFile3

        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name("Font".to_string()));
        font_dict.set("FontDescriptor", Object::Dictionary(descriptor));

        assert!(!has_embedded_font_data(&font_dict));
    }

    #[test]
    fn test_has_embedded_font_data_descriptor_as_reference() {
        use crate::objects::{Dictionary, Object, ObjectId};

        // FontDescriptor is a reference (common in real PDFs)
        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name("Font".to_string()));
        font_dict.set("FontDescriptor", Object::Reference(ObjectId::new(100, 0)));

        // Conservative: assume reference MIGHT have embedded data
        assert!(has_embedded_font_data(&font_dict));
    }

    #[test]
    fn test_has_embedded_font_data_standard_font() {
        use crate::objects::{Dictionary, Object};

        // Standard font (Helvetica) - no FontDescriptor
        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name("Font".to_string()));
        font_dict.set("Subtype", Object::Name("Type1".to_string()));
        font_dict.set("BaseFont", Object::Name("Helvetica".to_string()));
        // NO FontDescriptor

        assert!(!has_embedded_font_data(&font_dict));
    }

    #[test]
    fn test_has_embedded_font_data_multiple_font_files() {
        use crate::objects::{Dictionary, Object, ObjectId};

        // Font with BOTH FontFile2 and FontFile3 (unusual but test it)
        let mut descriptor = Dictionary::new();
        descriptor.set("Type", Object::Name("FontDescriptor".to_string()));
        descriptor.set("FontFile2", Object::Reference(ObjectId::new(10, 0)));
        descriptor.set("FontFile3", Object::Reference(ObjectId::new(11, 0)));

        let mut font_dict = Dictionary::new();
        font_dict.set("Type", Object::Name("Font".to_string()));
        font_dict.set("FontDescriptor", Object::Dictionary(descriptor));

        // Should detect embedded data (has at least one stream)
        assert!(has_embedded_font_data(&font_dict));
    }

    #[test]
    fn test_has_embedded_font_data_empty_dict() {
        use crate::objects::Dictionary;

        // Empty font dictionary
        let font_dict = Dictionary::new();

        assert!(!has_embedded_font_data(&font_dict));
    }
}
