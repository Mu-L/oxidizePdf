//! Shading support for PDF graphics according to ISO 32000-1 Section 8.7.4
//!
//! This module provides basic support for PDF shadings including:
//! - Axial shadings (linear gradients)
//! - Radial shadings (radial gradients)
//! - Function-based shadings
//! - Shading dictionaries and patterns

use crate::error::{PdfError, Result};
use crate::graphics::Color;
use crate::objects::{Dictionary, Object};
use std::collections::HashMap;

/// Shading type enumeration according to ISO 32000-1
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShadingType {
    /// Function-based shading (Type 1)
    FunctionBased = 1,
    /// Axial shading (Type 2) - linear gradient
    Axial = 2,
    /// Radial shading (Type 3) - radial gradient
    Radial = 3,
    /// Free-form Gouraud-shaded triangle mesh (Type 4)
    FreeFormGouraud = 4,
    /// Lattice-form Gouraud-shaded triangle mesh (Type 5)
    LatticeFormGouraud = 5,
    /// Coons patch mesh (Type 6)
    CoonsPatch = 6,
    /// Tensor-product patch mesh (Type 7)
    TensorProductPatch = 7,
}

/// Color stop for gradient definitions
#[derive(Debug, Clone, PartialEq)]
pub struct ColorStop {
    /// Position along gradient (0.0 to 1.0)
    pub position: f64,
    /// Color at this position
    pub color: Color,
}

impl ColorStop {
    /// Create a new color stop
    pub fn new(position: f64, color: Color) -> Self {
        Self {
            position: position.clamp(0.0, 1.0),
            color,
        }
    }
}

/// Resolve the PDF colour space name for a set of stops.
///
/// A shading dictionary carries a single `/ColorSpace` (ISO 32000-1
/// §8.7.4.3, Table 78), so all stops must share one space. If every stop
/// is already in the same device space that space is kept; any mix is
/// promoted to `DeviceRGB` (the lossless common denominator here, since
/// `Color::to_rgb` converts Gray/CMYK exactly for our device spaces).
fn resolve_color_space(stops: &[ColorStop]) -> &'static str {
    match stops.first() {
        Some(first) => {
            let name = first.color.color_space_name();
            if stops.iter().all(|s| s.color.color_space_name() == name) {
                name
            } else {
                "DeviceRGB"
            }
        }
        None => "DeviceRGB",
    }
}

/// Component values of `color` expressed in the given device space.
fn color_components(color: &Color, space: &str) -> Vec<f64> {
    match space {
        "DeviceGray" => vec![match color {
            Color::Gray(g) => *g,
            // `resolve_color_space` only yields "DeviceGray" when every stop
            // is `Color::Gray`, so a non-Gray colour here is a logic bug, not
            // a case to silently approximate.
            other => {
                unreachable!("color_components(DeviceGray) called with non-Gray color: {other:?}")
            }
        }],
        "DeviceCMYK" => {
            let (c, m, y, k) = color.cmyk_components();
            vec![c, m, y, k]
        }
        // DeviceRGB (and any unexpected name) → exact RGB conversion.
        _ => match color.to_rgb() {
            Color::Rgb(r, g, b) => vec![r, g, b],
            _ => unreachable!("to_rgb always yields Color::Rgb"),
        },
    }
}

/// Build a Type 2 (exponential interpolation) function dictionary mapping
/// the parametric domain `[0 1]` linearly from `c0` to `c1`
/// (ISO 32000-1 §7.10.3). Mirrors the Type 2 shape built by
/// `separation_color::TintTransform::to_pdf_dict`, but over `Color` rather
/// than raw component vectors.
fn type2_function(c0: &Color, c1: &Color, space: &str) -> Dictionary {
    let mut dict = Dictionary::new();
    dict.set("FunctionType", Object::Integer(2));
    dict.set(
        "Domain",
        Object::Array(vec![Object::Real(0.0), Object::Real(1.0)]),
    );
    dict.set(
        "C0",
        Object::Array(
            color_components(c0, space)
                .into_iter()
                .map(Object::Real)
                .collect(),
        ),
    );
    dict.set(
        "C1",
        Object::Array(
            color_components(c1, space)
                .into_iter()
                .map(Object::Real)
                .collect(),
        ),
    );
    dict.set("N", Object::Real(1.0));
    dict
}

/// Build the colour-interpolation `/Function` for a gradient from its
/// stops (ISO 32000-1 §7.10, Functions):
/// - 1 stop  → a constant Type 2 (`C0 == C1`),
/// - 2 stops → a single Type 2 (§7.10.3),
/// - N stops → a Type 3 stitching function (§7.10.4) wrapping `N-1` Type 2
///   subfunctions, with `/Bounds` at the interior stop positions and
///   `/Encode` mapping each segment back onto `[0 1]`.
fn build_color_function(stops: &[ColorStop], space: &str) -> Result<Dictionary> {
    match stops {
        [] => Err(PdfError::InvalidStructure(
            "Shading must have at least one color stop".to_string(),
        )),
        [only] => Ok(type2_function(&only.color, &only.color, space)),
        [a, b] => Ok(type2_function(&a.color, &b.color, space)),
        _ => {
            let subfunctions: Vec<Object> = stops
                .windows(2)
                .map(|w| Object::Dictionary(type2_function(&w[0].color, &w[1].color, space)))
                .collect();

            // Interior stop positions become the stitching bounds.
            let bounds: Vec<Object> = stops[1..stops.len() - 1]
                .iter()
                .map(|s| Object::Real(s.position))
                .collect();

            // Each subfunction consumes the full [0 1] sub-domain.
            let encode: Vec<Object> = (0..subfunctions.len())
                .flat_map(|_| [Object::Real(0.0), Object::Real(1.0)])
                .collect();

            let mut dict = Dictionary::new();
            dict.set("FunctionType", Object::Integer(3));
            dict.set(
                "Domain",
                Object::Array(vec![Object::Real(0.0), Object::Real(1.0)]),
            );
            dict.set("Functions", Object::Array(subfunctions));
            dict.set("Bounds", Object::Array(bounds));
            dict.set("Encode", Object::Array(encode));
            Ok(dict)
        }
    }
}

/// Assemble a complete axial/radial shading dictionary with a real,
/// renderable `/Function` and the required `/ColorSpace`. The function is
/// inlined here; the writer hoists it to an indirect object at emit time
/// (issue #297 B) so the dictionary is also valid standalone.
fn assemble_gradient_dict(
    shading_type: ShadingType,
    coords: Vec<Object>,
    stops: &[ColorStop],
    extend_start: bool,
    extend_end: bool,
) -> Result<Dictionary> {
    let space = resolve_color_space(stops);
    let function = build_color_function(stops, space)?;

    let mut dict = Dictionary::new();
    dict.set("ShadingType", Object::Integer(shading_type as i64));
    dict.set("ColorSpace", Object::Name(space.to_string()));
    dict.set("Coords", Object::Array(coords));
    dict.set(
        "Domain",
        Object::Array(vec![Object::Real(0.0), Object::Real(1.0)]),
    );
    dict.set("Function", Object::Dictionary(function));
    dict.set(
        "Extend",
        Object::Array(vec![
            Object::Boolean(extend_start),
            Object::Boolean(extend_end),
        ]),
    );
    Ok(dict)
}

/// MSB-first bit packer for Type 4 mesh vertex streams (ISO 32000-1
/// §8.7.4.5.5). Coordinate/component/flag values are written most-significant-
/// bit first; each vertex is padded to a byte boundary via [`align_to_byte`].
struct BitWriter {
    buffer: Vec<u8>,
    current_byte: u8,
    bits_filled: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            current_byte: 0,
            bits_filled: 0,
        }
    }

    /// Append the low `bits` bits of `value`, most-significant-bit first.
    fn write_bits(&mut self, value: u64, bits: u8) {
        for i in (0..bits).rev() {
            let bit = ((value >> i) & 1) as u8;
            self.current_byte = (self.current_byte << 1) | bit;
            self.bits_filled += 1;
            if self.bits_filled == 8 {
                self.buffer.push(self.current_byte);
                self.current_byte = 0;
                self.bits_filled = 0;
            }
        }
    }

    /// Zero-pad any partial byte up to the next byte boundary.
    fn align_to_byte(&mut self) {
        if self.bits_filled > 0 {
            self.current_byte <<= 8 - self.bits_filled;
            self.buffer.push(self.current_byte);
            self.current_byte = 0;
            self.bits_filled = 0;
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.buffer
    }
}

/// Map a real `value` in `[min, max]` to an unsigned integer of `bits` width
/// for a Type 4 mesh vertex stream (ISO 32000-1 §8.7.4.5.5, `/Decode`). The
/// value is clamped to the range, normalised to `[0, 1]`, then scaled to
/// `2^bits - 1` with round-half-away-from-zero (Rust `f64::round`).
fn encode_value(value: f64, min: f64, max: f64, bits: u8) -> u64 {
    let span = max - min;
    let frac = if span == 0.0 {
        0.0
    } else {
        ((value.clamp(min, max) - min) / span).clamp(0.0, 1.0)
    };
    let max_int = (1u64 << bits) - 1;
    (frac * max_int as f64).round() as u64
}

/// A single vertex of a Type 4 free-form Gouraud-shaded triangle mesh
/// (ISO 32000-1 §8.7.4.5.5). `flag` is the edge flag (0 starts a new
/// triangle; 1 and 2 share an edge with the previous triangle).
#[derive(Debug, Clone, PartialEq)]
pub struct GouraudVertex {
    /// Edge flag (0, 1, or 2).
    pub flag: u8,
    /// X coordinate in shading space.
    pub x: f64,
    /// Y coordinate in shading space.
    pub y: f64,
    /// Vertex colour.
    pub color: Color,
}

