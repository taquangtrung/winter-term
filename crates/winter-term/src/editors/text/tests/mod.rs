//! The editor's tests.

use super::*;

use crate::editors::git_exec;
use crate::editors::text_file;
use crate::model::editor::PromptMode;
use crate::model::input::KeyCode;
use crate::model::paint::{row_text, SyntaxStyle};

mod commit_messages;
mod completion;
mod conflicts_and_comparing;
mod files;
mod menus_and_clipboard;
mod painting;
mod prose;
mod servers;
mod vim;
mod window;

/// A temporary directory that removes itself on drop, so a test that
/// writes files never leaks state into the next run.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("winter-editor-page-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    /// A file in the directory, for opening as another buffer.
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).expect("temp file");
        path
    }

    /// An editor over a file holding `text`, opened at its first line: a
    /// file of its own each time, since a second editor on the same file
    /// would join the first one's buffer rather than read what was written.
    fn editor(&self, text: &str) -> TextEditor {
        let made = std::fs::read_dir(&self.0).map_or(0, |entries| entries.count());
        let name = match made {
            0 => "a.txt".to_string(),
            made => format!("a-{made}.txt"),
        };
        let path = self.0.join(name);
        std::fs::write(&path, text).expect("temp file");
        TextEditor::new(path, None).expect("opens")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(code: KeyCode) -> Key {
    Key {
        alt: false,
        code,
        ctrl: false,
        shift: false,
        sup: false,
    }
}

fn ctrl(c: char) -> Key {
    Key {
        alt: false,
        code: KeyCode::Char(c),
        ctrl: true,
        shift: false,
        sup: false,
    }
}

/// Type `keys` as bare characters, the way a user would.
fn press(editor: &mut TextEditor, keys: &str) {
    for c in keys.chars() {
        let outcome = editor.on_key(&key(KeyCode::Char(c)));
        answer_clipboard(editor, outcome);
    }
}

thread_local! {
    /// The clipboard a test's window would hold.
    static CLIPBOARD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Do with `outcome` what the window does with the clipboard: keep what a
/// yank or a delete put on it, and answer a paste from it.
fn answer_clipboard(editor: &mut TextEditor, outcome: EditorOutcome) {
    match outcome {
        EditorOutcome::Yank(text) | EditorOutcome::Cut(text) => {
            CLIPBOARD.with(|clipboard| *clipboard.borrow_mut() = text);
        }
        EditorOutcome::Paste => {
            let text = CLIPBOARD.with(|clipboard| clipboard.borrow().clone());
            let outcome = editor.on_paste(text);
            answer_clipboard(editor, outcome);
        }
        _ => {}
    }
}

/// The keys of a command, with `<esc>` for the one key a test cannot
/// spell as a character.
fn type_keys(editor: &mut TextEditor, keys: &str) {
    let mut rest = keys;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("<esc>") {
            editor.on_key(&key(KeyCode::Escape));
            rest = tail;
            continue;
        }
        let c = rest.chars().next().expect("a key");
        let outcome = editor.on_key(&key(KeyCode::Char(c)));
        answer_clipboard(editor, outcome);
        rest = &rest[c.len_utf8()..];
    }
}

/// An editor that has been painted once, so what it painted is known: a
/// click, a paging key, and `zz` all measure against the pane.
fn paint_frame(editor: &mut TextEditor, rows: usize) {
    editor.content(rows, 40, false);
}

fn press_alt(editor: &mut TextEditor, code: KeyCode) -> EditorOutcome {
    editor.on_key(&Key {
        alt: true,
        ..key(code)
    })
}

fn press_ctrl(editor: &mut TextEditor, c: char) -> EditorOutcome {
    editor.on_key(&Key {
        ctrl: true,
        ..key(KeyCode::Char(c))
    })
}

fn message_editor(tmp: &TempDir) -> (TextEditor, PathBuf) {
    let path = tmp.0.join("COMMIT_EDITMSG");
    std::fs::write(&path, "\n# Please enter\n").expect("writes");
    (TextEditor::new(path.clone(), None).expect("opens"), path)
}

/// The styles of one painted text row, run by run, with the line-number
/// gutter dropped.
fn painted(editor: &mut TextEditor, row: usize) -> Vec<(Style, String)> {
    let content = editor.content(20, 60, false);
    content.rows[row]
        .iter()
        .skip(1)
        .map(|span| (span.style, span.text.clone()))
        .collect()
}

/// The style text whose color and weight both come from the scope table's
/// row for `selector` is painted in.
fn scope(selector: &str) -> Style {
    let rules = winter_render::SCOPE_RULES;
    let row = rules
        .iter()
        .position(|rule| rule.scope == selector)
        .expect("a row of the scope table");
    Style::Syntax(SyntaxStyle {
        color: rules[row].colors().then_some(row as u16),
        weight: rules[row].weighs().then_some(row as u16),
        ..SyntaxStyle::default()
    })
}

/// The scope table row a painted style takes its color from.
fn color_of(style: Style) -> Option<&'static str> {
    match style {
        Style::Syntax(rows) => rows
            .color
            .map(|row| winter_render::SCOPE_RULES[row as usize].scope),
        _ => None,
    }
}

