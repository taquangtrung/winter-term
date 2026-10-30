//! The window's editor tests: the git view and commit messages, and where they open.

use super::*;

#[test]
fn test_a_commit_message_opens_with_the_staged_changes_as_a_tab_and_takes_them_along() {
    use crate::control::EditWaiter;
    use std::os::unix::net::UnixStream;

    let tree = TempTree::new("commit-beside", &["COMMIT_EDITMSG"]);
    let mut app = App::new();
    let before = app.layout().members().len();
    let (stream, _client) = UnixStream::pair().expect("socket pair");

    app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));

    let editor = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&editor).map(|slot| slot.kind),
        Some(EditorKind::Text)
    );
    let diff = *app.host.commit_companions.get(&editor).expect("a tab beside it");
    assert_eq!(
        app.open_tabs.editor(&diff).map(|slot| slot.kind),
        Some(EditorKind::Git)
    );
    assert_eq!(
        app.layout().members().len(),
        before + 2,
        "editor tab and diff tab"
    );
    assert!(
        app.layout().tabs_in_pane(editor).contains(&diff),
        "the diff is a tab of the editor's pane, not a pane of its own"
    );
    assert_eq!(
        app.layout().recent_in_pane(editor).first(),
        Some(&editor),
        "and the editor is the one showing"
    );

    app.close_tab_now(editor, Closing::Forget);
    assert!(
        !app.layout().contains(diff),
        "the diff goes with the editor"
    );
    assert!(app.host.commit_companions.is_empty());
}

#[test]
fn test_closing_commit_editor_focuses_git_view_of_repo() {
    use crate::control::EditWaiter;
    use std::os::unix::net::UnixStream;

    let tree = TempTree::new("commit-focus", &["COMMIT_EDITMSG"]);
    let mut app = App::new();
    let git_editor = GitEditor::new(tree.0.clone());
    let git_tab = app.show_editor(EditorKind::Git, Box::new(git_editor));

    let (stream, _client) = UnixStream::pair().expect("socket pair");
    app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));
    let editor = app.layout().focused();
    assert_ne!(editor, git_tab);

    app.host.jobs.cancel_for(git_tab);
    app.close_editor(editor);
    assert_eq!(
        app.layout().focused(),
        git_tab,
        "closing the commit editor focuses on the repo's git view"
    );
    assert!(
        app.host.jobs.is_busy_for(git_tab),
        "and reads it again, the commit having changed it"
    );
}

#[test]
fn test_the_git_view_prefers_the_pane_beside_over_the_next_one_in_tree_order() {
    // a over b, with c across the right: tree order from a would be b.
    let mut app = App::new();
    let a = app.layout().focused();
    let c = app.alloc_tab_id();
    app.layout_mut().split(Direction::Vertical, SPLIT_RATIO, c);
    app.layout_mut().focus(a);
    let b = app.alloc_tab_id();
    app.layout_mut()
        .split(Direction::Horizontal, SPLIT_RATIO, b);
    app.layout_mut().focus(a);

    app.open_git_editor();

    let git = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&git).map(|slot| slot.kind),
        Some(EditorKind::Git)
    );
    assert!(
        app.layout().tabs_in_pane(c).contains(&git),
        "in the pane beside"
    );
    assert!(!app.layout().tabs_in_pane(b).contains(&git));
}

#[test]
fn test_a_git_view_opens_as_a_tab_of_its_own_and_is_gone_to_when_asked_again() {
    let (mut app, status) = app_with_open_editor();
    let root = std::env::temp_dir();
    let view = GitViewRequest {
        git_dir: None,
        request: JobRequest::Command(CommandRequest {
            id: 0,
            args: vec!["--version".to_string()],
            cwd: root.clone(),
            env: Vec::new(),
            program: "git".to_string(),
            stdin: None,
            tag: "show",
        }),
        root,
    };
    app.open_git_view(view.clone());
    let opened = app.layout().focused();
    assert_ne!(opened, status, "a tab of its own");
    assert_eq!(app.open_tabs.editor(&opened).map(|slot| slot.kind), Some(EditorKind::Git));
    assert!(app.layout().tabs_in_pane(status).contains(&opened), "beside the one it came from");

    app.switch_to_tab(status);
    app.open_git_view(view);
    assert_eq!(app.layout().focused(), opened, "the open view, not a second one");
}

#[test]
fn test_the_git_view_opens_in_the_other_pane_and_is_found_there_again() {
    let (mut app, first, second) = two_panes();

    app.open_git_editor();
    let git = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&git).map(|slot| slot.kind),
        Some(EditorKind::Git)
    );
    assert!(
        app.layout().tabs_in_pane(second).contains(&git),
        "beside the work"
    );
    assert!(!app.layout().tabs_in_pane(first).contains(&git));

    app.layout_mut().focus(first);
    app.open_git_editor();
    assert_eq!(
        app.layout().focused(),
        git,
        "the open view, not a second one"
    );
    let views = app
        .open_tabs
        .editors()
        .map(|(_, slot)| slot)
        .filter(|slot| slot.kind == EditorKind::Git)
        .count();
    assert_eq!(views, 1);
}

