//! The text editor's tests: reading, saving, and files changed on disk.

use super::*;

#[test]
fn test_a_file_changed_underneath_a_clean_editor_is_read_again() {
    let tmp = TempDir::new("resume");
    let mut editor = tmp.editor("before\n");
    std::fs::write(tmp.0.join("a.txt"), "after\n").expect("rewrite");
    // The stamp is the file's mtime, which a second write may share.
    editor.doc().set_stamp(text_file::FileStamp::default());
    editor.on_resume();
    assert_eq!(editor.buffer().lines(), ["after"]);
}

#[test]
fn test_saving_writes_the_text_and_clears_the_unsaved_mark() {
    let tmp = TempDir::new("save");
    let mut editor = tmp.editor("one\ntwo\n");
    press(&mut editor, "x");
    assert!(editor.buffer().is_dirty());

    editor.on_key(&ctrl('s'));
    assert!(!editor.buffer().is_dirty());
    let written = std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads");
    assert_eq!(written, "ne\ntwo\n");
}

#[test]
fn test_saving_while_typing_goes_back_to_normal_as_escape_would() {
    let tmp = TempDir::new("save-normal");
    let mut editor = tmp.editor("one\n");
    press(&mut editor, "A!");
    assert_eq!(editor.modal.mode(), EditMode::Insert);

    editor.on_key(&ctrl('s'));
    assert_eq!(editor.modal.mode(), EditMode::Normal);
    assert_eq!(editor.buffer().col(), 3, "onto the character just typed");
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "one!\n"
    );
}

#[test]
fn test_saving_mid_selection_lets_the_selection_go() {
    let tmp = TempDir::new("save-visual");
    let mut editor = tmp.editor("one\n");
    press(&mut editor, "vl");
    assert_eq!(editor.modal.mode(), EditMode::Visual);

    editor.on_key(&ctrl('s'));
    assert_eq!(editor.modal.mode(), EditMode::Normal);
}

#[test]
fn test_ctrl_shift_s_saves_as_ctrl_s_does_rather_than_leaving_it_to_grep() {
    let tmp = TempDir::new("save-shift");
    let mut editor = tmp.editor("one\n");
    press(&mut editor, "x");

    let outcome = editor.on_key(&Key {
        shift: true,
        ..ctrl('s')
    });
    assert!(
        !matches!(outcome, EditorOutcome::Ignored),
        "the editor keeps it"
    );
    assert!(!editor.buffer().is_dirty());
    let written = std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads");
    assert_eq!(written, "ne\n");
}

#[test]
fn test_saving_over_a_file_changed_underneath_asks_first() {
    // Overwriting silently is the one thing an editor does that cannot be
    // undone from inside it.
    let tmp = TempDir::new("stale");
    let mut editor = tmp.editor("one\n");
    press(&mut editor, "x");
    std::fs::write(tmp.0.join("a.txt"), "written elsewhere\n").expect("outside write");

    let outcome = editor.on_key(&ctrl('s'));
    let EditorOutcome::Prompt(request) = outcome else {
        panic!("expected a question, got {outcome:?}");
    };
    assert_eq!(request.tag, ASK_OVERWRITE);
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "written elsewhere\n",
        "nothing was written while the question was open"
    );
}

#[test]
fn test_the_file_opens_on_the_line_the_editor_was_pointing_at() {
    // A grep hit opened at line one throws away what was searched for.
    let tmp = TempDir::new("line");
    let path = tmp.0.join("a.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").expect("temp file");
    let editor = TextEditor::new(path, Some(3)).expect("opens");
    assert_eq!(editor.buffer().row(), 2);
}

#[test]
fn test_handing_off_to_the_outside_editor_saves_first_and_carries_the_line() {
    // `$EDITOR` reads the file from disk, so unsaved edits would not be
    // there; landing it at line one would lose the place as well.
    let tmp = TempDir::new("escalate");
    let mut editor = tmp.editor("one\ntwo\nthree\n");
    press(&mut editor, "jx");
    let outcome = editor.on_key(&ctrl('o'));
    assert_eq!(
        outcome,
        EditorOutcome::SpawnEditor(OpenTarget::at_line(tmp.0.join("a.txt"), 2))
    );
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "one\nwo\nthree\n"
    );
}

