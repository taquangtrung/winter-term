//! The names of the commands editors take from the palette and the keymap.

// ========================================================================
// Constants
// ========================================================================

/// The editor command name that toggles token / scope inspection.
pub const SHOW_SCOPES: &str = "show_scopes";

/// The editor command name that puts token / scope inspection away, for
/// the window dismissing it: an Escape sent instead could also leave a mode
/// or drop a selection.
pub const HIDE_SCOPES: &str = "hide_scopes";

/// The editor command name that shows where the cursor's line of LaTeX was
/// set in the PDF built from it (SyncTeX forward).
pub const SYNCTEX_FORWARD: &str = "synctex_forward";

/// The same, the PDF shown in the other pane.
pub const SYNCTEX_FORWARD_BESIDE: &str = "synctex_forward_beside";

/// The PDF viewer command name that goes back to the LaTeX source of the spot
/// under the cursor or reading line (SyncTeX backward).
pub const SYNCTEX_BACKWARD: &str = "synctex_backward";

/// The editor command name that writes every file being edited to disk,
/// what a build asks for before it starts.
pub const SAVE_ALL: &str = "save_all";

/// The text editor's command that steps to the next merge conflict.
pub const MERGE_NEXT: &str = "merge_next_conflict";

/// The text editor's command that steps back to the previous merge conflict.
pub const MERGE_PREV: &str = "merge_prev_conflict";

/// The text editor's command that offers what of the merge conflict under
/// the cursor to keep.
pub const MERGE_RESOLVE: &str = "merge_resolve_conflict";

/// The text editor's command that compares the buffer with the file on disk
/// it was read from.
pub const DIFF_DISK: &str = "diff_buffer_disk";

/// The text editor's command that compares the buffer with the clipboard.
pub const DIFF_CLIPBOARD: &str = "diff_buffer_clipboard";

/// The text editor's command that compares the buffer with another one open.
pub const DIFF_BUFFER: &str = "diff_buffer_buffer";

/// The text editor's commands for writing Markdown and LaTeX, by the name
/// its `run_command` knows each by.
pub const WRITING_COMMANDS: &[&str] = &[
    "markdown_next_heading",
    "markdown_prev_heading",
    "markdown_promote",
    "markdown_demote",
    "markdown_promote_subtree",
    "markdown_demote_subtree",
    "markdown_bold",
    "markdown_italic",
    "markdown_code",
    "markdown_math",
    "markdown_task",
    "markdown_renumber",
    "markdown_word_count",
    "markdown_insert_link",
    "latex_new_environment",
    "latex_rename_environment",
    "latex_toggle_display",
    "latex_insert_link",
    "latex_extract_newcommand",
    "latex_bold",
    "latex_italic",
    "latex_emph",
    "latex_typewriter",
    "latex_sans",
    "latex_small_caps",
    "latex_word_count",
];

/// The text editor's command that copies the selected lines, or the whole
/// file, as rich text in the theme's syntax colors.
pub const COPY_RICH: &str = "editor_copy_rich";

/// The text editor's command that sends the selection, or the cursor's
/// line, to the terminal beside it, to run.
pub const SEND_TO_TERMINAL: &str = "editor_send_to_terminal";

/// The text editor's command for interactive query replace (`M-%`).
pub const QUERY_REPLACE: &str = "query_replace";

/// The text editor's command that grows the selection to the next piece
/// of code holding it.
pub const EXPAND_SELECTION: &str = "editor_expand_selection";

/// The text editor's command that gives the selection's last growth back.
pub const SHRINK_SELECTION: &str = "editor_shrink_selection";

/// The text editor's command that stops pinning what the top of the view
/// is inside over it, or starts again.
pub const TOGGLE_STICKY_SCROLL: &str = "editor_toggle_sticky_scroll";

/// The text editor's command that lists what can be done with the thing at
/// the cursor: an address, a path, a commit, a name.
pub const ACT_AT_POINT: &str = "editor_act_at_point";

/// The text editor's command that offers the undo tree's other branches.
pub const UNDO_BRANCHES: &str = "editor_undo_branches";

/// The text editor's command that hides its inlay hints, or shows them.
pub const TOGGLE_INLAY_HINTS: &str = "editor_toggle_inlay_hints";

/// The text editor's command that turns its color swatches off or on.
pub const TOGGLE_SWATCHES: &str = "editor_toggle_swatches";

// The text editor's language server commands, which the palette hands it
// by these names.

