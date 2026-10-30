//! The git view's tests: reading diffs, and folding them away.

use super::*;

#[test]
fn test_shift_g_and_g_are_different_keys_as_the_window_delivers_them() {
    // The window delivers S-g as `g` with Shift held, not as `G`: it goes
    // to the bottom, as Vim's `G` does.
    let shift_g = Key {
        shift: true,
        sup: false,
        ..Key::plain(KeyCode::Char('g'))
    };
    let mut editor = loaded_editor();
    assert_eq!(editor.on_key(&shift_g), EditorOutcome::Consumed);
    assert_eq!(editor.list.cursor(), editor.rows.len() - 1);
    let mut editor = loaded_editor();
    assert_eq!(
        editor.on_key(&Key::plain(KeyCode::Char('g'))),
        EditorOutcome::Consumed
    );
    assert!(editor.menus.popup.is_some(), "plain g opens the jump menu");
}

#[test]
fn test_unfolding_a_file_in_a_short_pane_keeps_its_row_where_it_was() {
    // Unfolding used to scroll the opened row to the top of the pane
    // whenever its diff ran past the bottom, moving the line the reader
    // had just pressed Tab on.
    let mut editor = editor_with_diff();
    editor.on_key(&press(KeyCode::Tab));
    editor.content(6, 80, false);
    let (cursor, scroll) = (editor.list.cursor(), editor.list.view().scroll());
    editor.on_key(&press(KeyCode::Tab));
    editor.content(6, 80, false);
    assert_eq!(
        (editor.list.cursor(), editor.list.view().scroll()),
        (cursor, scroll),
        "the row stays on the same line of the pane"
    );
}

#[test]
fn test_expanding_a_file_reads_its_diff_from_the_right_side() {
    // A staged file's diff is the index one; asking for the working-tree
    // diff there would show changes that are not staged.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "staged.rs");
    let command = command_of(&editor.on_key(&press(KeyCode::Tab)));
    assert!(
        command.args.contains(&"--cached".to_string()),
        "{:?}",
        command.args
    );
    answer_job(&mut editor, git_exec::TAG_DIFF, "");

    cursor_on(&mut editor, "working.rs");
    let command = command_of(&editor.on_key(&press(KeyCode::Tab)));
    assert!(
        !command.args.contains(&"--cached".to_string()),
        "{:?}",
        command.args
    );
}

#[test]
fn test_an_untracked_file_is_diffed_against_nothing() {
    // git has no indexed copy to compare against, so a plain `diff` prints
    // nothing at all and the file looks unchanged.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "new.rs");
    let command = command_of(&editor.on_key(&press(KeyCode::Tab)));
    assert!(
        command.args.contains(&"--no-index".to_string()),
        "{:?}",
        command.args
    );
}

#[test]
fn test_an_expanded_files_diff_decorates_the_edited_words() {
    // The whole point of the word diff: a replaced line paints its edit in
    // the line's style and the words the edit left alone recede.
    let editor = editor_with_diff();
    let added = editor
        .rows
        .iter()
        .find(|row| row_text(&row.spans).contains("new();"))
        .expect("the added line");
    assert!(
        added
            .spans
            .iter()
            .any(|span| span.style == Style::AddedEdit),
        "got {:?}",
        added.spans
    );
    assert!(
        added.spans.iter().any(|span| span.style == Style::Added),
        "got {:?}",
        added.spans
    );
}

#[test]
fn test_the_diff_popup_fills_the_view_with_a_decorated_diff() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('d')));
    let outcome = editor.on_key(&press(KeyCode::Char('d')));
    assert_eq!(command_of(&outcome).args[0], "diff");
    editor.on_job(reply(git_exec::TAG_DIFF_VIEW, DIFF_OUTPUT));
    let output = editor.output.as_ref().expect("the diff view");
    assert_eq!(output.rows.len(), output.lines.len());
    let added = output
        .rows
        .iter()
        .find(|row| row_text(&row.spans).contains("new();"))
        .expect("the added line");
    assert!(
        added
            .spans
            .iter()
            .any(|span| span.style == Style::AddedEdit),
        "got {added:?}"
    );
    assert!(
        added.spans.iter().any(|span| span.style == Style::Added),
        "got {added:?}"
    );
}

