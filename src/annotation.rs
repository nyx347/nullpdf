//! Annotation data model + sidecar persistence + undo/redo.
//!
//! Strokes are stored in **normalised page coordinates** (each component in `0.0..=1.0`,
//! origin at the page's top-left). This makes them independent of zoom and render
//! resolution, and lets an externally-edited base PDF reload with annotations still
//! aligned, as long as page order/count is unchanged.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A single freehand mark. `Freehand` keeps the (smoothed) sampled points — a single point
/// is a dot; `Line` keeps exactly two points (press + release) and is drawn as a straight
/// segment at any angle.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stroke {
    /// Page index this stroke belongs to.
    pub page: usize,
    pub kind: StrokeKind,
    /// Points in normalised page space (`0..1`, top-left origin).
    pub points: Vec<[f32; 2]>,
    /// Straight, non-premultiplied sRGBA. Alpha drives ink (opaque) vs highlight (transparent).
    pub color: [u8; 4],
    /// Stroke width in PDF points (scales with zoom when painted).
    pub width_pt: f32,
    /// Runtime-only identity used by undo/redo; never serialized.
    #[serde(skip)]
    pub id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeKind {
    Freehand,
    Line,
}

/// A piece of text pinned to a page. Like strokes, the anchor is stored in normalised
/// page space (top-left origin) so it survives zoom/rotation and an external reload.
/// `text` may contain `\n` line breaks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextAnnotation {
    /// Page index this text belongs to.
    pub page: usize,
    /// Anchor of the text's top-left corner, normalised page space (`0..1`, top-left origin).
    pub pos: [f32; 2],
    pub text: String,
    /// Straight, non-premultiplied sRGBA.
    pub color: [u8; 4],
    /// Font size in PDF points (scales with zoom when painted).
    pub size_pt: f32,
    /// Runtime-only identity used by undo/redo; never serialized.
    #[serde(skip)]
    pub id: u64,
}

/// A single annotation, tagged by kind. Both variants share one id space so undo/redo,
/// erase, and the sidecar can treat them uniformly.
#[derive(Clone, Debug)]
pub enum Annotation {
    Stroke(Stroke),
    Text(TextAnnotation),
}

impl Annotation {
    pub fn id(&self) -> u64 {
        match self {
            Annotation::Stroke(s) => s.id,
            Annotation::Text(t) => t.id,
        }
    }

    pub fn page(&self) -> usize {
        match self {
            Annotation::Stroke(s) => s.page,
            Annotation::Text(t) => t.page,
        }
    }
}

/// Result of looking for a sidecar beside a PDF.
pub enum SidecarLoad {
    /// No sidecar exists yet.
    Missing,
    Loaded(AnnotationSet),
    /// A sidecar exists but couldn't be read or parsed (message says why). It must not be
    /// silently overwritten.
    Unreadable(String),
}

/// All annotations for one document, plus enough metadata to sanity-check a reload.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnnotationSet {
    /// File name (not full path) of the PDF these annotations were made against.
    pub pdf_file: String,
    pub page_count: usize,
    pub strokes: Vec<Stroke>,
    /// Text annotations. Defaulted on load so older sidecars (strokes only) still parse.
    #[serde(default)]
    pub texts: Vec<TextAnnotation>,
    /// Monotonic id source shared by strokes and texts; not serialized.
    #[serde(skip)]
    next_id: u64,
}

