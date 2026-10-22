//! Headless smoke test for text search and the outline (bookmarks). Mirrors the pdfium calls
//! in `src/pdf.rs` — `PdfDoc::find_text` (`text().search()` + `find_next()` segments),
//! `read_outline` / `dest_of`, `PdfDoc::dest_offset`, and the `UserToPage` transform sampled
//! from `points_to_pixels` — so we can verify them without a GUI.
//!
//! The transform is the part worth checking: matches and bookmark targets come back in PDF
//! user space (y-up, offset by the crop box, before `/Rotate`) and must land on the page as
//! rendered. Each match rectangle is checked against a render of its page: the ink around
//! it must fall inside it. A misplaced rectangle (wrong flip, ignored crop offset or
//! rotation) lands on blank paper or on other text and fails.
//!
//!   cargo run --example search_check -- <input.pdf> <query> <out.png>

use std::collections::HashSet;

use pdfium_render::prelude::*;

const XFORM_SCALE: f32 = 64.0;
const XFORM_SPAN: f32 = 1024.0;
/// Render scale (px per point) for the registration check and the output image.
const SCALE: f32 = 2.0;
/// Share of the nearby ink that must fall inside a match. Anti-aliasing from a neighbouring
/// glyph a space away can reach in at small sizes (≈ 5% at 9 pt); a misplaced rectangle,
/// landing between lines or on another word, keeps well under this.
const REGISTERED: f64 = 0.9;

/// As `UserToPage` in src/pdf.rs: user space → page-local points (top-left origin).
struct UserToPage {
    origin: (f32, f32),
    ex: (f32, f32),
    ey: (f32, f32),
}

impl UserToPage {
    fn of(page: &PdfPage) -> Option<Self> {
        let config = PdfRenderConfig::new().scale_page_by_factor(XFORM_SCALE);
        let at = |x: f32, y: f32| {
            let (px, py) = page
                .points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &config)
                .ok()?;
            Some((px as f32 / XFORM_SCALE, py as f32 / XFORM_SCALE))
        };
        let o = at(0.0, 0.0)?;
        let x = at(XFORM_SPAN, 0.0)?;
        let y = at(0.0, XFORM_SPAN)?;
        Some(Self {
            origin: o,
            ex: ((x.0 - o.0) / XFORM_SPAN, (x.1 - o.1) / XFORM_SPAN),
            ey: ((y.0 - o.0) / XFORM_SPAN, (y.1 - o.1) / XFORM_SPAN),
        })
    }

    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.origin.0 + self.ex.0 * x + self.ey.0 * y,
            self.origin.1 + self.ex.1 * x + self.ey.1 * y,
        )
    }

    /// `[left, top, right, bottom]` in page-local points.
    fn rect(&self, r: &PdfRect) -> [f32; 4] {
        let a = self.point(r.left().value, r.top().value);
        let b = self.point(r.right().value, r.bottom().value);
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
    }
}

/// As `PdfDoc::find_text`: every match on the page, as its rectangles.
fn find_text(page: &PdfPage, query: &str) -> Vec<Vec<[f32; 4]>> {
    let mut hits = Vec::new();
    let (Ok(text), Some(xf)) = (page.text(), UserToPage::of(page)) else {
        return hits;
    };
    let options = PdfSearchOptions::new().match_case(false);
    let Ok(search) = text.search(query, &options) else {
        return hits;
    };
    while let Some(segments) = search.find_next() {
        let rects: Vec<[f32; 4]> = segments.iter().map(|s| xf.rect(&s.bounds())).collect();
        if !rects.is_empty() {
            hits.push(rects);
        }
    }
    hits
}

struct Item {
    title: String,
    depth: usize,
    dest: Option<(usize, Option<f32>, Option<f32>)>,
}

/// As `read_outline` + `dest_of`.
fn read_outline(doc: &PdfDocument) -> Vec<Item> {
    use PdfDestinationViewSettings as V;
    let dest_of = |d: &PdfDestination| {
        let page = d.page_index().ok()? as usize;
        let (x, y) = match d.view_settings() {
            Ok(V::SpecificCoordinatesAndZoom(x, y, _)) => (x, y),
            Ok(V::FitPageHorizontallyToWindow(y) | V::FitBoundsHorizontallyToWindow(y)) => {
                (None, y)
            }
            Ok(V::FitPageToRectangle(r)) => (Some(r.left()), Some(r.top())),
            _ => (None, None),
        };
        Some((page, x.map(|v| v.value), y.map(|v| v.value)))
    };
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(PdfBookmark, usize)> = Vec::new();
    stack.extend(doc.bookmarks().root().map(|b| (b, 0)));
    while let Some((b, depth)) = stack.pop() {
        if !seen.insert(b.clone()) {
            continue;
        }
        let dest = match b.destination() {
            Some(d) => dest_of(&d),
            None => b.action().and_then(|a| {
                let d = a.as_local_destination_action()?.destination().ok()?;
                dest_of(&d)
            }),
        };
        items.push(Item {
            title: b.title().unwrap_or_default(),
            depth,
            dest,
        });
        stack.extend(b.next_sibling().map(|s| (s, depth)));
        stack.extend(b.first_child().map(|c| (c, depth + 1)));
    }
    items
}

