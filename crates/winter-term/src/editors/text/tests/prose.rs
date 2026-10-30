//! The text editor's tests: LaTeX, Markdown, BibTeX, SyncTeX, and spelling.

use super::*;

#[test]
fn test_zz_writes_and_closes_while_zq_closes_without_writing() {
    let tmp = TempDir::new("leave");

    let mut written = tmp.editor("one\n");
    press(&mut written, "x");
    press(&mut written, "ZZ");
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "ne\n"
    );

    let mut discarded = tmp.editor("one\n");
    press(&mut discarded, "x");
    assert_eq!(
        discarded.on_key(&key(KeyCode::Char('Z'))),
        EditorOutcome::Consumed
    );
    assert_eq!(
        discarded.on_key(&key(KeyCode::Char('Q'))),
        EditorOutcome::Close
    );
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a-1.txt")).expect("reads"),
        "one\n",
        "the edits were dropped, not written"
    );
}

#[test]
fn test_synctex_forward_asks_where_the_cursor_s_line_was_set_and_shows_it() {
    let tmp = TempDir::new("synctex");
    let source = tmp.file("paper.tex", "\\documentclass{article}\nHello.\nWorld.\n");
    let pdf = tmp.file("paper.pdf", "");
    tmp.file("paper.synctex.gz", "");
    let mut editor = TextEditor::new(source.clone(), Some(3)).expect("opens");
    let Some(EditorOutcome::Job(JobRequest::Command(request))) =
        editor.run_command(SYNCTEX_FORWARD)
    else {
        panic!("the command runs synctex");
    };
    assert_eq!(request.tag, synctex::TAG_VIEW);
    assert_eq!(request.args[2], format!("3:1:{}", source.display()));
    assert_eq!(request.args[4], pdf.display().to_string());

    let output = CommandOutput {
        id: 0,
        code: Some(0),
        stderr: String::new(),
        stdout: "Page:2\nh:72\nv:120\nW:300\nH:10\n".to_string(),
        tag: synctex::TAG_VIEW,
    };
    let EditorOutcome::ShowInPdf { beside, mark, path } = editor.on_job(JobReply::Command(output)) else {
        panic!("the box is shown in the PDF");
    };
    assert_eq!(path, pdf);
    assert!(!beside, "F6 shows it in this pane");
    let mark = mark.expect("a box");
    assert_eq!((mark.page, mark.top), (1, 110.0));
}

#[test]
fn test_a_line_synctex_does_not_find_still_opens_the_pdf() {
    let tmp = TempDir::new("synctex-unfound");
    let source = tmp.file("paper.tex", "\\documentclass{article}\nHello.\n");
    let pdf = tmp.file("paper.pdf", "");
    tmp.file("paper.synctex.gz", "");
    let mut editor = TextEditor::new(source, None).expect("opens");
    assert!(matches!(editor.run_command(editor::SYNCTEX_FORWARD_BESIDE), Some(EditorOutcome::Job(_))));
    let output = CommandOutput {
        id: 0,
        code: Some(0),
        stderr: String::new(),
        stdout: "This is SyncTeX command line utility, version 1.5\n".to_string(),
        tag: synctex::TAG_VIEW,
    };
    let outcome = editor.on_job(JobReply::Command(output));
    assert_eq!(outcome, EditorOutcome::ShowInPdf { beside: true, mark: None, path: pdf });
}

