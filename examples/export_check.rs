//! Headless smoke test for flattened-PDF export. Mirrors the pdfium path-object +
//! save calls in `PdfDoc::export_flattened`, then re-renders the result so we can
//! visually confirm the burned-in strokes.
//!
//!   cargo run --example export_check -- <input.pdf> <out.pdf> <out.png>

use pdfium_render::prelude::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = args.get(1).expect("usage: export_check <pdf> <out.pdf> <out.png>");
    let out_pdf = args.get(2).map(String::as_str).unwrap_or("/tmp/exported.pdf");
    let out_png = args.get(3).map(String::as_str).unwrap_or("/tmp/exported.png");

    let pdfium = Pdfium::new(Pdfium::bind_to_system_library().expect("bind libpdfium"));

    // --- burn two strokes + one text onto page 0 (normalised, top-left origin) ---
    {
        let mut doc = pdfium.load_pdf_from_file(src, None).expect("load");
        let font = doc.fonts_mut().helvetica();
        let mut page = doc.pages().get(0).expect("page 0");
        let (w, h) = (page.width().value, page.height().value);
        let to_pt = |n: [f32; 2]| (PdfPoints::new(n[0] * w), PdfPoints::new((1.0 - n[1]) * h));

        // Opaque red diagonal line.
        let line = [[0.15, 0.20], [0.85, 0.80]];
        let (x0, y0) = to_pt(line[0]);
        let mut p = PdfPagePathObject::new(
            &doc,
            x0,
            y0,
            Some(PdfColor::new(220, 40, 40, 255)),
            Some(PdfPoints::new(3.0)),
            None,
        )
        .unwrap();
        let (x1, y1) = to_pt(line[1]);
        p.line_to(x1, y1).unwrap();
        p.set_line_cap(PdfPageObjectLineCap::Round).unwrap();
        p.set_line_join(PdfPageObjectLineJoin::Round).unwrap();
        page.objects_mut().add_path_object(p).unwrap();

        // Translucent yellow highlight (wide stroke, low alpha).
        let hl = [[0.10, 0.50], [0.90, 0.50]];
        let (hx0, hy0) = to_pt(hl[0]);
        let mut h2 = PdfPagePathObject::new(
            &doc,
            hx0,
            hy0,
            Some(PdfColor::new(255, 230, 0, 90)),
            Some(PdfPoints::new(18.0)),
            None,
        )
        .unwrap();
        let (hx1, hy1) = to_pt(hl[1]);
        h2.line_to(hx1, hy1).unwrap();
        h2.set_line_cap(PdfPageObjectLineCap::Round).unwrap();
        page.objects_mut().add_path_object(h2).unwrap();

        // Blue text label, anchored top-left (mirrors PdfDoc::export_flattened).
        let size = 24.0_f32;
        let (tx, ty) = to_pt([0.20, 0.30]);
        let mut text = PdfPageTextObject::new(&doc, "Hello, nullpdf", font, PdfPoints::new(size))
            .expect("text obj");
        text.set_fill_color(PdfColor::new(20, 40, 220, 255)).unwrap();
        text.translate(tx, ty - PdfPoints::new(size * 0.8)).unwrap();
        page.objects_mut().add_text_object(text).unwrap();

        page.regenerate_content().unwrap();
        doc.save_to_file(out_pdf).expect("save");
        println!("wrote {out_pdf}");
    }

    // --- re-render the exported file to confirm strokes are present ---
    let doc = pdfium.load_pdf_from_file(out_pdf, None).expect("reload exported");
    let page = doc.pages().get(0).unwrap();
    let cfg = PdfRenderConfig::new()
        .set_target_size((page.width().value * 1.5) as i32, (page.height().value * 1.5) as i32)
        .set_reverse_byte_order(true);
    let bmp = page.render_with_config(&cfg).unwrap();
    bmp.as_image().save(out_png).expect("save png");
    println!("rendered exported page 0 -> {out_png} ({} x {})", bmp.width(), bmp.height());
}
