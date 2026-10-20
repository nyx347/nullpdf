//! Keyboard bindings. This table is the single source of truth for both input handling
//! and the `?` cheat sheet, so the two can never disagree.
//!
//! Almost everything is a single key. `Ctrl` variants exist for the conventional
//! shortcuts and so that style changes still work while a text editor has focus (where
//! bare letters are typing).

use egui::{Key, Modifiers};

use crate::tools::Tool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Tool(Tool),
    /// Eraser on, or back to the previous drawing tool.
    ToggleEraser,
    Color(usize),
    SizeDown,
    SizeUp,
    Undo,
    Redo,
    ZoomIn,
    ZoomOut,
    /// Upright + fit width.
    ResetView,
    FitPage,
    RotateCw,
    RotateCcw,
    ScrollUp,
    ScrollDown,
    PrevPage,
    NextPage,
    FirstPage,
    LastPage,
    ScreenUp,
    ScreenDown,
    Open,
    Save,
    Export,
    Help,
    ToggleChrome,
    Cancel,
}

impl Action {
    /// Whether holding the key down should keep repeating the action.
    pub fn repeats(self) -> bool {
        matches!(
            self,
            Action::SizeDown
                | Action::SizeUp
                | Action::Undo
                | Action::Redo
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::ScrollUp
                | Action::ScrollDown
                | Action::PrevPage
                | Action::NextPage
                | Action::ScreenUp
                | Action::ScreenDown
        )
    }

    /// Actions that only make sense with a document open.
    pub fn needs_document(self) -> bool {
        !matches!(
            self,
            Action::Open | Action::Help | Action::ToggleChrome | Action::Cancel
        )
    }
}

/// Modifier requirement of a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mods {
    None,
    Shift,
    Ctrl,
    CtrlShift,
    /// Shift may or may not be held (symbols such as `?` and `+` need it on many layouts).
    AnyShift,
}

impl Mods {
    fn matches(self, m: Modifiers) -> bool {
        if m.alt {
            return false;
        }
        match self {
            Mods::None => !m.command && !m.shift,
            Mods::Shift => !m.command && m.shift,
            Mods::Ctrl => m.command && !m.shift,
            Mods::CtrlShift => m.command && m.shift,
            Mods::AnyShift => !m.command,
        }
    }

    fn uses_ctrl(self) -> bool {
        matches!(self, Mods::Ctrl | Mods::CtrlShift)
    }
}

pub struct Binding {
    pub key: Key,
    pub mods: Mods,
    pub action: Action,
}

const fn b(key: Key, mods: Mods, action: Action) -> Binding {
    Binding { key, mods, action }
}

use Action as A;
use Mods as M;

/// Every binding, in cheat-sheet order (the first binding of an action is its primary key).
pub const BINDINGS: &[Binding] = &[
    // Tools
    b(Key::Q, M::None, A::Tool(Tool::Nav)),
    b(Key::D, M::None, A::Tool(Tool::Pen)),
    b(Key::H, M::None, A::Tool(Tool::Highlighter)),
    b(Key::E, M::None, A::ToggleEraser),
    b(Key::T, M::None, A::Tool(Tool::Text)),
    // Style
    b(Key::Num1, M::None, A::Color(0)),
    b(Key::Num2, M::None, A::Color(1)),
    b(Key::Num3, M::None, A::Color(2)),
    b(Key::Num4, M::None, A::Color(3)),
    b(Key::Num5, M::None, A::Color(4)),
    b(Key::Num6, M::None, A::Color(5)),
    b(Key::Num7, M::None, A::Color(6)),
    b(Key::Num1, M::Ctrl, A::Color(0)),
    b(Key::Num2, M::Ctrl, A::Color(1)),
    b(Key::Num3, M::Ctrl, A::Color(2)),
    b(Key::Num4, M::Ctrl, A::Color(3)),
    b(Key::Num5, M::Ctrl, A::Color(4)),
    b(Key::Num6, M::Ctrl, A::Color(5)),
    b(Key::Num7, M::Ctrl, A::Color(6)),
    b(Key::OpenBracket, M::None, A::SizeDown),
    b(Key::CloseBracket, M::None, A::SizeUp),
    b(Key::OpenBracket, M::Ctrl, A::SizeDown),
    b(Key::CloseBracket, M::Ctrl, A::SizeUp),
    // Edit
    b(Key::Z, M::None, A::Undo),
    b(Key::Z, M::Shift, A::Redo),
    b(Key::Z, M::Ctrl, A::Undo),
    b(Key::Z, M::CtrlShift, A::Redo),
    b(Key::Y, M::Ctrl, A::Redo),
    // View
    b(Key::Plus, M::AnyShift, A::ZoomIn),
    b(Key::Equals, M::None, A::ZoomIn),
    b(Key::Minus, M::None, A::ZoomOut),
    b(Key::Plus, M::Ctrl, A::ZoomIn),
    b(Key::Equals, M::Ctrl, A::ZoomIn),
    b(Key::Minus, M::Ctrl, A::ZoomOut),
    b(Key::Num0, M::None, A::ResetView),
    b(Key::Num0, M::Ctrl, A::ResetView),
    b(Key::F, M::None, A::FitPage),
    b(Key::R, M::None, A::RotateCw),
    b(Key::R, M::Shift, A::RotateCcw),
    // Navigate
    b(Key::ArrowDown, M::None, A::ScrollDown),
    b(Key::ArrowUp, M::None, A::ScrollUp),
    b(Key::ArrowRight, M::None, A::NextPage),
    b(Key::ArrowLeft, M::None, A::PrevPage),
    b(Key::PageDown, M::None, A::NextPage),
    b(Key::PageUp, M::None, A::PrevPage),
    b(Key::Space, M::None, A::ScreenDown),
    b(Key::Space, M::Shift, A::ScreenUp),
    b(Key::Home, M::None, A::FirstPage),
    b(Key::End, M::None, A::LastPage),
    // File
    b(Key::O, M::None, A::Open),
    b(Key::S, M::None, A::Save),
    b(Key::X, M::None, A::Export),
    b(Key::O, M::Ctrl, A::Open),
    b(Key::S, M::Ctrl, A::Save),
    b(Key::E, M::Ctrl, A::Export),
    // Interface
    b(Key::Questionmark, M::AnyShift, A::Help),
    b(Key::F1, M::None, A::Help),
    b(Key::Tab, M::None, A::ToggleChrome),
    b(Key::Escape, M::None, A::Cancel),
];