#[test]
fn test_synctex_forward_from_a_bib_entry_searches_where_it_is_cited() {
    let tmp = TempDir::new("synctex-bib");
    let main = tmp.file("main.tex", "\\documentclass{article}\n\\begin{document}\nAs shown~\\citep[p.~2]{knuth84, smith20}.\n\\end{document}\n");
    tmp.file("main.pdf", "");
    tmp.file("main.synctex.gz", "");
    let bib = tmp.file("refs.bib", "@book{knuth84,\n  title = {TeX},\n}\n\n@article{smith20,\n  title = {X},\n  year = 2020,\n}\n");
    let mut editor = TextEditor::new(bib.clone(), Some(7)).expect("opens");
    let Some(EditorOutcome::Job(JobRequest::Command(request))) = editor.run_command(SYNCTEX_FORWARD) else {
        panic!("synctex runs from the citation");
    };
    let at = request.args[2].clone();
    assert!(at.starts_with("3:") && at.ends_with(&main.display().to_string()), "{at}");
    // Outside any entry there is no line to search: the PDF opens as it is.
    let mut editor = TextEditor::new(tmp.file("empty.bib", "% nothing yet\n"), None).expect("opens");
    assert!(matches!(
        editor.run_command(SYNCTEX_FORWARD),
        Some(EditorOutcome::ShowInPdf { mark: None, .. })
    ));
}

#[test]
fn test_synctex_forward_says_so_outside_a_built_latex_document() {
    let tmp = TempDir::new("synctex-none");
    let mut rust = TextEditor::new(tmp.file("a.rs", "fn main() {}\n"), None).expect("opens");
    assert_eq!(
        rust.run_command(SYNCTEX_FORWARD),
        Some(EditorOutcome::Consumed)
    );
    assert!(rust
        .take_notice()
        .is_some_and(|notice| notice.text.contains("not a LaTeX")));
    let mut unbuilt =
        TextEditor::new(tmp.file("b.tex", "\\documentclass{article}\n"), None).expect("opens");
    assert_eq!(
        unbuilt.run_command(SYNCTEX_FORWARD),
        Some(EditorOutcome::Consumed)
    );
    assert!(unbuilt
        .take_notice()
        .is_some_and(|notice| notice.text.contains("no PDF")));
}

