//! PDF loading and page rendering via the system Pdfium library, with a per-page
//! texture cache that re-renders only when the zoom crosses a resolution bucket.

use std::collections::HashMap;
use std::path::Path;

use egui::{Context, TextureHandle, TextureId, TextureOptions};
use pdfium_render::prelude::*;

use crate::annotation::{Stroke, TextAnnotation};
use crate::view::PageSize;

/// Fraction of the font size used to drop from the visual top of the text to its baseline,
/// so a text annotation's normalised (top-left) anchor lands where it does on screen.
/// Approximates a typical Latin ascent ratio (Pdfium positions text by its baseline).
const ASCENT_RATIO: f32 = 0.8;

/// Maximum render resolution, in pixels-per-point, to cap memory/time at high zoom.
const MAX_SCALE: f32 = 4.0;
/// Quantisation step for the render scale, so small zoom changes reuse the cached texture.
const SCALE_STEP: f32 = 0.5;

struct CachedTex {
    handle: TextureHandle,
    /// Scale (px per point) the texture was rendered at.
    scale: f32,
}

/// An open PDF document plus its render cache.
///
/// The `Pdfium` instance is leaked to obtain a `'static` document, sidestepping the
/// self-referential-lifetime problem of storing both the library and a borrowing
/// document in one struct. Exactly one `Pdfium` is leaked for the lifetime of the app.
pub struct PdfDoc {
    doc: PdfDocument<'static>,
    pub sizes: Vec<PageSize>,
    cache: HashMap<usize, CachedTex>,
}

impl PdfDoc {
    /// Bind to the system `libpdfium` once, returning a leaked `'static` handle.
    pub fn init_pdfium() -> Result<&'static Pdfium, PdfiumError> {
        let bindings = Pdfium::bind_to_system_library()?;
        Ok(Box::leak(Box::new(Pdfium::new(bindings))))
    }

    /// Open `path` and read every page's size (in points).
    pub fn open(pdfium: &'static Pdfium, path: &Path) -> Result<Self, PdfiumError> {
        let doc = pdfium.load_pdf_from_file(path, None)?;
        let mut sizes = Vec::new();
        {
            let pages = doc.pages();
            let count = pages.len();
            for i in 0..count {
                let page = pages.get(i)?;
                sizes.push(PageSize {
                    w: page.width().value,
                    h: page.height().value,
                });
            }
        }
        Ok(Self {
            doc,
            sizes,
            cache: HashMap::new(),
        })
    }

    /// Quantise a desired px-per-point scale into a cache bucket.
    fn bucket(scale: f32) -> f32 {
        let s = (scale / SCALE_STEP).ceil() * SCALE_STEP;
        s.clamp(SCALE_STEP, MAX_SCALE)
    }

    /// Return the texture for page `idx` rendered for the current `zoom` (px per point),
    /// rendering/uploading it if absent or rendered at a coarser scale than requested.
    pub fn page_texture(
        &mut self,
        ctx: &Context,
        idx: usize,
        zoom: f32,
    ) -> Option<(TextureId, [usize; 2])> {
        let want = Self::bucket(zoom * ctx.pixels_per_point());

        let needs_render = match self.cache.get(&idx) {
            Some(c) => c.scale + f32::EPSILON < want,
            None => true,
        };

        if needs_render {
            if let Some(handle) = self.render(ctx, idx, want) {
                self.cache.insert(idx, CachedTex { handle, scale: want });
            }
        }

        self.cache
            .get(&idx)
            .map(|c| (c.handle.id(), c.handle.size()))
    }

    /// Burn `strokes` into a fresh copy of `src` and write a flattened PDF to `out`.
    ///
    /// Loads the source independently so the live document/cache is untouched. Strokes
    /// are added as stroked path objects in page space (origin bottom-left, y-up), with
    /// round caps/joins and the stroke's RGBA (alpha preserved → highlights stay
    /// translucent). Pages use the default `AutomaticOnEveryChange` regeneration, so the
    /// added objects are committed before save.
    pub fn export_flattened(
        pdfium: &'static Pdfium,
        src: &Path,
        out: &Path,
        strokes: &[Stroke],
        texts: &[TextAnnotation],
    ) -> Result<(), PdfiumError> {
        use std::collections::BTreeSet;

        let mut doc = pdfium.load_pdf_from_file(src, None)?;
        let page_count = doc.pages().len() as usize;

        // A single built-in font token, reused for every text object.
        let font = doc.fonts_mut().helvetica();

        // Render page-by-page so each page's content stream is regenerated once.
        let pages_with_content: BTreeSet<usize> = strokes
            .iter()
            .map(|s| s.page)
            .chain(texts.iter().map(|t| t.page))
            .filter(|&p| p < page_count)
            .collect();

        for pidx in pages_with_content {
            let mut page = doc.pages().get(pidx as u16)?;
            let (w, h) = (page.width().value, page.height().value);
            // Normalised (top-left origin) → page points (bottom-left origin, y-up).
            let to_pt = |n: [f32; 2]| (PdfPoints::new(n[0] * w), PdfPoints::new((1.0 - n[1]) * h));

            for s in strokes.iter().filter(|s| s.page == pidx) {
                if s.points.len() < 2 {
                    continue;
                }
                let (x0, y0) = to_pt(s.points[0]);
                let col = PdfColor::new(s.color[0], s.color[1], s.color[2], s.color[3]);
                let mut path = PdfPagePathObject::new(
                    &doc,
                    x0,
                    y0,
                    Some(col),
                    Some(PdfPoints::new(s.width_pt)),
                    None,
                )?;
                for n in &s.points[1..] {
                    let (x, y) = to_pt(*n);
                    path.line_to(x, y)?;
                }
                path.set_line_cap(PdfPageObjectLineCap::Round)?;
                path.set_line_join(PdfPageObjectLineJoin::Round)?;
                page.objects_mut().add_path_object(path)?;
            }

            for t in texts.iter().filter(|t| t.page == pidx && !t.text.is_empty()) {
                let mut obj =
                    PdfPageTextObject::new(&doc, &t.text, font, PdfPoints::new(t.size_pt))?;
                obj.set_fill_color(PdfColor::new(t.color[0], t.color[1], t.color[2], t.color[3]))?;
                // Pdfium places text by its baseline; drop from the top-left anchor by ~ascent.
                let (x, y) = to_pt(t.pos);
                obj.translate(x, y - PdfPoints::new(t.size_pt * ASCENT_RATIO))?;
                page.objects_mut().add_text_object(obj)?;
            }

            page.regenerate_content()?;
        }

        doc.save_to_file(out)
    }

    /// Render page `idx` at `scale` px-per-point into an egui texture.
    fn render(&self, ctx: &Context, idx: usize, scale: f32) -> Option<TextureHandle> {
        let size = self.sizes[idx];
        let w_px = (size.w * scale).round().max(1.0) as i32;
        let h_px = (size.h * scale).round().max(1.0) as i32;

        let page = self.doc.pages().get(idx as u16).ok()?;
        let config = PdfRenderConfig::new()
            .set_target_size(w_px, h_px)
            .set_reverse_byte_order(true); // yields straight RGBA from as_rgba_bytes()
        let bitmap = page.render_with_config(&config).ok()?;

        let dims = [bitmap.width() as usize, bitmap.height() as usize];
        let rgba = bitmap.as_rgba_bytes();
        let image = egui::ColorImage::from_rgba_unmultiplied(dims, &rgba);
        Some(ctx.load_texture(format!("pdf_page_{idx}"), image, TextureOptions::LINEAR))
    }
}
