//! Winter's own commands, in one table: each one's id, what the palette
//! calls it, and what running it does. The palette lists them from here, a
//! key binding is checked against here, and [`App::run_command`] runs them
//! from here, so a command can be neither listed and dead nor live and
//! unlisted. An extension's commands come after them, by their own ids (see
//! `crate::extensions`).
//!
//! Most commands act on the window; the rest are the focused editor's to
//! run ([`Run::Editor`]), or a key binding's action the palette offers too
//! ([`Run::Action`]).

use crate::config::ThemeSetting;
use crate::model::editor::{FORWARDED_COMMANDS, LSP_COMMANDS, PROBLEM_COMMANDS, WRITING_COMMANDS};
use crate::model::input::{self, window_action_by_name, Keymap};
use crate::model::layout::{Direction, FocusDir, Rect, TabId};
use crate::model::mode::Mode;
use crate::model::paint::IconKind;
use crate::model::picker::{Picker, PickerEntry};
use crate::model::textedit::field::TextField;

use super::editor::compile::TRUST_TASKS_TITLE;
use super::input::overlay::{Overlay, RenameInput};
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// Every built-in command, in the order the palette lists them.
pub(crate) const BUILTIN: &[Builtin] = &[
    window("cd_recent", "CD: Recent Directory", |app, focused| {
        app.open_cd_recent(focused)
    }),
    window("close_pane", "Close Pane", |app, focused| {
        app.close_pane(focused)
    }),
    window(
        "toggle_dock",
        "View: Toggle the Dock (Bottom Pane)",
        |app, _| app.toggle_dock(),
    ),
    window("close_other_tabs", "Close Other Tabs", |app, focused| {
        app.close_other_tabs(focused)
    }),
    window("close_tab", "Close Tab", |app, focused| {
        app.close_tab(focused)
    }),
    window("rename_tab", "Rename Tab", |app, focused| {
        app.open_tab_rename(focused)
    }),
    window("copy_cwd", "Copy: Working Directory", |app, focused| {
        app.copy_pane_cwd(focused)
    }),
    window(
        "copy_scrollback",
        "Copy: Full Scrollback",
        |app, focused| app.copy_scrollback(focused),
    ),
    action("copy_selection", "Copy Selection"),
    window(
        "export_block_svg",
        "Export: Focused Block SVG to File",
        |app, focused| app.export_focused_block_svg(focused),
    ),
    window(
        "export_block_text",
        "Export: Focused Block Text to Clipboard",
        |app, focused| app.export_focused_block_text(focused),
    ),
    window(
        "export_scrollback_ansi",
        "Export: Scrollback (ANSI Colors)",
        |app, focused| app.export_scrollback_ansi(focused),
    ),
    window(
        "export_scrollback_editor",
        "Export: Scrollback to Editor",
        |app, focused| app.export_scrollback_editor(focused),
    ),
    window(
        "export_scrollback_html",
        "Export: Scrollback to HTML",
        |app, focused| app.export_scrollback_html(focused),
    ),
    window("dir_page", "Dir: Open Working Directory", |app, _| {
        app.open_dir_editor()
    }),
    window(
        "editor_show_scopes",
        "Developer: Inspect Tokens and Scopes",
        |app, focused| app.toggle_token_inspect(focused),
    ),
    window(
        "synctex_forward",
        "LaTeX: Show in PDF (SyncTeX Forward)",
        |app, focused| app.run_editor_command(focused, crate::model::editor::SYNCTEX_FORWARD),
    ),
    window(
        "synctex_forward_beside",
        "LaTeX: Show in PDF in the Other Pane (SyncTeX Forward)",
        |app, focused| {
            app.run_editor_command(focused, crate::model::editor::SYNCTEX_FORWARD_BESIDE)
        },
    ),
    window(
        "synctex_backward",
        "PDF: Go to LaTeX Source (SyncTeX Backward)",
        |app, focused| app.run_editor_command(focused, crate::model::editor::SYNCTEX_BACKWARD),
    ),
    editor("editor_toggle_swatches", "Editor: Toggle Color Swatches"),
    editor(
        "editor_copy_rich",
        "Editor: Copy as Rich Text (Syntax Colors)",
    ),
    window("copilot_sign_in", "Copilot: Sign In", |app, _| {
        app.copilot_sign_in()
    }),
    window("copilot_sign_out", "Copilot: Sign Out", |app, _| {
        app.copilot_sign_out()
    }),
    window("copilot_status", "Copilot: Show Status", |app, _| {
        app.copilot_status()
    }),
    window(
        "copilot_install",
        "Copilot: Install Language Server",
        |app, _| app.copilot_install(),
    ),
    window(
        "lsp_start",
        "LSP: Start the Language Server for This File",
        |app, _| app.start_lsp(),
    ),
    window("lsp_stop", "LSP: Stop the Language Server", |app, _| {
        app.stop_lsp()
    }),
    window(
        "lsp_restart",
        "LSP: Restart the Language Server",
        |app, _| app.restart_lsp(),
    ),
    window(
        "lsp_status",
        "LSP: Show Running Language Servers",
        |app, _| app.show_lsp_status(),
    ),
    editor("lsp_definition", "LSP: Go to Definition"),
    editor("lsp_declaration", "LSP: Go to Declaration"),
    editor("lsp_type_definition", "LSP: Go to Type Definition"),
    editor("lsp_implementation", "LSP: Go to Implementation"),
    editor("lsp_references", "LSP: Find References"),
    editor("lsp_hover", "LSP: Show Hover"),
    editor("lsp_signature_help", "LSP: Show Signature"),
    editor("lsp_rename", "LSP: Rename Symbol"),
    editor("lsp_code_action", "LSP: Quick Fix and Refactor..."),
    editor("lsp_format", "LSP: Format Document"),
    editor("problem_next_in_file", "Problems: Next Problem in File"),
    editor("problem_prev_in_file", "Problems: Previous Problem in File"),
    editor("problem_first", "Problems: First Problem in Project"),
    editor("problem_next", "Problems: Next Problem in Project"),
    editor("problem_prev", "Problems: Previous Problem in Project"),
    editor("problem_first_error", "Problems: First Error in Project"),
    editor("problem_next_error", "Problems: Next Error in Project"),
    editor("problem_prev_error", "Problems: Previous Error in Project"),
    editor(
        "problem_first_writing",
        "Problems: First Writing Issue in Project",
    ),
    editor(
        "problem_next_writing",
        "Problems: Next Writing Issue in Project",
    ),
    editor(
        "problem_prev_writing",
        "Problems: Previous Writing Issue in Project",
    ),
    editor("problem_list", "Problems: Problems in Project..."),
    editor("navigate_back", "Navigate: Back"),
    editor("navigate_forward", "Navigate: Forward"),
    editor("merge_next_conflict", "Merge: Next Conflict"),
    editor("merge_prev_conflict", "Merge: Previous Conflict"),
    editor("merge_resolve_conflict", "Merge: Resolve Conflict..."),
    editor("diff_buffer_disk", "Diff: Compare Buffer with File on Disk"),
    editor(
        "diff_buffer_clipboard",
        "Diff: Compare Buffer with Clipboard",
    ),
    editor(
        "diff_buffer_buffer",
        "Diff: Compare Buffer with Another Open Buffer...",
    ),
    editor("markdown_next_heading", "Markdown: Next Heading"),
    editor("markdown_prev_heading", "Markdown: Previous Heading"),
    editor("markdown_promote", "Markdown: Promote Heading or List Item"),
    editor("markdown_demote", "Markdown: Demote Heading or List Item"),
    editor("markdown_promote_subtree", "Markdown: Promote Subtree"),
    editor("markdown_demote_subtree", "Markdown: Demote Subtree"),
    #[cfg(feature = "preview")]
    window("markdown_preview", "Markdown: Preview Beside", |app, _| {
        app.open_markdown_preview()
    }),
    window(
        "clipboard_ring",
        "Clipboard: Paste from Clipboard Ring...",
        |app, _| app.open_clip_ring(),
    ),
    window("commander", "Dir: Open Two-Pane File Manager", |app, _| {
        app.open_commander()
    }),
    window("scratch", "File: Open Scratch File", |app, _| {
        app.open_scratch()
    }),
    window(
        "toggle_markup_markers",
        "Editor: Toggle Hide Markup Markers",
        |app, _| app.toggle_markup_markers(false),
    ),
    window(
        "toggle_always_hide_markup_markers",
        "Editor: Toggle Always Hide Markup Markers",
        |app, _| app.toggle_markup_markers(true),
    ),
    window("cargo_fix", "Rust: Run Cargo Fix", |app, _| app.cargo_fix()),
    editor("editor_recenter", "Editor: Recenter Top Bottom"),
    editor("editor_flash_line", "Editor: Flash Current Line"),
    editor("editor_goto_offset", "Editor: Go to Offset..."),
    editor("editor_show_offset", "Editor: Display Current Offset"),
    window(
        "clipboard_ring_clear",
        "Clipboard: Clear Clipboard Ring",
        |app, _| app.clear_clip_ring(),
    ),
    editor("diff_buffer_file", "Diff: Compare Buffer with File..."),
    editor(
        "diff_buffer_revision",
        "Diff: Compare Buffer with Revision...",
    ),
    editor(
        "diff_buffer_files",
        "Diff: Compare Buffer with Two Files...",
    ),
    editor("merge_file", "Merge: Merge Another File into This One..."),
    editor(
        "merge_conflicts_3way",
        "Merge: Resolve Conflicts in a 3-Way Merge",
    ),
    editor(
        "editor_insert_symbol",
        "Editor: Insert Math or Greek Symbol...",
    ),
    editor("dictionary_lookup", "Dictionary: Look Up Word at Cursor"),
    editor(
        "bibtex_sort_key_ascending",
        "BibTeX: Sort Entries by Citation Key (A-Z)",
    ),
    editor(
        "bibtex_sort_key_descending",
        "BibTeX: Sort Entries by Citation Key (Z-A)",
    ),
    editor(
        "bibtex_sort_year_ascending",
        "BibTeX: Sort Entries by Year (Earliest First)",
    ),
    editor(
        "bibtex_sort_year_descending",
        "BibTeX: Sort Entries by Year (Latest First)",
    ),
    editor(
        "editor_change_occurrences",
        "Editor: Change All Occurrences",
    ),
    editor(
        "editor_toggle_occurrences",
        "Editor: Toggle Occurrence Highlights",
    ),
    editor("editor_next_occurrence", "Editor: Next Occurrence"),
    editor("editor_prev_occurrence", "Editor: Previous Occurrence"),
    editor(
        "editor_definition_start",
        "Editor: Go to Start of Function/Module",
    ),
    editor(
        "editor_definition_end",
        "Editor: Go to End of Function/Module",
    ),
    editor(
        "editor_select_definition",
        "Editor: Select Current Function/Module",
    ),
    editor("editor_next_block", "Editor: Go to Next Fold"),
    editor("editor_prev_block", "Editor: Go to Previous Fold"),
    editor("markdown_bold", "Markdown: Toggle Bold"),
    editor("markdown_italic", "Markdown: Toggle Italic"),
    editor("markdown_code", "Markdown: Toggle Inline Code"),
    editor("markdown_math", "Markdown: Toggle Inline Math"),
    editor("markdown_task", "Markdown: Toggle Task Box"),
    editor("markdown_renumber", "Markdown: Number the List Again"),
    editor("markdown_word_count", "Markdown: Word Count"),
    editor("latex_new_environment", "LaTeX: Start an Environment..."),
    editor(
        "latex_rename_environment",
        "LaTeX: Rename the Enclosing Environment...",
    ),
    editor(
        "latex_toggle_display",
        "LaTeX: Toggle Display Math Delimiters",
    ),
    editor("latex_bold", "LaTeX: Toggle Bold"),
    editor("latex_italic", "LaTeX: Toggle Italic"),
    editor("latex_emph", "LaTeX: Toggle Emphasis"),
    editor("latex_typewriter", "LaTeX: Toggle Typewriter"),
    editor("latex_sans", "LaTeX: Toggle Sans Serif"),
    editor("latex_small_caps", "LaTeX: Toggle Small Caps"),
    editor("latex_word_count", "LaTeX: Word Count"),
    window("compile", "Compile: Run a Command...", |app, _| {
        app.compile_command()
    }),
    window("compile_task", "Compile: Run a Task...", |app, _| {
        app.compile_task()
    }),
    window(
        "recompile",
        "Compile: Run the Last Build Again",
        |app, _| app.recompile(),
    ),
    window("trust_project_tasks", TRUST_TASKS_TITLE, |app, _| {
        app.trust_project_tasks()
    }),
    window("latex_compile", "LaTeX: Compile Document", |app, _| {
        app.compile_latex()
    }),
    window(
        "next_error",
        "Errors: Open the Next File Reference in the Last Output",
        |app, focused| app.step_error(focused, true),
    ),
    window(
        "prev_error",
        "Errors: Open the Previous File Reference in the Last Output",
        |app, focused| app.step_error(focused, false),
    ),
    window("file_browser", "File: Browse Files", |app, _| {
        app.open_file_browser()
    }),
    window(
        "file_browser_root",
        "File: Browse Files as Root",
        |app, _| app.open_root_browser(),
    ),
    window(
        "file_browser_docker",
        "File: Browse Docker Containers",
        |app, _| app.open_target_browser(crate::remote::Kind::Docker),
    ),
    window("file_browser_ssh", "File: Browse SSH Hosts", |app, _| {
        app.open_target_browser(crate::remote::Kind::Ssh)
    }),
    window("focus_down", "Focus Pane Down", |app, _| {
        app.focus_toward(FocusDir::Down)
    }),
    window("git_page", "Git: Status", |app, _| app.open_git_editor()),
    window("grep_page", "Grep: Search Files", |app, _| {
        app.open_grep_editor()
    }),
    window("todo_page", "Todo: List Keywords in Project", |app, _| {
        app.open_todo_editor()
    }),
    window("focus_left", "Focus Pane Left", |app, _| {
        app.focus_toward(FocusDir::Left)
    }),
    window("focus_right", "Focus Pane Right", |app, _| {
        app.focus_toward(FocusDir::Right)
    }),
    window("focus_up", "Focus Pane Up", |app, _| {
        app.focus_toward(FocusDir::Up)
    }),
    action("font_decrease", "Font Size: Decrease"),
    action("font_increase", "Font Size: Increase"),
    action("font_reset", "Font Size: Reset"),
    window("jump", "Navigate: Jump to Target", |app, focused| {
        app.open_jump(focused)
    }),
    window(
        "outline",
        "Navigate: Outline of This File...",
        |app, focused| app.open_outline(focused),
    ),
    window("keys_page", "Keys View: Show Every Command", |app, _| {
        app.open_keys_editor()
    }),
    window("live_search", "Search: Project as You Type", |app, _| {
        app.open_live_search()
    }),
    window("tab_keys", "Help: Keys of This Tab", |app, focused| {
        app.open_tab_keys(focused)
    }),
    window("proc_page", "Processes: Monitor", |app, _| {
        app.open_proc_editor()
    }),
    window(
        "docker_page",
        "Docker: Containers, Images, Volumes, Networks",
        |app, _| app.open_docker_editor(),
    ),
    window("project_files", "File: Find in Project", |app, _| {
        app.open_project_files()
    }),
    window(
        "workspace_mark",
        "Workspace: Mark Folder as Workspace",
        |app, _| app.mark_workspace(),
    ),
    window("workspace_switch", "Workspace: Switch", |app, _| {
        app.open_workspace_switcher()
    }),
    window("workspace_add_folder", "Workspace: Add Folder", |app, _| {
        app.add_workspace_folder()
    }),
    window("workspace_rename", "Workspace: Rename", |app, _| {
        app.rename_workspace()
    }),
    window("workspace_remove", "Workspace: Remove", |app, _| {
        app.open_workspace_remover()
    }),
    window("find_file", "File: Find File", |app, _| {
        app.open_find_file()
    }),
    window("quota_refresh", "Quota: Refresh", |app, _| {
        app.refresh_quotas()
    }),
    window("quota_zai_key", "Quota: Set Z.ai API Key", |app, _| {
        app.open_overlay(Overlay::ZaiKey(TextField::line("")))
    }),
    window("sys_page", "System: Monitor", |app, _| {
        app.open_sys_editor()
    }),
    window("move_tab_down", "Move Tab to Pane Below", |app, _| {
        app.move_tab_in_direction(FocusDir::Down)
    }),
    window("move_tab_left", "Move Tab to Left Pane", |app, _| {
        app.move_tab_in_direction(FocusDir::Left)
    }),
    window("move_tab_right", "Move Tab to Right Pane", |app, _| {
        app.move_tab_in_direction(FocusDir::Right)
    }),
    window("move_tab_up", "Move Tab to Pane Above", |app, _| {
        app.move_tab_in_direction(FocusDir::Up)
    }),
    window(
        "mux_attach_remote",
        "Mux: Attach Remote Session...",
        |app, _| {
            app.open_picker(
                Picker::open_mux_attach_remote().with_query_history(app.recent.palette.clone()),
            )
        },
    ),
    window(
        "mux_detach_session",
        "Mux: Detach Current Session",
        |app, focused| app.detach_mux_session(focused),
    ),
    window(
        "mux_kill_session",
        "Mux: Kill Background Session...",
        |app, _| app.open_mux_kill_palette(),
    ),
    window(
        "mux_list_sessions",
        "Mux: List / Attach Background Sessions",
        |app, _| app.open_mux_palette(),
    ),
    window("mux_new_session", "Mux: New Session...", |app, _| {
        app.open_picker(Picker::open_mux_new().with_query_history(app.recent.palette.clone()))
    }),
    window("new_tab", "New Tab", |app, _| app.new_tab()),
    window("new_window", "New Window", |app, _| app.new_window()),
    window("next_block", "Next Block", |app, focused| {
        app.focus_block(input::BlockNav::Next, focused)
    }),
    window("next_tab", "Next Tab", |app, _| app.cycle_tab(true)),
    window("notifications", "Notifications: Show History", |app, _| {
        app.open_notifications()
    }),
    window("clear_notifications", "Notifications: Clear", |app, _| {
        app.clear_notifications()
    }),
    window(
        "open_under_cursor",
        "Open: Path or URL at Point",
        |app, focused| app.open_under_cursor(focused),
    ),
    window("open_with", "Open: This File With...", |app, focused| {
        app.open_with_list(focused)
    }),
    window("open_settings", "Settings", |app, _| app.open_settings()),
    action("paste_from_clipboard", "Paste from Clipboard"),
    window("prev_block", "Previous Block", |app, focused| {
        app.focus_block(input::BlockNav::Previous, focused)
    }),
    window("prev_tab", "Previous Tab", |app, _| app.cycle_tab(false)),
    window("quick_select", "Quick Select", |app, focused| {
        app.enter_quick_select(focused)
    }),
    window("recent_tab_back", "Recent Tab (Backward)", |app, _| {
        app.recent_tab(false)
    }),
    window("recent_tab_forward", "Recent Tab (Forward)", |app, _| {
        app.recent_tab(true)
    }),
    window("reload", "Reload Window", |app, _| {
        app.runtime.exit.reload_pending = true
    }),
    window(
        "reveal_terminal",
        "Terminal: Reveal Most Recent",
        |app, _| app.reveal_recent_terminal(),
    ),
    window("switch_to_buffer", "Switch to Buffer...", |app, _| {
        app.open_buffer_list()
    }),
    window("kill_buffer", "Kill Buffer", |app, focused| {
        app.kill_buffer(focused)
    }),
    window(
        "recent_list",
        "Go To Recent: Tabs, Files, Directories...",
        |app, _| app.open_recent_list(),
    ),
    window("search", "Search Blocks", |app, _| {
        app.search.query = Some(String::new())
    }),
    window("select_pane", "Go to Pane", |app, _| app.open_pane_picker()),
    action("scroll_page_up", "Scroll Page Up"),
    action("scroll_page_down", "Scroll Page Down"),
    action("scroll_line_up", "Scroll Line Up"),
    action("scroll_line_down", "Scroll Line Down"),
    action("scroll_to_top", "Scroll to Top"),
    action("scroll_to_bottom", "Scroll to Bottom"),
    window("split_horizontal", "Split Horizontal", |app, _| {
        app.split_pane(Direction::Horizontal)
    }),
    window("split_vertical", "Split Vertical", |app, _| {
        app.split_pane(Direction::Vertical)
    }),
    window(
        "split_horizontal_terminal",
        "Split Horizontal: Terminal",
        |app, _| app.split_pane_with_terminal(Direction::Horizontal),
    ),
    window(
        "split_vertical_terminal",
        "Split Vertical: Terminal",
        |app, _| app.split_pane_with_terminal(Direction::Vertical),
    ),
    window("stop_tasks", "Stop Running Tasks", |app, focused| {
        app.stop_tasks(focused)
    })
    .with_hint("C-g"),
    window("swoop", "Navigate: Buffer Swoop", |app, focused| {
        app.open_swoop(focused)
    }),
    window("theme_auto", "Theme: Auto", |app, _| {
        app.set_theme(ThemeSetting::Auto)
    }),
    window("theme_dark", "Theme: Dark", |app, _| {
        app.set_theme(ThemeSetting::Dark)
    }),
    window("theme_light", "Theme: Light", |app, _| {
        app.set_theme(ThemeSetting::Light)
    }),
    window("theme_new", "Theme: Create New...", |app, _| {
        app.open_overlay(Overlay::ThemeName(TextField::line("")))
    }),
    window("toggle_fold", "Toggle Fold", |app, focused| {
        app.toggle_block_folds(focused)
    }),
    window(
        "toggle_mode",
        "Toggle Mode (Insert/Normal)",
        |app, focused| app.toggle_terminal_mode(focused),
    ),
    window("toggle_pane_zoom", "Zoom Pane", |app, _| {
        app.toggle_pane_zoom()
    }),
    window(
        "toggle_rainbow_parens",
        "View: Toggle Rainbow Parentheses",
        |app, _| app.toggle_rainbow_parens(),
    ),
    window(
        "toggle_sentence_highlight",
        "View: Toggle Sentence Highlight",
        |app, _| app.toggle_sentence_highlight(),
    ),
    action("macro_start_recording", "Macro: Start Recording"),
    action("macro_stop_recording", "Macro: Stop Recording"),
    action("macro_replay", "Macro: Replay Last Macro"),
    editor("query_replace", "Edit: Query Replace..."),
    window("yank_block", "Yank Block Source", |app, focused| {
        app.yank_block_source(focused)
    }),
];