#[test]
fn test_markdown_chords_demote_a_heading_wrap_a_word_and_tick_a_task() {
    let tmp = TempDir::new("markdown-chords");
    let path = tmp.file("notes.md", "## Plan\n- buy milk\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    chord(&mut editor, '=');
    assert_eq!(editor.buffer().lines()[0], "### Plan");
    press(&mut editor, "jww");
    chord(&mut editor, 'b');
    assert_eq!(editor.buffer().lines()[1], "- buy **milk**");
    chord(&mut editor, 'x');
    assert_eq!(editor.buffer().lines()[1], "- [ ] buy **milk**");
    press_alt(&mut editor, KeyCode::Enter);
    assert_eq!(editor.buffer().lines()[2], "- [ ] ");
    assert!(editor.modal.is_typing(), "typing into the new item");
    press(&mut editor, "eggs");
    assert_eq!(editor.buffer().lines()[2], "- [ ] eggs");
}

#[test]
fn test_latex_chords_rename_the_environment_and_toggle_a_display() {
    let tmp = TempDir::new("latex-chords");
    let path = tmp.file(
        "paper.tex",
        "\\begin{itemize}\n\\item one\n\\end{itemize}\n\\[\nx\n\\]\n",
    );
    let mut editor = TextEditor::new(path, None).expect("opens");
    press(&mut editor, "j");
    let EditorOutcome::Prompt(question) = chord(&mut editor, 'e') else {
        panic!("asks for the new name");
    };
    assert_eq!(question.initial, "itemize");
    editor.on_prompt(PromptReply {
        answer: Some("enumerate".to_string()),
        tag: question.tag,
    });
    assert_eq!(editor.buffer().lines()[0], "\\begin{enumerate}");
    assert_eq!(editor.buffer().lines()[2], "\\end{enumerate}");
    press(&mut editor, "jjj");
    chord(&mut editor, 'd');
    assert_eq!(editor.buffer().lines()[3], "\\begin{equation}");
    assert_eq!(editor.buffer().lines()[5], "\\end{equation}");
    press(&mut editor, "ggjww");
    chord(&mut editor, 'b');
    assert_eq!(editor.buffer().lines()[1], "\\item \\textbf{one}");
}

#[test]
fn test_a_writing_command_from_the_palette_needs_its_own_kind_of_file() {
    let tmp = TempDir::new("writing-palette");
    let mut editor = tmp.editor("# not markdown here\n");
    assert_eq!(
        editor.run_command("markdown_demote"),
        Some(EditorOutcome::Consumed)
    );
    assert_eq!(editor.buffer().lines()[0], "# not markdown here", "a .txt file");
    let path = tmp.file("a.md", "# Title\n");
    let mut markdown = TextEditor::new(path, None).expect("opens");
    markdown.run_command("markdown_demote");
    assert_eq!(markdown.buffer().lines()[0], "## Title");
    assert!(markdown
        .keys()
        .iter()
        .any(|help| help.label == "Toggle the task box"));
}

#[test]
fn test_the_outline_of_a_program_lists_its_definitions_indented_by_depth() {
    let tmp = TempDir::new("outline-code");
    let source = "struct Foo;\n\nimpl Foo {\n    pub fn new() -> Self {\n        helper()\n    }\n}\n";
    let path = tmp.file("lib.rs", source);
    let mut editor = TextEditor::new(path, None).expect("opens");
    let outline = editor.outline_targets().expect("an outline");
    let labels: Vec<&str> = outline.iter().map(|entry| entry.label.as_str()).collect();
    assert_eq!(
        labels,
        ["    1  struct Foo;", "    3  impl Foo {", "    4    pub fn new() -> Self {"]
    );
    assert_eq!(outline[2].target, 3, "a call is no definition");
}

#[test]
fn test_a_color_the_file_writes_is_drawn_on_that_color_until_turned_off() {
    let tmp = TempDir::new("swatch");
    let mut editor = tmp.editor("cursor \"#51afef\"\n");
    let swatched = |editor: &mut TextEditor| {
        editor.content(5, 60, false).rows[0]
            .iter()
            .any(|span| matches!(span.style, Style::Terminal(_)) && span.text.contains("#51afef"))
    };
    assert!(swatched(&mut editor));
    editor.run_command(TOGGLE_SWATCHES);
    assert!(!swatched(&mut editor));
}

#[test]
fn test_a_word_taken_into_the_project_or_ignored_is_no_longer_spelled_wrong() {
    let tmp = TempDir::new("spell-lists");
    std::fs::create_dir_all(tmp.0.join(".git")).expect("a project");
    let path = tmp.file("notes.md", "zqxwvb and kpwqzm.\n");
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    let flagged = |editor: &mut TextEditor| {
        let frame = editor.content(10, 60, false);
        squiggled(&frame.rows[0])
    };
    assert_eq!(flagged(&mut editor), ["zqxwvb", "kpwqzm"]);
    let EditorOutcome::Pick(pick) = editor.on_key(&Key {
        alt: true,
        ..key(KeyCode::Char('\\'))
    }) else {
        panic!("the spelling fixes are offered");
    };
    let to_project = "Add \"zqxwvb\" to project dictionary";
    assert!(pick.items.iter().any(|item| item == to_project), "{:?}", pick.items);
    editor.on_prompt(PromptReply {
        answer: Some(to_project.to_string()),
        tag: pick.tag,
    });
    let words = std::fs::read_to_string(tmp.0.join(".winter/words.txt")).expect("the project's list");
    assert!(words.lines().any(|line| line == "zqxwvb"), "{words}");
    assert_eq!(flagged(&mut editor), ["kpwqzm"], "taken as right in the project");
    crate::editors::spell::accept("kpwqzm", &crate::editors::spell::WordList::Session).expect("ignored");
    assert!(flagged(&mut editor).is_empty(), "ignored, and every editor checks again");
}

#[test]
fn test_a_markdown_file_s_misspellings_are_underlined_stepped_to_and_put_right() {
    let tmp = TempDir::new("spelling");
    let path = tmp.file("notes.md", "We recieve mail.\n\n```\nteh code\n```\nAnd teh reply.\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let frame = editor.content(10, 60, false);
    assert_eq!(squiggled(&frame.rows[0]), ["recieve"], "checked from the start");
    assert!(squiggled(&frame.rows[3]).is_empty(), "code is no prose");
    // `]s` to the next, past the code, and back round with `[s`.
    press(&mut editor, "]s");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 3));
    assert!(editor.take_notice().is_some_and(|notice| notice.text.contains("receive")));
    press(&mut editor, "]s");
    assert_eq!(editor.buffer().row(), 5);
    press(&mut editor, "[s");
    assert_eq!(editor.buffer().row(), 0);
    // `z=` offers what to put in its place.
    press(&mut editor, "z");
    let EditorOutcome::Pick(pick) = editor.on_key(&key(KeyCode::Char('='))) else {
        panic!("suggestions are offered");
    };
    assert_eq!(pick.items.first().map(String::as_str), Some("Change to \"receive\""));
    assert!(
        pick.icons.iter().all(|icon| *icon == crate::model::paint::IconKind::Symbol { name: "lightbulb" }),
        "each fix wears a lightbulb"
    );
    assert_eq!(pick.placement, crate::model::editor::PickPlacement::Cursor, "a word's spellings float at the cursor");
    editor.on_prompt(PromptReply {
        answer: pick.items.first().cloned(),
        tag: pick.tag,
    });
    assert_eq!(editor.buffer().lines()[0], "We receive mail.");
    let frame = editor.content(10, 60, false);
    assert!(squiggled(&frame.rows[0]).is_empty(), "checked again after the edit");
    // `M-\` offers the same fixes with no server to ask, a capital kept.
    press(&mut editor, "]s");
    let fixes = editor.on_key(&Key {
        alt: true,
        ..key(KeyCode::Char('\\'))
    });
    let EditorOutcome::Pick(pick) = fixes else {
        panic!("the spelling fixes are offered");
    };
    let last = pick.items.last().map(String::as_str).unwrap_or_default();
    assert_eq!(last, "Ignore \"teh\" for this session");
    assert!(pick.items.iter().any(|item| item == "Add \"teh\" to user dictionary"), "{:?}", pick.items);
    let the = pick.items.iter().find(|item| *item == "Change to \"the\"").cloned();
    editor.on_prompt(PromptReply { answer: the, tag: pick.tag });
    assert_eq!(editor.buffer().lines()[5], "And the reply.");
    // `M-S-i` stops checking.
    editor.on_key(&Key {
        alt: true,
        shift: true,
        ..key(KeyCode::Char('i'))
    });
    let frame = editor.content(10, 60, false);
    assert!(squiggled(&frame.rows[5]).is_empty());
}