/// Pack one mesh vertex into its byte-aligned binary form (ISO 32000-1
/// §8.7.4.5.5): edge flag, then x, y, then colour components, each written at
/// its declared bit width via [`BitWriter`] with coordinates/components mapped
/// through `decode`. Each vertex's data is an integral number of bytes.
fn pack_vertex(
    vertex: &GouraudVertex,
    bits_per_flag: u8,
    bits_per_coordinate: u8,
    bits_per_component: u8,
    decode: &[f64],
    color_space: &str,
) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(vertex.flag as u64, bits_per_flag);
    w.write_bits(
        encode_value(vertex.x, decode[0], decode[1], bits_per_coordinate),
        bits_per_coordinate,
    );
    w.write_bits(
        encode_value(vertex.y, decode[2], decode[3], bits_per_coordinate),
        bits_per_coordinate,
    );
    for (i, comp) in color_components(&vertex.color, color_space)
        .into_iter()
        .enumerate()
    {
        let lo = decode[4 + 2 * i];
        let hi = decode[4 + 2 * i + 1];
        w.write_bits(
            encode_value(comp, lo, hi, bits_per_component),
            bits_per_component,
        );
    }
    w.align_to_byte();
    w.into_bytes()
}

/// Number of colour components for a device colour space name (used to size
/// the `/Decode` array and validate mesh vertex colours).
fn n_components(color_space: &str) -> usize {
    match color_space {
        "DeviceGray" => 1,
        "DeviceCMYK" => 4,
        // DeviceRGB and any unexpected name default to 3-component RGB.
        _ => 3,
    }
}

/// Free-form Gouraud-shaded triangle mesh (Type 4 shading, ISO 32000-1
/// §8.7.4.5.5). Emitted as a PDF stream: the shading dictionary plus a binary
/// body of packed vertex data. Construct with [`FreeFormGouraudShading::new`]
/// (defaulting to 16-bit coordinates and 8-bit components/flags) and adjust the
/// bit widths with [`with_bits`](FreeFormGouraudShading::with_bits).
///
/// Marked `#[non_exhaustive]`: future additive fields (e.g. an optional
/// `/Function` over the vertices) must not break external construction, so
/// build via `new`/`with_bits` rather than a struct literal across crates.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FreeFormGouraudShading {
    /// Shading name for referencing.
    pub name: String,
    /// Device colour space name (`DeviceGray`/`DeviceRGB`/`DeviceCMYK`).
    pub color_space: String,
    /// Bits per coordinate (∈ {1,2,4,8,12,16,24,32}).
    pub bits_per_coordinate: u8,
    /// Bits per colour component (∈ {1,2,4,8,12,16}).
    pub bits_per_component: u8,
    /// Bits per edge flag (∈ {2,4,8}).
    pub bits_per_flag: u8,
    /// Decode array `[xmin xmax ymin ymax c1min c1max …]` (§8.7.4.5.5).
    pub decode: Vec<f64>,
    /// Mesh vertices in emission order.
    pub vertices: Vec<GouraudVertex>,
}

impl FreeFormGouraudShading {
    /// Create a mesh shading with default bit widths (16-bit coordinates,
    /// 8-bit components, 8-bit flags). `decode` must have
    /// `4 + 2 * n_components(color_space)` entries.
    pub fn new(
        name: impl Into<String>,
        color_space: impl Into<String>,
        decode: Vec<f64>,
        vertices: Vec<GouraudVertex>,
    ) -> Self {
        Self {
            name: name.into(),
            color_space: color_space.into(),
            bits_per_coordinate: 16,
            bits_per_component: 8,
            bits_per_flag: 8,
            decode,
            vertices,
        }
    }

    /// Override the packed bit widths.
    pub fn with_bits(
        mut self,
        bits_per_coordinate: u8,
        bits_per_component: u8,
        bits_per_flag: u8,
    ) -> Self {
        self.bits_per_coordinate = bits_per_coordinate;
        self.bits_per_component = bits_per_component;
        self.bits_per_flag = bits_per_flag;
        self
    }

    /// Validate the mesh against the Type 4 constraints (ISO 32000-1
    /// §8.7.4.5.5): permitted bit widths, `/Decode` length matching the colour
    /// space, at least one vertex, and a leading edge flag of 0.
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.bits_per_coordinate, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
            return Err(PdfError::InvalidStructure(format!(
                "BitsPerCoordinate must be 1,2,4,8,12,16,24 or 32, got {}",
                self.bits_per_coordinate
            )));
        }
        if !matches!(self.bits_per_component, 1 | 2 | 4 | 8 | 12 | 16) {
            return Err(PdfError::InvalidStructure(format!(
                "BitsPerComponent must be 1,2,4,8,12 or 16, got {}",
                self.bits_per_component
            )));
        }
        if !matches!(self.bits_per_flag, 2 | 4 | 8) {
            return Err(PdfError::InvalidStructure(format!(
                "BitsPerFlag must be 2, 4 or 8, got {}",
                self.bits_per_flag
            )));
        }
        let expected = 4 + 2 * n_components(&self.color_space);
        if self.decode.len() != expected {
            return Err(PdfError::InvalidStructure(format!(
                "Decode must have {} entries for {}, got {}",
                expected,
                self.color_space,
                self.decode.len()
            )));
        }
        if self.vertices.is_empty() {
            return Err(PdfError::InvalidStructure(
                "Mesh shading must have at least one vertex".to_string(),
            ));
        }
        if self.vertices[0].flag != 0 {
            return Err(PdfError::InvalidStructure(
                "First mesh vertex must have edge flag 0".to_string(),
            ));
        }
        Ok(())
    }

    /// Build the Type 4 shading as a PDF stream object: the shading dictionary
    /// plus the byte-aligned packed vertex data. A mesh cannot be inlined as a
    /// plain dictionary, so this is the emission entry point (the writer hoists
    /// it to an indirect object).
    pub fn to_pdf_object(&self) -> Result<Object> {
        self.validate()?;

        let mut dict = Dictionary::new();
        dict.set(
            "ShadingType",
            Object::Integer(ShadingType::FreeFormGouraud as i64),
        );
        dict.set("ColorSpace", Object::Name(self.color_space.clone()));
        dict.set(
            "BitsPerCoordinate",
            Object::Integer(self.bits_per_coordinate as i64),
        );
        dict.set(
            "BitsPerComponent",
            Object::Integer(self.bits_per_component as i64),
        );
        dict.set("BitsPerFlag", Object::Integer(self.bits_per_flag as i64));
        dict.set(
            "Decode",
            Object::Array(self.decode.iter().map(|&d| Object::Real(d)).collect()),
        );

        let mut data = Vec::new();
        for v in &self.vertices {
            data.extend(pack_vertex(
                v,
                self.bits_per_flag,
                self.bits_per_coordinate,
                self.bits_per_component,
                &self.decode,
                &self.color_space,
            ));
        }

        Ok(Object::Stream(dict, data))
    }
}

/// Assemble a Type 4 (PostScript calculator) function object (ISO 32000-1
/// §7.10.5): a stream whose body is the calculator program `code` verbatim,
/// with the given `/Domain` and `/Range`. Mirrors the shape used elsewhere in
/// the crate (`devicen_color`), returned as `(dict, bytes)` for the writer to
/// hoist to an indirect object.
fn postscript_type4_function(code: &str, domain: &[f64], range: &[f64]) -> (Dictionary, Vec<u8>) {
    let mut dict = Dictionary::new();
    dict.set("FunctionType", Object::Integer(4));
    dict.set(
        "Domain",
        Object::Array(domain.iter().map(|&d| Object::Real(d)).collect()),
    );
    dict.set(
        "Range",
        Object::Array(range.iter().map(|&r| Object::Real(r)).collect()),
    );
    (dict, code.as_bytes().to_vec())
}

/// PostScript that maps a local parameter `t ∈ [0,1]` on the stack to the `n`
/// colour components of a two-stop linear interpolation `start → end`. Ends
/// with the components in order (component 0 deepest).
fn ramp2_ps(start: &Color, end: &Color, space: &str) -> String {
    let s = color_components(start, space);
    let e = color_components(end, space);
    let n = s.len();
    let mut parts = Vec::with_capacity(n);
    for j in 0..n {
        let block = format!("{} mul {} add", e[j] - s[j], s[j]);
        if j + 1 < n {
            // Keep a fresh copy of t for the next component and tuck the result
            // beneath it, preserving output order.
            parts.push(format!("dup {block} exch"));
        } else {
            parts.push(block);
        }
    }
    parts.join(" ")
}

/// PostScript that remaps a global `t` on the stack to a segment-local
/// parameter `(t - lo) / (hi - lo)`.
fn remap_local_ps(lo: f64, hi: f64) -> String {
    format!("{} sub {} div", lo, hi - lo)
}

/// PostScript mapping a global `t ∈ [0,1]` on the stack to colour components
/// across `stops` (≥ 1). One stop → constant colour (discards `t`); two →
/// [`ramp2_ps`]; more → nested `ifelse` split at the interior stop positions,
/// each segment remapped to `[0,1]` then interpolated.
fn build_color_ramp_ps(stops: &[ColorStop], space: &str) -> String {
    match stops {
        [] => String::new(),
        [only] => {
            let mut s = String::from("pop");
            for c in color_components(&only.color, space) {
                s.push_str(&format!(" {c}"));
            }
            s
        }
        [a, b] => ramp2_ps(&a.color, &b.color, space),
        _ => build_ramp_nested_ps(stops, space),
    }
}