/// The style the first character of `text` is painted in, on text row `row`.
fn style_at(editor: &mut TextEditor, row: usize, text: &str) -> Style {
    let cells: Vec<(char, Style)> = painted(editor, row)
        .into_iter()
        .flat_map(|(style, run)| run.chars().map(move |c| (c, style)).collect::<Vec<_>>())
        .collect();
    let line: String = cells.iter().map(|(c, _)| c).collect();
    let at = line.find(text).expect("the text is on the row");
    cells[line[..at].chars().count()].1
}

fn editor_key(editor: &mut TextEditor, code: KeyCode) {
    editor.on_key(&key(code));
}

/// Each entry of `editor`'s menu that is not a separator, by label.
fn menu_labels(editor: &TextEditor) -> Vec<String> {
    editor
        .context_items()
        .into_iter()
        .filter(|item| !item.is_separator())
        .map(|item| item.label)
        .collect()
}

/// Type the keys of `editor`'s menu entry `label`, and hand back what the last
/// one did.
fn choose(editor: &mut TextEditor, label: &str) -> EditorOutcome {
    let item = editor
        .context_items()
        .into_iter()
        .find(|item| item.label == label)
        .unwrap_or_else(|| panic!("{label} in {:?}", menu_labels(editor)));
    let mut last = EditorOutcome::Ignored;
    for key in &item.keys {
        last = editor.on_key(key);
    }
    last
}

const CONFLICTED: &str =
    "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nb\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> feature\n";

/// Press `C-c` and then `rest`, the way a chord of the file's own is typed.
fn chord(editor: &mut TextEditor, rest: char) -> EditorOutcome {
    press_ctrl(editor, 'c');
    editor.on_key(&key(KeyCode::Char(rest)))
}

/// An editor over one long line folded to a pane 25 columns wide (20 of
/// text), and a short line after it.
fn folded(tmp: &TempDir) -> TextEditor {
    // Rows of 20 cells: "aaaa bbbb cccc dddd " / "eeee ffff gggg hhhh " / "iiii".
    let mut editor = tmp.editor("aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii\nshort\n");
    editor.content(10, 25, true);
    editor
}

/// Let typing pause long enough for the popup waiting on it to open.
fn pause(editor: &mut TextEditor) {
    editor.on_tick(Instant::now() + crate::config::CompletionConfig::default().delay);
}

fn ctrl_space() -> Key {
    Key {
        ctrl: true,
        ..key(KeyCode::Space)
    }
}

/// The text of `row` that is underlined, run by run.
fn squiggled(row: &[crate::model::paint::Span]) -> Vec<String> {
    row.iter()
        .filter(|span| span.squiggle.is_some())
        .map(|span| span.text.clone())
        .collect()
}

// ========================================================================
// Occurrences and the shape of the code
// ========================================================================

mod shape {
    use super::*;
    use crate::model::editor::{
        EDITOR_DEFINITION_END, EDITOR_DEFINITION_START, EDITOR_NEXT_BLOCK, EDITOR_NEXT_OCCURRENCE,
        EDITOR_SELECT_DEFINITION,
    };

    fn alt(c: char) -> Key {
        Key {
            alt: true,
            ..key(KeyCode::Char(c))
        }
    }

    #[test]
    fn test_every_occurrence_changes_together_and_one_undo_takes_a_key_back() {
        let dir = TempDir::new("occur");
        let path = dir.file("a.rs", "let count = 1;\nprint(count, recount);\ncount += count;\n");
        let mut editor = TextEditor::new(path, None).expect("opens");
        type_keys(&mut editor, "w");
        editor.on_key(&ctrl(';'));
        assert!(editor.status().segments.contains(&"occurrence 1 of 4".to_string()));
        editor.on_key(&key(KeyCode::Backspace));
        editor.on_key(&key(KeyCode::Backspace));
        press(&mut editor, "Z");
        assert_eq!(editor.buffer().lines(), ["let Zcount = 1;", "print(Zcount, recount);", "Zcount += Zcount;"]);
        editor.on_key(&key(KeyCode::Tab));
        assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 7), "into the next, at the same place");
        press(&mut editor, "_");
        assert_eq!(editor.buffer().lines()[2], "Z_count += Z_count;");
        editor.on_key(&key(KeyCode::Escape));
        assert!(editor.occur_status().is_none());
        type_keys(&mut editor, "u");
        assert_eq!(editor.buffer().lines()[2], "Zcount += Zcount;", "one undo, one key");
    }

    #[test]
    fn test_occurrences_are_lit_and_stepped_through_as_whole_words() {
        let dir = TempDir::new("occur-lit");
        let path = dir.file("a.rs", "let count = 1;\nprint(count, recount);\n");
        let mut editor = TextEditor::new(path, None).expect("opens");
        type_keys(&mut editor, "w");
        let (lit, changing) = editor.lit_columns(1, "print(count, recount);");
        assert_eq!((lit, changing), (vec![(6, 11)], false), "recount is not count");
        editor.run_command(EDITOR_NEXT_OCCURRENCE);
        assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 6));
        editor.on_key(&alt('n'));
        assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 4), "round from the end");
    }

    #[test]
    fn test_the_definition_s_edges_are_reached_and_it_is_selected_by_lines() {
        let dir = TempDir::new("structure");
        let text = "impl A {\n    fn f(&self) {\n        let x = 1;\n    }\n\n    fn g() {}\n}\n";
        let path = dir.file("a.rs", text);
        let mut editor = TextEditor::new(path, None).expect("opens");
        type_keys(&mut editor, "jj");
        editor.run_command(EDITOR_DEFINITION_END);
        assert_eq!(editor.buffer().row(), 3);
        editor.run_command(EDITOR_DEFINITION_START);
        assert_eq!(editor.buffer().row(), 1);
        editor.run_command(EDITOR_DEFINITION_START);
        assert_eq!(editor.buffer().row(), 0, "from a definition's first line, the one around it");
        type_keys(&mut editor, "jj");
        editor.run_command(EDITOR_SELECT_DEFINITION);
        let selection = editor.selection().expect("selected");
        assert_eq!((selection.start.0, selection.end.0, selection.linewise), (1, 3, true));
        editor.run_command(EDITOR_SELECT_DEFINITION);
        let wider = editor.selection().expect("selected");
        assert_eq!((wider.start.0, wider.end.0), (0, 6), "then the impl around it");
        type_keys(&mut editor, "<esc>gg");
        editor.run_command(EDITOR_NEXT_BLOCK);
        assert_eq!(editor.buffer().row(), 1);
        type_keys(&mut editor, "zk");
        assert_eq!(editor.buffer().row(), 0, "`zk` back to the block before");
    }
}

