//! Keyboard action dispatch: translates resolved [`Action`]s into state changes.

use crate::model::input::{self, Action, CursorMove, InsertAt, VisualKind};
use crate::model::layout::{Rect, TabId};
use crate::model::mode::{Mode, ModeEvent};
use crate::model::picker::{Picker, PickerMode};
use crate::model::textedit::modal::shared;

use crate::window::commands::builtin_commands;
use crate::window::terminal::navigation::search::SearchFrom;
use crate::window::terminal::prompt_edit::PromptDelete;
use crate::window::{App, FONT_SIZE_STEP, SEARCH_DEBOUNCE_MILLIS};

// ========================================================================
// App: action handling
// ========================================================================

impl App {
    pub(in crate::window) fn handle_action(&mut self, action: Action, focused: TabId) {
        // `.`-repeat and `g;`/`g,` bookkeeping: records the change this
        // dispatch makes (or the session it opens/closes) before it runs.
        self.track_change(&action, focused);
        match action {
            Action::SendBytes(bytes) => {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    // Typing means looking at the prompt: a view scrolled back
                    // through history comes down to the live screen, or the
                    // keys would land where nothing shows them.
                    if terminal.grid().scroll_offset() != 0 {
                        terminal.grid_mut().set_scroll_offset(0);
                        self.dirty = true;
                    }
                    terminal.write(&bytes);
                }
            }
            Action::SwitchMode(new_mode) => self.switch_mode(new_mode, focused),
            Action::EnterInsert(at) => {
                // `a`/`o` place the cursor before the switch, so the alignment
                // below (which walks the shell cursor to the nav column) carries
                // it to the right spot in one go.
                match at {
                    InsertAt::Cursor => {}
                    InsertAt::After => self.move_nav_cursor(CursorMove::Right, focused),
                    InsertAt::LineEnd => self.move_nav_cursor(CursorMove::LineEnd, focused),
                }
                let mode = self
                    .open_tabs
                    .mode(&focused)
                    .unwrap_or_default()
                    .apply(ModeEvent::ToInsert);
                self.handle_action(Action::SwitchMode(mode), focused);
            }
            Action::MoveCursor(mv) => {
                self.move_nav_cursor(mv, focused);
                if self.open_tabs.mode(&focused) == Some(Mode::Visual) {
                    self.update_visual_selection(focused);
                }
            }
            Action::MoveCursorN { count, mv } => {
                for _ in 0..count.max(1) {
                    self.move_nav_cursor(mv, focused);
                }
                if self.open_tabs.mode(&focused) == Some(Mode::Visual) {
                    self.update_visual_selection(focused);
                }
            }
            Action::JumpOlder => {
                self.jump_older(focused);
            }
            Action::JumpNewer => {
                self.jump_newer(focused);
            }
            Action::ChangeOlder => {
                self.change_older(focused);
            }
            Action::ChangeNewer => {
                self.change_newer(focused);
            }
            Action::JumpToPrompt => {
                self.jump_to_prompt(focused);
            }
            Action::JumpToPreviousPrompt => {
                self.jump_to_previous_prompt(focused);
            }
            Action::SelectSearchMatch { forward } => {
                self.select_search_match(focused, forward);
            }
            Action::ChangeSearchMatch { forward } => {
                self.change_search_match(focused, forward);
            }
            Action::DeleteSearchMatch { forward } => {
                self.delete_search_match(focused, forward);
            }
            Action::RepeatLastChange => {
                self.repeat_last_change(focused);
            }
            Action::OpenUnderCursor => {
                self.open_under_cursor(focused);
            }
            Action::SetMark(mark) => {
                self.set_mark(focused, mark);
            }
            Action::GotoMark(goto) => {
                self.goto_mark(focused, goto.mark, goto.exact);
            }
            Action::SwapVisualEnds => {
                self.swap_visual_ends(focused);
            }
            Action::RestoreVisual => {
                self.restore_visual(focused);
            }
            Action::EnterVisual(kind) => {
                self.toggle_visual(kind, focused);
            }
            Action::SelectParagraph => {
                self.select_paragraph(focused);
            }
            Action::FindChar(find) => {
                self.chrome.labels.last_find = Some(find);
                // Several candidates put up the jump overlay and hold the keymap
                // for the label key; one or none just moves (or doesn't).
                if self.find_char_overlay(find, focused) {
                    self.input.keys.prefix = input::PendingPrefix::FindLabel;
                }
            }
            Action::FindJump(label) => {
                self.find_jump(focused, label);
            }
            Action::FindCancel => {
                self.clear_find_labels();
            }
            Action::FindRepeat { reverse } => {
                if let Some(find) = self.chrome.labels.last_find {
                    let target = if reverse { find.reversed() } else { find };
                    self.find_char_move(target, focused);
                }
            }
            Action::YankLine { register } => {
                self.yank_line(focused, register);
            }
            Action::YankMotion { motion, register } => {
                self.yank_motion(focused, motion, register);
            }
            Action::YankTextObject { spec, register } => {
                self.yank_text_object(focused, spec, register);
            }
            Action::YankSelection => {
                self.copy_selection();
                self.finish_visual_yank(focused);
            }
            Action::YankSelectionLines => {
                self.selection.visual_kind = VisualKind::Line;
                self.update_visual_selection(focused);
                self.copy_selection();
                self.finish_visual_yank(focused);
            }
            Action::YankSelectionRegister(reg) => {
                if let Some(text) = self.selected_text() {
                    if reg == '+' || reg == '*' || reg == '"' {
                        self.copy_selection();
                    } else {
                        shared::set_register_text(reg, text);
                        self.set_notice(format!("Yanked into register \"{reg}"));
                    }
                }
                self.finish_visual_yank(focused);
            }
            Action::Paste => {
                self.paste_from_clipboard();
            }
            Action::PasteRegister { register, after: _ } => {
                let text = if register == '+' || register == '*' {
                    self.clipboard_text()
                } else {
                    shared::register_text(register)
                };
                if let Some(text) = text {
                    self.paste_text(&text);
                }
            }
            Action::Copy => {
                self.copy_selection();
            }
            Action::ChangeSurround {
                target,
                replacement,
            } => {
                self.change_surround(focused, target, replacement);
            }
            Action::DeleteSurround(target) => {
                self.delete_surround(focused, target);
            }
            Action::SurroundTextObject { spec, delimiter } => {
                self.surround_text_object(focused, spec.around, spec.object, delimiter);
            }
            Action::PromptUndo => self.prompt_history_apply(focused, true),
            Action::PromptRedo => self.prompt_history_apply(focused, false),
            Action::Edit(edit) => self.edit_on_prompt(edit, focused),
            Action::ChangeLine => self.change_on_prompt(PromptDelete::Line, focused),
            Action::ChangeToLineEnd => self.change_on_prompt(PromptDelete::ToLineEnd, focused),
            Action::ChangeToLineStart => self.change_on_prompt(PromptDelete::ToLineStart, focused),
            Action::ChangeWordBack => self.change_on_prompt(PromptDelete::WordBack, focused),
            Action::ChangeWordForward => self.change_on_prompt(PromptDelete::WordForward, focused),
            Action::ChangeTextObject(spec) => {
                self.change_text_object(focused, spec.around, spec.object);
            }
            Action::SubstituteChar => self.change_on_prompt(PromptDelete::CharForward, focused),
            Action::ReplaceChar(ch) => self.replace_char_on_prompt(ch, focused),
            Action::ToggleCaseChar => self.toggle_case_on_prompt(focused),
            Action::DeleteCharForward => self.delete_on_prompt(PromptDelete::CharForward, focused),
            Action::DeleteLine => self.delete_on_prompt(PromptDelete::Line, focused),
            Action::DeleteSelection => self.delete_selection(focused),
            Action::DeleteTextObject(spec) => {
                self.delete_text_object(focused, spec.around, spec.object);
            }
            Action::SelectTextObject(spec) => {
                self.select_text_object(focused, spec.around, spec.object);
            }
            Action::DeleteToLineEnd => self.delete_on_prompt(PromptDelete::ToLineEnd, focused),
            Action::DeleteToLineStart => self.delete_on_prompt(PromptDelete::ToLineStart, focused),
            Action::DeleteWordBack => self.delete_on_prompt(PromptDelete::WordBack, focused),
            Action::DeleteWordForward => self.delete_on_prompt(PromptDelete::WordForward, focused),
            Action::SplitPane(direction) => {
                self.split_pane(direction);
            }
            Action::ClosePane => {
                self.close_pane(focused);
            }
            Action::CloseOtherPanes => {
                self.close_other_panes(focused);
            }
            Action::ZoomPane => self.toggle_pane_zoom(),
            Action::MoveTabLeft => {
                self.move_tab(false);
            }
            Action::MoveTabRight => {
                self.move_tab(true);
            }
            Action::MoveTabToPane(dir) => {
                self.move_tab_in_direction(dir);
            }
            Action::NewTab => {
                self.new_tab();
            }
            Action::NewWindow => {
                self.new_window();
            }
            Action::NextTab => {
                self.cycle_tab(true);
            }
            Action::PrevTab => {
                self.cycle_tab(false);
            }
            Action::GotoTab(n) => {
                self.switch_tab(n.saturating_sub(1));
            }
            Action::CloseOtherTabs => {
                self.close_other_tabs(focused);
            }
            Action::CloseTab(which) => {
                let tab = match which {
                    Some(n) => self
                        .layout()
                        .tabs_in_pane(focused)
                        .get(n.saturating_sub(1))
                        .copied(),
                    None => Some(focused),
                };
                if let Some(tab) = tab {
                    self.close_tab(tab);
                }
            }
            Action::FocusPane(dir) => {
                let viewport = self.viewport_rect();
                let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
                self.layout_mut().focus_in_direction(dir, layout_vp);
            }
            Action::FocusPaneByIndex(n) => {
                self.layout_mut().focus_by_index(n.saturating_sub(1));
            }
            Action::ClosePaneByIndex(n) => {
                if let Some(&id) = self.layout().shown_tabs().get(n.saturating_sub(1)) {
                    self.close_pane(id);
                }
            }
            Action::FocusBlock(nav) => {
                self.focus_block(nav, focused);
            }
            Action::SearchStart => self.start_search(false, focused),
            Action::SearchStartBackward => self.start_search(true, focused),
            Action::SearchEdit(key) => {
                // A key that only moved the caret has nothing to search again.
                self.dirty = true;
                if !self.search.edit_query(&key) {
                    return;
                }
                self.update_search_preview(focused);
                self.search.debounce = Some((
                    std::time::Instant::now()
                        + std::time::Duration::from_millis(SEARCH_DEBOUNCE_MILLIS),
                    focused,
                ));
                self.dirty = true;
            }
            Action::SearchExecute => {
                self.search.debounce = None;
                // Enter accepts the match the incremental search already landed
                // on, so it commits to the jumplist and reveals the match from origin.
                self.push_jump(focused);
                self.search_step(focused, self.search_start_direction(), SearchFrom::Origin);
                self.search.origin = None;
                self.dirty = true;
            }
            // Also the plain `Esc` in Normal mode (vim's `:nohlsearch`), which no
            // longer leaves Normal, so this does nothing at all when there is no
            // search to clear rather than forcing a status-bar resize.
            Action::SearchCancel => {
                self.search.debounce = None;
                if let Some((origin_tab, origin_pos)) = self.search.origin {
                    if origin_tab == focused {
                        self.reveal_position(focused, origin_pos);
                    }
                }
                self.dismiss_search();
            }
            Action::SearchNext => {
                self.resume_last_search();
                self.search_in_pane(focused, self.search_start_direction());
            }
            Action::SearchPrevious => {
                self.resume_last_search();
                self.search_in_pane(focused, self.search_start_direction().reversed());
            }
            Action::SearchWord { forward } => {
                self.search_word_under_cursor(focused, forward);
            }
            Action::YankBlock => {
                self.yank_block_source(focused);
            }
            Action::ToggleFold => {
                self.toggle_fold(focused);
            }
            Action::QuickSelect => {
                self.enter_quick_select(focused);
            }
            Action::QuickJump(c) => {
                self.quick_jump(focused, c);
            }
            Action::QuickCancel => {
                self.chrome.labels.quick = None;
            }
            Action::ScrollPageUp => {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    let rows = terminal.grid().rows();
                    terminal.grid_mut().scroll_up_history(rows);
                    self.scroll_app_view(focused, rows as isize);
                }
                self.dirty = true;
            }
            Action::ScrollPageDown => {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    let rows = terminal.grid().rows();
                    terminal.grid_mut().scroll_down_history(rows);
                    self.scroll_app_view(focused, -(rows as isize));
                }
                self.dirty = true;
            }
            Action::ScrollLineUp => {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    terminal.grid_mut().scroll_up_history(1);
                    self.scroll_app_view(focused, 1);
                }
                self.dirty = true;
            }
            Action::ScrollLineDown => {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    terminal.grid_mut().scroll_down_history(1);
                    self.scroll_app_view(focused, -1);
                }
                self.dirty = true;
            }
            Action::ScrollToTop => {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    let limit = terminal.grid().scrollback_len();
                    terminal.grid_mut().set_scroll_offset(limit);
                }
                self.dirty = true;
            }
            Action::ScrollToBottom => {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    terminal.grid_mut().set_scroll_offset(0);
                }
                self.dirty = true;
            }
            Action::OpenSettings => {
                self.open_settings();
            }
            Action::IncreaseFontSize => {
                self.change_font_size(FONT_SIZE_STEP);
            }
            Action::DecreaseFontSize => {
                self.change_font_size(-FONT_SIZE_STEP);
            }
            Action::ResetFontSize => self.reset_font_size(),
            Action::TogglePalette => self.toggle_palette(Picker::open(builtin_commands(&self.keymap))),
            Action::ToggleHistoryPalette => self.toggle_palette(Picker::open_history()),
            Action::TogglePaneSwitcher => {
                if self.close_open_palette() {
                    self.run_command("select_pane", focused);
                }
            }
            Action::ToggleSwoop => {
                if self.close_open_palette() {
                    self.open_swoop(focused);
                }
            }
            Action::Jump => {
                if self
                    .chrome.picker()
                    .is_some_and(|pal| pal.mode == PickerMode::Jump)
                {
                    self.cancel_jump(focused);
                    self.dismiss_overlay();
                    return;
                }
                if self.close_open_palette() {
                    self.open_jump(focused);
                }
            }
            Action::CopyCwd => self.copy_pane_cwd(focused),
            Action::Macro(step) => self.run_macro_step(step),
            Action::RunCommand(name) => self.run_command(&name, focused),
            Action::Ignore => {}
        }
    }

    // --------------------------------------------------------------------
    // Mode switching
    // --------------------------------------------------------------------

    // --------------------------------------------------------------------
    // Shared action steps
    // --------------------------------------------------------------------

    /// Open an empty `/` (or `?`, when `reverse`) search anchored at the
    /// focused pane's cursor.
    fn start_search(&mut self, reverse: bool, focused: TabId) {
        self.search.query = Some(String::new());
        self.search.reverse = reverse;
        self.set_search_origin(focused);
        // Forces the status bar on (see `status_bar_visible`) when it's
        // configured hidden, which shrinks the pane area by one row; resize now
        // so the PTY's row count matches immediately rather than waiting for
        // some unrelated resize event.
        if !self.config.status_bar.enabled {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Put the live search away, leaving the cursor on the match it landed on:
    /// ending a search puts away the highlight and the query, not the
    /// navigation it just did. [`SearchState::last`] and
    /// [`SearchState::reverse`] survive, so `n`/`N` can resume the same search,
    /// the same way round, from here. A no-op when no search is running.
    ///
    /// [`SearchState::last`]: crate::window::terminal::navigation::SearchState::last
    /// [`SearchState::reverse`]: crate::window::terminal::navigation::SearchState::reverse
    pub(in crate::window) fn dismiss_search(&mut self) {
        self.search.debounce = None;
        if self.search.query.is_none() {
            return;
        }
        self.search.query = None;
        self.search.match_index = 0;
        self.search.match_total = 0;
        self.search.current = None;
        self.search.origin = None;
        // Mirrors `start_search`: the status bar drops back to its configured
        // (hidden) visibility, giving the pane area its row back, so resize
        // immediately to match.
        if !self.config.status_bar.enabled {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Close the palette when one is open. Returns whether the caller should
    /// open one instead, i.e. whether there was nothing to close.
    fn close_open_palette(&mut self) -> bool {
        self.dirty = true;
        if self.chrome.picker().is_none() {
            return true;
        }
        self.dismiss_overlay();
        false
    }

    /// Close the palette when one is open, otherwise show `palette`, seeded
    /// with the persisted query history.
    fn toggle_palette(&mut self, palette: Picker) {
        if self.close_open_palette() {
            self.open_picker(palette.with_query_history(self.recent.palette.clone()));
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::terminal::session::Terminal;
    use crate::window::test_support::*;

    use super::*;

    #[test]
    fn test_typing_or_pasting_brings_a_scrolled_back_view_to_the_prompt() {
        // The keys go to the shell at the live screen, so a view left in
        // history would show nothing of what was typed.
        let mut app = App::new();
        let focused = app.layout().focused();
        let mut terminal = Terminal::with_command(
            40,
            8,
            portable_pty::CommandBuilder::new("cat"),
            winter_core::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        for _ in 0..20 {
            terminal.grid_mut().line_feed();
        }
        app.open_tabs.insert_terminal(focused, terminal);

        app.open_tabs
            .terminal_mut(&focused)
            .expect("terminal")
            .grid_mut()
            .scroll_up_history(6);
        app.handle_action(
            crate::model::input::Action::SendBytes(b"a".to_vec()),
            focused,
        );
        assert_eq!(
            app.open_tabs
                .terminal(&focused)
                .unwrap()
                .grid()
                .scroll_offset(),
            0,
            "typed"
        );

        app.open_tabs
            .terminal_mut(&focused)
            .expect("terminal")
            .grid_mut()
            .scroll_up_history(6);
        app.paste_text("b");
        assert_eq!(
            app.open_tabs
                .terminal(&focused)
                .unwrap()
                .grid()
                .scroll_offset(),
            0,
            "pasted"
        );
    }

    #[test]
    fn test_prompt_editing_is_enabled_by_default() {
        let app = App::new();
        assert!(app.prompt_editing_enabled());
    }

    #[test]
    fn test_prompt_editing_is_declined_under_non_emacs_bindings() {
        // The operators are realized as readline chords, so a shell in vi mode
        // has them bound to something else. Declining leaves the user with the
        // shell's own Vim editing; translating anyway would fire whatever
        // `C-w` happens to mean there.
        let mut app = App::new();
        app.config.prompt_edit_bindings = crate::config::PromptEditBindings::None;
        assert!(!app.prompt_editing_enabled());
    }

    #[test]
    fn test_declined_prompt_operator_writes_nothing_to_the_pty() {
        // The important half: a declined operator must not send stray control
        // bytes to a shell that would misinterpret them.
        let mut app = App::new();
        app.config.prompt_edit_bindings = crate::config::PromptEditBindings::None;
        let focused = app.layout().focused();
        app.delete_on_prompt(
            crate::window::terminal::prompt_edit::PromptDelete::WordBack,
            focused,
        );
        assert!(
            app.chrome.notice_text().is_some(),
            "the declined keypress should be reported, not silently dropped"
        );
    }

    #[test]
    fn test_entering_insert_scrolls_back_down_to_the_prompt() {
        // Typing lands at the prompt, so whichever way Insert is entered (`i`,
        // `a`, `o`, Visual's `i`, or a plain mode switch from a menu/click), the
        // pane snaps out of scrolled-back history to the live bottom.
        let entries: Vec<input::Action> = vec![
            input::Action::EnterInsert(input::InsertAt::Cursor),
            input::Action::EnterInsert(input::InsertAt::After),
            input::Action::EnterInsert(input::InsertAt::LineEnd),
            input::Action::SwitchMode(Mode::Insert),
        ];
        for action in entries {
            let mut app = App::new();
            let focused = app.layout().focused();
            let mut terminal = Terminal::with_command(
                40,
                8,
                portable_pty::CommandBuilder::new("cat"),
                winter_core::MAX_SCROLLBACK,
            )
            .expect("test pane spawn");
            // Build a page of scrollback, then scroll up into it.
            for _ in 0..16 {
                terminal.grid_mut().line_feed();
            }
            terminal.grid_mut().scroll_up_history(5);
            assert_eq!(
                terminal.grid().scroll_offset(),
                5,
                "fixture must start scrolled up"
            );
            app.open_tabs.insert_terminal(focused, terminal);
            app.open_tabs.set_mode(&focused, Mode::Normal);
            app.set_nav_cursor(focused, (0, 0));

            app.handle_action(action.clone(), focused);

            assert_eq!(
                app.open_tabs
                    .terminal(&focused)
                    .unwrap()
                    .grid()
                    .scroll_offset(),
                0,
                "{action:?} should scroll back down to the prompt"
            );
            assert_eq!(app.open_tabs.mode(&focused), Some(Mode::Insert));
        }
    }

    #[test]
    fn test_escape_in_normal_mode_stays_in_normal_and_only_clears_the_search() {
        // `Esc` used to drop straight back to Insert, handing the next keystroke
        // to the shell mid-navigation. It now behaves like vim's `:nohlsearch`:
        // the search highlight goes, the mode does not.
        let mut app = App::new();
        let focused = app.layout().focused();
        app.open_tabs.insert_terminal(focused, idle_terminal());
        app.open_tabs.set_mode(&focused, Mode::Normal);
        app.search.query = Some("foo".to_string());
        app.search.match_total = 2;

        app.handle_action(input::Action::SearchCancel, focused);

        assert_eq!(app.open_tabs.mode(&focused), Some(Mode::Normal));
        assert!(app.search.query.is_none());
        assert_eq!(app.search.match_total, 0);
    }

    #[test]
    fn test_enter_insert_is_the_only_way_out_of_normal_mode() {
        // `i`, `a` and `o` all reach Insert (they differ only in where the cursor
        // lands, which `move_nav_cursor` handles), from Normal and from Visual.
        for (mode, at) in [
            (Mode::Normal, input::InsertAt::Cursor),
            (Mode::Normal, input::InsertAt::After),
            (Mode::Normal, input::InsertAt::LineEnd),
            (Mode::Visual, input::InsertAt::Cursor),
        ] {
            let mut app = App::new();
            let focused = app.layout().focused();
            app.open_tabs.insert_terminal(focused, idle_terminal());
            app.open_tabs.set_mode(&focused, mode);
            app.set_nav_cursor(focused, (0, 0));

            app.handle_action(input::Action::EnterInsert(at), focused);

            assert_eq!(
                app.open_tabs.mode(&focused),
                Some(Mode::Insert),
                "{at:?} from {mode:?} should reach Insert"
            );
            assert_eq!(app.nav_cursor(focused), None, "the nav cursor is dropped");
        }
    }

    #[test]
    fn test_leaving_normal_mode_ends_a_search_and_hides_the_forced_status_bar() {
        let mut app = app_with_tabs(1);
        app.config.status_bar.enabled = false;
        let focused = app.layout().focused();
        app.open_tabs.insert_terminal(focused, idle_terminal());
        app.open_tabs.set_mode(&focused, Mode::Normal);

        // Search executed, now browsing matches in Normal mode: the bar is
        // forced visible even though it's configured off.
        app.search.query = Some("foo".to_string());
        app.search.match_index = 1;
        app.search.match_total = 3;
        assert!(app.status_bar_visible());

        // `i` back to Insert (done browsing, not a `SearchCancel` mid-input)
        // still ends the search and lets the bar drop back to hidden.
        app.handle_action(input::Action::SwitchMode(Mode::Insert), focused);
        assert!(app.search.query.is_none());
        assert_eq!(app.search.match_index, 0);
        assert_eq!(app.search.match_total, 0);
        assert!(!app.status_bar_visible());
    }
}