/// Other names some commands answer to, each beside the command's id.
const ALIASES: &[(&str, &str)] = &[
    ("keys_view", "keys_page"),
    ("move_tab_down_pane", "move_tab_down"),
    ("move_tab_left_pane", "move_tab_left"),
    ("move_tab_right_pane", "move_tab_right"),
    ("move_tab_to_down_pane", "move_tab_down"),
    ("move_tab_to_left_pane", "move_tab_left"),
    ("move_tab_to_right_pane", "move_tab_right"),
    ("move_tab_to_up_pane", "move_tab_up"),
    ("move_tab_up_pane", "move_tab_up"),
    ("next-tab", "next_tab"),
    ("prev-tab", "prev_tab"),
    ("previous-tab", "prev_tab"),
    ("previous_tab", "prev_tab"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// One of Winter's own commands.
pub(crate) struct Builtin {
    /// The chord the palette shows beside it, where no binding says one.
    pub(crate) hint: &'static str,
    /// What the palette, the menus, and a key binding name it by.
    pub(crate) id: &'static str,
    /// What running it does.
    pub(crate) run: Run,
    /// What the palette calls it.
    pub(crate) title: &'static str,
}

/// What running a built-in command does.
#[derive(Clone, Copy)]
pub(crate) enum Run {
    /// The key binding action of the same name, as its chord runs it.
    Action,
    /// The focused editor runs it, by its id.
    Editor,
    /// This, on the window, with the tab in front.
    Window(fn(&mut App, TabId)),
}

// ========================================================================
// Builtin
// ========================================================================

impl Builtin {
    /// The same command, with `hint` shown beside it.
    const fn with_hint(self, hint: &'static str) -> Self {
        Self { hint, ..self }
    }
}

// ========================================================================
// App: commands
// ========================================================================

impl App {
    /// Run the command `id` names, Winter's own or an extension's, with the
    /// tab `focused` in front: what the palette, the menus, and key
    /// bindings run.
    pub(in crate::window) fn run_command(&mut self, id: &str, focused: TabId) {
        let Some(builtin) = find(id) else {
            self.run_unlisted(id, focused);
            return;
        };
        match builtin.run {
            Run::Action => {
                if let Some(action) = window_action_by_name(builtin.id) {
                    self.handle_action(action, focused);
                }
            }
            Run::Editor => self.run_editor_command(focused, builtin.id),
            Run::Window(run) => run(self, focused),
        }
    }

    /// Run the command `id` names that the palette does not list: one the
    /// focused editor knows, a key binding's action, or an extension's.
    fn run_unlisted(&mut self, id: &str, focused: TabId) {
        if is_editor_command(id) {
            self.run_editor_command(focused, id);
        } else if let Some(action) = window_action_by_name(id) {
            self.handle_action(action, focused);
        } else if let Some(command) = crate::extensions::command(id) {
            self.run_extension_command(command, focused);
        }
    }

    /// Empty the clipboard ring.
    fn clear_clip_ring(&mut self) {
        self.input.clipboards.clear_ring();
        self.set_notice("clipboard ring cleared");
    }

    /// Detach the terminal in tab `focused` from its mux session, which
    /// keeps running.
    fn detach_mux_session(&mut self, focused: TabId) {
        let session = self
            .open_tabs
            .terminal(&focused)
            .and_then(|terminal| terminal.mux_session().map(str::to_string));
        match session {
            Some(name) => {
                if self.layout().members().len() <= 1 {
                    self.set_notice(
                        "cannot detach the only tab: close the window or open another tab",
                    );
                } else {
                    self.set_notice(format!("detached from mux session '{name}'"));
                    // Closing the tab drops the mux client, which
                    // detaches server-side; the session keeps running.
                    self.close_tab(focused);
                }
            }
            None => self.set_notice("this pane is not attached to a mux session"),
        }
    }

    /// Move the focus to the pane beside the focused one toward `dir`.
    fn focus_toward(&mut self, dir: FocusDir) {
        let viewport = self.viewport_rect();
        let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
        self.layout_mut().focus_in_direction(dir, layout_vp);
    }

    /// Pick from the directories the terminal in tab `focused` has run
    /// commands in, the latest first.
    fn open_cd_recent(&mut self, focused: TabId) {
        let dirs = self
            .open_tabs
            .terminal(&focused)
            .map(|p| {
                let mut seen = std::collections::HashSet::new();
                p.scrollback()
                    .blocks()
                    .iter()
                    .rev()
                    .filter_map(|b| b.cwd.as_deref())
                    .filter(|&d| seen.insert(d.to_string()))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.open_picker(
            Picker::open_recent_dirs(dirs).with_query_history(self.recent.palette.clone()),
        );
    }

    /// Pick a tab from every pane, each named after its pane.
    fn open_pane_picker(&mut self) {
        let mut panes_list = Vec::new();
        for (pane_index, &shown) in self.layout().shown_tabs().iter().enumerate() {
            // Name each tab after the pane holding it ("<pane
            // number>: <title>"); its place on that pane's strip goes
            // on the right.
            for (tab_index, tab_id) in self.layout().tabs_in_pane(shown).into_iter().enumerate() {
                let label = format!("{}: {}", pane_index + 1, self.tab_title(tab_id));
                panes_list.push(
                    PickerEntry::new(tab_id.0.to_string(), label, (tab_index + 1).to_string())
                        .with_icon(self.tab_icon_kind(tab_id)),
                );
            }
        }
        self.open_picker(
            Picker::open_panes(panes_list).with_query_history(self.recent.palette.clone()),
        );
    }

    /// Ask for a new name for tab `focused`.
    fn open_tab_rename(&mut self, focused: TabId) {
        let text = self.tabs.name(focused).unwrap_or_default().to_string();
        self.open_overlay(Overlay::Rename(RenameInput::new(focused, &text)));
    }

    /// Use the theme `theme` from now on.
    fn set_theme(&mut self, theme: ThemeSetting) {
        self.config.theme = theme;
        self.rebuild_theme();
    }

    /// Fold every block of the terminal in tab `focused`, or unfold them
    /// all where any is folded.
    fn toggle_block_folds(&mut self, focused: TabId) {
        if let Some(tab) = self.open_tabs.terminal_tab_mut(&focused) {
            let folded = &mut tab.folded_blocks;
            if folded.is_empty() {
                folded.insert(0);
            } else {
                folded.clear();
            }
        }
        self.dirty = true;
    }

    /// Zoom the focused pane to fill the window, or back.
    pub(in crate::window) fn toggle_pane_zoom(&mut self) {
        self.layout_mut().toggle_zoom();
        // Zooming changes the focused pane's rect, so the new size goes to
        // the PTY: without it btop keeps reporting the old (small) split
        // dimensions after maximizing.
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Switch the terminal in tab `focused` between Insert and Normal; an
    /// editor tab has no mode to switch, and is left as it is.
    fn toggle_terminal_mode(&mut self, focused: TabId) {
        // An editor tab has no mode to toggle, and is left as it is.
        if let Some(mode) = self.open_tabs.mode(&focused) {
            let new_mode = match mode {
                Mode::Insert => Mode::Normal,
                Mode::Normal | Mode::Visual => Mode::Insert,
            };
            self.open_tabs.set_mode(&focused, new_mode);
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The built-in command `id` names, by its id or another name it answers to.
pub(crate) fn find(id: &str) -> Option<&'static Builtin> {
    let id = ALIASES
        .iter()
        .find(|(alias, _)| *alias == id)
        .map_or(id, |(_, named)| named);
    BUILTIN.iter().find(|builtin| builtin.id == id)
}

/// Whether `id` is a command the focused editor runs.
fn is_editor_command(id: &str) -> bool {
    [
        FORWARDED_COMMANDS,
        WRITING_COMMANDS,
        LSP_COMMANDS,
        PROBLEM_COMMANDS,
    ]
    .iter()
    .any(|names| names.contains(&id))
}

/// A command acting on the window.
const fn window(id: &'static str, title: &'static str, run: fn(&mut App, TabId)) -> Builtin {
    Builtin {
        hint: "",
        id,
        run: Run::Window(run),
        title,
    }
}

/// A command the focused editor runs.
const fn editor(id: &'static str, title: &'static str) -> Builtin {
    Builtin {
        hint: "",
        id,
        run: Run::Editor,
        title,
    }
}

/// A key binding's action, offered in the palette too.
const fn action(id: &'static str, title: &'static str) -> Builtin {
    Builtin {
        hint: "",
        id,
        run: Run::Action,
        title,
    }
}

/// Every built-in command, labelled and annotated with the chord `keymap`
/// binds it to.
pub(crate) fn builtin_commands(keymap: &Keymap) -> Vec<PickerEntry> {
    let builtin = BUILTIN.iter().map(|builtin| {
        let shortcut = match builtin.hint {
            "" => keymap.chord_hint(builtin.id),
            hint => hint.to_string(),
        };
        let entry = PickerEntry::new(builtin.id, builtin.title, shortcut);
        match command_icon(builtin.id) {
            Some(icon) => entry.with_icon(icon),
            None => entry,
        }
    });
    // The extensions' commands after Winter's own, under their titles.
    let extensions = crate::extensions::registry().commands().iter().map(|command| {
        PickerEntry::new(command.id.as_str(), command.title.as_str(), keymap.chord_hint(&command.id))
    });
    builtin.chain(extensions).collect()
}

/// The icon for a command that opens something with an icon of its own: a
/// folder, an editor, a terminal. Every other command is drawn without one, since
/// an icon shared by them all would say nothing a row does not.
fn command_icon(action: &str) -> Option<IconKind> {
    match action {
        "cd_recent"
        | "copy_cwd"
        | "file_browser"
        | "file_browser_root"
        | "file_browser_docker"
        | "file_browser_ssh"
        | "find_file"
        | "project_files"
        | "workspace_add_folder"
        | "workspace_mark"
        | "workspace_remove"
        | "workspace_rename"
        | "workspace_switch" => Some(IconKind::Dir {
            expanded: false,
            name: String::new(),
        }),
        "dir_page" => Some(IconKind::Editor { kind: "dir" }),
        "git_page" => Some(IconKind::Editor { kind: "git" }),
        "docker_page" => Some(IconKind::Editor { kind: "docker" }),
        "grep_page" => Some(IconKind::Editor { kind: "grep" }),
        "todo_page" => Some(IconKind::Editor { kind: "todo" }),
        "new_tab"
        | "reveal_terminal"
        | "split_horizontal_terminal"
        | "split_vertical_terminal" => Some(IconKind::Terminal),
        _ => None,
    }
}

/// What the palette calls the command `action`, Winter's own or an
/// extension's, or `None` when there is no such command.
pub(crate) fn command_label(action: &str) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|builtin| builtin.id == action)
        .map(|builtin| builtin.title)
        .or_else(|| crate::extensions::command(action).map(|command| command.title.as_str()))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::window::editor::EditorKind;

    #[test]
    fn test_each_command_is_listed_once_and_runs_as_its_kind_says() {
        let mut seen = std::collections::HashSet::new();
        for builtin in BUILTIN {
            assert!(seen.insert(builtin.id), "{} is listed twice", builtin.id);
            match builtin.run {
                Run::Action => assert!(
                    window_action_by_name(builtin.id).is_some(),
                    "{} is no key binding's action",
                    builtin.id
                ),
                Run::Editor => assert!(
                    is_editor_command(builtin.id),
                    "{} is no editor's command",
                    builtin.id
                ),
                Run::Window(_) => {}
            }
        }
    }

    #[test]
    fn test_an_editor_s_command_chosen_in_the_palette_reaches_the_editor_in_front() {
        let dir = std::env::temp_dir().join(format!("winter-commands-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "text\n").expect("file");
        let mut app = App::new();
        let editor = crate::editors::text::TextEditor::new(file, None).expect("opens");
        let tab = app.show_editor(EditorKind::Text, Box::new(editor));
        assert!(app.chrome.overlay.is_none());
        app.run_command("query_replace", tab);
        assert!(app.chrome.overlay.is_some(), "it asks what to replace");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_an_alias_finds_the_command_it_names() {
        for (alias, id) in ALIASES {
            assert_eq!(find(alias).map(|builtin| builtin.id), Some(*id), "{alias}");
            assert!(find(alias).is_some_and(|builtin| builtin.id != *alias));
        }
        assert!(find("no_such_command").is_none());
    }

    #[test]
    fn test_every_command_icon_names_an_editor_with_artwork() {
        // The command table names editors by string, apart from the kinds the
        // icons are keyed by: a misspelt one would draw the generic file icon
        // beside the command rather than fail anywhere.
        let commands = builtin_commands(&Keymap::default());
        let editors: Vec<&str> = commands
            .iter()
            .filter_map(|entry| match entry.icon.as_ref()? {
                IconKind::Editor { kind } => Some(*kind),
                IconKind::Dir { .. }
                | IconKind::File { .. }
                | IconKind::Git { .. }
                | IconKind::Gutter { .. }
                | IconKind::Program { .. }
                | IconKind::Symbol { .. }
                | IconKind::Terminal => None,
            })
            .collect();
        assert!(!editors.is_empty());
        for kind in editors {
            assert_ne!(
                crate::icons::name_for(&IconKind::Editor { kind }),
                crate::icons::name_for(&IconKind::Editor { kind: "" }),
                "{kind} draws the generic file icon"
            );
        }
    }
}
