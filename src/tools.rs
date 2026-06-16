//! Tool/mode state for the interaction model.

use egui::Vec2;

use crate::annotation::TextAnnotation;

/// Top-level interaction mode. `D` enters [`Mode::Draw`], `T` [`Mode::Text`], `Q` returns
/// to [`Mode::Nav`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Navigation: left-drag pans, right-drag rotates, wheel scrolls, ctrl+wheel zooms.
    Nav,
    /// Drawing: left-drag draws (or erases); shift = straight line; right-drag still rotates.
    Draw,
    /// Text: click empty space to add text, click existing text to edit, drag it to move.
    Text,
}

/// An open text editor (creating a new annotation, or re-editing an existing one).
#[derive(Clone, Debug)]
pub struct TextEditing {
    pub page: usize,
    /// Anchor (normalised page coords) the editor is pinned to.
    pub pos: [f32; 2],
    pub buffer: String,
    /// `Some(id)` when editing an existing annotation; `None` when creating a new one.
    pub editing_id: Option<u64>,
    /// Set on the first frame so the editor can grab keyboard focus once.
    pub just_opened: bool,
}

/// An in-progress drag of an existing text annotation.
#[derive(Clone, Debug)]
pub struct TextDrag {
    pub id: u64,
    /// The annotation as it was before the drag, for a single undo entry on release.
    pub before: TextAnnotation,
    /// Offset (doc points) from the anchor to the grab point, kept fixed during the drag.
    pub grab: Vec2,
}

/// A freehand stroke being drawn this gesture. Points are normalised page coords,
/// locked to the page the gesture started on.
#[derive(Clone, Debug)]
pub struct ActiveStroke {
    pub page: usize,
    pub points: Vec<[f32; 2]>,
}

/// Mutable tool state carried across frames.
pub struct Tools {
    pub mode: Mode,
    pub eraser: bool,
    pub active: Option<ActiveStroke>,
    /// Pointer angle (radians) at the previous rotate-drag frame, if rotating.
    pub last_rotate_angle: Option<f32>,
    /// The open text editor, if any (Text mode).
    pub text_edit: Option<TextEditing>,
    /// The text annotation being dragged, if any (Text mode).
    pub text_drag: Option<TextDrag>,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            mode: Mode::Nav,
            eraser: false,
            active: None,
            last_rotate_angle: None,
            text_edit: None,
            text_drag: None,
        }
    }
}
