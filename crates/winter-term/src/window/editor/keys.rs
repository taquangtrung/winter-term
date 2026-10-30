//! Offering keys, clicks, and scrolls to the focused editor before the window
//! keymap sees them, and acting on what the editor answers.

use std::path::{Path, PathBuf};

use winter_lsp::protocol::TextEdit;
use crate::model::editor::{EditorOutcome, EditorRequest, PromptMode};
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::layout::TabId;
use crate::model::paint::Point;
use crate::model::picker::Picker;
use crate::model::textedit::modal::EditMode;

use crate::window::input::overlay::{Overlay, PickerOverlay};
use crate::window::runtime::jobs::STOP_TASKS_KEY;
use crate::window::App;

use super::prompt::{ActivePick, ActivePrompt};
use super::EditorKind;
use crate::editors::diff::DiffEditor;
use crate::editors::text::TextEditor;

// ========================================================================
// App: editor input
// ========================================================================

impl App {
    /// Offer `key` to the editor covering `tab_id`. Returns whether the key was
    /// spent, so a key the editor declines still reaches the ordinary keymap.
    pub(in crate::window) fn offer_key_to_editor(&mut self, tab_id: TabId, key: &Key) -> bool {
        // The editor's running tasks stop on the stop key, ahead of any use the
        // editor makes of it (the PDF editor's `C-g`), but only while there is
        // something to stop.
        if *key == STOP_TASKS_KEY && self.host.jobs.is_busy_for(tab_id) {
            self.stop_tasks(tab_id);
            return true;
        }
        // Wrap belongs to the pane, not the editor: every editor gets the toggle,
        // ahead of its own keys, so no editor has to claim it itself.
        if key.chord() == (Mods::ALT, KeyCode::Char('z')) {
            if let Some(slot) = self.open_tabs.editor_mut(&tab_id) {
                slot.view.wrap = !slot.view.wrap;
                self.dirty = true;
                return true;
            }
        }
        // The editor binds the plain key; `S-RET` is a different key, and `s-x`
        // is none an editor binds. Both are the window's to match.
        let windows_key = key.is_shifted_plain_key() || key.sup; // key-exact: routing, not matching
        if windows_key {
            return false;
        }
        let neighbors: Vec<PathBuf> = self
            .layout()
            .shown_tabs()
            .into_iter()
            .filter(|pane| *pane != tab_id)
            .filter_map(|pane| {
                self.open_tabs
                    .editor(&pane)
                    .and_then(|slot| slot.editor.root_dir())
            })
            .collect();
        let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
            return false;
        };
        slot.editor.set_neighbors(neighbors);
        let outcome = slot.editor.on_key(&key.canonical());
        if slot.editor.picks_rows()
            && slot.editor.status().mode != Some(EditMode::Insert)
            && is_row_step_key(key)
        {
            slot.view.column.place(0);
        }
        self.act_on_editor_outcome(tab_id, outcome)
    }

    /// Offer a pointer press or drag to the editor covering `tab_id`, as the
    /// row and column of the pane it landed on. Returns whether the editor took
    /// it, so a click it declines still reaches the pane underneath.
    pub(in crate::window) fn offer_mouse_to_editor(&mut self, tab_id: TabId, at: Point) -> bool {
        let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.editor.on_mouse(slot.view.skipped.editor_point(at));
        self.act_on_editor_outcome(tab_id, outcome)
    }

    /// Offer a double-click to the editor covering `tab_id`, after the press
    /// ending it went through [`Self::offer_mouse_to_editor`].
    pub(in crate::window) fn offer_double_click_to_editor(&mut self, tab_id: TabId, at: Point) {
        let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
            return;
        };
        let outcome = slot.editor.on_double_click(slot.view.skipped.editor_point(at));
        self.act_on_editor_outcome(tab_id, outcome);
    }

    /// Offer a touchpad's scroll, in pixels, to the editor covering `tab_id`.
    pub(in crate::window) fn offer_pixel_scroll_to_editor(
        &mut self,
        tab_id: TabId,
        pixels: f32,
    ) -> bool {
        let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.editor.on_scroll_pixels(pixels);
        self.act_on_editor_outcome(tab_id, outcome)
    }

    /// Offer a turn of the wheel to the editor covering `tab_id`.
    pub(in crate::window) fn offer_scroll_to_editor(
        &mut self,
        tab_id: TabId,
        lines: isize,
    ) -> bool {
        let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.editor.on_scroll(lines);
        self.act_on_editor_outcome(tab_id, outcome)
    }

    /// Offer a press of the mouse's back (`back`) or forward side button to
    /// the editor covering `tab_id`.
    pub(in crate::window) fn offer_side_button_to_editor(
        &mut self,
        tab_id: TabId,
        back: bool,
    ) -> bool {
        let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.editor.on_side_button(back);
        self.act_on_editor_outcome(tab_id, outcome)
    }

    /// Tell the editor covering `tab_id` the cell the pointer rests on, or
    /// that it has left (`None`).
    pub(in crate::window) fn offer_hover_to_editor(
        &mut self,
        tab_id: TabId,
        at: Option<Point>,
    ) -> bool {
        let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.editor.on_hover(at.map(|at| slot.view.skipped.editor_point(at)));
        self.act_on_editor_outcome(tab_id, outcome)
    }

    /// Put `text` on the clipboard for an editor and say `notice`, or say the
    /// clipboard could not be reached.
    fn copy_for_editor(&mut self, text: &str, notice: String) {
        let copied = self
            .input.clipboard()
            .and_then(|clipboard| clipboard.set_text(text).ok())
            .is_some();
        // Kept either way: a paste falls back on the ring where the system has
        // no clipboard to give.
        self.input.remember_copy(text);
        if copied {
            self.set_notice(notice);
        } else {
            self.set_error("clipboard unavailable");
        }
    }

    /// Do what an editor asked of the window beyond its own file.
    fn carry_out_request(&mut self, request: EditorRequest) {
        match request {
            EditorRequest::Open(target) => self.open_path_editor(target),
            EditorRequest::Edit { edits, path } => self.edit_file(&path, &edits),
        }
    }

    /// Make `edits` to the file at `path` in the buffer holding it, shown or
    /// buried, else in a buffer opened for it and left buried, unsaved, as
    /// the edits were made to it rather than to the disk.
    pub(in crate::window) fn edit_file(&mut self, path: &Path, edits: &[TextEdit]) {
        let held = self
            .open_tabs
            .editors()
            .find(|(_, slot)| slot.kind() == EditorKind::Text && slot.editor.holds_file(path))
            .map(|(tab, _)| *tab);
        let tab = match held {
            Some(tab) => tab,
            None => match TextEditor::new(path.to_path_buf(), None) {
                Ok(editor) => {
                    let tab = self.alloc_tab_id();
                    self.open_tabs
                        .insert_editor(tab, EditorKind::Text, Box::new(editor));
                    self.open_tabs.bury(tab);
                    tab
                }
                Err(e) => {
                    self.set_error(format!("{}: {e}", path.display()));
                    return;
                }
            },
        };
        if let Some(slot) = self.open_tabs.editor_mut(&tab) {
            slot.editor.with_kind_mut(|text: &mut TextEditor| text.apply_text_edits(edits));
        }
    }

    /// Put what a delete took on the clipboard and in the clipboard ring,
    /// saying nothing: a clipboard the system cannot give still leaves the
    /// ring, which a paste falls back on.
    fn copy_quietly(&mut self, text: &str) {
        if let Some(clipboard) = self.input.clipboard() {
            let _ = clipboard.set_text(text);
        }
        self.input.remember_copy(text);
    }

    /// Put `rows` on the clipboard as rich text in the theme's colors, with
    /// `text` for what takes only plain text.
    fn copy_styled(&mut self, rows: &[crate::model::paint::Row], text: String) {
        let theme = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.theme())
            .cloned()
            .unwrap_or_default();
        let html = crate::window::frame::frame_grid::styled_html(rows, &theme);
        let lines = rows.len();
        let copied = self
            .input.clipboard()
            .and_then(|clipboard| clipboard.set().html(html, Some(text)).ok())
            .is_some();
        match copied {
            true => self.set_notice(format!("copied {lines} lines as rich text")),
            false => self.set_error("clipboard unavailable"),
        }
    }

    /// Carry out what an editor asked for. Returns whether the input that produced
    /// it was spent.
    pub(in crate::window) fn act_on_editor_outcome(
        &mut self,
        tab_id: TabId,
        outcome: EditorOutcome,
    ) -> bool {
        let notice = self.open_tabs.editor_mut(&tab_id).and_then(|slot| {
            let source = slot.kind().title();
            slot.editor
                .take_notice()
                .map(|notice| notice.from_source(source))
        });
        if let Some(notice) = notice {
            self.notify(notice);
        }
        let search = self
            .open_tabs
            .editor_mut(&tab_id)
            .and_then(|slot| slot.editor.take_search());
        if let Some(search) = search {
            self.adopt_editor_search(search);
        }
        let requests = self
            .open_tabs
            .editor_mut(&tab_id)
            .map(|slot| slot.editor.take_requests())
            .unwrap_or_default();
        for request in requests {
            self.carry_out_request(request);
        }
        let calls = self
            .open_tabs
            .editor_mut(&tab_id)
            .map(|slot| slot.editor.take_extension_calls())
            .unwrap_or_default();
        for (extension, call) in calls {
            self.as_extension(extension, tab_id, call);
        }
        match outcome {
            EditorOutcome::Close => {
                self.close_editor(tab_id);
                true
            }
            EditorOutcome::Consumed => {
                self.dirty = true;
                true
            }
            EditorOutcome::Ignored => false,
            EditorOutcome::OpenExternal(path) => {
                match ::open::that_detached(&path) {
                    Ok(()) => self.set_notice(format!("opened {}", path.display())),
                    Err(e) => self.set_error(format!("could not open {}: {e}", path.display())),
                }
                true
            }
            EditorOutcome::OpenPath(target) => {
                self.open_path_editor(target);
                true
            }
            EditorOutcome::Build(build) => {
                self.on_build_start(tab_id, build);
                true
            }
            EditorOutcome::Compare(request) => {
                self.open_comparison(request);
                true
            }
            EditorOutcome::GitView(view) => {
                self.open_git_view(view);
                true
            }
            EditorOutcome::Diff(request) => {
                self.show_editor(EditorKind::Diff, Box::new(DiffEditor::new(request)));
                true
            }
            EditorOutcome::CopyStyled { rows, text } => {
                self.copy_styled(&rows, text);
                true
            }
            EditorOutcome::OpenBeside(target) => {
                self.open_beside(tab_id, target);
                true
            }
            EditorOutcome::Wrote(path) => {
                self.dirty = true;
                self.on_file_written(&path);
                true
            }
            EditorOutcome::ShowInPdf { beside, mark, path } => {
                self.show_in_pdf(path, mark, beside);
                true
            }
            EditorOutcome::SpawnEditor(target) => {
                self.open_file_in_new_tab(target.path, target.line);
                true
            }
            EditorOutcome::CancelJobs => {
                self.host.jobs.cancel_for(tab_id);
                self.dirty = true;
                true
            }
            EditorOutcome::Job(request) => {
                self.host.jobs.spawn(tab_id, request);
                true
            }
            EditorOutcome::Launch(launch) => {
                self.launch_from(tab_id, launch);
                true
            }
            EditorOutcome::Spawn(request) => {
                self.spawn_in_tab(request);
                true
            }
            EditorOutcome::Paste => {
                // The system clipboard, else the newest copy this window made,
                // where the system has no clipboard to give.
                let text = self
                    .clipboard_text()
                    .or_else(|| self.input.clipboards.latest().map(str::to_string))
                    .unwrap_or_default();
                let Some(slot) = self.open_tabs.editor_mut(&tab_id) else {
                    return true;
                };
                let outcome = slot.editor.on_paste(text);
                self.act_on_editor_outcome(tab_id, outcome)
            }
            EditorOutcome::Yank(value) => {
                let notice = format!("copied {value}");
                self.copy_for_editor(&value, notice);
                true
            }
            EditorOutcome::Cut(text) => {
                self.copy_quietly(&text);
                true
            }
            EditorOutcome::Copy { notice, text } => {
                self.copy_for_editor(&text, notice);
                true
            }
            EditorOutcome::Prompt(request) => {
                // A new search counts its own matches, not the last one's.
                if matches!(request.mode, PromptMode::Search { .. }) {
                    self.search.match_index = 0;
                    self.search.match_total = 0;
                }
                self.open_overlay(Overlay::EditorPrompt(ActivePrompt::new(tab_id, request)));
                true
            }
            EditorOutcome::Pick(request) => {
                // The picker is the list picker Winter already has: what an
                // editor adds is where the choice goes when it is made.
                self.open_overlay(Overlay::Picker(PickerOverlay {
                    picker: Picker::open_pick(request.items, request.icons),
                    pick: Some(ActivePick {
                        label: request.label,
                        placement: request.placement,
                        tab: tab_id,
                        tag: request.tag,
                    }),
                }));
                true
            }
            EditorOutcome::RunAction(action) => {
                // The editor is done once it has named the command, and the
                // command runs against the tab shown in its place.
                self.close_editor(tab_id);
                let focused = self.layout().focused();
                self.run_command(&action, focused);
                true
            }
            EditorOutcome::RunActionHere(action) => {
                self.run_command(&action, tab_id);
                true
            }
            EditorOutcome::Macro(step) => {
                self.run_macro_step(step);
                true
            }
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Whether `key` is a vertical motion, stepping from row to row.
fn is_row_step_key(key: &Key) -> bool {
    matches!(
        key.chord(),
        (
            Mods::NONE,
            KeyCode::Char('j' | 'k' | 'G' | 'g')
                | KeyCode::Down
                | KeyCode::Up
                | KeyCode::PageDown
                | KeyCode::PageUp
                | KeyCode::Home
                | KeyCode::End
        ) | (Mods::CTRL, KeyCode::Char('d' | 'u' | 'f' | 'b'))
            | (Mods::ALT, KeyCode::Char('n' | 'p' | 'u' | 'd'))
    )
}