#[cfg(unix)]
#[test]
fn test_a_file_the_user_cannot_write_asks_to_save_as_root_and_stays_open() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("as-root");
    let mut editor = tmp.editor("one\n");
    let path = tmp.0.join("a.txt");
    // A read-only file in a read-only directory: neither a rename beside it
    // nor a write in place can land.
    let locked = |dir, file| {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(file))
            .and_then(|()| std::fs::set_permissions(&tmp.0, std::fs::Permissions::from_mode(dir)))
    };
    locked(0o555, 0o444).expect("read-only");
    // Root writes anywhere, so there is nothing to ask it for.
    if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
        locked(0o755, 0o644).expect("writable again");
        return;
    }
    press(&mut editor, "x");
    editor.on_key(&key(KeyCode::Char('Z')));
    let outcome = editor.on_key(&key(KeyCode::Char('Z')));
    locked(0o755, 0o644).expect("writable again");

    let EditorOutcome::Prompt(request) = outcome else {
        panic!("ZZ asks rather than closing over the edit: {outcome:?}");
    };
    assert_eq!(request.tag, ASK_AS_ROOT);
    assert_eq!(request.mode, PromptMode::Confirm);
    assert!(editor.is_dirty(), "the edit is still held");
    assert_eq!(std::fs::read_to_string(&path).expect("reads"), "one\n");
}

#[test]
fn test_an_unread_buffer_reads_its_file_before_taking_any_edit() {
    let tmp = TempDir::new("unread");
    let path = tmp.file("a.txt", "one\ntwo\n");
    let unread = || TextEditor::with_doc(Document::unread(path.clone(), Some(2)), EditMode::Normal);

    let mut keyed = unread();
    keyed.on_key(&key(KeyCode::Char('x')));
    assert_eq!(
        keyed.buffer().lines(),
        ["one", "two"],
        "the key read, and edited nothing"
    );
    assert_eq!(keyed.buffer().row(), 1, "at the line it was restored on");

    let mut pasted = unread();
    pasted.on_paste("pasted".to_string());
    assert_eq!(pasted.buffer().lines(), ["one", "two"]);

    let mut saved = unread();
    saved.write_file();
    assert_eq!(
        std::fs::read_to_string(&path).expect("reads"),
        "one\ntwo\n",
        "a save reads first rather than writing the empty stand-in over the file"
    );
    assert!(!saved.is_dirty());
}

#[test]
fn test_a_restored_remote_buffer_waits_to_be_shown_before_reaching_out() {
    // Nothing is run: a host that is not there would otherwise hold the
    // restart until it timed out.
    let editor = TextEditor::restored(PathBuf::from("ssh://nowhere.invalid/etc/motd"), Some(3))
        .expect("restores without reading");
    assert!(editor.doc().unread);
    assert_eq!(editor.title(), "motd");
    assert_eq!(
        editor.cwd(),
        None,
        "no local shell can start in a remote directory"
    );
}

#[test]
fn test_a_file_changed_elsewhere_is_read_in_unless_it_is_edited_here_too() {
    let tmp = TempDir::new("disk-watch");
    let path = tmp.file("notes.txt", "one\n");
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    assert!(editor.next_tick().is_some(), "it looks at the disk");
    std::fs::write(&path, "one\ntwo, written elsewhere\n").expect("an outside write");
    let later = Instant::now() + DISK_CHECK * 2;
    editor.on_tick(later);
    assert_eq!(editor.buffer().lines(), ["one", "two, written elsewhere"]);
    assert_eq!(
        editor.take_notice().map(|n| n.text),
        Some("changed on disk, re-read".to_string())
    );

    press(&mut editor, "ix");
    editor.on_key(&key(KeyCode::Escape));
    std::fs::write(&path, "replaced\n").expect("another outside write");
    editor.on_tick(later + DISK_CHECK * 2);
    assert_eq!(editor.buffer().lines()[0], "xone", "the edits here are kept");
    assert!(editor.take_notice().is_some_and(|n| n.text.contains("edited here too")));
    editor.on_tick(later + DISK_CHECK * 4);
    assert!(editor.take_notice().is_none(), "said once");
}

#[test]
fn test_a_save_from_a_diff_goes_through_the_open_buffer_in_the_file_s_own_shape() {
    // Written straight to disk, the merge left the tab on the file holding
    // its older text, which its next save put back; and the file's line
    // endings were whatever the diff had guessed.
    let tmp = TempDir::new("save-through");
    let path = tmp.file("crlf.txt", "one\r\ntwo\r\n");
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    let merged = vec!["one".to_string(), "merged".to_string()];
    let written = crate::editors::text::save_through(&path, &merged);
    assert!(matches!(written, Some(Ok(_))), "the open buffer took it");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\r\nmerged\r\n");
    editor.catch_up_buffers();
    assert_eq!(editor.buffer().lines(), merged.as_slice(), "the tab shows what was written");
    assert!(!editor.is_dirty(), "and has nothing left to save");
    // A file no tab has open is the caller's to write.
    let closed = tmp.0.join("closed.txt");
    assert!(crate::editors::text::save_through(&closed, &merged).is_none());
}