#[test]
fn test_two_files_opened_quickly_each_get_their_own_diff() {
    // One diff slot with replies matched by tag: opening a second file
    // before the first's diff landed filed the first's diff under the
    // second, and `s` on it staged the wrong change.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let first = editor.on_key(&press(KeyCode::Tab));
    assert!(command_of(&first).args.iter().any(|arg| arg == "working.rs"));
    cursor_on(&mut editor, "staged.rs");
    assert_eq!(
        editor.on_key(&press(KeyCode::Tab)),
        EditorOutcome::Consumed,
        "the second waits for the first"
    );
    let next = answer_job(&mut editor, git_exec::TAG_DIFF, DIFF_OUTPUT);
    assert_eq!(editor.repo.diffs.len(), 1);
    assert!(editor.repo.diffs.keys().all(|file| file.path == "working.rs"));
    assert!(command_of(&next).args.iter().any(|arg| arg == "staged.rs"));
}

#[test]
fn test_a_diff_asked_for_before_a_new_status_is_read_again() {
    // It was read against the files as they were; showing it after the
    // status changed lets a hunk be staged against lines that moved.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    editor.on_key(&press(KeyCode::Tab));
    editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT));
    let again = answer_job(&mut editor, git_exec::TAG_DIFF, DIFF_OUTPUT);
    assert!(editor.repo.diffs.is_empty(), "the old answer is dropped");
    assert_eq!(command_of(&again).tag, git_exec::TAG_DIFF);
}

#[test]
fn test_a_new_status_drops_the_diffs_it_invalidated() {
    // Staging a hunk changes the file's diff; applying a patch built from
    // the old one would hit the wrong lines.
    let mut editor = editor_with_diff();
    assert!(!editor.repo.diffs.is_empty());
    editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT));
    assert!(editor.repo.diffs.is_empty(), "the stale diff is gone");
}

#[test]
fn test_a_file_the_status_no_longer_lists_stops_being_expanded() {
    let mut editor = editor_with_diff();
    editor.on_job(reply(
        git_exec::TAG_STATUS,
        "# branch.head main
",
    ));
    assert!(editor.expanded.is_empty());
}

#[test]
fn test_folding_an_expanded_file_hides_its_diff_without_re_reading() {
    let mut editor = editor_with_diff();
    cursor_on(&mut editor, "working.rs");
    let outcome = editor.on_key(&press(KeyCode::Tab));
    assert_eq!(outcome, EditorOutcome::Consumed, "nothing to run");
    assert!(!editor
        .rows
        .iter()
        .any(|row| matches!(row.item, Item::Hunk(_))));
}

#[test]
fn test_shift_tab_leaves_an_output_view_with_nothing_to_fold_alone() {
    // A blame, a log, or a listing is the lines it came back as: there is
    // no commit under it to fold, and the key must not disturb the view.
    let mut editor = loaded_editor();
    editor.on_job(reply(
        git_exec::TAG_BLAME,
        "abc1234 (Someone 2026-09-18) fn main() {",
    ));
    let before = editor.output.as_ref().expect("the blame").lines.clone();

    assert_eq!(editor.on_key(&shift(KeyCode::Tab)), EditorOutcome::Consumed);
    assert_eq!(editor.output.as_ref().unwrap().lines, before);
}

#[test]
fn test_folding_reaches_the_painted_content_the_pane_draws() {
    // The fold has to change what `content` returns, not just the row list,
    // or the pane keeps drawing the unfolded view.
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    editor.on_key(&shift(KeyCode::Tab));
    let drawn = |editor: &mut GitEditor| -> String {
        editor
            .content(40, 100, false)
            .rows
            .iter()
            .map(row_text)
            .collect::<Vec<String>>()
            .join("\n")
    };
    assert!(drawn(&mut editor).contains("new();"));

    output_cursor_on(&mut editor, "src/a.rs");
    editor.on_key(&press(KeyCode::Tab));
    let after = drawn(&mut editor);
    assert!(
        !after.contains("new();"),
        "the pane draws the folded view, got {after:?}"
    );
    assert!(after.contains("src/a.rs"), "the band is still drawn");
}

#[test]
fn test_shift_tab_still_folds_while_a_menu_has_never_opened() {
    let mut editor = loaded_editor();
    editor.on_key(&shift(KeyCode::Tab));
    assert!(!editor.expanded.is_empty());
}