/// Go to where the name under the cursor is defined.
pub const LSP_DEFINITION: &str = "lsp_definition";

/// Go to where it is declared.
pub const LSP_DECLARATION: &str = "lsp_declaration";

/// Go to where its type is defined.
pub const LSP_TYPE_DEFINITION: &str = "lsp_type_definition";

/// Go to what implements it.
pub const LSP_IMPLEMENTATION: &str = "lsp_implementation";

/// List every place it is used.
pub const LSP_REFERENCES: &str = "lsp_references";

/// Show where the name is defined in a card, without going there.
pub const LSP_PEEK_DEFINITION: &str = "lsp_peek_definition";

/// Show every place the name is used in a card, stepped through.
pub const LSP_PEEK_REFERENCES: &str = "lsp_peek_references";

/// Say what it is, and the problems found there.
pub const LSP_HOVER: &str = "lsp_hover";

/// Show the signature of the call being typed.
pub const LSP_SIGNATURE: &str = "lsp_signature_help";

/// Rename it everywhere.
pub const LSP_RENAME: &str = "lsp_rename";

/// Offer the fixes and refactorings there.
pub const LSP_CODE_ACTION: &str = "lsp_code_action";

/// Format the file.
pub const LSP_FORMAT: &str = "lsp_format";

/// Back to where the last jump left from.
pub const NAVIGATE_BACK: &str = "navigate_back";

/// Forward again, after going back.
pub const NAVIGATE_FORWARD: &str = "navigate_forward";

/// Every language server command the text editor runs by name.
pub const LSP_COMMANDS: &[&str] = &[
    LSP_DEFINITION,
    LSP_DECLARATION,
    LSP_TYPE_DEFINITION,
    LSP_IMPLEMENTATION,
    LSP_REFERENCES,
    LSP_PEEK_DEFINITION,
    LSP_PEEK_REFERENCES,
    LSP_HOVER,
    LSP_SIGNATURE,
    LSP_RENAME,
    LSP_CODE_ACTION,
    LSP_FORMAT,
    NAVIGATE_BACK,
    NAVIGATE_FORWARD,
];

// The text editor's problem commands, over what every back-end found
// wrong (a language server, a build, a spell check), which the palette hands
// it by these names.

/// On to the next problem in the file.
pub const PROBLEM_NEXT_IN_FILE: &str = "problem_next_in_file";

/// Back to the problem before, in the file.
pub const PROBLEM_PREV_IN_FILE: &str = "problem_prev_in_file";

/// To the first problem in the project, an error first.
pub const PROBLEM_FIRST: &str = "problem_first";

/// On to the next problem in the project.
pub const PROBLEM_NEXT: &str = "problem_next";

/// Back to the problem before, in the project.
pub const PROBLEM_PREV: &str = "problem_prev";

/// To the first error in the project.
pub const PROBLEM_FIRST_ERROR: &str = "problem_first_error";

/// On to the next error in the project.
pub const PROBLEM_NEXT_ERROR: &str = "problem_next_error";

/// Back to the error before, in the project.
pub const PROBLEM_PREV_ERROR: &str = "problem_prev_error";

/// To the first writing issue in the project.
pub const PROBLEM_FIRST_WRITING: &str = "problem_first_writing";

/// On to the next writing issue in the project.
pub const PROBLEM_NEXT_WRITING: &str = "problem_next_writing";

/// Back to the writing issue before, in the project.
pub const PROBLEM_PREV_WRITING: &str = "problem_prev_writing";

/// List every problem in the project.
pub const PROBLEM_LIST: &str = "problem_list";

/// Every problem command the text editor runs by name.
pub const PROBLEM_COMMANDS: &[&str] = &[
    PROBLEM_NEXT_IN_FILE,
    PROBLEM_PREV_IN_FILE,
    PROBLEM_FIRST,
    PROBLEM_NEXT,
    PROBLEM_PREV,
    PROBLEM_FIRST_ERROR,
    PROBLEM_NEXT_ERROR,
    PROBLEM_PREV_ERROR,
    PROBLEM_FIRST_WRITING,
    PROBLEM_NEXT_WRITING,
    PROBLEM_PREV_WRITING,
    PROBLEM_LIST,
];

/// Change every occurrence of the word under the cursor at once.
pub const EDITOR_CHANGE_OCCURRENCES: &str = "editor_change_occurrences";

/// Expand the word before the cursor (Hippie Expand / dabbrev-expand).
pub const EDITOR_EXPAND_WORD: &str = "editor_expand_word";

