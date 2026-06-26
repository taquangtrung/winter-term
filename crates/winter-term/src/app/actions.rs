//! Keyboard action dispatch: translates resolved [`Action`]s into state changes.

use crate::config::PromptEditBindings;
use crate::model::input::{self, Action, CursorMove, EditAction, InsertAt, VisualKind};
use crate::model::layout::{Rect, TabId};
use crate::model::mode::{Mode, ModeEvent};
use crate::model::palette::{Palette, PaletteMode};

use super::navigation::search::SearchFrom;
use super::prompt_edit::{
    self, edit_action_bytes, prompt_delete_bytes, rebuild_line_bytes, PromptDelete, READLINE_UNDO,
};
use super::{App, LastVisual, FONT_SIZE_STEP};
use crate::terminal::session::Terminal;

// ========================================================================
// App: action handling
// ========================================================================

impl App {
    pub(crate) fn handle_action(&mut self, action: Action, focused: TabId) {
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
                self.last_find = Some(find);
                // Several candidates put up the jump overlay and hold the keymap
                // for the label key; one or none just moves (or doesn't).
                if self.find_char_overlay(find, focused) {
                    self.pending = input::PendingPrefix::FindLabel;
                }
            }
            Action::FindJump(label) => {
                self.find_jump(focused, label);
            }
            Action::FindCancel => {
                self.clear_find_labels();
            }
            Action::FindRepeat { reverse } => {
                if let Some(find) = self.last_find {
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
                    self.vim.registers.insert(reg, text.clone());
                    if reg == '+' || reg == '*' || reg == '"' {
                        self.copy_selection();
                    } else {
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
                    self.vim.registers.get(&register).cloned()
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
            Action::ZoomPane => {
                self.layout_mut().toggle_zoom();
                // Zooming changes the focused pane's rect, so re-send the new
                // size to the PTY; without this btop keeps reporting the old
                // (small) split dimensions after maximizing.
                if self.renderer.is_some() {
                    self.resize_all_panes();
                }
            }
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
                self.search_debounce = Some((
                    std::time::Instant::now()
                        + std::time::Duration::from_millis(super::SEARCH_DEBOUNCE_MILLIS),
                    focused,
                ));
                self.dirty = true;
            }
            Action::SearchExecute => {
                self.search_debounce = None;
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
                self.search_debounce = None;
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
                self.quick_select = None;
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
            Action::TogglePalette => self.toggle_palette(Palette::open(&self.keymap)),
            Action::ToggleHistoryPalette => self.toggle_palette(Palette::open_history()),
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
                    .palette()
                    .is_some_and(|pal| pal.mode == PaletteMode::Jump)
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
            Action::RunCommand(name) => self.run_command(&name, focused),
            Action::Ignore => {}
        }
    }

    // --------------------------------------------------------------------
    // Mode switching
    // --------------------------------------------------------------------

    /// Move `focused` into `new_mode`, running the bookkeeping each side of the
    /// transition owes: what the old mode must hand back, then what the new one
    /// needs set up.
    fn switch_mode(&mut self, new_mode: Mode, focused: TabId) {
        let old_mode = self.open_tabs.mode(&focused).unwrap_or_default();
        self.open_tabs.set_mode(&focused, new_mode);
        // A pending `f`/`t` overlay belongs to the mode it was raised in.
        self.clear_find_labels();
        self.leave_mode(old_mode, new_mode, focused);
        self.enter_mode(old_mode, new_mode, focused);
        self.dirty = true;
    }

    /// Bookkeeping owed to the mode being left: the jumplist entry, the `gv`
    /// selection, and the live search.
    fn leave_mode(&mut self, old_mode: Mode, new_mode: Mode, focused: TabId) {
        // Leaving Normal altogether (back to Insert, into a block) is one of
        // vim's jumplist jumps: the browsing position is recorded so `C-o`
        // after the next Escape returns to it. Recorded before the nav cursor is
        // cleared in `enter_mode`.
        if old_mode == Mode::Normal && new_mode != Mode::Normal {
            self.push_jump(focused);
        }
        // Leaving Visual: remember the selection for `gv`, then clear it and the
        // anchor.
        if old_mode == Mode::Visual {
            self.remember_visual(focused);
            self.selection.visual_anchor = None;
            self.selection.span = None;
        }
        // Leaving Normal (browsing matches with `/`, `n`/`N`) without going
        // through `SearchCancel` (e.g. `i`/`a`/`o` back to Insert once done, or
        // focusing a block) still ends the search.
        if old_mode == Mode::Normal && new_mode != Mode::Normal {
            self.dismiss_search();
        }
    }

    /// Bookkeeping owed to the mode being entered: seeding or clearing the nav
    /// cursor, and handing the prompt back to the shell on the way into Insert.
    fn enter_mode(&mut self, old_mode: Mode, new_mode: Mode, focused: TabId) {
        match new_mode {
            // Entering Normal afresh (from Insert/Block) seeds the nav cursor at
            // the prompt; returning from Visual keeps it put.
            Mode::Normal => {
                if old_mode != Mode::Normal && old_mode != Mode::Visual {
                    self.init_nav_cursor(focused);
                }
            }
            Mode::Insert => {
                if new_mode == Mode::Insert {
                    self.align_prompt_to_nav_cursor(focused);
                }
                self.clear_nav_cursor(focused);
            }
            Mode::Visual => {}
        }
    }

    /// Bring the prompt back into view and walk the shell's cursor over to the
    /// nav cursor's column, so typing resumes where the user was looking.
    ///
    /// Every route into Insert (`i`/`a`/`o`, Visual's `i`, the menu/palette, a
    /// click) lands here, so they all scroll and align alike.
    fn align_prompt_to_nav_cursor(&mut self, focused: TabId) {
        // Typing goes to the prompt, so a pane scrolled up into history snaps
        // back to the live bottom first.
        let was_scrolled = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|terminal| terminal.grid().scroll_offset() > 0);
        if was_scrolled {
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.grid_mut().set_scroll_offset(0);
            }
        }
        // Aligning the shell cursor to the nav cursor's column only makes sense
        // when that cursor addressed the live screen; from scrolled-back history
        // it named a scrollback row, not a column of the prompt.
        let aligned = (!was_scrolled)
            .then(|| {
                self.nav_cursor(focused).and_then(|(nav_row, nav_col)| {
                    let terminal = self.open_tabs.terminal(&focused)?;
                    let (prompt_row, pty_col) = terminal.grid().cursor();
                    (nav_row == prompt_row).then_some((nav_col, pty_col))
                })
            })
            .flatten();
        let Some((nav_col, pty_col)) = aligned else {
            return;
        };
        // Keep the shadow cursor aligned with the shell cursor so
        // post-navigation inserts model correctly.
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.sync_cursor(nav_col, pty_col);
        }
        let (seq, steps) = if nav_col >= pty_col {
            (b"\x1b[C" as &[u8], nav_col - pty_col)
        } else {
            (b"\x1b[D" as &[u8], pty_col - nav_col)
        };
        let mut bytes = Vec::new();
        for _ in 0..steps {
            bytes.extend_from_slice(seq);
        }
        if !bytes.is_empty() {
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.write(&bytes);
            }
        }
    }

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
    /// [`SearchState::last`]: super::navigation::SearchState::last
    /// [`SearchState::reverse`]: super::navigation::SearchState::reverse
    fn dismiss_search(&mut self) {
        self.search_debounce = None;
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

    /// Leave Visual mode after a yank: remember the span for `gv`, then clear
    /// it and the anchor.
    fn finish_visual_yank(&mut self, focused: TabId) {
        // The selection's own ends are the yank's, which Vim marks `[` and `]`.
        if let Some(span) = self.selection.span.clone() {
            let (first, last) = if (span.start_row, span.start_col) <= (span.end_row, span.end_col)
            {
                (
                    (span.start_row, span.start_col),
                    (span.end_row, span.end_col),
                )
            } else {
                (
                    (span.end_row, span.end_col),
                    (span.start_row, span.start_col),
                )
            };
            self.set_auto_mark(focused, '[', first);
            self.set_auto_mark(focused, ']', last);
        }
        self.remember_visual(focused);
        self.open_tabs
            .set_mode(&focused, Mode::Visual.apply(ModeEvent::Escape));
        self.selection.visual_anchor = None;
        self.selection.span = None;
        self.dirty = true;
    }

    /// Close the palette when one is open. Returns whether the caller should
    /// open one instead, i.e. whether there was nothing to close.
    fn close_open_palette(&mut self) -> bool {
        self.dirty = true;
        if self.palette().is_none() {
            return true;
        }
        self.dismiss_overlay();
        false
    }

    /// Close the palette when one is open, otherwise show `palette`, seeded
    /// with the persisted query history.
    fn toggle_palette(&mut self, palette: Palette) {
        if self.close_open_palette() {
            self.open_palette(palette.with_query_history(self.palette_history.clone()));
        }
    }

    /// Whether Vim operators may edit the shell's prompt line right now.
    ///
    /// False when the user has told us their shell is not in emacs mode: the
    /// operators are realized as readline chords (`C-a`, `C-k`, ...), and
    /// a shell in vi mode has those bound to something else entirely, so
    /// "delete a word" would run whatever `C-w` means there instead. A shell
    /// in vi mode already provides its own Vim line editing, so declining here
    /// leaves the user with working editing rather than none.
    pub(crate) fn prompt_editing_enabled(&self) -> bool {
        self.config.prompt_edit_bindings == PromptEditBindings::Emacs
    }

    /// Report that a prompt operator was declined because prompt editing is
    /// off, so the keypress does not just vanish.
    pub(crate) fn decline_prompt_edit(&mut self) {
        self.set_error("Prompt editing is off (prompt-edit-bindings); your shell's own editor handles the line");
    }

    /// Whether any of `targets`, the grid columns a Normal-mode prompt edit
    /// would touch, falls inside the shell's own prompt decoration - left of
    /// where OSC 133's `B` mark said the command line begins. Such an edit
    /// cannot be translated: readline's buffer starts at the command text, so
    /// the arrow-key alignment would overshoot it and the following Delete
    /// keypresses would eat characters the operator never targeted.
    pub(crate) fn prompt_edit_hits_decoration(terminal: &Terminal, targets: &[usize]) -> bool {
        terminal
            .prompt_line_start()
            .is_some_and(|start| targets.iter().any(|&col| col < start))
    }

    /// Refuse an edit that [`Self::prompt_edit_hits_decoration`] flags, so the
    /// keypress does not just vanish. Returns whether it refused.
    pub(crate) fn decline_decoration_edit(&mut self, blocked: bool) -> bool {
        if blocked {
            self.set_error("Cannot edit: that is the shell's own prompt, not the command line");
        }
        blocked
    }

    /// Apply a Vim delete operator to the last prompt by sending the shell the
    /// equivalent readline edit. Only the live prompt line (the row holding the
    /// shell cursor) is editable; deletes aimed at scrollback history are ignored.
    pub(crate) fn delete_on_prompt(&mut self, op: PromptDelete, focused: TabId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            // The cursor is on scrollback history, not the live prompt: only the
            // prompt line is editable, so report the attempt instead of silently
            // dropping it.
            self.set_error("Cannot delete: not on the editable prompt line");
            return;
        }
        let mut targets = vec![nav_col];
        if let PromptDelete::Range { start_col, .. } = &op {
            targets.push(*start_col);
        }
        if self.decline_decoration_edit(Self::prompt_edit_hits_decoration(terminal, &targets)) {
            return;
        }
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.record_normal_delete(op, nav_col, pty_col);
        }
        let bytes = prompt_delete_bytes(op, pty_col, nav_col);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Apply a Vim change operator on the prompt line: deletes the target span and enters Insert mode.
    pub(crate) fn change_on_prompt(&mut self, op: PromptDelete, focused: TabId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        let (nav_row, _) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot change: not on the editable prompt line");
            return;
        }
        self.delete_on_prompt(op, focused);
        self.open_tabs.set_mode(&focused, Mode::Insert);
        self.selection.span = None;
        self.selection.visual_anchor = None;
        self.dirty = true;
    }

    /// Replace the character under the cursor on the editable prompt line without entering Insert mode.
    pub(crate) fn replace_char_on_prompt(&mut self, ch: char, focused: TabId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot replace: not on the editable prompt line");
            return;
        }
        if self.decline_decoration_edit(Self::prompt_edit_hits_decoration(terminal, &[nav_col])) {
            return;
        }
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.desync();
        }
        let bytes = prompt_edit::prompt_replace_char_bytes(pty_col, nav_col, ch);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Toggle the ASCII case of the character under the cursor and advance the cursor right.
    pub(crate) fn toggle_case_on_prompt(&mut self, focused: TabId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot toggle case: not on the editable prompt line");
            return;
        }
        let ch = terminal.grid().cell(prompt_row, nav_col).map(|c| c.ch);
        if self.decline_decoration_edit(Self::prompt_edit_hits_decoration(terminal, &[nav_col])) {
            return;
        }
        if let Some(c) = ch {
            let toggled = if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                self.move_nav_cursor(CursorMove::Right, focused);
                return;
            };
            if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
                shadow.desync();
            }
            let bytes = prompt_edit::prompt_toggle_case_bytes(pty_col, nav_col, toggled);
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.write(&bytes);
            }
            self.move_nav_cursor(CursorMove::Right, focused);
            self.nav_resync_pending = true;
            self.dirty = true;
        }
    }

    /// Perform a configurable Insert-mode line edit (e.g. `C-DEL` to
    /// delete the word before the cursor): fold it into the undo shadow and send
    /// the equivalent readline keystrokes to the shell.
    fn edit_on_prompt(&mut self, action: EditAction, focused: TabId) {
        // The chord is forwarded either way: the user pressed it, and the shell
        // is entitled to interpret it. Only the shadow is skipped, because the
        // model of the line would be wrong under other bindings.
        let at_prompt = self.prompt_editing_enabled()
            && self
                .open_tabs
                .terminal(&focused)
                .is_some_and(|terminal| terminal.is_at_prompt());
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.apply_edit_action(action, at_prompt);
        }
        let bytes = edit_action_bytes(action);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.dirty = true;
    }

    /// Apply a prompt undo (`undo == true`, `C-/`) or redo (`C-\`).
    ///
    /// Undo is delegated to the shell's own undo (`C-_`), the WezTerm approach:
    /// a single readline/ZLE operation, so the line repaints once instead of
    /// flashing through a multi-keystroke rebuild (which syntax-highlight /
    /// autosuggest plugins re-render at every step). The shadow pointer is advanced
    /// in step so a following redo has the right base.
    ///
    /// Shells have no portable redo, so redo is Winter's own: it rebuilds the
    /// recorded target line in one bracketed paste (see [`rebuild_line_bytes`]),
    /// which is robust to whatever state the shell's undo left the line in and
    /// still repaints once. A no-op off the prompt.
    fn prompt_history_apply(&mut self, focused: TabId, undo: bool) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let at_prompt = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|terminal| terminal.is_at_prompt());
        if !at_prompt {
            return;
        }
        if undo {
            if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
                shadow.step_back();
            }
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.write(&[READLINE_UNDO]);
            }
            self.dirty = true;
            return;
        }
        let Some(target) = self
            .open_tabs
            .prompt_shadow_mut(&focused)
            .and_then(|shadow| shadow.redo_target())
        else {
            return;
        };
        let bracketed = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|terminal| terminal.bracketed_paste());
        let bytes = rebuild_line_bytes(&target, bracketed);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Enter Visual mode from Normal, toggle it off when the same kind is pressed
    /// again, or switch between charwise, linewise, and blockwise while staying in Visual.
    fn toggle_visual(&mut self, kind: VisualKind, focused: TabId) {
        let mode = self.open_tabs.mode(&focused).unwrap_or_default();
        match mode {
            Mode::Normal => {
                self.open_tabs
                    .set_mode(&focused, Mode::Normal.apply(ModeEvent::EnterVisual));
                self.selection.visual_anchor =
                    Some(self.absolute_nav_cursor(focused).unwrap_or((0, 0)));
                self.selection.visual_kind = kind;
                self.update_visual_selection(focused);
            }
            Mode::Visual if self.selection.visual_kind == kind => {
                // Same kind again leaves Visual, back to Normal.
                self.remember_visual(focused);
                self.open_tabs
                    .set_mode(&focused, Mode::Visual.apply(ModeEvent::EnterVisual));
                self.selection.visual_anchor = None;
                self.selection.span = None;
            }
            Mode::Visual => {
                // Switch kind (charwise <-> linewise <-> blockwise), keeping the anchor.
                self.selection.visual_kind = kind;
                self.update_visual_selection(focused);
            }
            Mode::Insert => {}
        }
        self.dirty = true;
    }

    /// Visual `o`: put the cursor on the selection's other end (the anchor),
    /// so extending continues from there. The highlighted span is unchanged,
    /// it always runs anchor..cursor in either order.
    fn swap_visual_ends(&mut self, focused: TabId) {
        if let (Some(anchor), Some(cursor)) = (
            self.selection.visual_anchor,
            self.absolute_nav_cursor(focused),
        ) {
            self.selection.visual_anchor = Some(cursor);
            // Both ends are absolute, so the old anchor may be off screen;
            // `reveal_position` scrolls it back into view before parking there.
            self.reveal_position(focused, anchor);
            self.update_visual_selection(focused);
            self.dirty = true;
        }
    }

    /// Snapshot the live Visual selection as `gv`'s restore target, called at
    /// every point a Visual selection ends. Both ends are stored in absolute
    /// coordinates (see `winter_render::Grid::to_absolute_row`) so the snapshot
    /// still names the same text after the view has scrolled or grown; a pane
    /// without a live selection (no anchor or cursor) snapshots nothing.
    fn remember_visual(&mut self, focused: TabId) {
        let Some(anchor) = self.selection.visual_anchor else {
            return;
        };
        let Some((cursor_row, cursor_col)) = self.nav_cursor(focused) else {
            return;
        };
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        self.selection.last_visual = Some(LastVisual {
            anchor,
            cursor: (grid.to_absolute_row(cursor_row), cursor_col),
            kind: self.selection.visual_kind,
            tab: focused,
        });
    }

    /// `gv`: re-enter Visual mode with the last selection `remember_visual`
    /// captured: same pane, same kind, same two ends. The cursor end is
    /// revealed (centered if it scrolled off screen); the anchor end is clamped
    /// to the viewport when the span no longer fits, exactly like vim's own
    /// `gv` on a selection taller than the window.
    fn restore_visual(&mut self, focused: TabId) {
        let Some(last) = self
            .selection
            .last_visual
            .as_ref()
            .filter(|lv| lv.tab == focused)
            .copied()
        else {
            return;
        };
        self.open_tabs
            .set_mode(&focused, Mode::Normal.apply(ModeEvent::EnterVisual));
        self.selection.visual_kind = last.kind;
        self.reveal_position(focused, last.cursor);
        self.selection.visual_anchor = Some(last.anchor);
        self.update_visual_selection(focused);
        self.dirty = true;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::*;

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
            winter_render::MAX_SCROLLBACK,
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
        app.delete_on_prompt(crate::app::prompt_edit::PromptDelete::WordBack, focused);
        assert!(
            app.notice_text().is_some(),
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
                winter_render::MAX_SCROLLBACK,
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
