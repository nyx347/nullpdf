//! Headless smoke test for the PDF rendering pipeline.
//!
//! Mirrors the exact pdfium calls in `src/pdf.rs` — whole-page renders (system-library
//! binding, `set_target_size` + `set_reverse_byte_order`, `as_rgba_bytes`) and detail-tile
//! renders of a page region (`set_fixed_size` + `scale_page_by_factor` + `translate`) — so we
//! can verify rendering works on this machine without a GUI/Wayland screenshot.
//!
//! It also checks the tile math (a translate in page points, applied before the scale): a
//! tile must register exactly with the same region cropped from an exact-scale render of the
//! whole page. That reference goes through the same transform path, because the whole-page
//! path stretches the page to its rounded bitmap size (a sub-pixel, non-uniform scale on
//! pages with fractional sizes such as A4). Glyph anti-aliasing still differs on a few pixels;
//! a misregistration of even half a pixel changes ~60x as many pixels strongly, so the check
//! counts only strong differences.
//!
//!   cargo run --example render_check -- <input.pdf> <page_index> <output.png>

use pdfium_render::prelude::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: render_check <pdf> <page> <out.png>");
    let page_idx: u16 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(0);
    let out = args
        .get(3)
        .map(String::as_str)
        .unwrap_or("/tmp/render_check.png");

    let pdfium = Pdfium::new(Pdfium::bind_to_system_library().expect("bind libpdfium"));
    let doc = pdfium.load_pdf_from_file(path, None).expect("load pdf");
    let pages = doc.pages();
    println!("pages = {}", pages.len());

    let page = pages.get(page_idx).expect("get page");
    let (wp, hp) = (page.width().value, page.height().value);
    println!("page {page_idx} size = {wp:.1} x {hp:.1} pts");

    // Whole page, as PdfDoc::render_page.
    let scale = 2.0_f32;
    let cfg = PdfRenderConfig::new()
        .set_target_size((wp * scale).round() as i32, (hp * scale).round() as i32)
        .set_reverse_byte_order(true);
    let bitmap = page.render_with_config(&cfg).expect("render");
    let (fw, fh) = (bitmap.width() as usize, bitmap.height() as usize);
    println!("rendered {fw} x {fh} px");

    // Sanity: a rendered text page should not be uniformly one colour.
    let full = bitmap.as_rgba_bytes();
    let first = &full[0..4];
    let uniform = full.as_chunks::<4>().0.iter().all(|px| px == first);
    println!("uniform-colour image: {uniform} (expect false for a real page)");

    // A region of the page, as PdfDoc::render_region (page-local points, top-left origin).
    // Whole-point offsets keep the crop below on exact pixel boundaries.
    let (x0, y0) = ((wp * 0.25).round(), (hp * 0.3).round());
    let (tw, th) = (
        (wp * 0.4 * scale).round() as i32,
        (hp * 0.2 * scale).round() as i32,
    );
    let tile_cfg = PdfRenderConfig::new()
        .set_fixed_size(tw, th)
        .scale_page_by_factor(scale)
        .set_reverse_byte_order(true)
        .translate(PdfPoints::new(-x0), PdfPoints::new(-y0))
        .expect("tile transform");
    let tile = page
        .render_with_config(&tile_cfg)
        .expect("render tile")
        .as_rgba_bytes();
    // Reference: the whole page through the same exact-scale transform path.
    let (rw, rh) = ((wp * scale).ceil() as i32, (hp * scale).ceil() as i32);
    let reference_cfg = PdfRenderConfig::new()
        .set_fixed_size(rw, rh)
        .scale_page_by_factor(scale)
        .set_reverse_byte_order(true)
        .translate(PdfPoints::ZERO, PdfPoints::ZERO)
        .expect("reference transform");
    let full = page
        .render_with_config(&reference_cfg)
        .expect("render reference")
        .as_rgba_bytes();
    let fw = rw as usize;
    let (ox, oy) = ((x0 * scale) as usize, (y0 * scale) as usize);
    let (tw, th) = (tw as usize, th as usize);
    let strong = (0..th)
        .flat_map(|y| (0..tw).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let t = (y * tw + x) * 4;
            let f = ((oy + y) * fw + ox + x) * 4;
            (0..3).any(|c| tile[t + c].abs_diff(full[f + c]) > 24)
        })
        .count();
    let share = strong as f64 / (tw * th) as f64 * 100.0;
    println!(
        "tile {tw} x {th} px at ({ox}, {oy}): {strong} pixels ({share:.2}%) differ strongly \
         from the crop -> {} (expect registered: < 1%)",
        if share < 1.0 {
            "registered"
        } else {
            "MISREGISTERED"
        }
    );

    bitmap.as_image().save(out).expect("save png");
    println!("wrote {out}");
}
