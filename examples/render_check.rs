//! Headless smoke test for the PDF rendering pipeline.
//!
//! Mirrors the exact pdfium calls in `src/pdf.rs` (system-library binding,
//! `set_target_size` + `set_reverse_byte_order`, `as_rgba_bytes`) so we can verify
//! rendering works on this machine without a GUI/Wayland screenshot.
//!
//!   cargo run --example render_check -- <input.pdf> <page_index> <output.png>

use pdfium_render::prelude::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("usage: render_check <pdf> <page> <out.png>");
    let page_idx: u16 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(0);
    let out = args.get(3).map(String::as_str).unwrap_or("/tmp/render_check.png");

    let pdfium = Pdfium::new(Pdfium::bind_to_system_library().expect("bind libpdfium"));
    let doc = pdfium.load_pdf_from_file(path, None).expect("load pdf");
    let pages = doc.pages();
    println!("pages = {}", pages.len());

    let page = pages.get(page_idx).expect("get page");
    let (wp, hp) = (page.width().value, page.height().value);
    println!("page {page_idx} size = {wp:.1} x {hp:.1} pts");

    let scale = 1.5_f32;
    let cfg = PdfRenderConfig::new()
        .set_target_size((wp * scale) as i32, (hp * scale) as i32)
        .set_reverse_byte_order(true);
    let bitmap = page.render_with_config(&cfg).expect("render");
    println!("rendered {} x {} px", bitmap.width(), bitmap.height());

    // Sanity: a rendered text page should not be uniformly one colour.
    let rgba = bitmap.as_rgba_bytes();
    let first = &rgba[0..4];
    let uniform = rgba.chunks_exact(4).all(|px| px == first);
    println!("uniform-colour image: {uniform} (expect false for a real page)");

    bitmap.as_image().save(out).expect("save png");
    println!("wrote {out}");
}
