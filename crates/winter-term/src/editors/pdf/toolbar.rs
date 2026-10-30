//! The bar over the pages: the section being read, which opens the outline,
//! then the zoom, the page, and the color inversion, each a button.
//!
//! Drawn by the window in its own font over the pane's top row, which the
//! viewer leaves blank for it, as the status bar is drawn; a press on it
//! comes back as the button it landed on.

use winter_render::{ToolbarItem, ToolbarTone};

use crate::model::editor::Toolbar;

// ========================================================================
// Constants
// ========================================================================

/// The marks the buttons are drawn with: plain text, so any font has them.
const OUTLINE_MARK: &str = "≡";
const OPEN_MARK: &str = "▾";
const ZOOM_OUT_MARK: &str = "−";
const ZOOM_IN_MARK: &str = "+";
const PREVIOUS_MARK: &str = "‹";
const NEXT_MARK: &str = "›";
const INVERT_MARK: &str = "◐";

/// What the outline button says when the reading spot is ahead of every
/// section, and when the document lists none.
const NO_SECTION: &str = "Outline";
const NO_OUTLINE: &str = "No outline";

/// Between two groups of buttons.
const SEPARATOR: &str = "│";

// ========================================================================
// Data Structures
// ========================================================================

/// A button on the bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button {
    /// Invert the pages' colors, or put them back.
    Invert,
    /// The page after the one being read.
    NextPage,
    /// Open or close the outline.
    Outline,
    /// Ask which page to go to.
    Page,
    /// The page before the one being read.
    PreviousPage,
    /// Ask for a zoom.
    Zoom,
    /// Zoom in a step.
    ZoomIn,
    /// Zoom out a step.
    ZoomOut,
}

/// What the bar shows.
#[derive(Clone, Debug, PartialEq)]
pub struct State<'a> {
    /// Whether the document lists any sections.
    pub has_outline: bool,
    /// Whether the pages are drawn inverted.
    pub inverted: bool,
    /// Whether the outline is open under the bar.
    pub outline_open: bool,
    /// The page being read, from one, and how many there are: none while
    /// the document is still opening.
    pub page: Option<(usize, usize)>,
    /// The section the reading spot is in.
    pub section: Option<&'a str>,
    /// The zoom, in percent of a pixel a point.
    pub zoom: u32,
}

// ========================================================================
// Button
// ========================================================================

impl Button {
    /// Every button, in the order of the ids a press comes back as.
    const ALL: [Button; 8] = [
        Button::Invert,
        Button::NextPage,
        Button::Outline,
        Button::Page,
        Button::PreviousPage,
        Button::Zoom,
        Button::ZoomIn,
        Button::ZoomOut,
    ];

    /// What a press on the button comes back as.
    pub fn id(self) -> u16 {
        Self::ALL.iter().position(|button| *button == self).unwrap_or(0) as u16
    }

    /// The button a press came back as.
    pub fn from_id(id: u16) -> Option<Self> {
        Self::ALL.get(id as usize).copied()
    }