#[test]
fn test_a_bibtex_file_s_entries_sort_by_year_and_by_key() {
    use crate::model::editor::{BIBTEX_SORT_KEY_ASC, BIBTEX_SORT_YEAR_DESC};
    let dir = TempDir::new("bibtex");
    let text = "% head\n\n@article{zed,\n  year = {2001},\n}\n\n@book{alpha,\n  year = 2020,\n}\n";
    let path = dir.file("refs.bib", text);
    let mut editor = TextEditor::new(path, None).expect("opens");
    editor.run_command(BIBTEX_SORT_YEAR_DESC);
    let keys: Vec<&str> = editor.buffer().lines().iter().filter(|line| line.starts_with('@')).map(String::as_str).collect();
    assert_eq!(keys, ["@book{alpha,", "@article{zed,"]);
    editor.run_command(BIBTEX_SORT_KEY_ASC);
    assert_eq!(editor.buffer().lines()[0..3], ["% head", "", "@book{alpha,"]);
    // Already A to Z, the second sort changed nothing: one undo takes the
    // first back.
    type_keys(&mut editor, "u");
    assert_eq!(editor.buffer().lines()[2], "@article{zed,");
}

#[test]
fn test_a_symbol_goes_in_as_latex_in_a_latex_file_and_as_itself_elsewhere() {
    use crate::model::editor::WRITING_INSERT_SYMBOL;
    let tmp = TempDir::new("symbol");
    for (name, expected) in [("a.tex", "\\alpha"), ("a.md", "α")] {
        let path = tmp.file(name, "\n");
        let mut editor = TextEditor::new(path, None).expect("opens");
        type_keys(&mut editor, "i");
        let EditorOutcome::Pick(pick) = editor.run_command(WRITING_INSERT_SYMBOL).expect("a command") else {
            panic!("a list");
        };
        let alpha = pick.items.iter().find(|item| item.starts_with('α')).cloned().expect("alpha");
        editor.on_prompt(PromptReply {
            answer: Some(alpha),
            tag: pick.tag,
        });
        assert_eq!(editor.buffer().lines()[0], expected, "{name}");
    }
}

