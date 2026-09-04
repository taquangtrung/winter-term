//! Keyboard action dispatch — translates resolved [`Action`]s into state changes.

use crate::config::PromptEditBindings;
use crate::model::input::{self, Action, CursorMove, EditAction, InsertAt, VisualKind};
use crate::model::layout::{PaneId, Rect};
use crate::model::mode::{Mode, ModeEvent};
use crate::model::palette::Palette;

use super::navigation::search::SearchFrom;
use super::prompt_edit::{
    self, edit_action_bytes, prompt_delete_bytes, rebuild_line_bytes, PromptDelete, READLINE_UNDO,
};
use super::{App, LastVisual, FONT_SIZE_STEP};

// ========================================================================
// App: action handling
// ========================================================================

impl App {
    pub(crate) fn handle_action(&mut self, action: Action, focused: PaneId) {
        // `.`-repeat and `g;`/`g,` bookkeeping: records the change this
        // dispatch makes (or the session it opens/closes) before it runs.
        self.track_change(&action, focused);
        match action {
            Action::SendBytes(bytes) => {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.write(&bytes);
                }
            }
            Action::SwitchMode(new_mode) => {
                let old_mode = self.modes.get(&focused).copied().unwrap_or_default();
                self.modes.insert(focused, new_mode);
                // A pending `f`/`t` overlay belongs to the mode it was raised in.
                self.clear_find_labels();
                // Leaving Normal altogether (back to Insert, into a block) is one
                // of vim's jumplist jumps: the browsing position is recorded so
                // `Ctrl+O` after the next Escape returns to it. Recorded before
                // the nav cursor is cleared below.
                if old_mode == Mode::Normal && new_mode != Mode::Normal {
                    self.push_jump(focused);
                }
                // Leaving Visual: remember the selection for `gv`, then clear it
                // and the anchor.
                if old_mode == Mode::Visual {
                    self.remember_visual(focused);
                    self.visual_anchor = None;
                    self.selection = None;
                }
                // Leaving Normal (browsing matches with `/`, `n`/`N`) without
                // going through `SearchCancel` (e.g. `i`/`a`/`o` back to Insert
                // once done, or focusing a block) still ends the search — clear it
                // here too, so a status bar forced on only for the search (see
                // `status_bar_visible`) drops back to its configured visibility.
                // As with `SearchCancel`, the query itself is remembered
                // (`search_last`) so `n`/`N` back in Normal mode resume it.
                if old_mode == Mode::Normal
                    && new_mode != Mode::Normal
                    && self.search_query.is_some()
                {
                    self.search_query = None;
                    self.search_match_index = 0;
                    self.search_match_total = 0;
                    self.search_current = None;
                    self.search_origin = None;
                    if !self.config.status_bar.enabled {
                        self.resize_all_panes();
                    }
                }
                match new_mode {
                    // Entering Normal afresh (from Insert/Block) seeds the nav
                    // cursor at the prompt; returning from Visual keeps it put.
                    Mode::Normal => {
                        if old_mode != Mode::Normal && old_mode != Mode::Visual {
                            self.init_nav_cursor(focused);
                        }
                    }
                    Mode::Insert | Mode::BlockFocus => {
                        if new_mode == Mode::Insert {
                            // Typing goes to the prompt, so bring the prompt back
                            // into view: a pane scrolled up into history snaps to
                            // the live bottom. Every route into Insert (`i`/`a`/`o`,
                            // Visual's `i`, the menu/palette, a click) lands here,
                            // so they all scroll alike.
                            let was_scrolled = self
                                .panes
                                .get(&focused)
                                .is_some_and(|pane| pane.grid().scroll_offset() > 0);
                            if was_scrolled {
                                if let Some(pane) = self.panes.get_mut(&focused) {
                                    pane.grid_mut().set_scroll_offset(0);
                                }
                            }
                            // Aligning the shell cursor to the nav cursor's column
                            // only makes sense when that cursor addressed the live
                            // screen; from scrolled-back history it named a
                            // scrollback row, not a column of the prompt.
                            let aligned = (!was_scrolled)
                                .then(|| {
                                    self.nav_cursor(focused).and_then(|(nav_row, nav_col)| {
                                        let pane = self.panes.get(&focused)?;
                                        let (prompt_row, pty_col) = pane.grid().cursor();
                                        (nav_row == prompt_row).then_some((nav_col, pty_col))
                                    })
                                })
                                .flatten();
                            if let Some((nav_col, pty_col)) = aligned {
                                // Keep the shadow cursor aligned with the shell
                                // cursor so post-navigation inserts model correctly.
                                if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
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
                                    if let Some(pane) = self.panes.get_mut(&focused) {
                                        pane.write(&bytes);
                                    }
                                }
                            }
                        }
                        self.clear_nav_cursor(focused);
                    }
                    Mode::Visual => {}
                }
                self.dirty = true;
            }
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
                    .modes
                    .get(&focused)
                    .copied()
                    .unwrap_or_default()
                    .apply(ModeEvent::ToInsert);
                self.handle_action(Action::SwitchMode(mode), focused);
            }
            Action::MoveCursor(mv) => {
                self.move_nav_cursor(mv, focused);
                if self.modes.get(&focused) == Some(&Mode::Visual) {
                    self.update_visual_selection(focused);
                }
            }
            Action::MoveCursorN { count, mv } => {
                for _ in 0..count.max(1) {
                    self.move_nav_cursor(mv, focused);
                }
                if self.modes.get(&focused) == Some(&Mode::Visual) {
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
            Action::YankSelection => {
                self.copy_selection();
                self.remember_visual(focused);
                self.modes
                    .insert(focused, Mode::Visual.apply(ModeEvent::Escape));
                self.visual_anchor = None;
                self.selection = None;
                self.dirty = true;
            }
            Action::YankSelectionRegister(reg) => {
                if let Some(text) = self.selected_text() {
                    self.registers.insert(reg, text.clone());
                    if reg == '+' || reg == '*' || reg == '"' {
                        self.copy_selection();
                    } else {
                        self.set_notice(format!("Yanked into register \"{reg}"));
                    }
                }
                self.remember_visual(focused);
                self.modes
                    .insert(focused, Mode::Visual.apply(ModeEvent::Escape));
                self.visual_anchor = None;
                self.selection = None;
                self.dirty = true;
            }
            Action::Paste => {
                self.paste_from_clipboard();
            }
            Action::PasteRegister { register, after: _ } => {
                let text = if register == '+' || register == '*' {
                    self.clipboard_text()
                } else {
                    self.registers.get(&register).cloned()
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
                self.tab_mut().toggle_zoom();
                // Zooming changes the focused pane's rect, so re-send the new
                // size to the PTY; without this btop keeps reporting the old
                // (small) split dimensions after maximizing.
                if self.renderer.is_some() {
                    self.resize_all_panes();
                }
            }
            Action::MoveTabLeft => {
                if self.active_tab > 0 {
                    let dst = self.active_tab - 1;
                    self.swap_tabs(self.active_tab, dst);
                }
            }
            Action::MoveTabRight => {
                let last = self.tabs.len().saturating_sub(1);
                if self.active_tab < last {
                    let dst = self.active_tab + 1;
                    self.swap_tabs(self.active_tab, dst);
                }
            }
            Action::NewTab => {
                self.new_tab();
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
            Action::CloseTab(which) => {
                let index = which
                    .map(|n| n.saturating_sub(1))
                    .unwrap_or(self.active_tab);
                self.close_tab(index);
            }
            Action::FocusPane(dir) => {
                let viewport = self.viewport_rect();
                let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
                self.tab_mut().focus_in_direction(dir, layout_vp);
            }
            Action::FocusPaneByIndex(n) => {
                self.tab_mut().focus_by_index(n.saturating_sub(1));
            }
            Action::ClosePaneByIndex(n) => {
                if let Some(&id) = self.tab().panes().get(n.saturating_sub(1)) {
                    self.close_pane(id);
                }
            }
            Action::FocusBlock(nav) => {
                self.focus_block(nav, focused);
            }
            Action::ForwardToBlock(bytes) => {
                self.webview_mgr.forward_key_event(focused, &bytes);
            }
            Action::SearchStart => {
                self.search_query = Some(String::new());
                self.search_reverse = false;
                self.set_search_origin(focused);
                // Forces the status bar on (see `status_bar_visible`) when it's
                // configured hidden, which shrinks the pane area by one row;
                // resize now so the PTY's row count matches immediately rather
                // than waiting for some unrelated resize event.
                if !self.config.status_bar.enabled {
                    self.resize_all_panes();
                }
                self.dirty = true;
            }
            Action::SearchStartBackward => {
                self.search_query = Some(String::new());
                self.search_reverse = true;
                self.set_search_origin(focused);
                if !self.config.status_bar.enabled {
                    self.resize_all_panes();
                }
                self.dirty = true;
            }
            Action::SearchChar(c) => {
                if let Some(q) = &mut self.search_query {
                    q.push(c);
                }
                self.search_step(focused, self.search_start_direction(), SearchFrom::Origin);
                self.dirty = true;
            }
            Action::SearchBackspace => {
                if let Some(q) = &mut self.search_query {
                    q.pop();
                }
                self.search_step(focused, self.search_start_direction(), SearchFrom::Origin);
                self.dirty = true;
            }
            Action::SearchExecute => {
                // Enter accepts the match the incremental search already landed
                // on, so it scans from the origin again (idempotent) rather than
                // stepping forward — `n` is what advances.
                self.search_step(focused, self.search_start_direction(), SearchFrom::Origin);
                self.dirty = true;
            }
            Action::SearchCancel => {
                // Also the plain `Esc` in Normal mode (vim's `:nohlsearch`), which
                // no longer leaves Normal — so do nothing at all when there's no
                // search to clear, rather than forcing a status-bar resize.
                if self.search_query.is_none() {
                    return;
                }
                // The cursor stays on the match the search landed on: ending the
                // search puts away the highlight and the query, not the navigation
                // it just did. `search_last` and `search_reverse` survive, so `n`/`N`
                // can resume the same search, the same way round, from here.
                self.search_query = None;
                self.search_match_index = 0;
                self.search_match_total = 0;
                self.search_current = None;
                self.search_origin = None;
                // Mirrors `SearchStart`: the status bar drops back to its
                // configured (hidden) visibility, giving the pane area its row
                // back, so resize immediately to match.
                if !self.config.status_bar.enabled {
                    self.resize_all_panes();
                }
                self.dirty = true;
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
                if let Some(pane) = self.panes.get_mut(&focused) {
                    let rows = pane.grid().rows();
                    pane.grid_mut().scroll_up_history(rows);
                }
                self.dirty = true;
            }
            Action::ScrollPageDown => {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    let rows = pane.grid().rows();
                    pane.grid_mut().scroll_down_history(rows);
                }
                self.dirty = true;
            }
            Action::ScrollLineUp => {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.grid_mut().scroll_up_history(1);
                }
                self.dirty = true;
            }
            Action::ScrollLineDown => {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.grid_mut().scroll_down_history(1);
                }
                self.dirty = true;
            }
            Action::ScrollToTop => {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    let limit = pane.grid().scrollback_len();
                    pane.grid_mut().set_scroll_offset(limit);
                }
                self.dirty = true;
            }
            Action::ScrollToBottom => {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.grid_mut().set_scroll_offset(0);
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
            Action::ResetFontSize => {
                let base = self.base_font_size;
                self.change_font_size_to(base);
            }
            Action::TogglePalette => {
                if self.palette.is_some() {
                    self.palette = None;
                } else {
                    self.palette = Some(
                        Palette::open(&self.window_keymap)
                            .with_query_history(self.palette_history.clone()),
                    );
                }
                self.dirty = true;
            }
            Action::ToggleHistoryPalette => {
                if self.palette.is_some() {
                    self.palette = None;
                } else {
                    self.palette = Some(
                        Palette::open_history().with_query_history(self.palette_history.clone()),
                    );
                }
                self.dirty = true;
            }
            Action::TogglePaneSwitcher => {
                if self.palette.is_some() {
                    self.palette = None;
                } else {
                    self.run_command("select_pane", focused);
                }
                self.dirty = true;
            }
            Action::ToggleSwoop => {
                if self.palette.is_some() {
                    self.palette = None;
                } else {
                    self.open_swoop(focused);
                }
                self.dirty = true;
            }
            Action::RunCommand(name) => self.run_command(&name, focused),
            Action::Ignore => {}
        }
    }

    /// Apply a Vim delete operator to the last prompt by sending the shell the
    /// equivalent readline edit. Only the live prompt line (the row holding the
    /// shell cursor) is editable; deletes aimed at scrollback history are ignored.
    /// Whether Vim operators may edit the shell's prompt line right now.
    ///
    /// False when the user has told us their shell is not in emacs mode: the
    /// operators are realized as readline chords (`Ctrl-A`, `Ctrl-K`, ...), and
    /// a shell in vi mode has those bound to something else entirely, so
    /// "delete a word" would run whatever `Ctrl-W` means there instead. A shell
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

    pub(crate) fn delete_on_prompt(&mut self, op: PromptDelete, focused: PaneId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = pane.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            // The cursor is on scrollback history, not the live prompt: only the
            // prompt line is editable, so report the attempt instead of silently
            // dropping it.
            self.set_error("Cannot delete: not on the editable prompt line");
            return;
        }
        if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
            shadow.record_normal_delete(op, nav_col, pty_col);
        }
        let bytes = prompt_delete_bytes(op, pty_col, nav_col);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Apply a Vim change operator on the prompt line: deletes the target span and enters Insert mode.
    pub(crate) fn change_on_prompt(&mut self, op: PromptDelete, focused: PaneId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = pane.grid().cursor();
        let (nav_row, _) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot change: not on the editable prompt line");
            return;
        }
        self.delete_on_prompt(op, focused);
        self.modes.insert(focused, Mode::Insert);
        self.selection = None;
        self.visual_anchor = None;
        self.dirty = true;
    }

    /// Replace the character under the cursor on the editable prompt line without entering Insert mode.
    pub(crate) fn replace_char_on_prompt(&mut self, ch: char, focused: PaneId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = pane.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot replace: not on the editable prompt line");
            return;
        }
        if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
            shadow.desync();
        }
        let bytes = prompt_edit::prompt_replace_char_bytes(pty_col, nav_col, ch);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Toggle the ASCII case of the character under the cursor and advance the cursor right.
    pub(crate) fn toggle_case_on_prompt(&mut self, focused: PaneId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = pane.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot toggle case: not on the editable prompt line");
            return;
        }
        let ch = pane.grid().cell(prompt_row, nav_col).map(|c| c.ch);
        if let Some(c) = ch {
            let toggled = if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                self.move_nav_cursor(CursorMove::Right, focused);
                return;
            };
            if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
                shadow.desync();
            }
            let bytes = prompt_edit::prompt_toggle_case_bytes(pty_col, nav_col, toggled);
            if let Some(pane) = self.panes.get_mut(&focused) {
                pane.write(&bytes);
            }
            self.move_nav_cursor(CursorMove::Right, focused);
            self.nav_resync_pending = true;
            self.dirty = true;
        }
    }

    /// Perform a configurable Insert-mode line edit (e.g. `Ctrl-Backspace` to
    /// delete the word before the cursor): fold it into the undo shadow and send
    /// the equivalent readline keystrokes to the shell.
    fn edit_on_prompt(&mut self, action: EditAction, focused: PaneId) {
        // The chord is forwarded either way: the user pressed it, and the shell
        // is entitled to interpret it. Only the shadow is skipped, because the
        // model of the line would be wrong under other bindings.
        let at_prompt = self.prompt_editing_enabled()
            && self
                .panes
                .get(&focused)
                .is_some_and(|pane| pane.is_at_prompt());
        if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
            shadow.apply_edit_action(action, at_prompt);
        }
        let bytes = edit_action_bytes(action);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
        self.dirty = true;
    }

    /// Apply a prompt undo (`undo == true`, `Ctrl-/`) or redo (`Ctrl-\`).
    ///
    /// Undo is delegated to the shell's own undo (`Ctrl-_`), the WezTerm approach:
    /// a single readline/ZLE operation, so the line repaints once instead of
    /// flashing through a multi-keystroke rebuild (which syntax-highlight /
    /// autosuggest plugins re-render at every step). The shadow pointer is advanced
    /// in step so a following redo has the right base.
    ///
    /// Shells have no portable redo, so redo is Winter's own: it rebuilds the
    /// recorded target line in one bracketed paste (see [`rebuild_line_bytes`]),
    /// which is robust to whatever state the shell's undo left the line in and
    /// still repaints once. A no-op off the prompt.
    fn prompt_history_apply(&mut self, focused: PaneId, undo: bool) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let at_prompt = self
            .panes
            .get(&focused)
            .is_some_and(|pane| pane.is_at_prompt());
        if !at_prompt {
            return;
        }
        if undo {
            if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
                shadow.step_back();
            }
            if let Some(pane) = self.panes.get_mut(&focused) {
                pane.write(&[READLINE_UNDO]);
            }
            self.dirty = true;
            return;
        }
        let Some(target) = self
            .prompt_shadows
            .get_mut(&focused)
            .and_then(|shadow| shadow.redo_target())
        else {
            return;
        };
        let bracketed = self
            .panes
            .get(&focused)
            .is_some_and(|pane| pane.bracketed_paste());
        let bytes = rebuild_line_bytes(&target, bracketed);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Enter Visual mode from Normal, toggle it off when the same kind is pressed
    /// again, or switch between charwise, linewise, and blockwise while staying in Visual.
    fn toggle_visual(&mut self, kind: VisualKind, focused: PaneId) {
        let mode = self.modes.get(&focused).copied().unwrap_or_default();
        match mode {
            Mode::Normal => {
                self.modes
                    .insert(focused, Mode::Normal.apply(ModeEvent::EnterVisual));
                self.visual_anchor = Some(self.nav_cursor(focused).unwrap_or((0, 0)));
                self.visual_kind = kind;
                self.update_visual_selection(focused);
            }
            Mode::Visual if self.visual_kind == kind => {
                // Same kind again leaves Visual, back to Normal.
                self.remember_visual(focused);
                self.modes
                    .insert(focused, Mode::Visual.apply(ModeEvent::EnterVisual));
                self.visual_anchor = None;
                self.selection = None;
            }
            Mode::Visual => {
                // Switch kind (charwise <-> linewise <-> blockwise), keeping the anchor.
                self.visual_kind = kind;
                self.update_visual_selection(focused);
            }
            Mode::Insert | Mode::BlockFocus => {}
        }
        self.dirty = true;
    }

    /// Visual `o`: put the cursor on the selection's other end (the anchor),
    /// so extending continues from there. The highlighted span is unchanged —
    /// it always runs anchor..cursor in either order.
    fn swap_visual_ends(&mut self, focused: PaneId) {
        if let (Some(anchor), Some(cursor)) = (self.visual_anchor, self.nav_cursor(focused)) {
            self.visual_anchor = Some(cursor);
            self.set_nav_cursor(focused, anchor);
            self.update_visual_selection(focused);
            self.dirty = true;
        }
    }

    /// Snapshot the live Visual selection as `gv`'s restore target, called at
    /// every point a Visual selection ends. Both ends are stored in absolute
    /// coordinates (see `winter_render::Grid::to_absolute_row`) so the snapshot
    /// still names the same text after the view has scrolled or grown; a pane
    /// without a live selection (no anchor or cursor) snapshots nothing.
    fn remember_visual(&mut self, focused: PaneId) {
        let Some((anchor_row, anchor_col)) = self.visual_anchor else {
            return;
        };
        let Some((cursor_row, cursor_col)) = self.nav_cursor(focused) else {
            return;
        };
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
        self.last_visual = Some(LastVisual {
            anchor: (grid.to_absolute_row(anchor_row), anchor_col),
            cursor: (grid.to_absolute_row(cursor_row), cursor_col),
            kind: self.visual_kind,
            pane: focused,
        });
    }

    /// `gv`: re-enter Visual mode with the last selection `remember_visual`
    /// captured — same pane, same kind, same two ends. The cursor end is
    /// revealed (centered if it scrolled off screen); the anchor end is clamped
    /// to the viewport when the span no longer fits, exactly like vim's own
    /// `gv` on a selection taller than the window.
    fn restore_visual(&mut self, focused: PaneId) {
        let Some(last) = self
            .last_visual
            .as_ref()
            .filter(|lv| lv.pane == focused)
            .copied()
        else {
            return;
        };
        self.modes
            .insert(focused, Mode::Normal.apply(ModeEvent::EnterVisual));
        self.visual_kind = last.kind;
        self.reveal_position(focused, last.cursor);
        // Anchor back to viewport coordinates under the (possibly moved) view.
        if let Some(pane) = self.panes.get(&focused) {
            let grid = pane.grid();
            let top = grid.scrollback_len() - grid.scroll_offset().min(grid.scrollback_len());
            let anchor_row = last
                .anchor
                .0
                .saturating_sub(top)
                .min(grid.rows().saturating_sub(1));
            self.visual_anchor = Some((anchor_row, last.anchor.1));
        }
        self.update_visual_selection(focused);
        self.dirty = true;
    }
}