impl AnnotationSet {
    pub fn new(pdf_file: String, page_count: usize) -> Self {
        Self {
            pdf_file,
            page_count,
            strokes: Vec::new(),
            texts: Vec::new(),
            next_id: 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty() && self.texts.is_empty()
    }

    /// Assign a fresh runtime id (used after load and when creating strokes).
    fn assign_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Push a new stroke, giving it a fresh id. Returns the id.
    pub fn add(&mut self, mut stroke: Stroke) -> u64 {
        let id = self.assign_id();
        stroke.id = id;
        self.strokes.push(stroke);
        id
    }

    /// Push a new text annotation, giving it a fresh id. Returns the id.
    pub fn add_text(&mut self, mut text: TextAnnotation) -> u64 {
        let id = self.assign_id();
        text.id = id;
        self.texts.push(text);
        id
    }

    /// Borrow the text annotation with the given id, if present.
    pub fn text_by_id(&self, id: u64) -> Option<&TextAnnotation> {
        self.texts.iter().find(|t| t.id == id)
    }

    /// Overwrite the text annotation that shares `text`'s id (content/style/position edit).
    pub fn replace_text(&mut self, text: TextAnnotation) {
        if let Some(t) = self.texts.iter_mut().find(|t| t.id == text.id) {
            *t = text;
        }
    }

    /// Move the text annotation with the given id to a new normalised anchor.
    pub fn set_text_pos(&mut self, id: u64, pos: [f32; 2]) {
        if let Some(t) = self.texts.iter_mut().find(|t| t.id == id) {
            t.pos = pos;
        }
    }

    /// Re-insert a previously-removed annotation at its old index within its own list
    /// (keeps its id and its stacking order). Used by undo/redo.
    pub fn insert_at(&mut self, index: usize, ann: Annotation) {
        match ann {
            Annotation::Stroke(s) => self.strokes.insert(index.min(self.strokes.len()), s),
            Annotation::Text(t) => self.texts.insert(index.min(self.texts.len()), t),
        }
    }

    /// Remove the annotation (stroke or text) with the given id, returning its index within
    /// its own list and the annotation itself.
    pub fn remove(&mut self, id: u64) -> Option<(usize, Annotation)> {
        if let Some(i) = self.strokes.iter().position(|s| s.id == id) {
            return Some((i, Annotation::Stroke(self.strokes.remove(i))));
        }
        if let Some(i) = self.texts.iter().position(|t| t.id == id) {
            return Some((i, Annotation::Text(self.texts.remove(i))));
        }
        None
    }

    /// Sidecar path next to the PDF: `<pdf>.nullpdf.json`.
    pub fn sidecar_path(pdf_path: &Path) -> PathBuf {
        let mut p = pdf_path.as_os_str().to_owned();
        p.push(".nullpdf.json");
        PathBuf::from(p)
    }

    /// Load annotations from the sidecar beside `pdf_path`.
    pub fn load_for(pdf_path: &Path) -> SidecarLoad {
        let path = Self::sidecar_path(pdf_path);
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SidecarLoad::Missing,
            Err(e) => return SidecarLoad::Unreadable(e.to_string()),
        };
        let mut set: AnnotationSet = match serde_json::from_str(&data) {
            Ok(s) => s,
            Err(e) => return SidecarLoad::Unreadable(e.to_string()),
        };
        // Re-establish runtime ids deterministically after deserialization.
        set.next_id = 1;
        for i in 0..set.strokes.len() {
            let id = set.assign_id();
            set.strokes[i].id = id;
        }
        for i in 0..set.texts.len() {
            let id = set.assign_id();
            set.texts[i].id = id;
        }
        SidecarLoad::Loaded(set)
    }

