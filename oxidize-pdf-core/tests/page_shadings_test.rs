//! Task 6 of the v2.5.6 gap-closing series.
//!
//! `ShadingDefinition` (Axial / Radial / FunctionBased, under
//! `graphics::shadings`) already has a `to_pdf_dictionary` serialiser
//! per ISO 32000-1 §8.7.4, but Page had no way to register a shading
//! resource and the writer emitted no `/Resources/Shading`. So any
//! attempt to paint a gradient with the `sh` operator or a type-2
//! `ShadingPattern` failed to resolve the shading name.
//!
//! Contract being exercised:
//!   * `Page::add_shading(name, ShadingDefinition)` registers a shading
//!     resource.
//!   * `Page::shadings()` exposes the registry.
//!   * The writer emits each shading as an indirect dictionary object
//!     (per §8.7.4 shadings are dicts, not streams) and references it
//!     from `/Resources/Shading/<Name>`.

use oxidize_pdf::graphics::{
    AxialShading, Color, ColorStop, ConicShading, FreeFormGouraudShading, FunctionBasedShading,
    GouraudVertex, Point as ShadingPoint, RadialShading, ShadingDefinition,
};
use oxidize_pdf::parser::objects::PdfObject;
use oxidize_pdf::parser::{ParseOptions, PdfReader};
use oxidize_pdf::{Document, Page};
use std::io::{Cursor, Read, Seek};

fn first_page_ref<R: std::io::Read + std::io::Seek>(reader: &mut PdfReader<R>) -> (u32, u16) {
    let pages = reader.pages().expect("/Pages").clone();
    let kids = pages
        .get("Kids")
        .and_then(|o| o.as_array())
        .expect("/Pages/Kids");
    kids.0
        .first()
        .expect("/Pages/Kids[0]")
        .as_reference()
        .expect("/Pages/Kids[0] reference")
}

fn resolve_page0_shading_dict<R: std::io::Read + std::io::Seek>(
    reader: &mut PdfReader<R>,
) -> oxidize_pdf::parser::objects::PdfDictionary {
    let (page_n, page_g) = first_page_ref(reader);
    let page_obj = reader.get_object(page_n, page_g).expect("page").clone();
    let page_dict = page_obj.as_dict().expect("page dict").clone();
    let resources = match page_dict.get("Resources").expect("/Resources") {
        PdfObject::Dictionary(d) => d.clone(),
        PdfObject::Reference(n, g) => reader
            .get_object(*n, *g)
            .expect("resolve /Resources")
            .clone()
            .as_dict()
            .expect("/Resources dict")
            .clone(),
        other => panic!("/Resources: unexpected {:?}", other),
    };
    match resources.get("Shading").expect("/Resources/Shading") {
        PdfObject::Dictionary(d) => d.clone(),
        PdfObject::Reference(n, g) => reader
            .get_object(*n, *g)
            .expect("resolve /Shading")
            .clone()
            .as_dict()
            .expect("/Shading dict")
            .clone(),
        other => panic!("/Shading: unexpected {:?}", other),
    }
}

