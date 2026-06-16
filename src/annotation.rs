//! Annotation data model + sidecar persistence + undo/redo.
//!
//! Strokes are stored in **normalised page coordinates** (each component in `0.0..=1.0`,
//! origin at the page's top-left). This makes them independent of zoom and render
//! resolution, and lets an externally-edited base PDF reload with annotations still
//! aligned, as long as page order/count is unchanged.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A single freehand mark. `Freehand` keeps every sampled point; `Line` keeps exactly
/// two points (press + release) and is drawn as a straight segment at any angle.
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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

    /// Re-insert a previously-removed annotation (keeps its existing id). Used by undo/redo.
    pub fn reinsert(&mut self, ann: Annotation) {
        match ann {
            Annotation::Stroke(s) => self.strokes.push(s),
            Annotation::Text(t) => self.texts.push(t),
        }
    }

    /// Remove the annotation (stroke or text) with the given id, returning it if present.
    pub fn remove(&mut self, id: u64) -> Option<Annotation> {
        if let Some(pos) = self.strokes.iter().position(|s| s.id == id) {
            return Some(Annotation::Stroke(self.strokes.remove(pos)));
        }
        if let Some(pos) = self.texts.iter().position(|t| t.id == id) {
            return Some(Annotation::Text(self.texts.remove(pos)));
        }
        None
    }

    /// Sidecar path next to the PDF: `<pdf>.nullpdf.json`.
    pub fn sidecar_path(pdf_path: &Path) -> PathBuf {
        let mut p = pdf_path.as_os_str().to_owned();
        p.push(".nullpdf.json");
        PathBuf::from(p)
    }

    /// Load annotations from the sidecar beside `pdf_path`, if it exists and parses.
    pub fn load_for(pdf_path: &Path) -> Option<Self> {
        let path = Self::sidecar_path(pdf_path);
        let data = std::fs::read_to_string(&path).ok()?;
        let mut set: AnnotationSet = serde_json::from_str(&data).ok()?;
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
        Some(set)
    }

    /// Write annotations to the sidecar beside `pdf_path`.
    pub fn save_for(&self, pdf_path: &Path) -> std::io::Result<()> {
        let path = Self::sidecar_path(pdf_path);
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, data)
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

/// One reversible edit. The variant describes what *was done* to the document.
#[derive(Clone, Debug)]
pub enum Edit {
    Added(Annotation),
    Erased(Annotation),
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

    /// Record that `ann` was just erased. Clears the redo stack.
    pub fn record_erase(&mut self, ann: Annotation) {
        self.undo.push(Edit::Erased(ann));
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

    /// Reverse the most recent edit against `set`.
    pub fn undo(&mut self, set: &mut AnnotationSet) {
        let Some(edit) = self.undo.pop() else { return };
        match &edit {
            Edit::Added(a) => {
                set.remove(a.id());
            }
            Edit::Erased(a) => {
                set.reinsert(a.clone());
            }
            Edit::EditedText { before, .. } => {
                set.replace_text(before.clone());
            }
        }
        self.redo.push(edit);
    }

    /// Re-apply the most recently undone edit against `set`.
    pub fn redo(&mut self, set: &mut AnnotationSet) {
        let Some(edit) = self.redo.pop() else { return };
        match &edit {
            Edit::Added(a) => {
                set.reinsert(a.clone());
            }
            Edit::Erased(a) => {
                set.remove(a.id());
            }
            Edit::EditedText { after, .. } => {
                set.replace_text(after.clone());
            }
        }
        self.undo.push(edit);
    }
}