/// Recursive nested-`ifelse` ramp for 3+ stops. Each level splits at the next
/// interior stop position; the deepest level (two stops) is a remapped
/// [`ramp2_ps`].
fn build_ramp_nested_ps(stops: &[ColorStop], space: &str) -> String {
    debug_assert!(stops.len() >= 2);
    let lo = stops[0].position;
    let hi = stops[1].position;
    let seg0 = format!(
        "{} {}",
        remap_local_ps(lo, hi),
        ramp2_ps(&stops[0].color, &stops[1].color, space)
    );
    if stops.len() == 2 {
        return seg0;
    }
    let split = stops[1].position;
    let rest = build_ramp_nested_ps(&stops[1..], space);
    format!("dup {split} lt {{ {seg0} }} {{ {rest} }} ifelse")
}

/// PostScript prologue for a conic (angular) gradient: given `x y` on the
/// stack, computes the angle of the vector from `center` and normalises it to
/// `t = angle / 360 ∈ [0,1)`. `atan` (ISO 32000-1 Table 42) takes `num den`
/// and returns `atan2(num, den)` in degrees `[0,360)`; with `dy dx` on the
/// stack it yields the angle of `(dx, dy)`.
fn build_conic_angle_prologue(center: Point) -> String {
    // stack: x y → (y - cy)=dy, exch, (x - cx)=dx → dy dx → atan → /360.
    format!("{} sub exch {} sub atan 360 div", center.y, center.x)
}

/// Conic (angular / "sweep") gradient, emitted as an exact Type 1
/// function-based shading (ISO 32000-1 §8.7.4.5.2) whose `/Function` is a real
/// Type 4 PostScript calculator: the colour is a resolution-independent
/// function of the angle around `center`, not a piecewise mesh approximation.
///
/// Marked `#[non_exhaustive]`: build via [`ConicShading::new`] /
/// [`with_matrix`](ConicShading::with_matrix) so future additive fields stay
/// non-breaking.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConicShading {
    /// Shading name for referencing.
    pub name: String,
    /// Centre of the angular sweep, in the shading's domain coordinates.
    pub center: Point,
    /// Domain `[xmin xmax ymin ymax]` the function is evaluated over.
    pub domain: [f64; 4],
    /// Optional matrix mapping domain space to the shading target space.
    pub matrix: Option<[f64; 6]>,
    /// Colour stops swept from angle 0 (t=0) to a full turn (t=1).
    pub color_stops: Vec<ColorStop>,
}

impl ConicShading {
    /// Create a conic gradient centred at `center` over `domain`.
    pub fn new(
        name: impl Into<String>,
        center: Point,
        domain: [f64; 4],
        color_stops: Vec<ColorStop>,
    ) -> Self {
        Self {
            name: name.into(),
            center,
            domain,
            matrix: None,
            color_stops,
        }
    }

    /// Set the shading-to-target transformation matrix.
    pub fn with_matrix(mut self, matrix: [f64; 6]) -> Self {
        self.matrix = Some(matrix);
        self
    }