#[test]
fn test_the_z_keys_fold_and_unfold_what_the_cursor_is_on() {
    let mut editor = loaded_editor();
    let at = editor
        .rows
        .iter()
        .position(|row| row.item == Item::Heading(Section::Untracked))
        .expect("the untracked heading");
    let rows = editor.rows.len();
    editor.list.set_cursor(at);
    editor.on_key(&press_char('z'));
    editor.on_key(&press_char('c'));
    let folded = editor.rows.len();
    assert!(folded < rows, "zc folds the section");
    editor.on_key(&press_char('z'));
    editor.on_key(&press_char('c'));
    assert_eq!(editor.rows.len(), folded, "zc on a fold leaves it folded");
    editor.on_key(&press_char('z'));
    editor.on_key(&press_char('o'));
    assert_eq!(editor.rows.len(), rows, "zo unfolds it");
}

#[test]
fn test_folding_a_section_hides_its_files_but_not_the_others() {
    let mut editor = loaded_editor();
    let at = editor
        .rows
        .iter()
        .position(|row| row.item == Item::Heading(Section::Untracked))
        .expect("the untracked heading");
    editor.list.set_cursor(at);
    editor.on_key(&press(KeyCode::Tab));

    let listed: Vec<String> = editor
        .rows
        .iter()
        .filter_map(|row| match &row.item {
            Item::File(file) => Some(file.path.clone()),
            _ => None,
        })
        .collect();
    assert!(!listed.contains(&"new.rs".to_string()), "got {listed:?}");
    assert!(listed.contains(&"working.rs".to_string()), "got {listed:?}");
}

#[test]
fn test_shift_tab_shows_every_file_s_diff_then_hides_them_all() {
    // As magic-vscode's recursive toggle: every file of every section, its
    // section opened if it was shut, each diff read one at a time.
    let mut editor = loaded_editor();
    let first = editor.repo.status.files[0].section;
    editor.collapsed.insert(Block::Files(first));
    let shown = editor.on_key(&shift(KeyCode::Tab));
    assert!(
        matches!(shown, EditorOutcome::Job(_)),
        "the first diff is read"
    );
    assert_eq!(editor.expanded.len(), editor.repo.status.files.len());
    assert!(!editor.collapsed.contains(&Block::Files(first)));
    answer_job(&mut editor, git_exec::TAG_DIFF, DIFF_OUTPUT);
    assert!(editor
        .rows
        .iter()
        .any(|row| matches!(row.item, Item::Hunk(_))));

    // The first file is open, so the key now shuts every one, leaving the
    // files listed.
    assert_eq!(editor.on_key(&shift(KeyCode::Tab)), EditorOutcome::Consumed);
    assert!(editor.expanded.is_empty());
    assert!(!editor
        .rows
        .iter()
        .any(|row| matches!(row.item, Item::Hunk(_))));
    assert!(editor
        .rows
        .iter()
        .any(|row| matches!(row.item, Item::File(_))));
}

#[test]
fn test_the_tab_says_which_view_it_shows_and_of_which_project() {
    assert_eq!(
        GitEditor::new(PathBuf::from("/repo")).title(),
        "Git",
        "not read yet"
    );
    let mut editor = loaded_editor();
    assert_eq!(editor.title(), "Git Status: repo");

    editor.on_job(reply(
        git_exec::TAG_LOG_VIEW,
        &log_output(&[("abc1234", "one")]),
    ));
    assert_eq!(editor.title(), "Git Log: repo");

    let shown = ["commit abc1234567890", "", "    do the thing"].join("\n");
    editor.on_job(reply(git_exec::TAG_SHOW, &shown));
    assert!(
        editor.title().starts_with("Git Commit abc1234"),
        "{}",
        editor.title()
    );
    assert!(editor.title().ends_with(": repo"));

    editor.on_key(&press_char('q'));
    assert_eq!(editor.title(), "Git Status: repo", "back to the status");
}

