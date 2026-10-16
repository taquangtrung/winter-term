//! Input methods (IME): text composed in an input method (Vietnamese Telex,
//! Pinyin, Kana, ...) is shown at the cursor while it is being composed and
//! typed into the focused tab once committed, and the input method's
//! candidate window is kept next to the cursor.

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::Ime;
use winit::event_loop::ActiveEventLoop;
use winit::window::{ImePurpose, Window};
use winter_render::{CellOverlay, Style, UnderlineStyle, PANE_H_PAD};

use crate::model::layout::TabId;
use crate::model::mode::Mode;

use super::App;

// ========================================================================
// Data Structures
// ========================================================================

/// What the app keeps of the input method's state.
#[derive(Clone, Debug, Default)]
pub(crate) struct ImeState {
    /// The candidate-window anchor last reported to the input method, so it
    /// is only told again when the cursor moves.
    anchor: Option<ImeAnchor>,
    /// Text being composed and not yet committed, if any.
    preedit: Option<String>,
}

/// Where the input method's candidate window anchors: the cursor's cell, in
/// physical pixels of the window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ImeAnchor {
    height: u32,
    width: u32,
    x: i32,
    y: i32,
}

// ========================================================================
// ImeState
// ========================================================================

impl ImeState {
    /// Note `preedit` as the text being composed; an empty one is none.
    fn set_preedit(&mut self, preedit: String) {
        self.preedit = (!preedit.is_empty()).then_some(preedit);
    }
}

// ========================================================================
// App: input methods
// ========================================================================

impl App {
    /// Handle one input-method event: track the text being composed, and
    /// type what is committed into the focused tab.
    pub(crate) fn on_ime(&mut self, ime: Ime, event_loop: &ActiveEventLoop) {
        match ime {
            Ime::Enabled => {}
            Ime::Preedit(text, _) => self.ime.set_preedit(text),
            Ime::Commit(text) => {
                self.ime.preedit = None;
                if self.type_committed_text(&text) {
                    self.quit(event_loop);
                    return;
                }
            }
            Ime::Disabled => self.ime.preedit = None,
        }
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The text being composed, drawn over the focused terminal at its
    /// cursor, underlined to mark it as not yet typed. `None` when nothing is
    /// being composed, when the focused tab is not a terminal taking typed
    /// text (Insert mode), or when its cursor is scrolled out of view.
    pub(crate) fn preedit_overlay(&self, focused: TabId) -> Option<CellOverlay> {
        let preedit = self.ime.preedit.as_deref()?;
        if self.open_tabs.mode(&focused) != Some(Mode::Insert) {
            return None;
        }
        let (row, col) = self.viewport_cursor(focused)?;
        let style = Style {
            underline: UnderlineStyle::Single,
            ..Style::default()
        };
        Some(CellOverlay::from_text(row, col, preedit, style))
    }

    /// Tell the input method where the focused terminal's cursor is, so its
    /// candidate window opens beside the text being composed. Only sent
    /// when the cursor has moved since it was last told.
    pub(crate) fn update_ime_anchor(&mut self, focused: TabId) {
        let Some(anchor) = self.ime_anchor(focused) else {
            return;
        };
        if self.ime.anchor == Some(anchor) {
            return;
        }
        if let Some(window) = &self.window {
            window.set_ime_cursor_area(
                PhysicalPosition::new(anchor.x, anchor.y),
                PhysicalSize::new(anchor.width, anchor.height),
            );
            self.ime.anchor = Some(anchor);
        }
    }

    /// The focused terminal's cursor as a viewport `(row, col)`, or `None`
    /// when it is scrolled out of view.
    fn viewport_cursor(&self, focused: TabId) -> Option<(usize, usize)> {
        let grid = self.open_tabs.terminal(&focused)?.grid();
        let (row, col) = grid.cursor();
        let row = row + grid.scroll_offset();
        (row < grid.rows()).then_some((row, col))
    }

    fn ime_anchor(&self, focused: TabId) -> Option<ImeAnchor> {
        let (row, col) = self.viewport_cursor(focused)?;
        let rect = self.pane_rect(focused)?;
        let (cell_w, cell_h) = self.renderer.as_ref()?.cell_size();
        Some(ImeAnchor {
            height: cell_h.round() as u32,
            width: cell_w.round() as u32,
            x: (rect.x + PANE_H_PAD + col as f32 * cell_w).round() as i32,
            y: (rect.y + row as f32 * cell_h).round() as i32,
        })
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Let `window` receive input-method events, as a terminal: the input
/// method then composes text for it rather than passing keys straight
/// through.
pub(crate) fn enable_ime(window: &Window) {
    window.set_ime_allowed(true);
    window.set_ime_purpose(ImePurpose::Terminal);
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_empty_preedit_means_nothing_is_being_composed() {
        // Input methods end a composition by sending an empty preedit; left
        // as `Some("")` it would keep the cursor hidden for nothing.
        let mut state = ImeState::default();
        state.set_preedit("vie".into());
        assert_eq!(state.preedit.as_deref(), Some("vie"));
        state.set_preedit(String::new());
        assert_eq!(state.preedit, None);
    }
}