/// Light the occurrences of the word under the cursor, or stop.
pub const EDITOR_TOGGLE_OCCURRENCES: &str = "editor_toggle_occurrences";

/// On to the next occurrence of the word under the cursor.
pub const EDITOR_NEXT_OCCURRENCE: &str = "editor_next_occurrence";

/// Light every occurrence of the selection, or the word under the cursor, in
/// a color of its own.
pub const EDITOR_HIGHLIGHT_TEXT: &str = "editor_highlight_text";

/// Put out the highlight under the cursor.
pub const EDITOR_HIGHLIGHT_DELETE: &str = "editor_highlight_delete";

/// Put out every highlight in the file.
pub const EDITOR_HIGHLIGHT_DELETE_ALL: &str = "editor_highlight_delete_all";

/// Back to the occurrence before.
pub const EDITOR_PREV_OCCURRENCE: &str = "editor_prev_occurrence";

/// To the start of the function or module the cursor is in.
pub const EDITOR_DEFINITION_START: &str = "editor_definition_start";

/// To its end.
pub const EDITOR_DEFINITION_END: &str = "editor_definition_end";

/// Select the function or module the cursor is in.
pub const EDITOR_SELECT_DEFINITION: &str = "editor_select_definition";

/// On to the next block the indentation makes, as a fold.
pub const EDITOR_NEXT_BLOCK: &str = "editor_next_block";

/// Back to the block before.
pub const EDITOR_PREV_BLOCK: &str = "editor_prev_block";

/// Compare the buffer with another file, in the diff tool.
pub const DIFF_FILE: &str = "diff_buffer_file";

/// Compare the buffer with a revision git has of its file.
pub const DIFF_REVISION: &str = "diff_buffer_revision";

/// Compare the buffer with two other files at once.
pub const DIFF_FILES: &str = "diff_buffer_files";

/// Merge another file into the buffer, in the diff tool.
pub const MERGE_FILE: &str = "merge_file";

/// Resolve the buffer's merge conflicts in a 3-way merge.
pub const MERGE_CONFLICTS_3WAY: &str = "merge_conflicts_3way";

/// Insert a math or Greek symbol picked from a list.
pub const WRITING_INSERT_SYMBOL: &str = "editor_insert_symbol";

/// Look the word under the cursor up in the dictionary.
pub const DICTIONARY_LOOKUP: &str = "dictionary_lookup";

/// Sort a BibTeX file's entries by citation key, A to Z.
pub const BIBTEX_SORT_KEY_ASC: &str = "bibtex_sort_key_ascending";

/// Sort them by citation key, Z to A.
pub const BIBTEX_SORT_KEY_DESC: &str = "bibtex_sort_key_descending";

/// Sort them by year, the earliest first.
pub const BIBTEX_SORT_YEAR_ASC: &str = "bibtex_sort_year_ascending";

/// Sort them by year, the latest first.
pub const BIBTEX_SORT_YEAR_DESC: &str = "bibtex_sort_year_descending";

/// Put the cursor's line in the middle of the pane, then at its top, then its bottom.
pub const EDITOR_RECENTER: &str = "editor_recenter";

/// Light the cursor's line for a moment.
pub const EDITOR_FLASH_LINE: &str = "editor_flash_line";

/// Go to a character offset from the file's start.
pub const EDITOR_GOTO_OFFSET: &str = "editor_goto_offset";

/// Say the character offset the cursor is at.
pub const EDITOR_SHOW_OFFSET: &str = "editor_show_offset";

/// Run `cargo fix` on the Rust project the file is in.
pub const RUST_CARGO_FIX: &str = "rust_cargo_fix";

/// Delete the whole line under the cursor, or the selection.
pub const EDITOR_DELETE_LINE: &str = "editor_delete_line";

/// Sort the selected lines (or whole file) alphabetically A to Z.
pub const EDITOR_SORT_LINES_ASC: &str = "editor_sort_lines_asc";

/// Sort the selected lines (or whole file) alphabetically Z to A.
pub const EDITOR_SORT_LINES_DESC: &str = "editor_sort_lines_desc";

/// Remove duplicate lines from the selection (or whole file), keeping the first occurrence.
pub const EDITOR_UNIQ_LINES: &str = "editor_uniq_lines";

/// Reverse the order of the selected lines (or whole file).
pub const EDITOR_REVERSE_LINES: &str = "editor_reverse_lines";

/// Open the current file in the hex editor.
pub const EDITOR_OPEN_HEX: &str = "editor_open_hex";