    /// Validate stops (non-empty, ascending) and domain (min < max).
    pub fn validate(&self) -> Result<()> {
        if self.color_stops.is_empty() {
            return Err(PdfError::InvalidStructure(
                "Conic shading must have at least one color stop".to_string(),
            ));
        }
        if self.domain[0] >= self.domain[1] || self.domain[2] >= self.domain[3] {
            return Err(PdfError::InvalidStructure(
                "Invalid domain: min values must be less than max values".to_string(),
            ));
        }
        for window in self.color_stops.windows(2) {
            if window[0].position > window[1].position {
                return Err(PdfError::InvalidStructure(
                    "Color stops must be in ascending order".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Build the Type 1 function-based shading dictionary with a real Type 4
    /// PostScript `/Function` (angle prologue + colour ramp) and the required
    /// `/ColorSpace`. The `/Function` is inlined as a stream; the writer hoists
    /// it to an indirect object (a stream cannot be a dictionary value).
    pub fn to_pdf_dictionary(&self) -> Result<Dictionary> {
        self.validate()?;
        let space = resolve_color_space(&self.color_stops);
        let code = format!(
            "{{ {} {} }}",
            build_conic_angle_prologue(self.center),
            build_color_ramp_ps(&self.color_stops, space)
        );
        let range: Vec<f64> = (0..n_components(space)).flat_map(|_| [0.0, 1.0]).collect();
        let (fdict, fbytes) = postscript_type4_function(&code, &self.domain, &range);

        let mut dict = Dictionary::new();
        dict.set(
            "ShadingType",
            Object::Integer(ShadingType::FunctionBased as i64),
        );
        dict.set("ColorSpace", Object::Name(space.to_string()));
        dict.set(
            "Domain",
            Object::Array(self.domain.iter().map(|&d| Object::Real(d)).collect()),
        );
        dict.set("Function", Object::Stream(fdict, fbytes));
        if let Some(matrix) = self.matrix {
            dict.set(
                "Matrix",
                Object::Array(matrix.iter().map(|&v| Object::Real(v)).collect()),
            );
        }
        Ok(dict)
    }
}

/// Internal wrapper for the additive shading types (Type 4 mesh, Type 1 conic)
/// registered via [`Page::add_mesh_shading`](crate::Page::add_mesh_shading) and
/// [`Page::add_conic_shading`](crate::Page::add_conic_shading). Kept in a
/// separate page collection from [`ShadingDefinition`] so the public gradient
/// enum stays unchanged (folding these in is a 5.0.0 breaking-bundle item).
#[derive(Debug, Clone)]
pub(crate) enum AdvancedShading {
    /// Type 4 free-form Gouraud mesh (emitted as a stream).
    Mesh(FreeFormGouraudShading),
    /// Type 1 conic gradient (emitted as a dictionary with a `/Function`
    /// stream the writer hoists).
    Conic(ConicShading),
}

impl AdvancedShading {
    /// Emit the shading as a PDF object: a stream for the mesh, a dictionary
    /// for the conic.
    pub(crate) fn to_pdf_object(&self) -> Result<Object> {
        match self {
            AdvancedShading::Mesh(m) => m.to_pdf_object(),
            AdvancedShading::Conic(c) => Ok(Object::Dictionary(c.to_pdf_dictionary()?)),
        }
    }
}

/// Coordinate point for shading definitions
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    /// Create a new point
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Axial (linear) shading definition
#[derive(Debug, Clone)]
pub struct AxialShading {
    /// Shading name for referencing
    pub name: String,
    /// Start point of the gradient
    pub start_point: Point,
    /// End point of the gradient
    pub end_point: Point,
    /// Color stops along the gradient
    pub color_stops: Vec<ColorStop>,
    /// Whether to extend beyond the start point
    pub extend_start: bool,
    /// Whether to extend beyond the end point
    pub extend_end: bool,
}

impl AxialShading {
    /// Create a new axial shading
    pub fn new(
        name: String,
        start_point: Point,
        end_point: Point,
        color_stops: Vec<ColorStop>,
    ) -> Self {
        Self {
            name,
            start_point,
            end_point,
            color_stops,
            extend_start: false,
            extend_end: false,
        }
    }

    /// Set extension options
    pub fn with_extend(mut self, extend_start: bool, extend_end: bool) -> Self {
        self.extend_start = extend_start;
        self.extend_end = extend_end;
        self
    }

    /// Create a simple two-color linear gradient
    pub fn linear_gradient(
        name: String,
        start_point: Point,
        end_point: Point,
        start_color: Color,
        end_color: Color,
    ) -> Self {
        let color_stops = vec![
            ColorStop::new(0.0, start_color),
            ColorStop::new(1.0, end_color),
        ];

        Self::new(name, start_point, end_point, color_stops)
    }

    /// Generate PDF shading dictionary (ISO 32000-1 §8.7.4.3, Table 78).
    ///
    /// Emits a real `/Function` interpolating the `color_stops` and the
    /// required `/ColorSpace`. The function is inlined; the writer hoists
    /// it to an indirect object when emitting the page (issue #297).
    pub fn to_pdf_dictionary(&self) -> Result<Dictionary> {
        let coords = vec![
            Object::Real(self.start_point.x),
            Object::Real(self.start_point.y),
            Object::Real(self.end_point.x),
            Object::Real(self.end_point.y),
        ];
        assemble_gradient_dict(
            ShadingType::Axial,
            coords,
            &self.color_stops,
            self.extend_start,
            self.extend_end,
        )
    }

    /// Validate axial shading parameters
    pub fn validate(&self) -> Result<()> {
        if self.color_stops.is_empty() {
            return Err(PdfError::InvalidStructure(
                "Axial shading must have at least one color stop".to_string(),
            ));
        }

        // Check that color stops are in order
        for window in self.color_stops.windows(2) {
            if window[0].position > window[1].position {
                return Err(PdfError::InvalidStructure(
                    "Color stops must be in ascending order".to_string(),
                ));
            }
        }

        // Check start and end points are different
        if (self.start_point.x - self.end_point.x).abs() < f64::EPSILON
            && (self.start_point.y - self.end_point.y).abs() < f64::EPSILON
        {
            return Err(PdfError::InvalidStructure(
                "Start and end points cannot be the same".to_string(),
            ));
        }

        Ok(())
    }
}

/// Radial shading definition
#[derive(Debug, Clone)]
pub struct RadialShading {
    /// Shading name for referencing
    pub name: String,
    /// Center point of the start circle
    pub start_center: Point,
    /// Radius of the start circle
    pub start_radius: f64,
    /// Center point of the end circle
    pub end_center: Point,
    /// Radius of the end circle
    pub end_radius: f64,
    /// Color stops along the gradient
    pub color_stops: Vec<ColorStop>,
    /// Whether to extend beyond the start circle
    pub extend_start: bool,
    /// Whether to extend beyond the end circle
    pub extend_end: bool,
}

impl RadialShading {
    /// Create a new radial shading
    pub fn new(
        name: String,
        start_center: Point,
        start_radius: f64,
        end_center: Point,
        end_radius: f64,
        color_stops: Vec<ColorStop>,
    ) -> Self {
        Self {
            name,
            start_center,
            start_radius: start_radius.max(0.0),
            end_center,
            end_radius: end_radius.max(0.0),
            color_stops,
            extend_start: false,
            extend_end: false,
        }
    }

    /// Set extension options
    pub fn with_extend(mut self, extend_start: bool, extend_end: bool) -> Self {
        self.extend_start = extend_start;
        self.extend_end = extend_end;
        self
    }

    /// Create a simple two-color radial gradient
    pub fn radial_gradient(
        name: String,
        center: Point,
        start_radius: f64,
        end_radius: f64,
        start_color: Color,
        end_color: Color,
    ) -> Self {
        let color_stops = vec![
            ColorStop::new(0.0, start_color),
            ColorStop::new(1.0, end_color),
        ];

        Self::new(name, center, start_radius, center, end_radius, color_stops)
    }

    /// Generate PDF shading dictionary (ISO 32000-1 §8.7.4.4, Table 79).
    ///
    /// Emits a real `/Function` interpolating the `color_stops` and the
    /// required `/ColorSpace`. The function is inlined; the writer hoists
    /// it to an indirect object when emitting the page (issue #297).
    pub fn to_pdf_dictionary(&self) -> Result<Dictionary> {
        let coords = vec![
            Object::Real(self.start_center.x),
            Object::Real(self.start_center.y),
            Object::Real(self.start_radius),
            Object::Real(self.end_center.x),
            Object::Real(self.end_center.y),
            Object::Real(self.end_radius),
        ];
        assemble_gradient_dict(
            ShadingType::Radial,
            coords,
            &self.color_stops,
            self.extend_start,
            self.extend_end,
        )
    }

    /// Validate radial shading parameters
    pub fn validate(&self) -> Result<()> {
        if self.color_stops.is_empty() {
            return Err(PdfError::InvalidStructure(
                "Radial shading must have at least one color stop".to_string(),
            ));
        }

        // Check that color stops are in order
        for window in self.color_stops.windows(2) {
            if window[0].position > window[1].position {
                return Err(PdfError::InvalidStructure(
                    "Color stops must be in ascending order".to_string(),
                ));
            }
        }

        // Check for valid radii
        if self.start_radius < 0.0 || self.end_radius < 0.0 {
            return Err(PdfError::InvalidStructure(
                "Radii cannot be negative".to_string(),
            ));
        }

        Ok(())
    }
}

/// Function-based shading definition (simplified)
#[derive(Debug, Clone)]
pub struct FunctionBasedShading {
    /// Shading name for referencing
    pub name: String,
    /// Domain of the function [xmin, xmax, ymin, ymax]
    pub domain: [f64; 4],
    /// Transformation matrix
    pub matrix: Option<[f64; 6]>,
    /// Function reference (placeholder)
    pub function_id: u32,
}

impl FunctionBasedShading {
    /// Create a new function-based shading
    pub fn new(name: String, domain: [f64; 4], function_id: u32) -> Self {
        Self {
            name,
            domain,
            matrix: None,
            function_id,
        }
    }

    /// Set transformation matrix
    pub fn with_matrix(mut self, matrix: [f64; 6]) -> Self {
        self.matrix = Some(matrix);
        self
    }

    /// Generate PDF shading dictionary
    pub fn to_pdf_dictionary(&self) -> Result<Dictionary> {
        let mut shading_dict = Dictionary::new();

        // Basic shading properties
        shading_dict.set(
            "ShadingType",
            Object::Integer(ShadingType::FunctionBased as i64),
        );

        // Domain array
        let domain = vec![
            Object::Real(self.domain[0]),
            Object::Real(self.domain[1]),
            Object::Real(self.domain[2]),
            Object::Real(self.domain[3]),
        ];
        shading_dict.set("Domain", Object::Array(domain));

        // Matrix (if specified)
        if let Some(matrix) = self.matrix {
            let matrix_objects: Vec<Object> = matrix.iter().map(|&x| Object::Real(x)).collect();
            shading_dict.set("Matrix", Object::Array(matrix_objects));
        }

        // Function reference
        shading_dict.set("Function", Object::Integer(self.function_id as i64));

        Ok(shading_dict)
    }

    /// Validate function-based shading parameters
    pub fn validate(&self) -> Result<()> {
        // Check domain validity
        if self.domain[0] >= self.domain[1] || self.domain[2] >= self.domain[3] {
            return Err(PdfError::InvalidStructure(
                "Invalid domain: min values must be less than max values".to_string(),
            ));
        }

        Ok(())
    }
}

/// Shading pattern that combines a shading with pattern properties
#[derive(Debug, Clone)]
pub struct ShadingPattern {
    /// Pattern name for referencing
    pub name: String,
    /// The underlying shading
    pub shading: ShadingDefinition,
    /// Pattern transformation matrix
    pub matrix: Option<[f64; 6]>,
}

/// Enumeration of different shading types
#[derive(Debug, Clone)]
pub enum ShadingDefinition {
    /// Axial (linear) shading
    Axial(AxialShading),
    /// Radial shading
    Radial(RadialShading),
    /// Function-based shading
    FunctionBased(FunctionBasedShading),
}

impl ShadingDefinition {
    /// Get the name of the shading
    pub fn name(&self) -> &str {
        match self {
            ShadingDefinition::Axial(shading) => &shading.name,
            ShadingDefinition::Radial(shading) => &shading.name,
            ShadingDefinition::FunctionBased(shading) => &shading.name,
        }
    }

    /// Validate the shading
    pub fn validate(&self) -> Result<()> {
        match self {
            ShadingDefinition::Axial(shading) => shading.validate(),
            ShadingDefinition::Radial(shading) => shading.validate(),
            ShadingDefinition::FunctionBased(shading) => shading.validate(),
        }
    }

    /// Generate PDF shading dictionary
    pub fn to_pdf_dictionary(&self) -> Result<Dictionary> {
        match self {
            ShadingDefinition::Axial(shading) => shading.to_pdf_dictionary(),
            ShadingDefinition::Radial(shading) => shading.to_pdf_dictionary(),
            ShadingDefinition::FunctionBased(shading) => shading.to_pdf_dictionary(),
        }
    }
}

impl ShadingPattern {
    /// Create a new shading pattern
    pub fn new(name: String, shading: ShadingDefinition) -> Self {
        Self {
            name,
            shading,
            matrix: None,
        }
    }

    /// Set pattern transformation matrix
    pub fn with_matrix(mut self, matrix: [f64; 6]) -> Self {
        self.matrix = Some(matrix);
        self
    }

    /// Generate PDF pattern dictionary for shading pattern.
    ///
    /// NOTE: `ShadingPattern` is not yet wired through `Page` → writer (there
    /// is no `Page::add_shading_pattern` and the writer iterates only
    /// `page.shadings()`), so this method is not exercised by the
    /// serialisation pipeline today. The `sh` direct-paint path
    /// ([`GraphicsContext::paint_shading`] over [`Page::add_shading`]) is the
    /// wired, end-to-end gradient path. Because the inlined `/Shading` here
    /// carries its `/Function` inline (the writer's indirect-hoist only
    /// applies to `page.shadings()`), full PatternType-2 fill support remains
    /// a follow-up.
    pub fn to_pdf_pattern_dictionary(&self) -> Result<Dictionary> {
        let mut pattern_dict = Dictionary::new();

        // Pattern properties
        pattern_dict.set("Type", Object::Name("Pattern".to_string()));
        pattern_dict.set("PatternType", Object::Integer(2)); // Shading pattern

        // Inline the real shading dictionary (issue #297 C). A PatternType 2
        // /Shading may be a dictionary or an indirect reference (ISO 32000-1
        // §8.7.3.3, Table 76); inlining keeps the pattern self-contained and
        // renderable instead of the old `Object::Integer(1)` placeholder.
        pattern_dict.set(
            "Shading",
            Object::Dictionary(self.shading.to_pdf_dictionary()?),
        );

        // Matrix (if specified)
        if let Some(matrix) = self.matrix {
            let matrix_objects: Vec<Object> = matrix.iter().map(|&x| Object::Real(x)).collect();
            pattern_dict.set("Matrix", Object::Array(matrix_objects));
        }

        Ok(pattern_dict)
    }

    /// Validate shading pattern
    pub fn validate(&self) -> Result<()> {
        self.shading.validate()
    }
}

/// Shading manager for handling multiple shadings
#[derive(Debug, Clone)]
pub struct ShadingManager {
    /// Stored shadings
    shadings: HashMap<String, ShadingDefinition>,
    /// Stored shading patterns
    patterns: HashMap<String, ShadingPattern>,
    /// Next shading ID
    next_id: usize,
}

impl Default for ShadingManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ShadingManager {
    /// Create a new shading manager
    pub fn new() -> Self {
        Self {
            shadings: HashMap::new(),
            patterns: HashMap::new(),
            next_id: 1,
        }
    }

    /// Add a shading
    pub fn add_shading(&mut self, mut shading: ShadingDefinition) -> Result<String> {
        // Validate shading before adding
        shading.validate()?;

        let name = shading.name().to_string();

        // Generate unique name if empty or already exists
        let final_name = if name.is_empty() || self.shadings.contains_key(&name) {
            let auto_name = format!("Sh{}", self.next_id);
            self.next_id += 1;

            // Update the shading name
            match &mut shading {
                ShadingDefinition::Axial(s) => s.name = auto_name.clone(),
                ShadingDefinition::Radial(s) => s.name = auto_name.clone(),
                ShadingDefinition::FunctionBased(s) => s.name = auto_name.clone(),
            }

            auto_name
        } else {
            name
        };

        self.shadings.insert(final_name.clone(), shading);
        Ok(final_name)
    }

    /// Add a shading pattern
    pub fn add_shading_pattern(&mut self, mut pattern: ShadingPattern) -> Result<String> {
        // Validate pattern before adding
        pattern.validate()?;

        // Generate unique name if empty or already exists
        if pattern.name.is_empty() || self.patterns.contains_key(&pattern.name) {
            pattern.name = format!("SP{}", self.next_id);
            self.next_id += 1;
        }

        let name = pattern.name.clone();
        self.patterns.insert(name.clone(), pattern);
        Ok(name)
    }

    /// Get a shading by name
    pub fn get_shading(&self, name: &str) -> Option<&ShadingDefinition> {
        self.shadings.get(name)
    }

    /// Get a shading pattern by name
    pub fn get_pattern(&self, name: &str) -> Option<&ShadingPattern> {
        self.patterns.get(name)
    }

    /// Get all shadings
    pub fn shadings(&self) -> &HashMap<String, ShadingDefinition> {
        &self.shadings
    }

    /// Get all patterns
    pub fn patterns(&self) -> &HashMap<String, ShadingPattern> {
        &self.patterns
    }

    /// Clear all shadings and patterns
    pub fn clear(&mut self) {
        self.shadings.clear();
        self.patterns.clear();
        self.next_id = 1;
    }

    /// Count of registered shadings
    pub fn shading_count(&self) -> usize {
        self.shadings.len()
    }

    /// Count of registered patterns
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// Total count of all items
    pub fn total_count(&self) -> usize {
        self.shading_count() + self.pattern_count()
    }

    /// Create a simple linear gradient
    pub fn create_linear_gradient(
        &mut self,
        start_point: Point,
        end_point: Point,
        start_color: Color,
        end_color: Color,
    ) -> Result<String> {
        let shading = ShadingDefinition::Axial(AxialShading::linear_gradient(
            String::new(), // Auto-generated name
            start_point,
            end_point,
            start_color,
            end_color,
        ));

        self.add_shading(shading)
    }

    /// Create a simple radial gradient
    pub fn create_radial_gradient(
        &mut self,
        center: Point,
        start_radius: f64,
        end_radius: f64,
        start_color: Color,
        end_color: Color,
    ) -> Result<String> {
        let shading = ShadingDefinition::Radial(RadialShading::radial_gradient(
            String::new(), // Auto-generated name
            center,
            start_radius,
            end_radius,
            start_color,
            end_color,
        ));

        self.add_shading(shading)
    }

    /// Generate shading resource dictionary for PDF
    pub fn to_resource_dictionary(&self) -> Result<String> {
        if self.shadings.is_empty() && self.patterns.is_empty() {
            return Ok(String::new());
        }

        let mut dict = String::new();

        // Shadings
        if !self.shadings.is_empty() {
            dict.push_str("/Shading <<");
            for name in self.shadings.keys() {
                dict.push_str(&format!(" /{} {} 0 R", name, self.next_id));
            }
            dict.push_str(" >>");
        }

        // Patterns
        if !self.patterns.is_empty() {
            if !dict.is_empty() {
                dict.push('\n');
            }
            dict.push_str("/Pattern <<");
            for name in self.patterns.keys() {
                dict.push_str(&format!(" /{} {} 0 R", name, self.next_id));
            }
            dict.push_str(" >>");
        }

        Ok(dict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Issue #407 Track A: BitWriter (MSB-first packing for mesh vertex data) ──

    #[test]
    fn test_bitwriter_single_value_byte_aligned() {
        // 4 bits 0b1011 then pad to a byte → 0b1011_0000.
        let mut w = BitWriter::new();
        w.write_bits(0b1011, 4);
        w.align_to_byte();
        assert_eq!(w.into_bytes(), vec![0xB0]);
    }

    #[test]
    fn test_bitwriter_value_spans_two_bytes() {
        // 9-bit value 0x1FF crosses the byte boundary: 1111_1111 | 1___ then pad.
        let mut w = BitWriter::new();
        w.write_bits(0b1_1111_1111, 9);
        w.align_to_byte();
        assert_eq!(w.into_bytes(), vec![0xFF, 0x80]);
    }

    #[test]
    fn test_bitwriter_accumulates_across_writes() {
        // 0b10 then 0b110 → 0b10110, padded to 0b1011_0000.
        let mut w = BitWriter::new();
        w.write_bits(0b10, 2);
        w.write_bits(0b110, 3);
        w.align_to_byte();
        assert_eq!(w.into_bytes(), vec![0xB0]);
    }

    #[test]
    fn test_encode_value_maps_real_to_packed_integer() {
        // 50 in [0,100] over 8 bits → 0.5 * 255 = 127.5 → round half away → 128.
        assert_eq!(encode_value(50.0, 0.0, 100.0, 8), 128);
    }

    #[test]
    fn test_encode_value_clamps_out_of_range() {
        assert_eq!(encode_value(150.0, 0.0, 100.0, 8), 255);
        assert_eq!(encode_value(-10.0, 0.0, 100.0, 8), 0);
        assert_eq!(encode_value(100.0, 0.0, 100.0, 8), 255);
    }

    #[test]
    fn test_encode_value_16_bit_precision() {
        // 0.5 in [0,1] over 16 bits → 0.5 * 65535 = 32767.5 → round half away → 32768.
        assert_eq!(encode_value(0.5, 0.0, 1.0, 16), 32768);
    }

    #[test]
    fn test_gouraud_vertex_pack_byte_aligned() {
        // flag(8) + x,y(16 each) + 3 rgb components(8 each) = 64 bits = 8 bytes.
        let v = GouraudVertex {
            flag: 0,
            x: 10.0,
            y: 20.0,
            color: Color::Rgb(1.0, 0.0, 0.0),
        };
        let decode = [0.0, 100.0, 0.0, 100.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        let bytes = pack_vertex(&v, 8, 16, 8, &decode, "DeviceRGB");
        // x=10→0.1*65535=6554=0x199A, y=20→0x3333, r=255,g=0,b=0.
        assert_eq!(bytes, vec![0x00, 0x19, 0x9A, 0x33, 0x33, 0xFF, 0x00, 0x00]);
    }

    #[test]
    fn test_gouraud_vertex_pack_with_padding() {
        // flag(2) + x,y(8 each) + 1 gray component(8) = 26 bits → 4 bytes (6 pad).
        let v = GouraudVertex {
            flag: 1,
            x: 50.0,
            y: 25.0,
            color: Color::Gray(0.5),
        };
        let decode = [0.0, 100.0, 0.0, 100.0, 0.0, 1.0];
        let bytes = pack_vertex(&v, 2, 8, 8, &decode, "DeviceGray");
        // flag=01, x=128=0x80, y=64=0x40, gray=128=0x80 → 01 10000000 01000000 10000000.
        assert_eq!(bytes, vec![0x60, 0x10, 0x20, 0x00]);
    }

    /// Three valid RGB vertices for the mesh-level tests (flags 0,1,1).
    fn sample_rgb_mesh() -> FreeFormGouraudShading {
        FreeFormGouraudShading::new(
            "M".to_string(),
            "DeviceRGB".to_string(),
            vec![0.0, 100.0, 0.0, 100.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            vec![
                GouraudVertex {
                    flag: 0,
                    x: 10.0,
                    y: 20.0,
                    color: Color::Rgb(1.0, 0.0, 0.0),
                },
                GouraudVertex {
                    flag: 1,
                    x: 50.0,
                    y: 50.0,
                    color: Color::Rgb(0.0, 1.0, 0.0),
                },
                GouraudVertex {
                    flag: 1,
                    x: 90.0,
                    y: 10.0,
                    color: Color::Rgb(0.0, 0.0, 1.0),
                },
            ],
        )
    }

    #[test]
    fn test_freeform_gouraud_creation_defaults() {
        let mesh = sample_rgb_mesh();
        assert!(mesh.validate().is_ok());
        assert_eq!(mesh.name, "M");
        assert_eq!(mesh.color_space, "DeviceRGB");
        // new() defaults: 16-bit coords, 8-bit components, 8-bit flags.
        assert_eq!(mesh.bits_per_coordinate, 16);
        assert_eq!(mesh.bits_per_component, 8);
        assert_eq!(mesh.bits_per_flag, 8);
        assert_eq!(mesh.vertices.len(), 3);
    }

    #[test]
    fn test_freeform_gouraud_validate_rejects_invalid_bits() {
        let mut m = sample_rgb_mesh();
        m.bits_per_coordinate = 5; // not in {1,2,4,8,12,16,24,32}
        assert!(m.validate().is_err());

        let mut m = sample_rgb_mesh();
        m.bits_per_component = 3; // not in {1,2,4,8,12,16}
        assert!(m.validate().is_err());

        let mut m = sample_rgb_mesh();
        m.bits_per_flag = 3; // not in {2,4,8}
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_freeform_gouraud_validate_rejects_decode_length_mismatch() {
        let mut m = sample_rgb_mesh();
        // DeviceRGB needs 4 + 2*3 = 10 entries; give 8.
        m.decode = vec![0.0, 100.0, 0.0, 100.0, 0.0, 1.0, 0.0, 1.0];
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_freeform_gouraud_validate_rejects_empty_vertices() {
        let mut m = sample_rgb_mesh();
        m.vertices.clear();
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_freeform_gouraud_validate_rejects_nonzero_first_flag() {
        let mut m = sample_rgb_mesh();
        m.vertices[0].flag = 1;
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_freeform_gouraud_to_pdf_object_dict_keys() {
        let obj = sample_rgb_mesh().to_pdf_object().unwrap();
        let dict = match &obj {
            Object::Stream(d, _) => d,
            other => panic!("mesh must emit a Stream, got {other:?}"),
        };
        assert_eq!(dict.get("ShadingType"), Some(&Object::Integer(4)));
        assert_eq!(
            dict.get("ColorSpace"),
            Some(&Object::Name("DeviceRGB".to_string()))
        );
        assert_eq!(dict.get("BitsPerCoordinate"), Some(&Object::Integer(16)));
        assert_eq!(dict.get("BitsPerComponent"), Some(&Object::Integer(8)));
        assert_eq!(dict.get("BitsPerFlag"), Some(&Object::Integer(8)));
        assert_eq!(
            dict.get("Decode"),
            Some(&Object::Array(
                vec![0.0, 100.0, 0.0, 100.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]
                    .into_iter()
                    .map(Object::Real)
                    .collect()
            ))
        );
    }

    #[test]
    fn test_freeform_gouraud_stream_body_exact_bytes() {
        let obj = sample_rgb_mesh().to_pdf_object().unwrap();
        let data = match &obj {
            Object::Stream(_, d) => d,
            other => panic!("mesh must emit a Stream, got {other:?}"),
        };
        assert_eq!(
            *data,
            vec![
                // v0: flag0, x10, y20, red
                0x00, 0x19, 0x9A, 0x33, 0x33, 0xFF, 0x00, 0x00, //
                // v1: flag1, x50, y50, green
                0x01, 0x80, 0x00, 0x80, 0x00, 0x00, 0xFF, 0x00, //
                // v2: flag1, x90, y10, blue
                0x01, 0xE6, 0x66, 0x19, 0x9A, 0x00, 0x00, 0xFF,
            ]
        );
    }

    // ── Issue #407 Track B: Type 1 conic (PostScript Type 4 function) ──

    #[test]
    fn test_postscript_type4_function_shape() {
        // Wraps arbitrary calculator code with FunctionType 4 + Domain/Range;
        // the code bytes are stored verbatim (no transformation).
        let (dict, code) = postscript_type4_function(
            "{ 1 }",
            &[0.0, 1.0, 0.0, 1.0],
            &[0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
        );
        assert_eq!(dict.get("FunctionType"), Some(&Object::Integer(4)));
        assert_eq!(
            dict.get("Domain"),
            Some(&Object::Array(
                vec![0.0, 1.0, 0.0, 1.0]
                    .into_iter()
                    .map(Object::Real)
                    .collect()
            ))
        );
        assert_eq!(
            dict.get("Range"),
            Some(&Object::Array(
                vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0]
                    .into_iter()
                    .map(Object::Real)
                    .collect()
            ))
        );
        assert_eq!(code, b"{ 1 }");
    }

    #[test]
    fn test_color_ramp_ps_two_stops_no_branching() {
        // 2 stops → straight per-component linear interpolation of a local t on
        // the stack, no ifelse. red→blue over DeviceRGB: deltas (-1, 0, 1).
        let stops = vec![
            ColorStop::new(0.0, Color::Rgb(1.0, 0.0, 0.0)),
            ColorStop::new(1.0, Color::Rgb(0.0, 0.0, 1.0)),
        ];
        let ps = build_color_ramp_ps(&stops, "DeviceRGB");
        assert_eq!(ps, "dup -1 mul 1 add exch dup 0 mul 0 add exch 1 mul 0 add");
        assert!(!ps.contains("ifelse"));
    }

    #[test]
    fn test_color_ramp_ps_three_stops_has_bound_check() {
        // 3 stops → one ifelse splitting at the interior stop (0.5), two
        // interpolation segments (3 muls each over DeviceRGB → 6 total).
        let stops = vec![
            ColorStop::new(0.0, Color::red()),
            ColorStop::new(0.5, Color::green()),
            ColorStop::new(1.0, Color::blue()),
        ];
        let ps = build_color_ramp_ps(&stops, "DeviceRGB");
        assert_eq!(ps.matches("ifelse").count(), 1, "one split for three stops");
        assert!(ps.contains("0.5"), "interior bound present");
        assert_eq!(ps.matches("mul").count(), 6, "two RGB segments");
    }

    #[test]
    fn test_conic_angle_prologue_exact_ps() {
        // stack in: x y. dy=y-cy, dx=x-cx, angle=atan2(dy,dx), t=angle/360.
        let ps = build_conic_angle_prologue(Point::new(50.0, 50.0));
        assert_eq!(ps, "50 sub exch 50 sub atan 360 div");
    }

    #[test]
    fn test_conic_shading_emits_type1_with_ps_function_and_colorspace() {
        let stops = vec![
            ColorStop::new(0.0, Color::red()),
            ColorStop::new(1.0, Color::blue()),
        ];
        let conic = ConicShading::new(
            "C".to_string(),
            Point::new(50.0, 50.0),
            [0.0, 100.0, 0.0, 100.0],
            stops.clone(),
        );
        let dict = conic.to_pdf_dictionary().unwrap();

        // Function-based (Type 1) shading with the required ColorSpace + Domain.
        assert_eq!(dict.get("ShadingType"), Some(&Object::Integer(1)));
        assert_eq!(
            dict.get("ColorSpace"),
            Some(&Object::Name("DeviceRGB".to_string()))
        );
        assert_eq!(
            dict.get("Domain"),
            Some(&Object::Array(
                vec![0.0, 100.0, 0.0, 100.0]
                    .into_iter()
                    .map(Object::Real)
                    .collect()
            ))
        );

        // /Function is a real Type 4 PostScript stream, not a placeholder.
        let (fdict, fcode) = match dict.get("Function") {
            Some(Object::Stream(d, c)) => (d, c),
            other => panic!("Function must be a Type 4 stream, got {other:?}"),
        };
        assert_eq!(fdict.get("FunctionType"), Some(&Object::Integer(4)));
        // Function domain == shading domain (2 inputs x, y).
        assert_eq!(
            fdict.get("Domain"),
            Some(&Object::Array(
                vec![0.0, 100.0, 0.0, 100.0]
                    .into_iter()
                    .map(Object::Real)
                    .collect()
            ))
        );
        // Range == n_components pairs of [0, 1] (3 for RGB).
        assert_eq!(
            fdict.get("Range"),
            Some(&Object::Array(
                vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0]
                    .into_iter()
                    .map(Object::Real)
                    .collect()
            ))
        );
        // Program == "{ <angle prologue> <colour ramp> }".
        let expected = format!(
            "{{ {} {} }}",
            build_conic_angle_prologue(Point::new(50.0, 50.0)),
            build_color_ramp_ps(&stops, "DeviceRGB")
        );
        assert_eq!(fcode, &expected.into_bytes());
    }

    #[test]
    fn test_conic_shading_validate_rejects_bad_domain_and_empty_stops() {
        let stops = vec![
            ColorStop::new(0.0, Color::red()),
            ColorStop::new(1.0, Color::blue()),
        ];
        let bad_domain = ConicShading::new(
            "C".to_string(),
            Point::new(0.0, 0.0),
            [1.0, 0.0, 0.0, 1.0], // xmin > xmax
            stops,
        );
        assert!(bad_domain.validate().is_err());

        let empty = ConicShading::new(
            "C".to_string(),
            Point::new(0.0, 0.0),
            [0.0, 1.0, 0.0, 1.0],
            vec![],
        );
        assert!(empty.validate().is_err());
    }

    #[test]
    fn test_color_stop_creation() {
        let stop = ColorStop::new(0.5, Color::red());
        assert_eq!(stop.position, 0.5);
        assert_eq!(stop.color, Color::red());

        // Test clamping
        let stop_clamped = ColorStop::new(1.5, Color::blue());
        assert_eq!(stop_clamped.position, 1.0);
    }

    #[test]
    fn test_point_creation() {
        let point = Point::new(10.0, 20.0);
        assert_eq!(point.x, 10.0);
        assert_eq!(point.y, 20.0);
    }

    #[test]
    fn test_axial_shading_creation() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 100.0);
        let stops = vec![
            ColorStop::new(0.0, Color::red()),
            ColorStop::new(1.0, Color::blue()),
        ];

        let shading = AxialShading::new("TestGradient".to_string(), start, end, stops);
        assert_eq!(shading.name, "TestGradient");
        assert_eq!(shading.start_point, start);
        assert_eq!(shading.end_point, end);
        assert_eq!(shading.color_stops.len(), 2);
        assert!(!shading.extend_start);
        assert!(!shading.extend_end);
    }

    #[test]
    fn test_axial_shading_linear_gradient() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 0.0);
        let shading = AxialShading::linear_gradient(
            "LinearGrad".to_string(),
            start,
            end,
            Color::red(),
            Color::blue(),
        );

        assert_eq!(shading.color_stops.len(), 2);
        assert_eq!(shading.color_stops[0].position, 0.0);
        assert_eq!(shading.color_stops[1].position, 1.0);
    }

    #[test]
    fn test_axial_shading_with_extend() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 0.0);
        let shading = AxialShading::linear_gradient(
            "ExtendedGrad".to_string(),
            start,
            end,
            Color::red(),
            Color::blue(),
        )
        .with_extend(true, true);

        assert!(shading.extend_start);
        assert!(shading.extend_end);
    }

    #[test]
    fn test_axial_shading_validation_valid() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 0.0);
        let shading = AxialShading::linear_gradient(
            "ValidGrad".to_string(),
            start,
            end,
            Color::red(),
            Color::blue(),
        );

        assert!(shading.validate().is_ok());
    }

    #[test]
    fn test_axial_shading_validation_no_stops() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 0.0);
        let shading = AxialShading::new("EmptyGrad".to_string(), start, end, Vec::new());

        assert!(shading.validate().is_err());
    }

    #[test]
    fn test_axial_shading_validation_same_points() {
        let point = Point::new(50.0, 50.0);
        let shading = AxialShading::linear_gradient(
            "SamePointGrad".to_string(),
            point,
            point,
            Color::red(),
            Color::blue(),
        );

        assert!(shading.validate().is_err());
    }

    #[test]
    fn test_radial_shading_creation() {
        let center = Point::new(50.0, 50.0);
        let stops = vec![
            ColorStop::new(0.0, Color::red()),
            ColorStop::new(1.0, Color::blue()),
        ];

        let shading =
            RadialShading::new("RadialGrad".to_string(), center, 10.0, center, 50.0, stops);

        assert_eq!(shading.name, "RadialGrad");
        assert_eq!(shading.start_center, center);
        assert_eq!(shading.start_radius, 10.0);
        assert_eq!(shading.end_radius, 50.0);
    }

    #[test]
    fn test_radial_shading_gradient() {
        let center = Point::new(50.0, 50.0);
        let shading = RadialShading::radial_gradient(
            "SimpleRadial".to_string(),
            center,
            0.0,
            25.0,
            Color::white(),
            Color::black(),
        );

        assert_eq!(shading.color_stops.len(), 2);
        assert_eq!(shading.start_radius, 0.0);
        assert_eq!(shading.end_radius, 25.0);
    }

    #[test]
    fn test_radial_shading_radius_clamping() {
        let center = Point::new(50.0, 50.0);
        let stops = vec![ColorStop::new(0.0, Color::red())];

        let shading = RadialShading::new(
            "ClampedRadial".to_string(),
            center,
            -5.0, // Negative radius should be clamped to 0
            center,
            10.0,
            stops,
        );

        assert_eq!(shading.start_radius, 0.0);
    }

    #[test]
    fn test_radial_shading_validation_valid() {
        let center = Point::new(50.0, 50.0);
        let shading = RadialShading::radial_gradient(
            "ValidRadial".to_string(),
            center,
            0.0,
            25.0,
            Color::red(),
            Color::blue(),
        );

        assert!(shading.validate().is_ok());
    }

    #[test]
    fn test_function_based_shading_creation() {
        let domain = [0.0, 1.0, 0.0, 1.0];
        let shading = FunctionBasedShading::new("FuncShading".to_string(), domain, 1);

        assert_eq!(shading.name, "FuncShading");
        assert_eq!(shading.domain, domain);
        assert_eq!(shading.function_id, 1);
        assert!(shading.matrix.is_none());
    }

    #[test]
    fn test_function_based_shading_with_matrix() {
        let domain = [0.0, 1.0, 0.0, 1.0];
        let matrix = [2.0, 0.0, 0.0, 2.0, 10.0, 20.0];
        let shading =
            FunctionBasedShading::new("FuncShading".to_string(), domain, 1).with_matrix(matrix);

        assert_eq!(shading.matrix, Some(matrix));
    }

    #[test]
    fn test_function_based_shading_validation_valid() {
        let domain = [0.0, 1.0, 0.0, 1.0];
        let shading = FunctionBasedShading::new("ValidFunc".to_string(), domain, 1);

        assert!(shading.validate().is_ok());
    }

    #[test]
    fn test_function_based_shading_validation_invalid_domain() {
        let domain = [1.0, 0.0, 0.0, 1.0]; // min > max
        let shading = FunctionBasedShading::new("InvalidFunc".to_string(), domain, 1);

        assert!(shading.validate().is_err());
    }

    #[test]
    fn test_shading_pattern_creation() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 0.0);
        let axial = AxialShading::linear_gradient(
            "PatternGrad".to_string(),
            start,
            end,
            Color::red(),
            Color::blue(),
        );
        let shading = ShadingDefinition::Axial(axial);
        let pattern = ShadingPattern::new("Pattern1".to_string(), shading);

        assert_eq!(pattern.name, "Pattern1");
        assert!(pattern.matrix.is_none());
    }

