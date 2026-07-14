//! Type 4 free-form Gouraud mesh and exact conic (angular) gradient shadings
//! (issue #407).
//!
//! Both are additive APIs: `Page::add_mesh_shading` registers a Type 4 mesh
//! (emitted as a stream), `Page::add_conic_shading` registers a Type 1
//! function-based shading whose colour is an exact function of the angle around
//! a center (a real Type 4 PostScript calculator, not a mesh approximation).
//! Each is painted with the `sh` operator inside a clipped rectangle.

use oxidize_pdf::graphics::{
    Color, ColorStop, ConicShading, FreeFormGouraudShading, GouraudVertex, Point,
};
use oxidize_pdf::{Document, Page};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut doc = Document::new();
    let mut page = Page::a4();

    // A single RGB triangle: three vertices, red / green / blue corners.
    let mesh = FreeFormGouraudShading::new(
        "MeshTri",
        "DeviceRGB",
        // Decode: x,y in [0,200], each colour component in [0,1].
        vec![0.0, 200.0, 0.0, 200.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
        vec![
            GouraudVertex {
                flag: 0,
                x: 40.0,
                y: 40.0,
                color: Color::Rgb(1.0, 0.0, 0.0),
            },
            GouraudVertex {
                flag: 1,
                x: 160.0,
                y: 40.0,
                color: Color::Rgb(0.0, 1.0, 0.0),
            },
            GouraudVertex {
                flag: 1,
                x: 100.0,
                y: 160.0,
                color: Color::Rgb(0.0, 0.0, 1.0),
            },
        ],
    );
    page.add_mesh_shading("MeshTri", mesh)?;

    // An exact conic sweep: red → green → blue → red around (300, 500).
    let conic = ConicShading::new(
        "ConicSweep",
        Point::new(300.0, 500.0),
        [220.0, 380.0, 420.0, 580.0],
        vec![
            ColorStop::new(0.0, Color::Rgb(1.0, 0.0, 0.0)),
            ColorStop::new(0.33, Color::Rgb(0.0, 1.0, 0.0)),
            ColorStop::new(0.66, Color::Rgb(0.0, 0.0, 1.0)),
            ColorStop::new(1.0, Color::Rgb(1.0, 0.0, 0.0)),
        ],
    );
    page.add_conic_shading("ConicSweep", conic)?;

    // Paint the mesh into its triangle region.
    page.graphics()
        .save_state()
        .rectangle(40.0, 40.0, 120.0, 120.0)
        .clip()
        .end_path()
        .paint_shading("MeshTri")
        .restore_state();

    // Paint the conic into its square region.
    page.graphics()
        .save_state()
        .rectangle(220.0, 420.0, 160.0, 160.0)
        .clip()
        .end_path()
        .paint_shading("ConicSweep")
        .restore_state();

    doc.add_page(page);
    doc.save("examples/results/mesh_and_conic_shading.pdf")?;
    println!("Wrote examples/results/mesh_and_conic_shading.pdf");
    Ok(())
}