/// Open git blame for the current file.
pub const EDITOR_GIT_BLAME: &str = "editor_git_blame";

/// Jump directly to a character or 2-char pattern in the buffer (Hop / Avy).
pub const EDITOR_JUMP_CHAR: &str = "editor_jump_char";

/// List the copies of the file its saves kept, to compare one with it.
pub const FILE_HISTORY: &str = "file_history";

/// Add a cursor at the next occurrence of the word under the cursor, or of
/// the selection, after the cursor added last.
pub const CURSOR_ADD_NEXT: &str = "cursor_add_next";

/// Add a cursor at every occurrence of the word under the cursor, or of the
/// selection.
pub const CURSOR_ADD_ALL: &str = "cursor_add_all";

/// Add a cursor on the line below the lowest one, in its column.
pub const CURSOR_ADD_BELOW: &str = "cursor_add_below";

/// Add a cursor on the line above the highest one, in its column.
pub const CURSOR_ADD_ABOVE: &str = "cursor_add_above";

/// Move the cursor added last on to the occurrence after it.
pub const CURSOR_SKIP: &str = "cursor_skip";

/// Take away the cursor added last.
pub const CURSOR_REMOVE_LAST: &str = "cursor_remove_last";

/// Take away every cursor but the main one.
pub const CURSOR_CLEAR: &str = "cursor_clear";

/// Step to the previous location where text was modified in the current buffer (`g;`).
pub const CHANGE_LIST_OLDER: &str = "change_list_older";

/// Step forward to the next location where text was modified (`g,`).
pub const CHANGE_LIST_NEWER: &str = "change_list_newer";

/// Swap the words adjacent to or around the cursor (`M-t` / `C-M-t`).
pub const TRANSPOSE_WORDS: &str = "transpose_words";

/// Swap the current line with the line above it (`C-x C-t`).
pub const TRANSPOSE_LINES: &str = "transpose_lines";

/// Uppercase the word under the cursor and advance to the next word (`M-u`).
pub const UPPERCASE_WORD: &str = "uppercase_word";

/// Lowercase the word under the cursor and advance to the next word (`M-o`).
pub const LOWERCASE_WORD: &str = "lowercase_word";

/// Capitalize the word under the cursor and advance to the next word (`M-c`).
pub const CAPITALIZE_WORD: &str = "capitalize_word";

/// Toggle the paragraph under the cursor between wrapped and single line (`C-M-w`).
pub const TOGGLE_WRAP_PARAGRAPH: &str = "toggle_wrap_paragraph";

/// Select the paragraph under the cursor (`M-i`).
pub const SELECT_PARAGRAPH: &str = "select_paragraph";

/// Jump to the next balanced expression (`C-M-f`).
pub const SEXP_NEXT: &str = "sexp_next";

/// Jump to the previous balanced expression (`C-M-b`).
pub const SEXP_PREV: &str = "sexp_prev";

/// Jump out of current enclosing brackets backward (`C-M-u`).
pub const SEXP_UP_BACKWARD: &str = "sexp_up_backward";

/// Jump out of current enclosing brackets forward (`C-M-e`).
pub const SEXP_UP_FORWARD: &str = "sexp_up_forward";

/// Jump into inner child bracket pair forward (`C-M-d`).
pub const SEXP_DOWN_FORWARD: &str = "sexp_down_forward";

/// Slurp the next sibling expression into current enclosing brackets (`C-S-0`).
pub const SEXP_SLURP_FORWARD: &str = "sexp_slurp_forward";

/// Slurp the previous sibling expression into current enclosing brackets (`C-S-9`).
pub const SEXP_SLURP_BACKWARD: &str = "sexp_slurp_backward";

/// Barf the last child expression out of current enclosing brackets (`C-S--`).
pub const SEXP_BARF_FORWARD: &str = "sexp_barf_forward";

/// Barf the first child expression out of current enclosing brackets (`C-S-=`).
pub const SEXP_BARF_BACKWARD: &str = "sexp_barf_backward";

/// Unwrap enclosing brackets around current expression (`M--`).
pub const SEXP_SPLICE: &str = "sexp_splice";

/// Raise current expression, replacing enclosing brackets with it (`M-r`).
pub const SEXP_RAISE: &str = "sexp_raise";

/// Collapse contiguous whitespace to 1 space or delete if already 1 space (`M-SPC`).
pub const ONE_SPACE_OR_EMPTY: &str = "one_space_or_empty";