    /// Human-readable tooltip describing what the button does.
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Invert => "Invert Colors",
            Self::NextPage => "Next Page",
            Self::Outline => "Table of Contents",
            Self::Page => "Go to Page",
            Self::PreviousPage => "Previous Page",
            Self::Zoom => "Zoom Level",
            Self::ZoomIn => "Zoom In",
            Self::ZoomOut => "Zoom Out",
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The bar for `state`, `rows` of the pane tall: the section on the left,
/// cut short by the window to what room the buttons on the right leave.
pub fn bar(state: &State, rows: usize) -> Toolbar {
    Toolbar {
        left: vec![outline_item(state)],
        right: right_items(state),
        rows,
    }
}

/// The outline button: the section being read.
fn outline_item(state: &State) -> ToolbarItem {
    let title = match (state.has_outline, state.section) {
        (false, _) => NO_OUTLINE,
        (true, Some(section)) => section,
        (true, None) => NO_SECTION,
    };
    let tone = match (state.has_outline, state.outline_open) {
        (false, _) => ToolbarTone::Dim,
        (true, true) => ToolbarTone::Accent,
        (true, false) => ToolbarTone::Strong,
    };
    ToolbarItem {
        button: state.has_outline.then(|| Button::Outline.id()),
        shrinks: true,
        text: format!("{OUTLINE_MARK} {title} {OPEN_MARK}"),
        tone,
    }
}

/// The zoom, the page, and the inversion, each group set off by a line.
fn right_items(state: &State) -> Vec<ToolbarItem> {
    let button = |button: Button, tone: ToolbarTone, text: String| ToolbarItem {
        button: Some(button.id()),
        shrinks: false,
        text,
        tone,
    };
    let separator = || ToolbarItem {
        button: None,
        shrinks: false,
        text: SEPARATOR.to_string(),
        tone: ToolbarTone::Dim,
    };
    let mut items = vec![
        button(Button::ZoomOut, ToolbarTone::Strong, ZOOM_OUT_MARK.to_string()),
        button(Button::Zoom, ToolbarTone::Normal, format!("{}%", state.zoom)),
        button(Button::ZoomIn, ToolbarTone::Strong, ZOOM_IN_MARK.to_string()),
        separator(),
    ];
    if let Some((page, pages)) = state.page {
        items.extend([
            button(Button::PreviousPage, ToolbarTone::Strong, PREVIOUS_MARK.to_string()),
            button(Button::Page, ToolbarTone::Normal, format!("{page} / {pages}")),
            button(Button::NextPage, ToolbarTone::Strong, NEXT_MARK.to_string()),
            separator(),
        ]);
    }
    let invert_tone = match state.inverted {
        true => ToolbarTone::Accent,
        false => ToolbarTone::Strong,
    };
    items.push(button(Button::Invert, invert_tone, INVERT_MARK.to_string()));
    items
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State<'static> {
        State {
            has_outline: true,
            inverted: false,
            outline_open: false,
            page: Some((3, 12)),
            section: Some("2.1 Ethereum Virtual Machine"),
            zoom: 125,
        }
    }

    /// The button the item saying `text` is.
    fn button_of(bar: &Toolbar, text: &str) -> Option<Button> {
        bar.left
            .iter()
            .chain(&bar.right)
            .find(|item| item.text.contains(text))
            .and_then(|item| item.button)
            .and_then(Button::from_id)
    }

    #[test]
    fn test_each_button_comes_back_as_itself() {
        let bar = bar(&state(), 1);
        assert_eq!(button_of(&bar, "−"), Some(Button::ZoomOut));
        assert_eq!(button_of(&bar, "125%"), Some(Button::Zoom));
        assert_eq!(button_of(&bar, "+"), Some(Button::ZoomIn));
        assert_eq!(button_of(&bar, "‹"), Some(Button::PreviousPage));
        assert_eq!(button_of(&bar, "3 / 12"), Some(Button::Page));
        assert_eq!(button_of(&bar, "›"), Some(Button::NextPage));
        assert_eq!(button_of(&bar, "◐"), Some(Button::Invert));
        assert_eq!(button_of(&bar, "Ethereum"), Some(Button::Outline));
        assert_eq!(button_of(&bar, "│"), None, "a separator is no button");
        assert!(bar.left[0].shrinks, "the section gives way to the buttons");
        for button in Button::ALL {
            assert_eq!(Button::from_id(button.id()), Some(button));
        }
    }

    #[test]
    fn test_a_document_with_no_outline_has_no_outline_button() {
        let bar = bar(
            &State {
                has_outline: false,
                section: None,
                ..state()
            },
            1,
        );
        assert!(bar.left[0].text.contains(NO_OUTLINE));
        assert_eq!(bar.left[0].button, None);
    }
}