#[test]
fn test_an_untracked_files_diff_answers_with_exit_code_one_and_shows() {
    // `diff --no-index` says "they differ" with 1, which is the whole
    // point of asking; read as a failure, no untracked file could open.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "new.rs");
    let command = command_of(&editor.on_key(&press(KeyCode::Tab)));
    assert_eq!(command.tag, git_exec::TAG_DIFF_UNTRACKED);
    editor.on_job(JobReply::Command(CommandOutput {
        id: command.id,
        code: Some(1),
        stderr: String::new(),
        stdout: "diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n\
                 +++ b/new.rs\n@@ -0,0 +1 @@\n+fresh\n"
            .to_string(),
        tag: git_exec::TAG_DIFF_UNTRACKED,
    }));
    assert_eq!(notice_text(&mut editor), None, "no failure reported");
    assert!(
        editor
            .rows
            .iter()
            .any(|row| matches!(&row.item, Item::Hunk(hunk) if hunk.path == "new.rs")),
        "the file's hunk shows"
    );
}

#[test]
fn test_a_comparison_reads_both_files_with_no_repository_and_shows_the_diff_open() {
    let mut editor = comparison();
    let command = command_of(&EditorOutcome::Job(editor.initial_request()));
    assert_eq!(command.tag, git_exec::TAG_COMPARE);
    assert!(command.args.starts_with(&["diff".to_string(), "--no-index".to_string()]));
    assert!(command.args.ends_with(&[
        "/src/main.rs".to_string(),
        "/tmp/never-written-copy-main.rs".to_string()
    ]));
    assert_eq!(command.cwd, PathBuf::from("/"));
    editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(1),
        stderr: String::new(),
        stdout: COMPARED.to_string(),
        tag: git_exec::TAG_COMPARE,
    }));
    assert_eq!(editor.title(), "Diff: main.rs against the file on disk");
    let frame = editor.content(30, 80, false);
    let text: Vec<String> = frame.rows.iter().map(row_text).collect();
    assert!(text.iter().any(|row| row.contains("+B")), "opened to its lines: {text:?}");
    assert!(editor.snapshot().is_none(), "a comparison is not restored");
    let mut opened = None;
    for _ in 0..text.len() {
        opened = editor.file_at_point().filter(|target| target.line.is_some());
        if opened.is_some() {
            break;
        }
        editor.on_key(&press_char('j'));
    }
    assert_eq!(
        opened.map(|target| target.path),
        Some(PathBuf::from("/src/main.rs")),
        "a hunk opens the file the copy stands for"
    );
}

#[test]
fn test_two_same_files_say_so_and_leaving_a_comparison_closes_it() {
    let mut editor = comparison();
    editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(0),
        stderr: String::new(),
        stdout: String::new(),
        tag: git_exec::TAG_COMPARE,
    }));
    let frame = editor.content(30, 80, false);
    assert!(frame
        .rows
        .iter()
        .any(|row| row_text(row).contains("no differences")));
    assert_eq!(editor.on_key(&press_char('q')), EditorOutcome::Close);
}

#[test]
fn test_the_diff_tool_reads_the_working_tree_off_the_window_s_thread_in_its_own_shape() {
    // Read on the window's thread, a large file or a slow mount held the
    // window, and the diff read the file a second time to learn its line
    // endings. A file with no text to read was still a place to write the
    // empty side back to.
    let dir = std::env::temp_dir().join(format!("winter-head-compare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("crlf.txt"), "new\r\n").expect("file");
    std::fs::write(dir.join("binary.bin"), [0u8, 1, 2]).expect("file");
    let mut editor = GitEditor::new(dir.clone());
    editor.on_job(reply(git_exec::TAG_ROOT, &format!("{}\n{}/.git\n", dir.display(), dir.display())));
    let compared = |editor: &mut GitEditor, path: &str| {
        let head = CommandOutput {
            code: Some(0),
            stdout: "old\n".to_string(),
            tag: git_exec::TAG_SHOW_HEAD,
            ..CommandOutput::default()
        };
        assert_eq!(editor.head_read(&head, path.to_string()), EditorOutcome::Consumed, "not read here");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let EditorOutcome::Diff(request) = editor.on_tick(Instant::now()) {
                return request;
            }
            assert!(Instant::now() < deadline, "the read lands");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let text = compared(&mut editor, "crlf.txt");
    let worktree = &text.texts[1];
    assert_eq!(worktree.lines, ["new"]);
    assert!(worktree.shape.is_some_and(|shape| shape.crlf), "its line endings come with it");
    assert_eq!(worktree.path.as_deref(), Some(dir.join("crlf.txt").as_path()));
    let binary = compared(&mut editor, "binary.bin");
    assert_eq!(binary.texts[1].path, None, "no text, no file to write the empty side over");
    let _ = std::fs::remove_dir_all(&dir);
}