/// Decode the first page's content stream(s) to a UTF-8 string, applying
/// FlateDecode (compression is a default feature).
fn page0_content<R: Read + Seek>(reader: &mut PdfReader<R>) -> String {
    let (page_n, page_g) = first_page_ref(reader);
    let page = reader
        .get_object(page_n, page_g)
        .expect("page")
        .clone()
        .as_dict()
        .expect("page dict")
        .clone();
    let opts = ParseOptions::default();
    let refs: Vec<(u32, u16)> = match page.get("Contents").expect("/Contents") {
        PdfObject::Reference(n, g) => vec![(*n, *g)],
        PdfObject::Array(a) => {
            a.0.iter()
                .map(|el| el.as_reference().expect("/Contents element ref"))
                .collect()
        }
        other => panic!("/Contents: unexpected {other:?}"),
    };
    let mut out = Vec::new();
    for (n, g) in refs {
        let obj = reader.get_object(n, g).expect("content object").clone();
        let stream = obj.as_stream().expect("content stream");
        out.extend(stream.decode(&opts).expect("decode content stream"));
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolve the indirect `/Function` of a shading dict and return it.
fn resolve_function<R: Read + Seek>(
    reader: &mut PdfReader<R>,
    shading: &oxidize_pdf::parser::objects::PdfDictionary,
) -> oxidize_pdf::parser::objects::PdfDictionary {
    let (n, g) = shading
        .get("Function")
        .and_then(|o| o.as_reference())
        .expect("/Function must be an indirect reference (issue #297 B)");
    reader
        .get_object(n, g)
        .expect("resolve /Function")
        .clone()
        .as_dict()
        .expect("/Function dict")
        .clone()
}

fn reals(obj: &PdfObject) -> Vec<f64> {
    obj.as_array()
        .expect("array")
        .0
        .iter()
        .map(|o| o.as_real().expect("numeric component"))
        .collect()
}

fn make_axial(name: &str) -> ShadingDefinition {
    let stops = vec![
        ColorStop::new(0.0, Color::Rgb(1.0, 0.0, 0.0)),
        ColorStop::new(1.0, Color::Rgb(0.0, 0.0, 1.0)),
    ];
    let axial = AxialShading::new(
        name.to_string(),
        ShadingPoint::new(0.0, 0.0),
        ShadingPoint::new(100.0, 0.0),
        stops,
    );
    ShadingDefinition::Axial(axial)
}

/// Primary Task 6 assertion: a registered axial shading surfaces as an
/// INDIRECT dictionary under `/Resources/Shading/<Name>`, and the dict
/// carries `/ShadingType 2` (axial, ISO 32000-1 §8.7.4.5.2).
#[test]
fn page_shading_is_written_as_indirect_dict_with_shadingtype() {
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_shading("Sh1", make_axial("Sh1"))
        .expect("add_shading");
    doc.add_page(page);

    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");
    let sh = resolve_page0_shading_dict(&mut reader);

    let (n, g) = sh
        .get("Sh1")
        .and_then(|o| o.as_reference())
        .expect("/Sh1 must be an indirect reference");

    let obj = reader.get_object(n, g).expect("resolve Sh1").clone();
    let dict = obj.as_dict().expect("Sh1 must resolve to a dictionary");

    assert_eq!(
        dict.get("ShadingType").and_then(|o| o.as_integer()),
        Some(2),
        "/ShadingType must be 2 (axial) per ISO 32000-1 §8.7.4.5.2"
    );
    let coords = dict
        .get("Coords")
        .and_then(|o| o.as_array())
        .expect("/Coords required for axial shading");
    assert_eq!(
        coords.0.len(),
        4,
        "/Coords must be [x0 y0 x1 y1] per Table 80"
    );
}

/// Task 6 negative case: a page without shadings must omit `/Shading`
/// entirely rather than emit an empty dict.
#[test]
fn page_without_shadings_omits_shading_entry() {
    let mut doc = Document::new();
    doc.add_page(Page::a4());
    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");

    let (page_n, page_g) = first_page_ref(&mut reader);
    let page_obj = reader.get_object(page_n, page_g).expect("page").clone();
    let page_dict = page_obj.as_dict().expect("page dict").clone();
    let resources = page_dict
        .get("Resources")
        .and_then(|o| o.as_dict())
        .expect("/Resources");
    assert!(
        resources.get("Shading").is_none(),
        "/Shading must be absent when no shading was registered"
    );
}

/// Task 6 public-API regression.
#[test]
fn shadings_accessor_is_public_and_reflects_state() {
    let mut page = Page::a4();
    assert!(page.shadings().is_empty());
    page.add_shading("Sh1", make_axial("Sh1"))
        .expect("add_shading");
    let map = page.shadings();
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("Sh1"));
}

// ── Issue #297: gradients must render (real /Function, /ColorSpace, `sh`) ──

/// End-to-end: an axial shading's `/Function` resolves to an INDIRECT Type 2
/// function whose `C0`/`C1` carry the actual stop colours, and the shading
/// declares `/ColorSpace DeviceRGB`. Before the fix `/Function` was
/// `Object::Integer(1)` and `/ColorSpace` was absent.
#[test]
fn axial_function_is_indirect_type2_with_real_colors() {
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_shading("Sh1", make_axial("Sh1"))
        .expect("add_shading");
    doc.add_page(page);

    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");
    let sh = resolve_page0_shading_dict(&mut reader);
    let (sn, sg) = sh
        .get("Sh1")
        .and_then(|o| o.as_reference())
        .expect("/Sh1 indirect ref");
    let shading = reader
        .get_object(sn, sg)
        .expect("resolve Sh1")
        .clone()
        .as_dict()
        .expect("Sh1 dict")
        .clone();

    assert_eq!(
        shading
            .get("ColorSpace")
            .and_then(|o| o.as_name())
            .map(|n| n.0.as_str()),
        Some("DeviceRGB"),
        "/ColorSpace is required (ISO 32000-1 §8.7.4.3, Table 78)"
    );

    let func = resolve_function(&mut reader, &shading);
    assert_eq!(
        func.get("FunctionType").and_then(|o| o.as_integer()),
        Some(2),
        "2 stops → Type 2 exponential function"
    );
    assert_eq!(
        reals(func.get("C0").expect("/C0")),
        vec![1.0, 0.0, 0.0],
        "C0 = red"
    );
    assert_eq!(
        reals(func.get("C1").expect("/C1")),
        vec![0.0, 0.0, 1.0],
        "C1 = blue"
    );
    assert_eq!(func.get("N").and_then(|o| o.as_real()), Some(1.0));
}

/// End-to-end: a 3-stop radial shading's `/Function` resolves to an indirect
/// Type 3 stitching function wrapping two Type 2 subfunctions, with one
/// interior `/Bounds` entry.
#[test]
fn radial_three_stops_function_is_type3_stitching() {
    let radial = RadialShading::new(
        "Rad".to_string(),
        ShadingPoint::new(50.0, 50.0),
        0.0,
        ShadingPoint::new(50.0, 50.0),
        40.0,
        vec![
            ColorStop::new(0.0, Color::Rgb(1.0, 0.0, 0.0)),
            ColorStop::new(0.5, Color::Rgb(0.0, 1.0, 0.0)),
            ColorStop::new(1.0, Color::Rgb(0.0, 0.0, 1.0)),
        ],
    );
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_shading("Rad", ShadingDefinition::Radial(radial))
        .expect("add_shading");
    doc.add_page(page);

    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");
    let sh = resolve_page0_shading_dict(&mut reader);
    let (sn, sg) = sh
        .get("Rad")
        .and_then(|o| o.as_reference())
        .expect("/Rad ref");
    let shading = reader
        .get_object(sn, sg)
        .expect("resolve Rad")
        .clone()
        .as_dict()
        .expect("Rad dict")
        .clone();
    assert_eq!(
        shading.get("ShadingType").and_then(|o| o.as_integer()),
        Some(3),
        "radial = ShadingType 3"
    );
    let coords = shading
        .get("Coords")
        .and_then(|o| o.as_array())
        .expect("/Coords");
    assert_eq!(coords.0.len(), 6, "radial /Coords = [x0 y0 r0 x1 y1 r1]");

    let func = resolve_function(&mut reader, &shading);
    assert_eq!(
        func.get("FunctionType").and_then(|o| o.as_integer()),
        Some(3),
        "3 stops → Type 3 stitching function"
    );
    let subfns = func
        .get("Functions")
        .and_then(|o| o.as_array())
        .expect("/Functions");
    assert_eq!(subfns.0.len(), 2, "3 stops → 2 segments");
    assert_eq!(
        reals(func.get("Bounds").expect("/Bounds")),
        vec![0.5],
        "single interior bound at the middle stop"
    );
}

/// Writer guard: a `FunctionBased` shading carries an external function id
/// (an `Object::Integer`, not a dictionary). The function-hoisting logic
/// must leave it untouched — only dictionary `/Function` values are hoisted
/// to indirect objects (issue #297 B).
#[test]
fn function_based_shading_function_id_is_not_hoisted() {
    let fb = FunctionBasedShading::new("FB".to_string(), [0.0, 1.0, 0.0, 1.0], 7);
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_shading("FB", ShadingDefinition::FunctionBased(fb))
        .expect("add_shading");
    doc.add_page(page);

    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");
    let sh = resolve_page0_shading_dict(&mut reader);
    let (sn, sg) = sh
        .get("FB")
        .and_then(|o| o.as_reference())
        .expect("/FB ref");
    let shading = reader
        .get_object(sn, sg)
        .expect("resolve FB")
        .clone()
        .as_dict()
        .expect("FB dict")
        .clone();
    assert_eq!(
        shading.get("ShadingType").and_then(|o| o.as_integer()),
        Some(1),
        "function-based = ShadingType 1"
    );
    assert_eq!(
        shading.get("Function").and_then(|o| o.as_integer()),
        Some(7),
        "external function id stays an integer, not hoisted to a reference"
    );
}

/// End-to-end: `GraphicsContext::paint_shading` emits `/name sh` into the
/// page content stream (ISO 32000-1 §8.7.4.2) — the paint path that was
/// entirely absent before issue #297.
#[test]
fn paint_shading_emits_sh_operator_in_content_stream() {
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_shading("Sh1", make_axial("Sh1"))
        .expect("add_shading");
    // Clip to a rectangle, then paint the gradient into it.
    page.graphics()
        .save_state()
        .rectangle(50.0, 50.0, 200.0, 100.0)
        .clip()
        .end_path()
        .paint_shading("Sh1")
        .restore_state();
    doc.add_page(page);

    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");
    let content = page0_content(&mut reader);
    assert!(
        content.contains("/Sh1 sh"),
        "content stream must paint the shading with `/Sh1 sh`:\n{content}"
    );
}

// ── Issue #407 Phase 3: mesh (Type 4) + conic (Type 1) writer integration ──

/// The exact 3-vertex RGB mesh used across the integration tests (matches the
/// unit-level packing fixture: 24 bytes, 8 per vertex).
fn make_mesh() -> FreeFormGouraudShading {
    FreeFormGouraudShading::new(
        "Mesh1".to_string(),
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

fn make_conic() -> ConicShading {
    ConicShading::new(
        "Conic1".to_string(),
        ShadingPoint::new(50.0, 50.0),
        [0.0, 100.0, 0.0, 100.0],
        vec![
            ColorStop::new(0.0, Color::red()),
            ColorStop::new(1.0, Color::blue()),
        ],
    )
}

const MESH_PACKED_BYTES: [u8; 24] = [
    0x00, 0x19, 0x9A, 0x33, 0x33, 0xFF, 0x00, 0x00, // v0: flag0, x10, y20, red
    0x01, 0x80, 0x00, 0x80, 0x00, 0x00, 0xFF, 0x00, // v1: flag1, x50, y50, green
    0x01, 0xE6, 0x66, 0x19, 0x9A, 0x00, 0x00, 0xFF, // v2: flag1, x90, y10, blue
];

/// C1: a mesh and a conic registered on the same page both surface under
/// `/Resources/Shading`. The mesh resolves to an indirect STREAM with
/// `/ShadingType 4`; the conic resolves to an indirect DICTIONARY with
/// `/ShadingType 1` whose `/Function` was hoisted to an indirect Type 4
/// PostScript stream.
#[test]
fn page_mesh_and_conic_appear_in_shading_resource() {
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_mesh_shading("Mesh1", make_mesh()).expect("mesh");
    page.add_conic_shading("Conic1", make_conic())
        .expect("conic");
    doc.add_page(page);

    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");
    let sh = resolve_page0_shading_dict(&mut reader);

    // Mesh → indirect stream, ShadingType 4.
    let (mn, mg) = sh
        .get("Mesh1")
        .and_then(|o| o.as_reference())
        .expect("/Mesh1 must be an indirect reference");
    let mesh_obj = reader.get_object(mn, mg).expect("resolve Mesh1").clone();
    let mesh_stream = mesh_obj
        .as_stream()
        .expect("Type 4 mesh must resolve to a stream object");
    assert_eq!(
        mesh_stream
            .dict
            .get("ShadingType")
            .and_then(|o| o.as_integer()),
        Some(4),
        "mesh /ShadingType must be 4"
    );

    // Conic → indirect dict, ShadingType 1, /Function hoisted to a stream.
    let (cn, cg) = sh
        .get("Conic1")
        .and_then(|o| o.as_reference())
        .expect("/Conic1 must be an indirect reference");
    let conic_obj = reader.get_object(cn, cg).expect("resolve Conic1").clone();
    let conic_dict = conic_obj.as_dict().expect("conic must resolve to a dict");
    assert_eq!(
        conic_dict.get("ShadingType").and_then(|o| o.as_integer()),
        Some(1),
        "conic /ShadingType must be 1 (function-based)"
    );
    let (fnn, fng) = conic_dict
        .get("Function")
        .and_then(|o| o.as_reference())
        .expect("conic /Function must be an indirect reference, not inline");
    let func_obj = reader
        .get_object(fnn, fng)
        .expect("resolve conic /Function")
        .clone();
    let func_stream = func_obj
        .as_stream()
        .expect("conic /Function must be a Type 4 PostScript stream");
    assert_eq!(
        func_stream
            .dict
            .get("FunctionType")
            .and_then(|o| o.as_integer()),
        Some(4),
        "conic /Function must be a Type 4 calculator"
    );
}

/// C3: the packed mesh vertex bytes survive the full write → parse pipeline
/// byte-for-byte (no compression or serialisation corruption).
#[test]
fn page_mesh_stream_bytes_survive_roundtrip() {
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_mesh_shading("Mesh1", make_mesh()).expect("mesh");
    doc.add_page(page);

    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");
    let sh = resolve_page0_shading_dict(&mut reader);
    let (mn, mg) = sh
        .get("Mesh1")
        .and_then(|o| o.as_reference())
        .expect("/Mesh1 reference");
    let mesh_obj = reader.get_object(mn, mg).expect("resolve Mesh1").clone();
    let stream = mesh_obj.as_stream().expect("mesh stream");
    let data = stream
        .decode(&ParseOptions::default())
        .expect("decode mesh stream");
    assert_eq!(
        data,
        MESH_PACKED_BYTES.to_vec(),
        "packed mesh vertex bytes must round-trip through the writer unchanged"
    );
}

/// C2 regression: the conic PostScript program survives the pipeline with its
/// angular `atan` operator and both stop colours, decoded from the hoisted
/// Type 4 function stream (content verification, not a smoke test).
#[test]
fn conic_function_stream_contains_atan_and_stop_colors() {
    let mut doc = Document::new();
    let mut page = Page::a4();
    page.add_conic_shading("Conic1", make_conic())
        .expect("conic");
    doc.add_page(page);

    let bytes = doc.to_bytes().expect("serialize");
    let mut reader = PdfReader::new(Cursor::new(&bytes)).expect("parse");
    let sh = resolve_page0_shading_dict(&mut reader);
    let (cn, cg) = sh
        .get("Conic1")
        .and_then(|o| o.as_reference())
        .expect("/Conic1 reference");
    let conic_dict = reader
        .get_object(cn, cg)
        .expect("resolve Conic1")
        .clone()
        .as_dict()
        .expect("conic dict")
        .clone();
    let (fnn, fng) = conic_dict
        .get("Function")
        .and_then(|o| o.as_reference())
        .expect("conic /Function reference");
    let func_stream = reader
        .get_object(fnn, fng)
        .expect("resolve /Function")
        .clone();
    let func_stream = func_stream.as_stream().expect("Type 4 function stream");
    let code = func_stream
        .decode(&ParseOptions::default())
        .expect("decode function stream");
    let code = String::from_utf8(code).expect("PostScript is ASCII");

    assert!(
        code.contains("atan"),
        "conic function must be angular:\n{code}"
    );
    // red→blue over DeviceRGB: the ramp encodes deltas (-1, 0, 1) with starts
    // (1, 0, 0); the "-1 mul 1 add" for the red channel is the tell.
    assert!(
        code.contains("-1 mul 1 add"),
        "conic ramp must carry the red→blue interpolation:\n{code}"
    );
}
