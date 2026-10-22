//! Tool/style state for the interaction model.

use egui::{Color32, Pos2, Vec2};

use crate::annotation::{Annotation, TextAnnotation};

/// The active tool. Pen and Highlighter are the same freehand brush with different
/// colours/widths (a highlight is just translucent ink); Eraser removes whole marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Navigation: left-drag pans, wheel scrolls, ctrl+wheel zooms.
    Nav,
    /// Freehand ink; Shift = straight line.
    Pen,
    /// Freehand translucent ink; Shift = straight line.
    Highlighter,
    /// Drag over marks (strokes or text) to remove them.
    Eraser,
    /// Click empty space to add text, click existing text to edit, drag it to move.
    Text,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Nav => "Navigate",
            Tool::Pen => "Pen",
            Tool::Highlighter => "Highlighter",
            Tool::Eraser => "Eraser",
            Tool::Text => "Text",
        }
    }

    /// Tools that lay down freehand ink.
    pub fn draws(self) -> bool {
        matches!(self, Tool::Pen | Tool::Highlighter)
    }
}

/// The palette behind the `1`–`7` keys, ordered by hue after black.
pub const PALETTE: [(&str, Color32); 7] = [
    ("Black", Color32::from_rgb(30, 30, 30)),
    ("Red", Color32::from_rgb(220, 40, 40)),
    ("Orange", Color32::from_rgb(240, 120, 0)),
    ("Yellow", Color32::from_rgb(255, 205, 0)),
    ("Green", Color32::from_rgb(30, 160, 70)),
    ("Blue", Color32::from_rgb(30, 100, 220)),
    ("Purple", Color32::from_rgb(140, 60, 200)),
];

/// Alpha given to palette colours picked for the highlighter (≈ 38% opacity).
pub const HIGHLIGHT_ALPHA: u8 = 96;

/// Size ladders stepped through by `[` / `]`. The size control can still set any value
/// in between; stepping always lands on the next rung.
pub const STROKE_SIZES: [f32; 18] = [
    0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 20.0, 24.0, 32.0, 40.0,
    48.0,
];
pub const FONT_SIZES: [f32; 19] = [
    6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0, 48.0,
    64.0, 72.0, 96.0,
];
/// Eraser radius in screen pixels (the eraser works at screen scale, like a cursor).
pub const ERASER_SIZES: [f32; 8] = [4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0];

/// The next rung above (`dir > 0`) or below `value` on `ladder`, clamped at the ends.
pub fn step_size(ladder: &[f32], value: f32, dir: i32) -> f32 {
    let eps = value * 1e-3;
    if dir > 0 {
        ladder
            .iter()
            .copied()
            .find(|&v| v > value + eps)
            .unwrap_or(*ladder.last().unwrap())
    } else {
        ladder
            .iter()
            .rev()
            .copied()
            .find(|&v| v < value - eps)
            .unwrap_or(ladder[0])
    }
}

/// Colour + size of one brush.
#[derive(Clone, Copy, Debug)]
pub struct Brush {
    pub color: Color32,
    pub size: f32,
}

/// What the colour/size controls currently act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleTarget {
    Pen,
    Highlighter,
    Text,
}

/// An open text editor (creating a new annotation, or re-editing an existing one).
#[derive(Clone, Debug)]
pub struct TextEditing {
    pub page: usize,
    /// Anchor (normalised page coords) the editor is pinned to.
    pub pos: [f32; 2],
    pub buffer: String,
    /// Style of the text being edited; toolbar colour/size changes apply here.
    pub color: Color32,
    pub size_pt: f32,
    /// `Some(id)` when editing an existing annotation; `None` when creating a new one.
    pub editing_id: Option<u64>,
    /// Set on the first frame so the editor can grab keyboard focus once.
    pub just_opened: bool,
    /// Character index to put the caret at when the editor opens (`None` = end).
    pub caret: Option<usize>,
}

/// A stroke being drawn this gesture. Points are normalised page coords, locked to the
/// page the gesture started on.
#[derive(Clone, Debug)]
pub struct ActiveStroke {
    pub page: usize,
    pub points: Vec<[f32; 2]>,
    /// Screen position of the last kept sample, for distance-based filtering.
    pub last_screen: Pos2,
}

/// The pointer gesture in progress, from button press to release.
#[derive(Clone, Debug, Default)]
pub enum Gesture {
    #[default]
    None,
    /// Grab-and-drag the view (left-drag in Nav, middle-drag, Space+drag, or dragging empty
    /// space with other tools).
    Pan {
        last: Pos2,
    },
    /// Right-drag rotation. `raw` is the unsnapped accumulated angle; `start` the rotation
    /// before the drag (restored if cancelled).
    Rotate {
        last_angle: f32,
        raw: f32,
        start: f32,
    },
    Draw(ActiveStroke),
    /// Erase sweep; everything removed so far forms one undo step.
    Erase {
        last: Pos2,
        erased: Vec<(usize, Annotation)>,
    },
    /// Text tool press that hasn't yet become a click or a drag.
    TextPress {
        origin: Pos2,
        hit: Option<u64>,
    },
    /// Dragging an existing text annotation.
    TextDrag {
        id: u64,
        /// The annotation as it was before the drag, for a single undo entry on release.
        before: TextAnnotation,
        /// Offset (doc points) from the anchor to the grab point, kept fixed during the drag.
        grab: Vec2,
    },
    /// A press consumed by something else (e.g. it committed an open text editor); ignored
    /// until release.
    Swallowed,
}

impl Gesture {
    pub fn is_none(&self) -> bool {
        matches!(self, Gesture::None)
    }
}
