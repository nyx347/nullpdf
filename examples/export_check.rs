//! Headless smoke test for flattened-PDF export. Mirrors the pdfium calls in
//! `PdfDoc::export_flattened` — stroked paths with round caps/joins, translucent highlights,
//! filled-circle dots, real text in the embedded TrueType font the app draws text with, and
//! symbol/emoji glyphs as filled outlines — then re-renders the result so we can visually
//! confirm everything was burned in.
//!
//!   cargo run --example export_check -- <input.pdf> <out.pdf> <out.png>

use pdfium_render::prelude::*;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::MetadataProvider;

/// Collects a glyph outline as pdfium path calls (glyph space is y-up, like PDF space).
struct PathPen<'a, 'b> {
    path: &'b mut Option<PdfPagePathObject<'a>>,
    doc: &'b PdfDocument<'a>,
    origin: (f32, f32),
    last: (f32, f32),
    color: PdfColor,
}

impl PathPen<'_, '_> {
    fn pt(&self, x: f32, y: f32) -> (PdfPoints, PdfPoints) {
        (
            PdfPoints::new(self.origin.0 + x),
            PdfPoints::new(self.origin.1 + y),
        )
    }
}

impl OutlinePen for PathPen<'_, '_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (px, py) = self.pt(x, y);
        match self.path.as_mut() {
            Some(p) => p.move_to(px, py).unwrap(),
            None => {
                let p = PdfPagePathObject::new(self.doc, px, py, None, None, Some(self.color));
                *self.path = Some(p.unwrap());
            }
        }
        self.last = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (px, py) = self.pt(x, y);
        self.path.as_mut().unwrap().line_to(px, py).unwrap();
        self.last = (x, y);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        // Quadratic → cubic: control points 2/3 of the way from each end to the quad control.
        let (x0, y0) = self.last;
        let c1 = self.pt(x0 + (cx - x0) * 2.0 / 3.0, y0 + (cy - y0) * 2.0 / 3.0);
        let c2 = self.pt(x + (cx - x) * 2.0 / 3.0, y + (cy - y) * 2.0 / 3.0);
        let (px, py) = self.pt(x, y);
        let path = self.path.as_mut().unwrap();
        path.bezier_to(px, py, c1.0, c1.1, c2.0, c2.1).unwrap();
        self.last = (x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c1, c2, (px, py)) = (self.pt(cx0, cy0), self.pt(cx1, cy1), self.pt(x, y));
        let path = self.path.as_mut().unwrap();
        path.bezier_to(px, py, c1.0, c1.1, c2.0, c2.1).unwrap();
        self.last = (x, y);
    }

    fn close(&mut self) {
        self.path.as_mut().unwrap().close_path().unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = args
        .get(1)
        .expect("usage: export_check <pdf> <out.pdf> <out.png>");
    let out_pdf = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("/tmp/exported.pdf");
    let out_png = args
        .get(3)
        .map(String::as_str)
        .unwrap_or("/tmp/exported.png");

    let pdfium = Pdfium::new(Pdfium::bind_to_system_library().expect("bind libpdfium"));

    // egui's proportional fallback chain: the text font first, then symbol/emoji faces.
    let defs = egui::FontDefinitions::default();
    let faces: Vec<_> = defs.families[&egui::FontFamily::Proportional]
        .iter()
        .map(|name| defs.font_data[name].clone())
        .collect();

    // --- burn strokes, a dot, text and a symbol glyph onto page 0 ---
    {
        let mut doc = pdfium.load_pdf_from_file(src, None).expect("load");
        let font = doc
            .fonts_mut()
            .load_true_type_from_bytes(&faces[0].font, true)
            .expect("embed text font");
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
            Some(PdfColor::new(255, 205, 0, 96)),
            Some(PdfPoints::new(18.0)),
            None,
        )
        .unwrap();
        let (hx1, hy1) = to_pt(hl[1]);
        h2.line_to(hx1, hy1).unwrap();
        h2.set_line_cap(PdfPageObjectLineCap::Round).unwrap();
        page.objects_mut().add_path_object(h2).unwrap();

        // A dot: a single-point stroke becomes a filled circle of the stroke's width.
        let (dx, dy) = to_pt([0.85, 0.20]);
        let dot = PdfPagePathObject::new_circle_at(
            &doc,
            dx,
            dy,
            PdfPoints::new(6.0),
            None,
            None,
            Some(PdfColor::new(30, 100, 220, 255)),
        )
        .unwrap();
        page.objects_mut().add_path_object(dot).unwrap();

        // Text in the embedded on-screen font, placed by its baseline (the app takes the
        // baseline from egui's layout; here the font's ascent below the top-left anchor).
        let size = 24.0_f32;
        let face = skrifa::FontRef::new(&faces[0].font).unwrap();
        let ascent = face.metrics(Size::new(size), LocationRef::default()).ascent;
        let (tx, ty) = to_pt([0.20, 0.30]);
        let mut text =
            PdfPageTextObject::new(&doc, "Hello, nullpdf — é ü ß", font, PdfPoints::new(size))
                .expect("text obj");
        text.set_fill_color(PdfColor::new(20, 40, 220, 255))
            .unwrap();
        text.translate(tx, ty - PdfPoints::new(ascent)).unwrap();
        page.objects_mut().add_text_object(text).unwrap();

        // A symbol from a fallback face, as a filled outline (egui scales fallback faces by
        // their FontTweak).
        let (star_face, gid) = faces
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(i, f)| {
                let gid = skrifa::FontRef::new(&f.font).ok()?.charmap().map('★')?;
                Some((i, gid))
            })
            .expect("a fallback face with ★");
        let star = skrifa::FontRef::new(&faces[star_face].font).unwrap();
        let glyph = star.outline_glyphs().get(gid).unwrap();
        let (sx, sy) = to_pt([0.70, 0.30]);
        let mut path = None;
        let mut pen = PathPen {
            path: &mut path,
            doc: &doc,
            origin: (sx.value, sy.value - size * 0.8),
            last: (0.0, 0.0),
            color: PdfColor::new(200, 140, 0, 255),
        };
        let star_size = Size::new(size * faces[star_face].tweak.scale);
        glyph
            .draw(
                DrawSettings::unhinted(star_size, LocationRef::default()),
                &mut pen,
            )
            .unwrap();
        let mut path = path.expect("glyph outline");
        path.set_fill_and_stroke_mode(PdfPathFillMode::Winding, false)
            .unwrap();
        page.objects_mut().add_path_object(path).unwrap();

        page.regenerate_content().unwrap();
        doc.save_to_file(out_pdf).expect("save");
        println!("wrote {out_pdf}");
    }

    // --- re-render the exported file to confirm everything is present ---
    let doc = pdfium
        .load_pdf_from_file(out_pdf, None)
        .expect("reload exported");
    let page = doc.pages().get(0).unwrap();
    let cfg = PdfRenderConfig::new()
        .set_target_size(
            (page.width().value * 1.5) as i32,
            (page.height().value * 1.5) as i32,
        )
        .set_reverse_byte_order(true);
    let bmp = page.render_with_config(&cfg).unwrap();
    bmp.as_image().save(out_png).expect("save png");
    println!(
        "rendered exported page 0 -> {out_png} ({} x {})",
        bmp.width(),
        bmp.height()
    );
}
