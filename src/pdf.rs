//! PDF loading, page rendering and flattened export via the system Pdfium library.
//!
//! Rendering keeps one texture per page. Pages get a texture as soon as they scroll into
//! view; once the view settles, each visible page is re-rendered at *exactly* its on-screen
//! resolution so text is pixel-crisp. Whole-page textures are capped in size; beyond that a
//! "detail tile" covering just the visible part of the page is rendered at full resolution
//! on top. All rendering is time-budgeted per frame, nearest pages first, and far-away
//! textures are evicted to bound memory.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::time::Instant;

use egui::{Context, Rect, TextureHandle, TextureId, TextureOptions};
use pdfium_render::prelude::*;

use crate::annotation::Stroke;
use crate::view::PageSize;

/// Longest side (px) of a whole-page texture; beyond this a detail tile takes over.
const MAX_PAGE_SIDE: f32 = 4096.0;
/// Cached page pixels (≈ 256 MB of RGBA) beyond which the farthest pages are evicted.
const CACHE_BUDGET_PX: usize = 64 << 20;
/// Render time per frame (s) after which remaining work waits for the next frame.
const FRAME_BUDGET: f32 = 0.014;
/// Relative scale mismatch tolerated before a settled page is re-rendered.
const EXACT_TOLERANCE: f32 = 0.015;
/// Extra area rendered around the visible part of a page for a detail tile, so small pans
/// stay sharp.
const TILE_MARGIN: f32 = 0.35;

struct PageTex {
    handle: TextureHandle,
    /// Scale (px per point) the texture was rendered at.
    scale: f32,
    /// Rendered from an older version of the file (shown until replaced).
    stale: bool,
}

struct Tile {
    handle: TextureHandle,
    scale: f32,
    /// Region of the page it covers, page-local points (top-left origin).
    rect: Rect,
}

/// A page the current frame shows.
pub struct PageNeed {
    pub index: usize,
    /// Physical pixels per point the page is displayed at.
    pub scale: f32,
    /// Visible part of the page, page-local points (top-left origin).
    pub visible: Rect,
    /// Lower renders first (distance from the viewport centre).
    pub priority: f32,
}

/// A texture to draw for a page.
pub struct PageImage {
    pub id: TextureId,
    /// Texture size in pixels.
    pub size: [usize; 2],
}

/// An open PDF document plus its render cache.
///
/// The `Pdfium` instance is leaked to obtain a `'static` document, sidestepping the
/// self-referential-lifetime problem of storing both the library and a borrowing
/// document in one struct. Exactly one `Pdfium` is leaked for the lifetime of the app.
pub struct PdfDoc {
    doc: PdfDocument<'static>,
    pub sizes: Vec<PageSize>,
    /// Interactive forms only render without a transform, so they get no detail tiles.
    has_form: bool,
    pages: HashMap<usize, PageTex>,
    tiles: HashMap<usize, Tile>,
}

/// Text placed by the exporter: a run of characters in the embedded text font, with its
/// baseline origin in page-local points (top-left origin).
pub struct TextRun {
    pub page: usize,
    pub text: String,
    pub baseline: [f32; 2],
    pub size_pt: f32,
    pub color: [u8; 4],
}

