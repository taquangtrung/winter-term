//! The git view's tests: watching the tree for changes.

use super::*;

#[test]
fn test_a_file_changed_on_disk_makes_the_editor_read_the_status_again() {
    let dir = std::env::temp_dir().join(format!(
        "winter-gitpage-watch-{:?}",
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).expect("git dir");
    let mut editor = GitEditor::new(dir.clone());
    let root = format!("{}\n{}\n", dir.display(), dir.join(".git").display());
    editor.on_job(reply(git_exec::TAG_ROOT, &root));
    editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT));
    // Set up off the UI thread, and taken over on a tick.
    let deadline = Instant::now() + Duration::from_secs(5);
    while editor.watch.tree_watch.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        editor.adopt_started_watch();
    }
    assert!(editor.watch.tree_watch.is_some(), "the platform can watch it");
    std::thread::sleep(Duration::from_millis(150));

    std::fs::write(dir.join("new.rs"), "fn main() {}").expect("write");
    std::thread::sleep(Duration::from_millis(300));

    let now = Instant::now();
    assert_eq!(
        editor.on_tick(now),
        EditorOutcome::Consumed,
        "waiting out the burst"
    );
    let asked = editor.on_tick(now + Duration::from_secs(4));
    assert_eq!(command_of(&asked).tag, git_exec::TAG_STATUS);
    assert_eq!(
        editor.on_tick(now + Duration::from_secs(8)),
        EditorOutcome::Consumed,
        "one read at a time"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_change_to_the_files_reads_the_status_and_one_in_git_reads_everything() {
    let mut editor = loaded_editor();
    let changed = STATUS_OUTPUT.replace("? new.rs", "? newer.rs");

    editor.watch.watching = Some(super::watch::Read::Status);
    assert_eq!(
        editor.on_job(reply(git_exec::TAG_STATUS, &changed)),
        EditorOutcome::Consumed,
        "the files changed: no log, stash, or unpulled reads"
    );
    assert!(editor
        .rows
        .iter()
        .any(|row| matches!(&row.item, Item::File(file) if file.path == "newer.rs")));

    // The same status again, but the change was a commit or a fetch.
    editor.watch.watching = Some(super::watch::Read::Full);
    let next = editor.on_job(reply(git_exec::TAG_STATUS, &changed));
    assert_eq!(command_of(&next).tag, git_exec::TAG_LOG, "everything is read");
}

#[test]
fn test_a_quiet_tree_is_read_again_without_redrawing_anything() {
    let mut editor = loaded_editor();
    editor.watch.watching = Some(super::watch::Read::Status);

    // Nothing changed: no rows rebuilt, no log or stash reads chained on.
    let rows = editor.rows.len();
    assert_eq!(
        editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT)),
        EditorOutcome::Consumed
    );
    assert_eq!(editor.rows.len(), rows);
}

#[test]
fn test_the_watch_leaves_alone_a_view_the_user_is_reading() {
    let dir = std::env::temp_dir().join(format!(
        "winter-gitpage-reading-{:?}",
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).expect("git dir");
    let mut editor = loaded_editor();
    let starting = super::watch::TreeWatch::start_in_background(&dir, None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !starting.is_done() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    editor.watch.tree_watch_starting = Some(starting);
    assert!(editor.next_tick().is_some(), "a watch to take over");
    editor.show_output("Log", "abc one\n", false);
    assert_eq!(editor.next_tick(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_buried_view_lets_its_watch_go_and_watches_anew_when_shown() {
    let mut editor = loaded_editor();
    assert!(editor.watch.tree_watch_starting.is_some(), "watching once the root is known");
    editor.set_shown(false);
    assert!(editor.watch.tree_watch.is_none() && editor.watch.tree_watch_starting.is_none());
    assert_eq!(editor.next_tick(), None, "nothing to look at");
    editor.set_shown(true);
    assert!(editor.watch.tree_watch_starting.is_some(), "watching again");
}

#[test]
fn test_a_repository_that_cannot_be_watched_is_not_read_on_a_timer() {
    // The view is passive: with no watch it waits to be asked, and says once
    // that it will.
    let mut editor = loaded_editor();
    let starting = super::watch::TreeWatch::start_in_background(Path::new("/no/such/repo"), None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !starting.is_done() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    editor.watch.tree_watch_starting = Some(starting);
    assert_eq!(editor.on_tick(Instant::now()), EditorOutcome::Consumed);
    assert!(editor.take_notice().is_some(), "said once");
    assert_eq!(editor.next_tick(), None, "and never polled");
    assert_eq!(
        editor.on_tick(Instant::now() + Duration::from_secs(60)),
        EditorOutcome::Consumed
    );
}