/// Share of the ink near `r` (within a window a little larger than it) that lies inside
/// `r`, and whether there was any ink at all.
fn registration(rgba: &[u8], w: usize, h: usize, r: [f32; 4]) -> (f64, bool) {
    let rh = r[3] - r[1];
    let rw = r[2] - r[0];
    // Little slack along the line (less than a space), more across it (ascenders,
    // descenders) but less than the gap to the next line. Rotated pages run vertically.
    let (sx, sy) = if rw > rh {
        (1.5, rh * 0.3)
    } else {
        (rw * 0.3, 1.5)
    };
    let window = [r[0] - sx, r[1] - sy, r[2] + sx, r[3] + sy];
    let inside = [r[0] - 0.75, r[1] - 0.75, r[2] + 0.75, r[3] + 0.75];
    let (mut ink, mut ink_in) = (0usize, 0usize);
    let px = |v: f32, max: usize| ((v * SCALE).floor().max(0.0) as usize).min(max);
    for y in px(window[1], h)..px(window[3], h) {
        for x in px(window[0], w)..px(window[2], w) {
            let i = (y * w + x) * 4;
            let luma = 0.3 * rgba[i] as f32 + 0.59 * rgba[i + 1] as f32 + 0.11 * rgba[i + 2] as f32;
            if luma < 160.0 {
                ink += 1;
                let (fx, fy) = ((x as f32 + 0.5) / SCALE, (y as f32 + 0.5) / SCALE);
                if fx >= inside[0] && fx <= inside[2] && fy >= inside[1] && fy <= inside[3] {
                    ink_in += 1;
                }
            }
        }
    }
    if ink == 0 {
        return (0.0, false);
    }
    (ink_in as f64 / ink as f64, true)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: search_check <pdf> <query> <out.png>";
    let path = args.get(1).expect(usage);
    let query = args.get(2).expect(usage);
    let out = args
        .get(3)
        .map(String::as_str)
        .unwrap_or("/tmp/search_check.png");

    let pdfium = Pdfium::new(Pdfium::bind_to_system_library().expect("bind libpdfium"));
    let doc = pdfium.load_pdf_from_file(path, None).expect("load pdf");
    let pages = doc.pages();
    println!("pages = {}", pages.len());

    // Outline, with each target resolved as PdfDoc::dest_offset does.
    let outline = read_outline(&doc);
    println!("outline: {} bookmarks", outline.len());
    for it in &outline {
        let target = match it.dest {
            Some((page, x, y)) => {
                let offset = y.and_then(|y| {
                    let p = pages.get(page as u16).ok()?;
                    let h = p.height().value;
                    let pt = UserToPage::of(&p)?.point(x.unwrap_or(0.0), y);
                    Some(pt.1.clamp(0.0, h))
                });
                match offset {
                    Some(o) => format!("page {} at {o:.1} pt", page + 1),
                    None => format!("page {}", page + 1),
                }
            }
            None => "(outside this document)".to_string(),
        };
        println!("  {}{} -> {target}", "  ".repeat(it.depth), it.title);
    }

    // Search every page; check every match against a render of its page.
    let (mut total, mut checked, mut misplaced, mut blank) = (0, 0, 0, 0);
    let mut first_page_with_hits = None;
    for i in 0..pages.len() {
        let page = pages.get(i).expect("get page");
        let hits = find_text(&page, query);
        if hits.is_empty() {
            continue;
        }
        first_page_with_hits.get_or_insert(i);
        total += hits.len();
        let (wp, hp) = (page.width().value, page.height().value);
        let cfg = PdfRenderConfig::new()
            .set_target_size((wp * SCALE).round() as i32, (hp * SCALE).round() as i32)
            .set_reverse_byte_order(true);
        let bitmap = page.render_with_config(&cfg).expect("render");
        let (bw, bh) = (bitmap.width() as usize, bitmap.height() as usize);
        let rgba = bitmap.as_rgba_bytes();
        let rot = page
            .rotation()
            .map(|r| format!("{r:?}"))
            .unwrap_or_default();
        println!(
            "page {} ({wp:.0} x {hp:.0} pt, {rot}): {} matches, first at {:?}",
            i + 1,
            hits.len(),
            hits[0][0].map(|v| (v * 10.0).round() / 10.0)
        );
        for r in hits.iter().flatten() {
            checked += 1;
            let (share, any) = registration(&rgba, bw, bh, *r);
            if !any {
                blank += 1;
            } else if share < REGISTERED {
                misplaced += 1;
                println!(
                    "  rect {r:?}: only {:.0}% of nearby ink inside",
                    share * 100.0
                );
            }
        }
    }
    println!(
        "{total} matches for {query:?}; {checked} rects checked: {misplaced} misplaced, \
         {blank} on blank paper -> {}",
        if checked > 0 && misplaced == 0 && blank == 0 {
            "registered"
        } else {
            "CHECK FAILED"
        }
    );

    // The first page with matches, with every match outlined, for a visual check.
    let idx = first_page_with_hits.unwrap_or(0);
    let page = pages.get(idx).expect("get page");
    let (wp, hp) = (page.width().value, page.height().value);
    let cfg = PdfRenderConfig::new()
        .set_target_size((wp * SCALE).round() as i32, (hp * SCALE).round() as i32)
        .set_reverse_byte_order(true);
    let mut image = page
        .render_with_config(&cfg)
        .expect("render")
        .as_image()
        .into_rgba8();
    for r in find_text(&page, query).iter().flatten() {
        let [l, t, rr, b] = r.map(|v| (v * SCALE).round() as i64);
        for x in l..=rr {
            for y in [t, b] {
                if let Some(p) = image.get_pixel_mut_checked(x as u32, y as u32) {
                    p.0 = [255, 0, 0, 255];
                }
            }
        }
        for y in t..=b {
            for x in [l, rr] {
                if let Some(p) = image.get_pixel_mut_checked(x as u32, y as u32) {
                    p.0 = [255, 0, 0, 255];
                }
            }
        }
    }
    image.save(out).expect("save png");
    println!("wrote {out} (page {} with matches outlined)", idx + 1);
}