    #[test]
    fn test_shading_pattern_with_matrix() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 0.0);
        let axial = AxialShading::linear_gradient(
            "PatternGrad".to_string(),
            start,
            end,
            Color::red(),
            Color::blue(),
        );
        let shading = ShadingDefinition::Axial(axial);
        let matrix = [1.0, 0.0, 0.0, 1.0, 50.0, 50.0];
        let pattern = ShadingPattern::new("Pattern1".to_string(), shading).with_matrix(matrix);

        assert_eq!(pattern.matrix, Some(matrix));
    }

    #[test]
    fn test_shading_manager_creation() {
        let manager = ShadingManager::new();
        assert_eq!(manager.shading_count(), 0);
        assert_eq!(manager.pattern_count(), 0);
        assert_eq!(manager.total_count(), 0);
    }

    #[test]
    fn test_shading_manager_add_shading() {
        let mut manager = ShadingManager::new();
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 0.0);
        let axial = AxialShading::linear_gradient(
            "TestGrad".to_string(),
            start,
            end,
            Color::red(),
            Color::blue(),
        );
        let shading = ShadingDefinition::Axial(axial);

        let name = manager.add_shading(shading).unwrap();
        assert_eq!(name, "TestGrad");
        assert_eq!(manager.shading_count(), 1);

        let retrieved = manager.get_shading(&name).unwrap();
        assert_eq!(retrieved.name(), "TestGrad");
    }

    #[test]
    fn test_shading_manager_auto_naming() {
        let mut manager = ShadingManager::new();
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 0.0);
        let axial = AxialShading::linear_gradient(
            String::new(), // Empty name
            start,
            end,
            Color::red(),
            Color::blue(),
        );
        let shading = ShadingDefinition::Axial(axial);

        let name = manager.add_shading(shading).unwrap();
        assert_eq!(name, "Sh1");

        // Add another with empty name
        let axial2 = AxialShading::linear_gradient(
            String::new(),
            start,
            end,
            Color::green(),
            Color::yellow(),
        );
        let shading2 = ShadingDefinition::Axial(axial2);

        let name2 = manager.add_shading(shading2).unwrap();
        assert_eq!(name2, "Sh2");
    }

    #[test]
    fn test_shading_manager_create_gradients() {
        let mut manager = ShadingManager::new();

        let linear_name = manager
            .create_linear_gradient(
                Point::new(0.0, 0.0),
                Point::new(100.0, 0.0),
                Color::red(),
                Color::blue(),
            )
            .unwrap();

        let radial_name = manager
            .create_radial_gradient(
                Point::new(50.0, 50.0),
                0.0,
                25.0,
                Color::white(),
                Color::black(),
            )
            .unwrap();

        assert_eq!(manager.shading_count(), 2);
        assert!(manager.get_shading(&linear_name).is_some());
        assert!(manager.get_shading(&radial_name).is_some());
    }

    #[test]
    fn test_shading_manager_clear() {
        let mut manager = ShadingManager::new();

        manager
            .create_linear_gradient(
                Point::new(0.0, 0.0),
                Point::new(100.0, 0.0),
                Color::red(),
                Color::blue(),
            )
            .unwrap();

        assert_eq!(manager.shading_count(), 1);

        manager.clear();
        assert_eq!(manager.shading_count(), 0);
        assert_eq!(manager.total_count(), 0);
    }

    #[test]
    fn test_axial_shading_pdf_dictionary() {
        let start = Point::new(0.0, 0.0);
        let end = Point::new(100.0, 50.0);
        let shading = AxialShading::linear_gradient(
            "TestPDF".to_string(),
            start,
            end,
            Color::red(),
            Color::blue(),
        )
        .with_extend(true, false);

        let dict = shading.to_pdf_dictionary().unwrap();

        if let Some(Object::Integer(shading_type)) = dict.get("ShadingType") {
            assert_eq!(*shading_type, 2); // Axial type
        }

        if let Some(Object::Array(coords)) = dict.get("Coords") {
            assert_eq!(coords.len(), 4);
        }

        if let Some(Object::Array(extend)) = dict.get("Extend") {
            assert_eq!(extend.len(), 2);
            if let (Object::Boolean(start_extend), Object::Boolean(end_extend)) =
                (&extend[0], &extend[1])
            {
                assert!(*start_extend);
                assert!(!(*end_extend));
            }
        }
    }

    // ── Issue #297: real /Function, /ColorSpace and the `sh` paint path ──

    /// Extract the C0/C1 arrays of a Type 2 function dictionary as f64 vecs.
    fn type2_c0_c1(func: &Dictionary) -> (Vec<f64>, Vec<f64>) {
        let extract = |key: &str| -> Vec<f64> {
            match func.get(key) {
                Some(Object::Array(a)) => a
                    .iter()
                    .map(|o| match o {
                        Object::Real(v) => *v,
                        Object::Integer(v) => *v as f64,
                        _ => panic!("{key} component is not numeric"),
                    })
                    .collect(),
                other => panic!("{key} is not an array: {other:?}"),
            }
        };
        (extract("C0"), extract("C1"))
    }

    #[test]
    fn test_axial_two_stops_emits_real_type2_function() {
        // 2 stops red→blue must produce a Type 2 (exponential) function whose
        // endpoints carry the actual stop colours, not a placeholder integer.
        let shading = AxialShading::linear_gradient(
            "G".to_string(),
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Color::red(),
            Color::blue(),
        );
        let dict = shading.to_pdf_dictionary().unwrap();

        // /ColorSpace is REQUIRED by ISO 32000-1 §8.7.4.3 Table 78 — was missing.
        assert_eq!(
            dict.get("ColorSpace"),
            Some(&Object::Name("DeviceRGB".to_string())),
            "axial shading must declare /ColorSpace"
        );

        // /Function must be a real function dictionary, not Object::Integer(1).
        let func = match dict.get("Function") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("/Function must be a dictionary, got {other:?}"),
        };
        assert_eq!(func.get("FunctionType"), Some(&Object::Integer(2)));
        let (c0, c1) = type2_c0_c1(func);
        assert_eq!(c0, vec![1.0, 0.0, 0.0], "C0 must be red");
        assert_eq!(c1, vec![0.0, 0.0, 1.0], "C1 must be blue");
        assert_eq!(func.get("N"), Some(&Object::Real(1.0)));
        assert_eq!(
            func.get("Domain"),
            Some(&Object::Array(vec![Object::Real(0.0), Object::Real(1.0)]))
        );
    }

    #[test]
    fn test_axial_three_stops_emits_type3_stitching() {
        // 3 stops must produce a Type 3 stitching function wrapping 2 Type 2
        // subfunctions, with /Bounds at the interior stop and /Encode [0 1 0 1].
        let shading = AxialShading::new(
            "G".to_string(),
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            vec![
                ColorStop::new(0.0, Color::red()),
                ColorStop::new(0.5, Color::green()),
                ColorStop::new(1.0, Color::blue()),
            ],
        );
        let dict = shading.to_pdf_dictionary().unwrap();
        let func = match dict.get("Function") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("/Function must be a dictionary, got {other:?}"),
        };
        assert_eq!(func.get("FunctionType"), Some(&Object::Integer(3)));
        assert_eq!(
            func.get("Bounds"),
            Some(&Object::Array(vec![Object::Real(0.5)])),
            "interior stop position is the only bound"
        );
        assert_eq!(
            func.get("Encode"),
            Some(&Object::Array(vec![
                Object::Real(0.0),
                Object::Real(1.0),
                Object::Real(0.0),
                Object::Real(1.0),
            ]))
        );
        let subfuncs = match func.get("Functions") {
            Some(Object::Array(a)) => a,
            other => panic!("/Functions must be an array, got {other:?}"),
        };
        assert_eq!(subfuncs.len(), 2, "two segments for three stops");
        // First subfunction red→green.
        let f0 = match &subfuncs[0] {
            Object::Dictionary(d) => d,
            other => panic!("subfunction 0 not a dict: {other:?}"),
        };
        let (c0, c1) = type2_c0_c1(f0);
        assert_eq!(c0, vec![1.0, 0.0, 0.0]);
        assert_eq!(c1, vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn test_axial_gray_stops_emit_devicegray_function() {
        // Uniform Gray stops must keep DeviceGray (1 component), not promote to RGB.
        let shading = AxialShading::linear_gradient(
            "G".to_string(),
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Color::black(),
            Color::white(),
        );
        let dict = shading.to_pdf_dictionary().unwrap();
        assert_eq!(
            dict.get("ColorSpace"),
            Some(&Object::Name("DeviceGray".to_string()))
        );
        let func = match dict.get("Function") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("/Function must be a dictionary, got {other:?}"),
        };
        let (c0, c1) = type2_c0_c1(func);
        assert_eq!(c0, vec![0.0], "black");
        assert_eq!(c1, vec![1.0], "white");
    }

    #[test]
    fn test_axial_cmyk_stops_emit_devicecmyk_function() {
        // Uniform CMYK stops keep DeviceCMYK with 4-component C0/C1.
        let shading = AxialShading::linear_gradient(
            "G".to_string(),
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Color::Cmyk(1.0, 0.0, 0.0, 0.0),
            Color::Cmyk(0.0, 1.0, 0.0, 0.0),
        );
        let dict = shading.to_pdf_dictionary().unwrap();
        assert_eq!(
            dict.get("ColorSpace"),
            Some(&Object::Name("DeviceCMYK".to_string()))
        );
        let func = match dict.get("Function") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("/Function must be a dictionary, got {other:?}"),
        };
        let (c0, c1) = type2_c0_c1(func);
        assert_eq!(c0, vec![1.0, 0.0, 0.0, 0.0], "C0 = cyan, 4 components");
        assert_eq!(c1, vec![0.0, 1.0, 0.0, 0.0], "C1 = magenta, 4 components");
    }

    #[test]
    fn test_axial_four_stops_type3_has_three_subfunctions_two_bounds() {
        let shading = AxialShading::new(
            "G".to_string(),
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            vec![
                ColorStop::new(0.0, Color::red()),
                ColorStop::new(0.3, Color::green()),
                ColorStop::new(0.7, Color::blue()),
                ColorStop::new(1.0, Color::white()),
            ],
        );
        let dict = shading.to_pdf_dictionary().unwrap();
        let func = match dict.get("Function") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("/Function must be a dictionary, got {other:?}"),
        };
        assert_eq!(func.get("FunctionType"), Some(&Object::Integer(3)));
        let subfuncs = match func.get("Functions") {
            Some(Object::Array(a)) => a,
            other => panic!("/Functions array expected, got {other:?}"),
        };
        assert_eq!(subfuncs.len(), 3, "4 stops → 3 segments");
        assert_eq!(
            func.get("Bounds"),
            Some(&Object::Array(vec![Object::Real(0.3), Object::Real(0.7)])),
            "two interior bounds at the middle stops"
        );
        assert_eq!(
            func.get("Encode"),
            Some(&Object::Array(vec![
                Object::Real(0.0),
                Object::Real(1.0),
                Object::Real(0.0),
                Object::Real(1.0),
                Object::Real(0.0),
                Object::Real(1.0),
            ]))
        );
    }

    #[test]
    fn test_single_stop_emits_constant_type2() {
        // A lone stop is valid (validate() only rejects empty) → constant colour.
        let shading = AxialShading::new(
            "G".to_string(),
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            vec![ColorStop::new(0.0, Color::Rgb(0.2, 0.4, 0.6))],
        );
        let func = match shading.to_pdf_dictionary().unwrap().get("Function") {
            Some(Object::Dictionary(d)) => d.clone(),
            other => panic!("/Function must be a dictionary, got {other:?}"),
        };
        assert_eq!(func.get("FunctionType"), Some(&Object::Integer(2)));
        let (c0, c1) = type2_c0_c1(&func);
        assert_eq!(c0, c1, "constant colour: C0 == C1");
        assert_eq!(c0, vec![0.2, 0.4, 0.6]);
    }

    #[test]
    fn test_mixed_color_spaces_promote_to_rgb() {
        // Mixing Gray and RGB stops must promote the whole shading to DeviceRGB.
        let shading = AxialShading::new(
            "G".to_string(),
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            vec![
                ColorStop::new(0.0, Color::Gray(0.5)),
                ColorStop::new(1.0, Color::Rgb(1.0, 0.0, 0.0)),
            ],
        );
        let dict = shading.to_pdf_dictionary().unwrap();
        assert_eq!(
            dict.get("ColorSpace"),
            Some(&Object::Name("DeviceRGB".to_string()))
        );
        let func = match dict.get("Function") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("/Function must be a dictionary, got {other:?}"),
        };
        let (c0, c1) = type2_c0_c1(func);
        assert_eq!(c0, vec![0.5, 0.5, 0.5], "gray 0.5 promoted to RGB");
        assert_eq!(c1, vec![1.0, 0.0, 0.0]);
    }

    #[test]
    fn test_radial_emits_real_function_and_colorspace() {
        let center = Point::new(50.0, 50.0);
        let shading = RadialShading::radial_gradient(
            "R".to_string(),
            center,
            0.0,
            25.0,
            Color::cyan(),
            Color::magenta(),
        );
        let dict = shading.to_pdf_dictionary().unwrap();
        assert_eq!(
            dict.get("ColorSpace"),
            Some(&Object::Name("DeviceRGB".to_string()))
        );
        let func = match dict.get("Function") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("/Function must be a dictionary, got {other:?}"),
        };
        assert_eq!(func.get("FunctionType"), Some(&Object::Integer(2)));
    }

    #[test]
    fn test_shading_pattern_inlines_real_shading_not_placeholder() {
        // Issue #297 C: /Shading must be the real shading dict, never Integer(1).
        let axial = AxialShading::linear_gradient(
            "P".to_string(),
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Color::red(),
            Color::blue(),
        );
        let pattern = ShadingPattern::new("SP1".to_string(), ShadingDefinition::Axial(axial));
        let dict = pattern.to_pdf_pattern_dictionary().unwrap();
        assert_eq!(dict.get("PatternType"), Some(&Object::Integer(2)));
        let shading = match dict.get("Shading") {
            Some(Object::Dictionary(d)) => d,
            other => panic!("/Shading must be an inline dict, got {other:?}"),
        };
        assert_eq!(shading.get("ShadingType"), Some(&Object::Integer(2)));
        assert!(
            matches!(shading.get("Function"), Some(Object::Dictionary(_))),
            "inlined shading must carry a real /Function"
        );
    }

    #[test]
    fn test_radial_shading_pdf_dictionary() {
        let center = Point::new(50.0, 50.0);
        let shading = RadialShading::radial_gradient(
            "TestRadialPDF".to_string(),
            center,
            10.0,
            30.0,
            Color::yellow(),
            Color::red(),
        );

        let dict = shading.to_pdf_dictionary().unwrap();

        if let Some(Object::Integer(shading_type)) = dict.get("ShadingType") {
            assert_eq!(*shading_type, 3); // Radial type
        }

        if let Some(Object::Array(coords)) = dict.get("Coords") {
            assert_eq!(coords.len(), 6); // [x0 y0 r0 x1 y1 r1]
        }
    }
}