/// One step of a vector outline, in page-local points (top-left origin).
#[derive(Clone, Copy, Debug)]
pub enum PathCmd {
    Move([f32; 2]),
    Line([f32; 2]),
    /// Cubic Bézier: two control points, then the end point.
    Cubic([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

/// A filled vector shape: a glyph that isn't in the text font (symbols, emoji) drawn as
/// its outline, exactly as the screen shows it.
pub struct FilledShape {
    pub page: usize,
    pub color: [u8; 4],
    pub path: Vec<PathCmd>,
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
        let has_form = doc.form().is_some();
        Ok(Self {
            doc,
            sizes,
            has_form,
            pages: HashMap::new(),
            tiles: HashMap::new(),
        })
    }

    /// Take over `old`'s page textures (marked stale) so a reloaded document keeps showing
    /// the previous pixels until fresh ones are rendered, instead of flashing blank.
    pub fn adopt_textures(&mut self, old: PdfDoc) {
        for (i, mut tex) in old.pages {
            if i < self.sizes.len() {
                tex.stale = true;
                self.pages.insert(i, tex);
            }
        }
    }

    /// The best whole-page texture available for page `idx`.
    pub fn page_image(&self, idx: usize) -> Option<PageImage> {
        self.pages.get(&idx).map(|t| PageImage {
            id: t.handle.id(),
            size: t.handle.size(),
        })
    }

    /// The detail tile for page `idx`, with the page-local region (points) it covers.
    pub fn tile_image(&self, idx: usize) -> Option<(PageImage, Rect)> {
        self.tiles.get(&idx).map(|t| {
            (
                PageImage {
                    id: t.handle.id(),
                    size: t.handle.size(),
                },
                t.rect,
            )
        })
    }

    /// Whole-page render scale for `want`, capped so the texture stays a sane size.
    fn base_scale(&self, idx: usize, want: f32, max_side: f32) -> f32 {
        let s = self.sizes[idx];
        want.min(max_side / s.w.max(s.h).max(1.0))
    }

    /// Render what this frame shows, within a time budget, and evict what it doesn't.
    ///
    /// Pages without any texture are rendered immediately. Once the view has `settled`,
    /// visible pages are brought to exact resolution and detail tiles are made. `keep` is
    /// the range of pages worth keeping cached (visible plus a little look-ahead).
    /// Returns `true` if work is left over for a later frame.
    pub fn prepare(
        &mut self,
        ctx: &Context,
        needs: &[PageNeed],
        keep: Range<usize>,
        settled: bool,
    ) -> bool {
        let start = Instant::now();
        let max_side = MAX_PAGE_SIDE.min(ctx.input(|i| i.max_texture_side) as f32);
        let mut order: Vec<&PageNeed> = needs.iter().collect();
        order.sort_by(|a, b| a.priority.total_cmp(&b.priority));

        let over_budget = |rendered: bool| rendered && start.elapsed().as_secs_f32() > FRAME_BUDGET;
        let mut rendered = false;
        let mut pending = false;

        // Whole-page textures: missing ones first, then (when settled) resolution fixes.
        for pass in 0..2 {
            for need in &order {
                let base = self.base_scale(need.index, need.scale, max_side);
                let wanted = match self.pages.get(&need.index) {
                    None => pass == 0,
                    Some(t) => {
                        pass == 1
                            && settled
                            && (t.stale || ((t.scale - base) / base).abs() > EXACT_TOLERANCE)
                    }
                };
                if !wanted {
                    continue;
                }
                if over_budget(rendered) {
                    pending = true;
                    break;
                }
                if let Some(handle) = self.render_page(ctx, need.index, base) {
                    self.pages.insert(
                        need.index,
                        PageTex {
                            handle,
                            scale: base,
                            stale: false,
                        },
                    );
                }
                rendered = true;
            }
        }
        if !settled
            && needs
                .iter()
                .any(|n| self.pages.get(&n.index).is_some_and(|t| t.stale))
        {
            pending = true;
        }

        // Detail tiles, for pages shown sharper than their whole-page texture can be.
        let mut tiled = Vec::new();
        for need in &order {
            let base = self.base_scale(need.index, need.scale, max_side);
            if self.has_form || need.scale <= base * (1.0 + EXACT_TOLERANCE) {
                continue;
            }
            tiled.push(need.index);
            // Judge freshness against what a tile *can* cover: on huge views the tile is
            // capped at the texture limit and never contains the whole visible region.
            let rect = self.tile_rect(need, max_side);
            let fresh = self.tiles.get(&need.index).is_some_and(|t| {
                ((t.scale - need.scale) / need.scale).abs() <= EXACT_TOLERANCE
                    && t.rect.contains_rect(need.visible.intersect(rect))
            });
            if fresh || !settled {
                continue;
            }
            if over_budget(rendered) {
                pending = true;
                break;
            }
            if let Some(handle) = self.render_region(ctx, need.index, need.scale, rect) {
                self.tiles.insert(
                    need.index,
                    Tile {
                        handle,
                        scale: need.scale,
                        rect,
                    },
                );
            }
            rendered = true;
        }
        self.tiles.retain(|i, _| tiled.contains(i));

        self.evict(needs, keep);
        pending
    }

    /// Page-local region a detail tile should cover: the visible part plus a margin, kept
    /// within the page and within the maximum texture size.
    fn tile_rect(&self, need: &PageNeed, max_side: f32) -> Rect {
        let s = self.sizes[need.index];
        let page = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(s.w, s.h));
        let v = need.visible;
        let max_pt = max_side / need.scale;
        let half = egui::vec2(
            (v.width() * (0.5 + TILE_MARGIN)).min(max_pt * 0.5),
            (v.height() * (0.5 + TILE_MARGIN)).min(max_pt * 0.5),
        );
        Rect::from_center_size(v.center(), half * 2.0).intersect(page)
    }

    /// Drop textures for pages outside `keep`, then the farthest ones while over budget.
    fn evict(&mut self, needs: &[PageNeed], keep: Range<usize>) {
        self.pages
            .retain(|i, _| keep.contains(i) || needs.iter().any(|n| n.index == *i));
        let pixels = |t: &PageTex| t.handle.size()[0] * t.handle.size()[1];
        let mut total: usize = self.pages.values().map(pixels).sum();
        if total <= CACHE_BUDGET_PX {
            return;
        }
        let centre = (keep.start + keep.end) as f32 * 0.5;
        let mut by_distance: Vec<usize> = self
            .pages
            .keys()
            .copied()
            .filter(|i| !needs.iter().any(|n| n.index == *i))
            .collect();
        by_distance.sort_by(|a, b| {
            (*b as f32 - centre)
                .abs()
                .total_cmp(&(*a as f32 - centre).abs())
        });
        for i in by_distance {
            if total <= CACHE_BUDGET_PX {
                break;
            }
            if let Some(t) = self.pages.remove(&i) {
                total -= pixels(&t);
            }
        }
    }

    /// Render all of page `idx` at `scale` px-per-point into an egui texture.
    fn render_page(&self, ctx: &Context, idx: usize, scale: f32) -> Option<TextureHandle> {
        let size = self.sizes[idx];
        let w_px = (size.w * scale).round().max(1.0) as i32;
        let h_px = (size.h * scale).round().max(1.0) as i32;

        let page = self.doc.pages().get(idx as u16).ok()?;
        let config = PdfRenderConfig::new()
            .set_target_size(w_px, h_px)
            .set_reverse_byte_order(true); // yields straight RGBA from as_rgba_bytes()
        let bitmap = page.render_with_config(&config).ok()?;
        Some(Self::upload(ctx, format!("pdf_page_{idx}"), &bitmap))
    }

    /// Render the page-local `region` (points, top-left origin) of page `idx` at `scale`.
    /// Uses a render transform; the result registers exactly with the matching crop of a
    /// whole-page render (verified by `examples/render_check`).
    fn render_region(
        &self,
        ctx: &Context,
        idx: usize,
        scale: f32,
        region: Rect,
    ) -> Option<TextureHandle> {
        let w_px = (region.width() * scale).round().max(1.0) as i32;
        let h_px = (region.height() * scale).round().max(1.0) as i32;
        let page = self.doc.pages().get(idx as u16).ok()?;
        let config = PdfRenderConfig::new()
            .set_fixed_size(w_px, h_px)
            .scale_page_by_factor(scale)
            .set_reverse_byte_order(true)
            .translate(PdfPoints::new(-region.min.x), PdfPoints::new(-region.min.y))
            .ok()?;
        let bitmap = page.render_with_config(&config).ok()?;
        Some(Self::upload(ctx, format!("pdf_tile_{idx}"), &bitmap))
    }

    fn upload(ctx: &Context, name: String, bitmap: &PdfBitmap) -> TextureHandle {
        let dims = [bitmap.width() as usize, bitmap.height() as usize];
        let rgba = bitmap.as_rgba_bytes();
        let image = egui::ColorImage::from_rgba_unmultiplied(dims, &rgba);
        ctx.load_texture(name, image, TextureOptions::LINEAR)
    }

    /// Burn `strokes`, text `runs` and glyph `shapes` into a fresh copy of `src` and write
    /// it to `out`.
    ///
    /// Loads the source independently so the live document/cache is untouched. Strokes are
    /// added as stroked path objects in page space (origin bottom-left, y-up) with round
    /// caps/joins and the stroke's RGBA (alpha preserved → highlights stay translucent);
    /// single-point strokes become filled dots. Text is real, selectable text in `font` (the
    /// TrueType face the app draws it with, embedded only if used), and glyphs from the
    /// symbol/emoji fallback faces are filled outlines, so the export matches the screen.
    /// Pages use the default `AutomaticOnEveryChange` regeneration, so the added objects are
    /// committed before save.
    pub fn export_flattened(
        pdfium: &'static Pdfium,
        src: &Path,
        out: &Path,
        strokes: &[Stroke],
        runs: &[TextRun],
        shapes: &[FilledShape],
        font: &[u8],
    ) -> Result<(), PdfiumError> {
        use std::collections::BTreeSet;

        let mut doc = pdfium.load_pdf_from_file(src, None)?;
        let page_count = doc.pages().len() as usize;

        // Embed the text font only if there's text; fall back to built-in Helvetica if it
        // won't load.
        let token = (!runs.is_empty()).then(|| {
            doc.fonts_mut()
                .load_true_type_from_bytes(font, true)
                .unwrap_or_else(|_| doc.fonts_mut().helvetica())
        });

        // Render page-by-page so each page's content stream is regenerated once.
        let pages_with_content: BTreeSet<usize> = strokes
            .iter()
            .map(|s| s.page)
            .chain(runs.iter().map(|r| r.page))
            .chain(shapes.iter().map(|s| s.page))
            .filter(|&p| p < page_count)
            .collect();

        for pidx in pages_with_content {
            let mut page = doc.pages().get(pidx as u16)?;
            let (w, h) = (page.width().value, page.height().value);
            // Normalised (top-left origin) → page points (bottom-left origin, y-up).
            let to_pt = |n: [f32; 2]| (PdfPoints::new(n[0] * w), PdfPoints::new((1.0 - n[1]) * h));

            for s in strokes.iter().filter(|s| s.page == pidx) {
                let col = PdfColor::new(s.color[0], s.color[1], s.color[2], s.color[3]);
                let Some(&first) = s.points.first() else {
                    continue;
                };
                let (x0, y0) = to_pt(first);
                if s.points.len() == 1 {
                    let dot = PdfPagePathObject::new_circle_at(
                        &doc,
                        x0,
                        y0,
                        PdfPoints::new(s.width_pt * 0.5),
                        None,
                        None,
                        Some(col),
                    )?;
                    page.objects_mut().add_path_object(dot)?;
                    continue;
                }
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

            for run in runs.iter().filter(|r| r.page == pidx && !r.text.is_empty()) {
                let Some(font) = token else {
                    continue;
                };
                let mut obj =
                    PdfPageTextObject::new(&doc, &run.text, font, PdfPoints::new(run.size_pt))?;
                let c = run.color;
                obj.set_fill_color(PdfColor::new(c[0], c[1], c[2], c[3]))?;
                // Pdfium places text by its baseline origin, which the run already gives.
                obj.translate(
                    PdfPoints::new(run.baseline[0]),
                    PdfPoints::new(h - run.baseline[1]),
                )?;
                page.objects_mut().add_text_object(obj)?;
            }

            // Page-local (top-left origin) → page points (bottom-left origin, y-up).
            let at = |p: [f32; 2]| (PdfPoints::new(p[0]), PdfPoints::new(h - p[1]));
            for shape in shapes.iter().filter(|s| s.page == pidx) {
                let Some(PathCmd::Move(first)) = shape.path.first() else {
                    continue;
                };
                let c = shape.color;
                let (x0, y0) = at(*first);
                let col = PdfColor::new(c[0], c[1], c[2], c[3]);
                let mut path = PdfPagePathObject::new(&doc, x0, y0, None, None, Some(col))?;
                for cmd in &shape.path[1..] {
                    match *cmd {
                        PathCmd::Move(p) => {
                            let (x, y) = at(p);
                            path.move_to(x, y)?;
                        }
                        PathCmd::Line(p) => {
                            let (x, y) = at(p);
                            path.line_to(x, y)?;
                        }
                        PathCmd::Cubic(c1, c2, p) => {
                            let ((x, y), (x1, y1), (x2, y2)) = (at(p), at(c1), at(c2));
                            path.bezier_to(x, y, x1, y1, x2, y2)?;
                        }
                        PathCmd::Close => path.close_path()?,
                    }
                }
                path.set_fill_and_stroke_mode(PdfPathFillMode::Winding, false)?;
                page.objects_mut().add_path_object(path)?;
            }

            page.regenerate_content()?;
        }

        doc.save_to_file(out)
    }
}