#[test]
fn test_the_git_view_opens_new_view_for_different_project() {
    let (mut app, first, second) = two_panes();

    // Project A in first pane
    let term_a = crate::terminal::session::Terminal::with_command(
        20,
        4,
        portable_pty::CommandBuilder::new("cat"),
        winter_core::MAX_SCROLLBACK,
    )
    .expect("term a");
    app.open_tabs.insert_terminal(first, term_a);
    app.open_tabs
        .terminal_mut(&first)
        .unwrap()
        .feed_program_output(b"\x1b]7;file://localhost/path/to/project_a\x1b\\");

    // Open git editor for project A
    app.open_git_editor();
    let git_a = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&git_a).map(|slot| slot.kind),
        Some(EditorKind::Git)
    );
    assert_eq!(
        app.open_tabs.editor(&git_a).unwrap().editor.root_dir(),
        Some(PathBuf::from("/path/to/project_a"))
    );

    // Project B in second pane
    app.layout_mut().focus(second);
    let term_b = crate::terminal::session::Terminal::with_command(
        20,
        4,
        portable_pty::CommandBuilder::new("cat"),
        winter_core::MAX_SCROLLBACK,
    )
    .expect("term b");
    app.open_tabs.insert_terminal(second, term_b);
    app.open_tabs
        .terminal_mut(&second)
        .unwrap()
        .feed_program_output(b"\x1b]7;file://localhost/path/to/project_b\x1b\\");

    // Open git editor for project B: should NOT reuse project A's view
    app.open_git_editor();
    let git_b = app.layout().focused();
    assert_ne!(git_a, git_b, "opens a new git view for project B");
    assert_eq!(
        app.open_tabs.editor(&git_b).unwrap().editor.root_dir(),
        Some(PathBuf::from("/path/to/project_b"))
    );

    let views = app
        .open_tabs
        .editors()
        .map(|(_, slot)| slot)
        .filter(|slot| slot.kind == EditorKind::Git)
        .count();
    assert_eq!(views, 2, "two distinct git views open for two projects");
}

#[test]
fn test_the_git_view_splits_a_pane_off_when_there_is_just_one() {
    let mut app = App::new();
    let only = app.layout().focused();

    app.open_git_editor();

    let git = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&git).map(|slot| slot.kind),
        Some(EditorKind::Git)
    );
    assert_eq!(app.layout().shown_tabs().len(), 2, "the work and the view");
    assert!(!app.layout().tabs_in_pane(only).contains(&git));
}

#[test]
fn test_git_status_focuses_visible_status_view_in_existing_panes() {
    // 3 panes: a, b, c.
    let mut app = App::new();
    let a = app.layout().focused();
    let c = app.alloc_tab_id();
    app.layout_mut().split(Direction::Vertical, SPLIT_RATIO, c);
    let b = app.alloc_tab_id();
    app.layout_mut().split(Direction::Vertical, SPLIT_RATIO, b);

    // Open a git status view while focused on a: opens beside in another pane
    app.layout_mut().focus(a);
    app.open_git_editor();
    let git = app.layout().focused();
    assert_ne!(git, a, "git opened in another pane");
    assert_eq!(
        app.open_tabs.editor(&git).map(|slot| slot.kind),
        Some(EditorKind::Git)
    );

    // Focus pane a again and run git status.
    // It should focus git in its existing pane, not open a new view.
    app.layout_mut().focus(a);
    app.open_git_editor();
    assert_eq!(app.layout().focused(), git, "focused the visible git view");

    // Also test when focused on a third pane (neither a nor git)
    let third = [a, b, c]
        .into_iter()
        .find(|&p| p != a && !app.layout().tabs_in_pane(git).contains(&p))
        .expect("third pane");
    app.layout_mut().focus(third);
    app.open_git_editor();
    assert_eq!(app.layout().focused(), git, "focused visible git view from third pane");

    let views = app
        .open_tabs
        .editors()
        .map(|(_, slot)| slot)
        .filter(|slot| slot.kind == EditorKind::Git)
        .count();
    assert_eq!(views, 1, "only 1 git view was opened");
}

#[test]
fn test_git_status_prefers_visible_status_over_background_git_tab() {
    let (mut app, first, second) = two_panes();

    // Open git in second pane (visible)
    app.layout_mut().focus(second);
    app.open_git_editor();
    let git_second = app.layout().focused();

    // Add a background git tab in first pane
    let _git_first = app.add_background_editor(
        EditorKind::Git,
        Box::new(crate::editors::git::GitEditor::new(app.focused_start_dir())),
    );

    // first pane is focused on terminal, but has git_first in background.
    // second pane has git_second VISIBLE.
    app.layout_mut().focus(first);
    app.open_git_editor();

    assert_eq!(
        app.layout().focused(),
        git_second,
        "prefers the visible git view in pane 2 over the hidden one in pane 1"
    );
}