    /// Write annotations to the sidecar beside `pdf_path`. The file is written to a
    /// temporary sibling first and renamed into place, so a crash or full disk mid-write
    /// can never leave a truncated sidecar behind.
    pub fn save_for(&self, pdf_path: &Path) -> std::io::Result<()> {
        let path = Self::sidecar_path(pdf_path);
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut tmp: OsString = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

/// Chaikin corner-cutting smoothing over a normalised-coordinate polyline.
///
/// Endpoints are preserved; each iteration replaces every segment with two interior
/// points (¼ and ¾), rounding off corners. Two iterations gives a visibly smooth
/// freehand line without drifting away from the drawn path. Short paths (a dot or a
/// single segment) are returned unchanged.
pub fn chaikin(points: &[[f32; 2]], iterations: usize) -> Vec<[f32; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut pts = points.to_vec();
    for _ in 0..iterations {
        let mut out = Vec::with_capacity(pts.len() * 2);
        out.push(pts[0]);
        for w in pts.windows(2) {
            let (p, q) = (w[0], w[1]);
            out.push([0.75 * p[0] + 0.25 * q[0], 0.75 * p[1] + 0.25 * q[1]]);
            out.push([0.25 * p[0] + 0.75 * q[0], 0.25 * p[1] + 0.75 * q[1]]);
        }
        out.push(*pts.last().unwrap());
        pts = out;
    }
    pts
}

/// Ramer–Douglas–Peucker simplification of a normalised-coordinate polyline.
///
/// `scale` converts normalised units into a common metric (page points per axis, since
/// pages are rarely square) and `tolerance` is the maximum deviation allowed in that metric.
/// With a sub-pixel tolerance this is visually lossless; it just drops the redundant points
/// that full-rate sampling + Chaikin produce on gentle curves. Endpoints are always kept.
pub fn simplify(points: &[[f32; 2]], scale: [f32; 2], tolerance: f32) -> Vec<[f32; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let p = |i: usize| [points[i][0] * scale[0], points[i][1] * scale[1]];
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (p(a), p(b));
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len = dx.hypot(dy);
        let mut worst = (0.0f32, a);
        for i in a + 1..b {
            let pi = p(i);
            let d = if len <= f32::EPSILON {
                (pi[0] - pa[0]).hypot(pi[1] - pa[1])
            } else {
                ((pi[0] - pa[0]) * dy - (pi[1] - pa[1]) * dx).abs() / len
            };
            if d > worst.0 {
                worst = (d, i);
            }
        }
        if worst.0 > tolerance {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(pt, k)| k.then_some(*pt))
        .collect()
}

/// One reversible edit. The variant describes what *was done* to the document.
#[derive(Clone, Debug)]
pub enum Edit {
    Added(Annotation),
    /// One erase gesture (which may sweep over several annotations), in removal order. Each
    /// entry keeps the index the annotation had in its list when it was removed, so undo
    /// restores the exact stacking order.
    Erased(Vec<(usize, Annotation)>),
    /// A text annotation changed in place (content, style, or position). Stored as the
    /// full before/after so undo/redo just restores one or the other by id.
    EditedText {
        before: TextAnnotation,
        after: TextAnnotation,
    },
}

/// Undo/redo command stacks operating against an [`AnnotationSet`].
#[derive(Default)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    /// Record that `ann` was just added. Clears the redo stack.
    pub fn record_add(&mut self, ann: Annotation) {
        self.undo.push(Edit::Added(ann));
        self.redo.clear();
    }

    /// Record one erase gesture. Clears the redo stack; an empty gesture records nothing.
    pub fn record_erase(&mut self, erased: Vec<(usize, Annotation)>) {
        if erased.is_empty() {
            return;
        }
        self.undo.push(Edit::Erased(erased));
        self.redo.clear();
    }

    /// Record an in-place text edit (content/style/position). Clears the redo stack.
    pub fn record_edit_text(&mut self, before: TextAnnotation, after: TextAnnotation) {
        self.undo.push(Edit::EditedText { before, after });
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Reverse the most recent edit against `set`, returning the annotations it touched
    /// (so the caller can bring them into view), or `None` if there was nothing to undo.
    pub fn undo(&mut self, set: &mut AnnotationSet) -> Option<Vec<Annotation>> {
        let edit = self.undo.pop()?;
        let touched = match &edit {
            Edit::Added(a) => {
                set.remove(a.id());
                vec![a.clone()]
            }
            Edit::Erased(items) => {
                // Each index was recorded after the earlier removals of the same sweep, so
                // replaying the removals backwards rebuilds each list exactly as it was.
                for (i, a) in items.iter().rev() {
                    set.insert_at(*i, a.clone());
                }
                items.iter().map(|(_, a)| a.clone()).collect()
            }
            Edit::EditedText { before, .. } => {
                set.replace_text(before.clone());
                vec![Annotation::Text(before.clone())]
            }
        };
        self.redo.push(edit);
        Some(touched)
    }

    /// Re-apply the most recently undone edit against `set`, returning the annotations it
    /// touched, or `None` if there was nothing to redo.
    pub fn redo(&mut self, set: &mut AnnotationSet) -> Option<Vec<Annotation>> {
        let edit = self.redo.pop()?;
        let touched = match &edit {
            Edit::Added(a) => {
                set.insert_at(usize::MAX, a.clone());
                vec![a.clone()]
            }
            Edit::Erased(items) => {
                for (_, a) in items {
                    set.remove(a.id());
                }
                items.iter().map(|(_, a)| a.clone()).collect()
            }
            Edit::EditedText { after, .. } => {
                set.replace_text(after.clone());
                vec![Annotation::Text(after.clone())]
            }
        };
        self.undo.push(edit);
        Some(touched)
    }
}