/// Delete the word forward from the cursor (`M-d`).
pub const DELETE_WORD_FORWARD: &str = "delete_word_forward";

/// Toggle the executable permission bits on the selected entry (+x/-x).
pub const TOGGLE_EXECUTABLE: &str = "toggle_executable";

/// Show or hide the hidden entries of a directory listing (`.`).
pub const TOGGLE_HIDDEN: &str = "toggle_hidden";

/// Extract archive or compress in place.
pub const ARCHIVE_EXTRACT: &str = "archive_extract";

/// Compress selected items to an archive.
pub const ARCHIVE_COMPRESS: &str = "archive_compress";

/// The commands the palette hands to the editor in front by the name its
/// `run_command` knows them by: what only the text being edited can do.
pub const FORWARDED_COMMANDS: &[&str] = &[
    EDITOR_DELETE_LINE,
    EDITOR_SORT_LINES_ASC,
    EDITOR_SORT_LINES_DESC,
    EDITOR_UNIQ_LINES,
    EDITOR_REVERSE_LINES,
    EDITOR_OPEN_HEX,
    EDITOR_GIT_BLAME,
    EDITOR_JUMP_CHAR,
    CHANGE_LIST_OLDER,
    CHANGE_LIST_NEWER,
    TRANSPOSE_WORDS,
    TRANSPOSE_LINES,
    ONE_SPACE_OR_EMPTY,
    DELETE_WORD_FORWARD,
    UPPERCASE_WORD,
    LOWERCASE_WORD,
    CAPITALIZE_WORD,
    TOGGLE_WRAP_PARAGRAPH,
    SELECT_PARAGRAPH,
    SEXP_NEXT,
    SEXP_PREV,
    SEXP_UP_BACKWARD,
    SEXP_UP_FORWARD,
    SEXP_DOWN_FORWARD,
    SEXP_SLURP_FORWARD,
    SEXP_SLURP_BACKWARD,
    SEXP_BARF_FORWARD,
    SEXP_BARF_BACKWARD,
    SEXP_SPLICE,
    SEXP_RAISE,
    TOGGLE_EXECUTABLE,
    TOGGLE_HIDDEN,
    ARCHIVE_EXTRACT,
    ARCHIVE_COMPRESS,
    EDITOR_RECENTER,
    QUERY_REPLACE,
    EDITOR_FLASH_LINE,
    EDITOR_GOTO_OFFSET,
    EDITOR_SHOW_OFFSET,
    FILE_HISTORY,
    CURSOR_ADD_NEXT,
    CURSOR_ADD_ALL,
    CURSOR_ADD_BELOW,
    CURSOR_ADD_ABOVE,
    CURSOR_SKIP,
    CURSOR_REMOVE_LAST,
    CURSOR_CLEAR,
    WRITING_INSERT_SYMBOL,
    DICTIONARY_LOOKUP,
    BIBTEX_SORT_KEY_ASC,
    BIBTEX_SORT_KEY_DESC,
    BIBTEX_SORT_YEAR_ASC,
    BIBTEX_SORT_YEAR_DESC,
    DIFF_FILE,
    DIFF_REVISION,
    DIFF_FILES,
    MERGE_FILE,
    MERGE_CONFLICTS_3WAY,
    EDITOR_CHANGE_OCCURRENCES,
    EDITOR_EXPAND_WORD,
    EDITOR_TOGGLE_OCCURRENCES,
    EDITOR_NEXT_OCCURRENCE,
    EDITOR_PREV_OCCURRENCE,
    EDITOR_HIGHLIGHT_TEXT,
    EDITOR_HIGHLIGHT_DELETE,
    EDITOR_HIGHLIGHT_DELETE_ALL,
    EDITOR_DEFINITION_START,
    EDITOR_DEFINITION_END,
    EDITOR_SELECT_DEFINITION,
    EDITOR_NEXT_BLOCK,
    EDITOR_PREV_BLOCK,
    COPY_RICH,
    TOGGLE_SWATCHES,
    TOGGLE_INLAY_HINTS,
    SEND_TO_TERMINAL,
    UNDO_BRANCHES,
    ACT_AT_POINT,
    EXPAND_SELECTION,
    SHRINK_SELECTION,
    TOGGLE_STICKY_SCROLL,
    MERGE_NEXT,
    MERGE_PREV,
    MERGE_RESOLVE,
    DIFF_DISK,
    DIFF_CLIPBOARD,
    DIFF_BUFFER,
];