// ========================================================================
// Typing by context
// ========================================================================

mod by_context {
    use super::*;

    fn shift(code: KeyCode) -> Key {
        Key {
            shift: true,
            ..key(code)
        }
    }

    #[test]
    fn test_return_carries_a_comment_on_and_a_list_item_under_its_text() {
        let dir = TempDir::new("context-comment");
        let path = dir.file("a.rs", "    /// Docs\n");
        let mut editor = TextEditor::new(path, None).expect("opens");
        type_keys(&mut editor, "A");
        editor.on_key(&key(KeyCode::Enter));
        press(&mut editor, "more");
        assert_eq!(editor.buffer().lines()[1], "    /// more");
        let notes = dir.file("notes.md", "- [ ] buy milk\n");
        let mut editor = TextEditor::new(notes, None).expect("opens");
        type_keys(&mut editor, "A");
        editor.on_key(&key(KeyCode::Enter));
        press(&mut editor, "x");
        assert_eq!(editor.buffer().lines()[1], "      x", "under the item's text, past the box");
    }

    #[test]
    fn test_return_on_an_empty_item_moves_its_bullet_down() {
        let dir = TempDir::new("context-empty");
        let notes = dir.file("notes.md", "- one\n- \n");
        let mut editor = TextEditor::new(notes, None).expect("opens");
        type_keys(&mut editor, "jA");
        editor.on_key(&key(KeyCode::Enter));
        assert_eq!(&editor.buffer().lines()[1..3], ["", "- "]);
        assert_eq!((editor.buffer().row(), editor.buffer().col()), (2, 2));
    }

    #[test]
    fn test_tab_indents_a_bullet_and_a_heading_and_shift_backspace_blanks_a_bullet() {
        let dir = TempDir::new("context-tab");
        let notes = dir.file("notes.md", "## Head\n- item\n");
        let mut editor = TextEditor::new(notes, None).expect("opens");
        type_keys(&mut editor, "I");
        editor.on_key(&key(KeyCode::Tab));
        assert_eq!(editor.buffer().lines()[0], "### Head");
        editor.on_key(&shift(KeyCode::Tab));
        assert_eq!(editor.buffer().lines()[0], "## Head");
        editor.on_key(&key(KeyCode::Escape));
        type_keys(&mut editor, "jI");
        editor.on_key(&key(KeyCode::Tab));
        assert_eq!(editor.buffer().lines()[1], "  + item", "one deeper, the next bullet");
        editor.on_key(&shift(KeyCode::Tab));
        assert_eq!(editor.buffer().lines()[1], "- item");
        editor.on_key(&key(KeyCode::Escape));
        type_keys(&mut editor, "0");
        type_keys(&mut editor, "la");
        editor.on_key(&shift(KeyCode::Backspace));
        assert_eq!(editor.buffer().lines()[1], "  item");
    }
}

/// `c` typed with Shift the way the window hands it on: the lowercase
/// letter, Shift held.
fn shifted(c: char) -> Key {
    Key {
        shift: true,
        ..key(KeyCode::Char(c))
    }
    .canonical()
}

/// The text of painted row `row`, the gutter left off.
fn painted_text(frame: &Frame, row: usize) -> String {
    row_text(&frame.rows[row]).chars().skip(NUMBER_WIDTH + NUMBER_GAP.len()).collect::<String>().trim_end().to_string()
}