#[test]
fn test_a_pdf_built_without_synctex_data_opens_as_it_is() {
    let dir = TempDir::new("synctex-no-data");
    let path = dir.file("doc.tex", "\\documentclass{article}\n\\begin{document}\nHi\n\\end{document}\n");
    let pdf = dir.file("doc.pdf", "%PDF-1.5\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let outcome = editor.run_command(SYNCTEX_FORWARD);
    let said = editor.take_notice().map(|notice| notice.text).unwrap_or_default();
    assert_eq!(outcome, Some(EditorOutcome::ShowInPdf { beside: false, mark: None, path: pdf }), "{said}");
    assert!(said.contains("without SyncTeX data"), "{said}");
}

#[test]
fn test_markdown_s_markers_hide_and_the_caret_and_a_click_keep_to_the_text() {
    let tmp = TempDir::new("conceal-md");
    let path = tmp.file("notes.md", "# Title\nA **bold** word and `code`.\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let frame = editor.content(6, 60, false);
    assert_eq!(painted_text(&frame, 0), "Title");
    assert_eq!(painted_text(&frame, 1), "A bold word and code.");
    let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
    // A click on the `w` of `word` lands on it, past the hidden `**`s.
    editor.on_mouse(Point {
        col: gutter + 7,
        drag: false,
        row: 1,
    });
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 11));
    assert_eq!(editor.caret().map(|caret| caret.col), Some(gutter + 7), "the caret draws on it too");
    // The bold text is drawn bold.
    let bold = frame.rows[1].iter().find(|span| span.text.contains("bold")).expect("the span");
    assert!(matches!(bold.style, Style::Syntax(SyntaxStyle { weight: Some(_), .. })), "{:?}", bold.style);
    // A link's text is underlined once its URL hides.
    let path = tmp.file("links.md", "See [the docs](https://x.y) here.\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let frame = editor.content(4, 60, false);
    assert_eq!(painted_text(&frame, 0), "See the docs here.");
    let link = frame.rows[0].iter().find(|span| span.text.contains("docs")).expect("the span");
    assert!(matches!(link.style, Style::Syntax(SyntaxStyle { underline: true, .. })), "{:?}", link.style);
}

#[test]
fn test_latex_s_commands_hide_and_a_wrapped_line_folds_by_what_shows() {
    let tmp = TempDir::new("conceal-tex");
    let path = tmp.file("paper.tex", "\\textbf{aaaa} \\emph{bbbb} cccc dddd\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let frame = editor.content(6, 60, false);
    assert_eq!(painted_text(&frame, 0), "aaaa bbbb cccc dddd");
    // Folded to ten cells of text, the line breaks after what shows of it.
    let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
    let frame = editor.content(6, gutter + 10, true);
    assert_eq!(painted_text(&frame, 0), "aaaa bbbb");
}