/// The action bound to a key press, if any.
///
/// While `typing` (a text editor has focus) bare keys are text, so only `Ctrl` bindings
/// apply — minus undo/redo, which belong to the editor's own text history.
/// Space is excluded: it is a hold-to-pan modifier whose tap action fires on release.
pub fn lookup(key: Key, mods: Modifiers, typing: bool) -> Option<Action> {
    if key == Key::Space {
        return None;
    }
    let b = BINDINGS
        .iter()
        .find(|b| b.key == key && b.mods.matches(mods))?;
    if typing && (!b.mods.uses_ctrl() || matches!(b.action, Action::Undo | Action::Redo)) {
        return None;
    }
    Some(b.action)
}

/// One row of the cheat sheet: the actions whose primary keys are shown, and a label.
pub struct HelpRow {
    pub actions: &'static [Action],
    pub label: &'static str,
}

const fn row(actions: &'static [Action], label: &'static str) -> HelpRow {
    HelpRow { actions, label }
}

/// The cheat sheet, as `(section title, rows)`. Keys are looked up in [`BINDINGS`].
pub const HELP: &[(&str, &[HelpRow])] = &[
    (
        "Tools",
        &[
            row(&[A::Tool(Tool::Nav)], "Navigate"),
            row(&[A::Tool(Tool::Pen)], "Pen"),
            row(&[A::Tool(Tool::Highlighter)], "Highlighter"),
            row(&[A::ToggleEraser], "Eraser (again: back)"),
            row(&[A::Tool(Tool::Text)], "Text"),
        ],
    ),
    (
        "Style",
        &[
            row(&[A::Color(0), A::Color(6)], "Colour"),
            row(&[A::SizeDown, A::SizeUp], "Size"),
        ],
    ),
    ("Edit", &[row(&[A::Undo], "Undo"), row(&[A::Redo], "Redo")]),
    (
        "View",
        &[
            row(&[A::ZoomIn, A::ZoomOut], "Zoom"),
            row(&[A::ResetView], "Reset (upright, fit width)"),
            row(&[A::FitPage], "Fit page"),
            row(&[A::RotateCw, A::RotateCcw], "Rotate 90°"),
        ],
    ),
    (
        "Move",
        &[
            row(&[A::ScrollUp, A::ScrollDown], "Scroll"),
            row(&[A::PrevPage, A::NextPage], "Page"),
            row(&[A::ScreenDown], "Screen down (Shift: up)"),
            row(&[A::FirstPage, A::LastPage], "First / last page"),
        ],
    ),
    (
        "File",
        &[
            row(&[A::Open], "Open"),
            row(&[A::Save], "Save annotations"),
            row(&[A::Export], "Export flattened copy"),
        ],
    ),
    (
        "Interface",
        &[
            row(&[A::Help], "This sheet"),
            row(&[A::ToggleChrome], "Hide / show toolbar"),
            row(&[A::Cancel], "Cancel · back to Navigate"),
        ],
    ),
];

/// Pointer gestures and text-editor keys, which have no binding to look up.
pub const MOUSE_HELP: &[(&str, &str)] = &[
    ("Right-drag", "Rotate (Shift: 15° steps)"),
    ("Middle-drag · Space+drag", "Pan with any tool"),
    ("Wheel · Shift+wheel", "Scroll"),
    ("Ctrl+wheel · pinch", "Zoom at pointer"),
    ("Back · Forward buttons", "Previous / next page"),
    ("Shift while drawing", "Straight line"),
    ("Enter · Shift+Enter", "Finish text · new line"),
];

/// Keycap labels for the primary binding of `action`: e.g. `["Shift", "Z"]`.
pub fn primary_caps(action: Action) -> Vec<&'static str> {
    let Some(b) = BINDINGS.iter().find(|b| b.action == action) else {
        return Vec::new();
    };
    let mut caps = Vec::new();
    match b.mods {
        Mods::Shift => caps.push("Shift"),
        Mods::Ctrl => caps.push("Ctrl"),
        Mods::CtrlShift => caps.extend(["Ctrl", "Shift"]),
        Mods::None | Mods::AnyShift => {}
    }
    caps.push(key_name(b.key));
    caps
}

/// Short display name for a key. Arrow keys use arrow glyphs, which the keycap renderer
/// paints itself (the UI font has no arrows).
pub fn key_name(key: Key) -> &'static str {
    match key {
        Key::ArrowDown => "↓",
        Key::ArrowUp => "↑",
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Plus => "+",
        Key::Equals => "=",
        Key::Minus => "−",
        Key::Questionmark => "?",
        Key::PageDown => "PgDn",
        Key::PageUp => "PgUp",
        Key::Escape => "Esc",
        other => other.name(),
    }
}