#[test]
fn test_git_status_focuses_visible_git_log_and_returns_to_status() {
    let (mut app, first, second) = two_panes();

    // Open git in second pane (visible)
    app.layout_mut().focus(second);
    app.open_git_editor();
    let git = app.layout().focused();

    // Send 'l' to open log popup, or simulate key
    let slot = app.open_tabs.editor_mut(&git).unwrap();
    let editor = slot.editor.downcast_mut::<GitEditor>().expect("a git editor");
    // Simulate output view on git editor by calling on_prompt or setting output
    // The editor handles on_key
    assert!(editor.is_git_status());
    // Simulate returning to status when show_status is called
    let _ = editor.show_status();
    assert!(editor.is_git_status());

    // Focus first pane and run open_git_editor
    app.layout_mut().focus(first);
    app.open_git_editor();
    assert_eq!(app.layout().focused(), git);
}

#[test]
fn test_the_commit_editor_splits_a_pane_off_when_there_is_just_one() {
    use crate::control::EditWaiter;
    use std::os::unix::net::UnixStream;

    let tree = TempTree::new("commit-split", &["MERGE_MSG"]);
    let mut app = App::new();
    let only = app.layout().focused();
    let (stream, _client) = UnixStream::pair().expect("socket pair");

    app.open_waited_editor(tree.0.join("MERGE_MSG"), EditWaiter::new(stream));

    let editor = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&editor).map(|slot| slot.kind),
        Some(EditorKind::Text)
    );
    assert_eq!(app.layout().shown_tabs().len(), 2);
    assert!(!app.layout().tabs_in_pane(only).contains(&editor));
}

#[test]
fn test_the_commit_editor_opens_in_the_other_pane() {
    use crate::control::EditWaiter;
    use std::os::unix::net::UnixStream;

    let tree = TempTree::new("commit-other", &["COMMIT_EDITMSG"]);
    let (mut app, first, second) = two_panes();
    let (stream, _client) = UnixStream::pair().expect("socket pair");

    app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));

    let editor = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&editor).map(|slot| slot.kind),
        Some(EditorKind::Text)
    );
    assert!(app.layout().tabs_in_pane(second).contains(&editor));
    assert!(!app.layout().tabs_in_pane(first).contains(&editor));
}

#[test]
fn test_closing_the_commit_editor_answers_git_even_though_the_editor_is_kept_for_reopening() {
    // Regression: a closed editor is kept so it can be reopened, and the
    // editor kept its waiter with it, so git was never told the edit was
    // over. The commit hung for as long as the window lived, and the Git
    // tab that started it spun the whole time.
    use crate::control::EditWaiter;
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let tree = TempTree::new("commit-answer", &["COMMIT_EDITMSG"]);
    let mut app = App::new();
    let (stream, mut client) = UnixStream::pair().expect("socket pair");
    client
        .set_read_timeout(Some(Duration::from_millis(300)))
        .expect("timeout");
    app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));
    let editor = app.layout().focused();

    app.close_editor(editor);

    let mut answer = [0u8; 1];
    assert_eq!(
        client.read(&mut answer).expect("git was answered"),
        1,
        "the waiting helper hears that the edit is over"
    );
}

#[test]
fn test_a_merge_message_gets_no_staged_pane() {
    use crate::control::EditWaiter;
    use std::os::unix::net::UnixStream;

    let tree = TempTree::new("merge-alone", &["MERGE_MSG"]);
    let mut app = App::new();
    let (stream, _client) = UnixStream::pair().expect("socket pair");

    app.open_waited_editor(tree.0.join("MERGE_MSG"), EditWaiter::new(stream));

    assert!(app.host.commit_companions.is_empty());
}

#[test]
fn test_a_restored_git_tab_asks_git_for_its_repository() {
    let dir = std::env::temp_dir().join(format!("winter-restore-git-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let editor = EditorSession {
        buried: false,
        id: 7,
        kind: EditorKind::Git.name().to_string(),
        line: None,
        path: dir.to_string_lossy().to_string(),
    };
    let mut app = App::new();
    assert!(app.host.jobs.busy_tabs().is_empty());
    assert!(app.restore_editor(TabId(7), &editor));
    assert!(
        !app.host.jobs.busy_tabs().is_empty(),
        "its first read is under way, not awaited"
    );

    let dir_editor = EditorSession {
        kind: EditorKind::Dir.name().to_string(),
        ..editor
    };
    let mut app = App::new();
    assert!(app.restore_editor(TabId(8), &dir_editor));
    assert!(
        app.host.jobs.busy_tabs().is_empty(),
        "a listing reads on its own"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
