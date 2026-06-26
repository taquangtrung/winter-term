//! The status view's tests.

use super::reply::browser_url;
use super::*;
use crate::model::notify::Severity;
use crate::model::tool::{CommandOutput, CommandRequest};

const STATUS_OUTPUT: &str = "\
# branch.head main
# branch.upstream origin/main
# branch.ab +1 -0
1 M. N... 100644 100644 100644 aaa bbb staged.rs
1 .M N... 100644 100644 100644 aaa bbb working.rs
? new.rs
";

fn press(code: KeyCode) -> Key {
    Key {
        alt: false,
        code,
        ctrl: false,
        shift: false,
        sup: false,
    }
}

fn alt(code: KeyCode) -> Key {
    Key {
        alt: true,
        code,
        ctrl: false,
        shift: false,
        sup: false,
    }
}

fn ctrl_shift(c: char) -> Key {
    Key {
        alt: false,
        code: KeyCode::Char(c),
        ctrl: true,
        shift: true,
        sup: false,
    }
}

fn reply(tag: &'static str, stdout: &str) -> JobReply {
    JobReply::Command(CommandOutput {
        code: Some(0),
        stderr: String::new(),
        stdout: stdout.to_string(),
        tag,
    })
}

/// The command an outcome runs, with the settings every git request starts
/// with (`-c core.quotePath=false`) taken off its arguments, so a test reads
/// the command itself.
fn command_of(outcome: &ToolOutcome) -> CommandRequest {
    match outcome {
        ToolOutcome::Job(JobRequest::Command(command)) => {
            let mut command = command.clone();
            assert_eq!(command.args[..2], ["-c", "core.quotePath=false"]);
            command.args.drain(..2);
            command
        }
        other => panic!("expected a command, got {other:?}"),
    }
}

/// A tool that has heard back about its root, its status, and its log.
fn ctrl(code: KeyCode) -> Key {
    Key {
        alt: false,
        code,
        ctrl: true,
        shift: false,
        sup: false,
    }
}

/// One line of the decorated log format the view asks git for, so a test
/// feeds the shape [`parse::parse_decorated_log`] actually reads.
fn log_output(commits: &[(&str, &str)]) -> String {
    let sep = parse::FIELD_SEP;
    commits
        .iter()
        .map(|(hash, subject)| format!("{hash}{sep}{sep}Someone{sep}1700000000{sep}{subject}\n"))
        .collect()
}

#[test]
fn test_a_chord_with_an_extra_modifier_is_left_to_the_window() {
    // `C-S-d` is the window's `dir_page`; it must not be read as
    // the `C-d` half page, and `C-M-n` is not `M-n`.
    let mut tool = loaded_tool();
    for key in [
        ctrl_shift('d'),
        ctrl_shift('u'),
        ctrl_shift('l'),
        ctrl_shift('o'),
        Key {
            ctrl: true,
            ..alt(KeyCode::Char('n'))
        },
    ] {
        assert_eq!(tool.on_key(&key), ToolOutcome::Ignored, "{key:?}");
    }
}

#[test]
fn test_shift_g_and_g_are_different_keys_as_the_window_delivers_them() {
    // The window delivers S-g as `g` with Shift held, not as `G`.
    let shift_g = Key {
        shift: true,
        sup: false,
        ..Key::plain(KeyCode::Char('g'))
    };
    let mut tool = loaded_tool();
    assert!(matches!(tool.on_key(&shift_g), ToolOutcome::Job(_)));
    let mut tool = loaded_tool();
    assert_eq!(
        tool.on_key(&Key::plain(KeyCode::Char('g'))),
        ToolOutcome::Consumed
    );
    assert!(tool.popup.is_some(), "plain g opens the jump menu");
}

fn loaded_tool() -> GitTool {
    let mut tool = GitTool::new(PathBuf::from("/repo/sub"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
    tool.on_job(reply(
        exec::TAG_LOG,
        &log_output(&[("abc1234", "do the thing")]),
    ));
    tool
}

#[test]
fn test_the_address_is_the_repository_whatever_the_cursor_is_on() {
    // Before the repository is known there is nothing to name; afterwards
    // the title bar names the repository, the same text the `Repo:` row
    // carries, and not the folder of the file the cursor rests on.
    let mut tool = GitTool::new(PathBuf::from("/repo/sub"));
    assert_eq!(tool.address(), None);
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    assert_eq!(tool.address().as_deref(), Some("/repo"));
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "staged.rs");
    assert_eq!(tool.address().as_deref(), Some("/repo"));
}

#[test]
fn test_tag_root_failure_shows_error_instead_of_loading() {
    let mut tool = GitTool::new(PathBuf::from("/not-a-repo"));
    tool.on_job(JobReply::Command(CommandOutput {
        code: Some(128),
        stderr: "fatal: not a git repository (or any of the parent directories): .git".to_string(),
        stdout: String::new(),
        tag: exec::TAG_ROOT,
    }));
    let text = painted_text(&mut tool);
    assert!(text.contains(NOT_A_REPO), "{text}");
    assert!(!text.contains("fatal"), "git's words go to the status bar");
    let notice = tool.take_notice().expect("a failure is reported");
    assert_eq!(notice.severity, Severity::Error);
    assert!(
        notice
            .text
            .starts_with("failed: fatal: not a git repository"),
        "{}",
        notice.text
    );
}

#[test]
fn test_a_status_that_times_out_shows_the_failure_not_the_loading_note() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(JobReply::Command(CommandOutput {
        code: None,
        stderr: "command timed out after 15s".to_string(),
        stdout: String::new(),
        tag: exec::TAG_STATUS,
    }));
    let text = painted_text(&mut tool);
    assert!(text.contains(UNREADABLE), "{text}");
    assert!(!text.contains(LOADING));
    let notice = notice_text(&mut tool).unwrap_or_default();
    assert!(notice.contains("command timed out after 15s"), "{notice}");
}

#[test]
fn test_a_report_goes_to_the_status_bar_not_the_header() {
    // The header is the repository's state; a report drawn there pushed
    // the sections down a row and stayed until the next key.
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_FETCH, ""));
    assert!(!painted_text(&mut tool).contains("fetched"));
    let notice = tool.take_notice().expect("the fetch is reported");
    assert_eq!(notice, Notice::info("fetched"));
    assert_eq!(tool.take_notice(), None, "a report is shown once");
}

/// Everything the tool paints at a usual size, joined.
fn painted_text(tool: &mut GitTool) -> String {
    tool.content(24, 80, false)
        .rows
        .iter()
        .flatten()
        .map(|span| span.text.as_str())
        .collect()
}

/// The text of what the tool has to report, taking it.
fn notice_text(tool: &mut GitTool) -> Option<String> {
    tool.take_notice().map(|notice| notice.text)
}

#[test]
fn test_the_wheel_moves_the_view_not_the_cursor_and_a_key_returns_to_it() {
    // Like scrolling an editor: the cursor stays on its row while the
    // view moves, hidden when it has left the screen, and the next key
    // brings the view back to it.
    let mut tool = loaded_tool();
    tool.list.set_cursor(0);
    tool.content(8, 80, false);
    assert_eq!(tool.on_scroll(-3), ToolOutcome::Consumed);
    let shown = tool.content(8, 80, false);
    assert_eq!(tool.list.view().scroll(), 3);
    assert_eq!(tool.list.cursor(), 0, "the cursor did not move");
    assert_eq!(shown.cursor_line, None, "and it is off screen");
    tool.on_scroll(3);
    tool.content(8, 80, false);
    assert_eq!(tool.list.view().scroll(), 0);
    tool.on_scroll(-3);
    tool.content(8, 80, false);
    tool.on_key(&press_char('j'));
    let shown = tool.content(8, 80, false);
    assert_eq!(
        tool.list.view().scroll(),
        tool.list.cursor(),
        "a key reveals the cursor at the edge"
    );
    assert!(shown.cursor_line.is_some());
}

fn cursor_on(tool: &mut GitTool, path: &str) {
    let index = tool
        .rows
        .iter()
        .position(|row| matches!(&row.item, Item::File(file) if file.path == path))
        .unwrap_or_else(|| panic!("no row for {path}"));
    tool.list.set_cursor(index);
}

#[test]
fn test_resolving_a_conflict_takes_one_side_then_stages_the_file() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(
        exec::TAG_STATUS,
        "# branch.head main\nu UU N... 100644 100644 100644 100644 a b c fought.rs\n",
    ));
    cursor_on(&mut tool, "fought.rs");
    tool.on_key(&press(KeyCode::Char('.')));
    let command = command_of(&tool.on_key(&press(KeyCode::Char('t'))));
    assert_eq!(command.args, ["checkout", "--theirs", "--", "fought.rs"]);
    let next = command_of(&tool.on_job(reply(exec::TAG_RESOLVE, "")));
    assert_eq!(
        next.args,
        ["add", "--", "fought.rs"],
        "staging marks it resolved"
    );
}

#[test]
fn test_every_menu_entry_runs_a_key_the_view_binds() {
    // The menu offers keys rather than commands of its own, so an entry
    // naming a chord the tool does not match does nothing when chosen.
    // The commit entry is `C-S-c`, which is exactly the shape that
    // gets written with a modifier missing.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let over_file = tool.context_items();

    let mut header = loaded_tool();
    header.list.set_cursor(0);
    let over_header = header.context_items();

    for (items, on_file) in [(over_file, true), (over_header, false)] {
        assert!(!items.is_empty());
        for item in items.into_iter().filter(|item| !item.is_separator()) {
            let mut probe = loaded_tool();
            match on_file {
                true => cursor_on(&mut probe, "working.rs"),
                false => probe.list.set_cursor(0),
            }
            assert_ne!(
                probe.on_key(&item.keys[0]),
                ToolOutcome::Ignored,
                "the menu offers {:?}, which the view does not bind",
                item.label
            );
        }
    }
}

#[test]
fn test_the_menu_offers_each_row_what_that_row_can_do() {
    // A commit cannot be staged and a file has no hash to copy, so a menu
    // built without looking at the row under it offers both everywhere.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let labels: Vec<String> = tool
        .context_items()
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(labels.contains(&LABEL_STAGE.to_string()));
    assert!(!labels.contains(&LABEL_COPY_HASH.to_string()));

    let commit = tool
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("the log put a commit in the view");
    tool.list.set_cursor(commit);
    let labels: Vec<String> = tool
        .context_items()
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(labels.contains(&LABEL_COPY_HASH.to_string()));
    assert!(!labels.contains(&LABEL_STAGE.to_string()));
}

#[test]
fn test_the_root_lookup_leads_to_a_status_read() {
    // Every later command runs from the root, so the view cannot do
    // anything until that answer arrives.
    let mut tool = GitTool::new(PathBuf::from("/repo/sub"));
    let next = tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    let command = command_of(&next);
    assert_eq!(command.tag, exec::TAG_STATUS);
    assert_eq!(command.cwd, PathBuf::from("/repo"), "not the subdirectory");
}

#[test]
fn test_a_status_read_is_followed_by_the_recent_log() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    let next = tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
    assert_eq!(command_of(&next).tag, exec::TAG_LOG);
}

#[test]
fn test_plus_asks_the_status_view_for_more_recent_commits() {
    // Only a list that filled its request can have more behind it; a short
    // one has shown everything there is, so the key has nothing to ask.
    let mut tool = loaded_tool();
    let before = tool.recent_count;
    let short = tool.on_key(&press_char('+'));
    assert!(!matches!(short, ToolOutcome::Job(_)), "nothing to ask for");
    assert_eq!(tool.recent_count, before, "one commit is not a full list");

    // Exactly a page: the first commit is on screen, so no more.
    tool.on_job(reply(exec::TAG_LOG, &commits_output(before)));
    assert!(!offers_more(&tool), "the first commit is already shown");
    assert!(!matches!(
        tool.on_key(&press_char('+')),
        ToolOutcome::Job(_)
    ));

    // One past the page came back: more to load, and only a page shown.
    tool.on_job(reply(exec::TAG_LOG, &commits_output(before + 1)));
    assert_eq!(tool.commits.len(), before, "the peeked commit is not shown");
    assert!(offers_more(&tool));
    let more = tool.on_key(&press_char('+'));
    assert!(tool.recent_count > before, "asked for more");
    let command = command_of(&more);
    assert_eq!(command.tag, exec::TAG_LOG);
    assert!(
        command.args.contains(&format!(
            "--max-count={}",
            exec::with_peek(tool.recent_count)
        )),
        "{:?}",
        command.args
    );
}

#[test]
fn test_staging_acts_on_the_path_under_the_cursor() {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let command = command_of(&tool.on_key(&press(KeyCode::Char('s'))));
    assert_eq!(command.args, ["add", "--", "working.rs"]);
}

#[test]
fn test_staging_on_a_heading_acts_on_that_whole_section() {
    // The heading stands for its section, which is what makes "stage this
    // lot" one keystroke.
    let mut tool = loaded_tool();
    let at = tool
        .rows
        .iter()
        .position(|row| row.item == Item::Heading(Section::Unstaged))
        .expect("the unstaged heading");
    tool.list.set_cursor(at);
    let command = command_of(&tool.on_key(&press(KeyCode::Char('s'))));
    assert_eq!(command.args, ["add", "--", "working.rs"]);
}

#[test]
fn test_stage_all_from_the_untracked_section_stages_only_untracked_files() {
    // Magic's stage-all splits by section: from untracked it stages the
    // files that section lists, by name.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "new.rs");
    let command = command_of(&tool.on_key(&ctrl_shift('s')));
    assert_eq!(command.args, ["add", "--", "new.rs"]);
}

#[test]
fn test_stage_all_elsewhere_stages_tracked_changes_only() {
    // Anywhere else it is `add -u`, so an untracked file stays that way
    // until asked for by name.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let command = command_of(&tool.on_key(&ctrl_shift('s')));
    assert_eq!(command.args, ["add", "-u"]);
}

#[test]
fn test_stage_untracked_stages_every_untracked_file_from_anywhere() {
    // Unlike stage-all, the cursor's section does not matter: from a tracked
    // change it still stages the untracked files, and only those.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let command = command_of(&tool.on_key(&ctrl_shift('a')));
    assert_eq!(command.args, ["add", "--", "new.rs"]);
}

#[test]
fn test_unstaging_uses_the_staged_copy_of_a_file_changed_on_both_sides() {
    // `staged.rs` and `working.rs` differ, but a file in both sections must
    // unstage from the staged row and stage from the unstaged one.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "staged.rs");
    let command = command_of(&tool.on_key(&press(KeyCode::Char('u'))));
    assert_eq!(command.args, ["restore", "--staged", "--", "staged.rs"]);
}

#[test]
fn test_discarding_asks_first_and_names_the_file() {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let ToolOutcome::Prompt(request) = tool.on_key(&press(KeyCode::Char('x'))) else {
        panic!("expected a confirmation");
    };
    assert_eq!(request.mode, PromptMode::Confirm);
    assert!(
        request.label.contains("working.rs"),
        "got {:?}",
        request.label
    );
}

#[test]
fn test_discarding_an_untracked_file_deletes_it_instead_of_restoring() {
    // `restore` cannot reach a path that is not in the index, so the view
    // would report success while leaving the file exactly where it was.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "new.rs");
    tool.on_key(&press(KeyCode::Char('x')));
    let command = command_of(&tool.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: ASK_DISCARD,
    }));
    assert_eq!(command.args, ["clean", "--force", "-d", "--", "new.rs"]);
}

#[test]
fn test_declining_the_confirmation_runs_nothing() {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    tool.on_key(&press(KeyCode::Char('x')));
    let outcome = tool.on_prompt(PromptReply {
        answer: None,
        tag: ASK_DISCARD,
    });
    assert_eq!(outcome, ToolOutcome::Consumed);
}

#[test]
fn test_committing_nothing_is_refused_before_it_asks() {
    // `git commit` with an empty index fails with a wall of advice; not
    // asking for a message is the better answer.
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(exec::TAG_STATUS, "# branch.head main\n"));
    let outcome = tool.on_key(&Key {
        alt: false,
        code: KeyCode::Char('c'),
        ctrl: true,
        shift: true,
        sup: false,
    });
    assert_eq!(outcome, ToolOutcome::Consumed);
}

#[test]
fn test_a_command_that_changed_the_tree_re_reads_it() {
    // Without the re-read the view shows the state before the change, and
    // the next key acts on rows that no longer describe the repository.
    let mut tool = loaded_tool();
    let next = tool.on_job(reply(exec::TAG_STAGE, ""));
    assert_eq!(command_of(&next).tag, exec::TAG_STATUS);
    assert_eq!(
        tool.take_notice(),
        None,
        "the section headings already say it"
    );
}

#[test]
fn test_a_failed_command_reports_and_does_not_re_read() {
    let mut tool = loaded_tool();
    let outcome = tool.on_job(JobReply::Command(CommandOutput {
        code: Some(1),
        stderr: "error: pathspec 'nope' did not match\n".to_string(),
        stdout: String::new(),
        tag: exec::TAG_STAGE,
    }));
    assert_eq!(outcome, ToolOutcome::Consumed);
    let message = notice_text(&mut tool).unwrap_or_default();
    assert!(message.starts_with("failed:"), "got {message:?}");
    assert!(message.contains("pathspec"), "got {message:?}");
}

const DIFF_OUTPUT: &str = "\
diff --git a/working.rs b/working.rs
index aaa..bbb 100644
--- a/working.rs
+++ b/working.rs
@@ -1,3 +1,3 @@
 fn main() {
-    old();
+    new();
@@ -10,2 +10,3 @@ fn other() {
 keep();
+    added();
";

fn cursor_on_hunk(tool: &mut GitTool, index: usize) {
    let position = tool
        .rows
        .iter()
        .position(|row| matches!(&row.item, Item::Hunk(hunk) if hunk.index == index))
        .unwrap_or_else(|| panic!("no row for hunk {index}"));
    tool.list.set_cursor(position);
}

/// A tool with `working.rs` expanded and its diff read.
fn tool_with_diff() -> GitTool {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    tool.on_key(&press(KeyCode::Tab));
    tool.on_job(reply(exec::TAG_DIFF, DIFF_OUTPUT));
    tool
}

#[test]
fn test_enter_on_a_hunk_opens_its_file_at_the_line_the_hunk_changes() {
    // Opening the file at line one from a hunk throws away the one thing
    // the cursor's position said: which part of it is being read.
    let mut tool = tool_with_diff();
    cursor_on_hunk(&mut tool, 1);
    assert_eq!(
        tool.on_key(&press(KeyCode::Enter)),
        ToolOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 10))
    );

    // And the file's own band still opens it at the top, since it names
    // no line in particular.
    cursor_on(&mut tool, "working.rs");
    assert_eq!(
        tool.on_key(&press(KeyCode::Enter)),
        ToolOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/working.rs")))
    );
}

#[test]
fn test_enter_in_a_commits_patch_opens_the_file_that_row_belongs_to() {
    // A commit is read as its patch, and every file band and hunk in it
    // stands for a file in the working tree: Enter did nothing there.
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));

    output_cursor_on(&mut tool, "src/a.rs");
    assert_eq!(
        tool.on_key(&press(KeyCode::Enter)),
        ToolOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/src/a.rs"))),
        "the file band opens the file"
    );

    // Open the file's diff, then land on its second hunk.
    tool.on_key(&press(KeyCode::Tab));
    let output = tool.output.as_mut().expect("an output view");
    let at = output
        .rows
        .iter()
        .position(|row| row.item == Item::CommitHunk(0, 1))
        .expect("a row for the second hunk");
    output.list.set_cursor(at);
    assert_eq!(
        tool.on_key(&press(KeyCode::Enter)),
        ToolOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 9)),
        "and a hunk opens it at the line it changes"
    );
}

#[test]
fn test_enter_in_a_blame_opens_the_file_at_the_line_under_the_cursor() {
    // A blame is one row per line of the file, so the row the cursor is
    // on is the line to land on: opening at the top would throw away the
    // only thing the reader was pointing at.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    tool.on_key(&alt(KeyCode::Char('b')));
    tool.on_job(reply(
        exec::TAG_BLAME,
        "abc1234 (Someone 2026-09-18 1) fn main() {\n             abc1234 (Someone 2026-09-18 2)     old();\n             def5678 (Another 2026-09-18 3) }\n",
    ));

    let output = tool.output.as_mut().expect("the blame");
    output.list.set_cursor(2);
    assert_eq!(
        tool.on_key(&press(KeyCode::Enter)),
        ToolOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 3))
    );
}

#[test]
fn test_enter_on_a_row_belonging_to_no_file_is_left_alone() {
    // A log's rows stand for nothing to open, and swallowing the key
    // there would take it from the window for no gain.
    let mut tool = loaded_tool();
    tool.on_job(reply(
        exec::TAG_LOG,
        &log_output(&[("abc1234", "a commit")]),
    ));
    tool.on_key(&ctrl_shift('l'));
    if tool.output.is_some() {
        assert_eq!(tool.on_key(&press(KeyCode::Enter)), ToolOutcome::Ignored);
    }

    // In the status view, a section heading names a set of files rather
    // than one, so Enter has nothing to open there either.
    let mut status = loaded_tool();
    let heading = status
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Heading(_)))
        .expect("a heading row");
    status.list.set_cursor(heading);
    assert_eq!(status.on_key(&press(KeyCode::Enter)), ToolOutcome::Consumed);
}

#[test]
fn test_unfolding_a_file_in_a_short_pane_keeps_its_row_where_it_was() {
    // Unfolding used to scroll the opened row to the top of the pane
    // whenever its diff ran past the bottom, moving the line the reader
    // had just pressed Tab on.
    let mut tool = tool_with_diff();
    tool.on_key(&press(KeyCode::Tab));
    tool.content(6, 80, false);
    let (cursor, scroll) = (tool.list.cursor(), tool.list.view().scroll());
    tool.on_key(&press(KeyCode::Tab));
    tool.content(6, 80, false);
    assert_eq!(
        (tool.list.cursor(), tool.list.view().scroll()),
        (cursor, scroll),
        "the row stays on the same line of the pane"
    );
}

#[test]
fn test_opening_a_file_already_showing_in_full_leaves_the_tool_alone() {
    // The scroll only moves for a block that runs past the pane's bottom:
    // one that fits must not shift the tool under the reader.
    let mut tool = tool_with_diff();
    tool.content(40, 80, false);
    let before = tool.list.view().scroll();
    tool.on_key(&press(KeyCode::Tab));
    tool.content(40, 80, false);
    tool.on_key(&press(KeyCode::Tab));
    tool.content(40, 80, false);
    assert_eq!(
        tool.list.view().scroll(),
        before,
        "a pane with room to spare stays put"
    );
}

#[test]
fn test_unfolding_a_commits_file_in_a_short_pane_keeps_its_row_where_it_was() {
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    output_cursor_on(&mut tool, "src/a.rs");
    tool.content(5, 80, false);
    let scroll = tool.list.view().scroll();
    tool.on_key(&press(KeyCode::Tab));
    tool.content(5, 80, false);
    assert_eq!(
        tool.list.view().scroll(),
        scroll,
        "the opened file's row does not move"
    );
}

#[test]
fn test_expanding_a_file_reads_its_diff_from_the_right_side() {
    // A staged file's diff is the index one; asking for the working-tree
    // diff there would show changes that are not staged.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "staged.rs");
    let command = command_of(&tool.on_key(&press(KeyCode::Tab)));
    assert!(
        command.args.contains(&"--cached".to_string()),
        "{:?}",
        command.args
    );

    cursor_on(&mut tool, "working.rs");
    let command = command_of(&tool.on_key(&press(KeyCode::Tab)));
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
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "new.rs");
    let command = command_of(&tool.on_key(&press(KeyCode::Tab)));
    assert!(
        command.args.contains(&"--no-index".to_string()),
        "{:?}",
        command.args
    );
}

#[test]
fn test_an_expanded_file_lists_its_hunks_and_lines() {
    let tool = tool_with_diff();
    let hunks: Vec<usize> = tool
        .rows
        .iter()
        .filter_map(|row| match &row.item {
            Item::Hunk(hunk) => Some(hunk.index),
            _ => None,
        })
        .collect();
    assert!(hunks.contains(&0) && hunks.contains(&1), "got {hunks:?}");
    assert!(hunks.len() > 2, "the lines belong to their hunk too");
}

#[test]
fn test_an_expanded_files_diff_decorates_the_edited_words() {
    // The whole point of the word diff: a replaced line paints its edit in
    // the line's style and the words the edit left alone recede.
    let tool = tool_with_diff();
    let added = tool
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
fn test_wrapping_puts_the_cursor_on_its_wrapped_screen_row() {
    // A changed line wider than the pane takes several screen rows, so the
    // band must follow the wrapped offset of the cursor's row, not its
    // row number, or it lights up the wrong line.
    let long = "x".repeat(45);
    let text = format!(
        "diff --git a/working.rs b/working.rs\n--- a/working.rs\n+++ b/working.rs\n@@ -1 +1 @@\n-{long}\n+{long}y\n"
    );
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    tool.on_key(&press(KeyCode::Tab));
    tool.on_job(reply(exec::TAG_DIFF, &text));
    let added = tool
        .rows
        .iter()
        .position(|row| row_text(&row.spans).contains(&format!("{long}y")))
        .expect("the added line");
    tool.list.set_cursor(added);
    // Stated relative to the cursor's row index: the status header is a
    // labelled block whose height can change, and an absolute screen row
    // here would be measuring the header rather than the wrapping.
    let flat = tool.content(20, 80, false);
    assert_eq!(
        flat.cursor_line,
        Some(added),
        "unwrapped, every row is one screen row, so the band sits on its own index"
    );
    let wrapped = tool.content(20, 32, true);
    assert_eq!(
        wrapped.cursor_line,
        Some(added + 1),
        "the removed line's wrapped rows push the band down"
    );
    assert!(
        wrapped.wrap_indents.contains(&1),
        "the hunk's lines carry their marker column, got {:?}",
        wrapped.wrap_indents
    );
}

#[test]
fn test_the_diff_popup_fills_the_view_with_a_decorated_diff() {
    let mut tool = loaded_tool();
    tool.on_key(&press(KeyCode::Char('d')));
    let outcome = tool.on_key(&press(KeyCode::Char('d')));
    assert_eq!(command_of(&outcome).args[0], "diff");
    tool.on_job(reply(exec::TAG_DIFF_VIEW, DIFF_OUTPUT));
    let output = tool.output.as_ref().expect("the diff view");
    assert_eq!(output.title, "Diff");
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
fn test_staging_on_a_hunk_applies_that_hunk_alone() {
    // The whole point of hunk staging: the second hunk must not be in the
    // patch, and the patch goes in on standard input.
    let mut tool = tool_with_diff();
    cursor_on_hunk(&mut tool, 0);
    let command = command_of(&tool.on_key(&press(KeyCode::Char('s'))));
    assert_eq!(command.args, ["apply", "--unidiff-zero", "--cached", "-"]);
    let patch = command.stdin.clone().expect("the patch");
    assert!(patch.contains("-    old();"), "got {patch}");
    assert!(!patch.contains("added()"), "got {patch}");
}

#[test]
fn test_alt_s_stages_only_the_line_under_the_cursor() {
    let mut tool = tool_with_diff();
    cursor_on_hunk(&mut tool, 0);
    // Header, ` fn main() {`, then `-    old();`.
    tool.list.move_by(2);
    let alt_s = Key {
        alt: true,
        ..press(KeyCode::Char('s'))
    };
    let command = command_of(&tool.on_key(&alt_s));
    let patch = command.stdin.clone().expect("the patch");
    assert!(patch.contains("@@ -1,2 +1,1 @@"), "recounted: {patch}");
    assert!(
        patch.contains("\n-    old();\n"),
        "the chosen removal: {patch}"
    );
    assert!(
        !patch.contains("new();"),
        "the unchosen addition is dropped: {patch}"
    );
}

#[test]
fn test_the_mark_and_alt_s_stage_the_lines_between_them() {
    let mut tool = tool_with_diff();
    cursor_on_hunk(&mut tool, 0);
    tool.list.move_by(2);
    tool.on_key(&ctrl(KeyCode::Space));
    tool.list.move_by(1);
    let alt_s = Key {
        alt: true,
        ..press(KeyCode::Char('s'))
    };
    let patch = command_of(&tool.on_key(&alt_s)).stdin.expect("the patch");
    assert!(
        patch.contains("-    old();\n+    new();"),
        "both lines: {patch}"
    );
    assert_eq!(tool.line_mark, None, "the mark is spent");
}

#[test]
fn test_a_key_inside_a_hunk_acts_on_that_hunk() {
    // Only the header row would be a cruel target; every line of the hunk
    // stands for it.
    let mut tool = tool_with_diff();
    let line = tool
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(&row.item, Item::Hunk(hunk) if hunk.index == 1))
        .nth(1)
        .map(|(index, _)| index)
        .expect("a line inside the second hunk");
    tool.list.set_cursor(line);
    let command = command_of(&tool.on_key(&press(KeyCode::Char('s'))));
    let patch = command.stdin.clone().expect("the patch");
    assert!(patch.contains("+    added();"), "got {patch}");
}

#[test]
fn test_unstaging_a_hunk_reverses_it_out_of_the_index() {
    let mut tool = tool_with_diff();
    cursor_on_hunk(&mut tool, 0);
    let command = command_of(&tool.on_key(&press(KeyCode::Char('u'))));
    assert_eq!(
        command.args,
        ["apply", "--unidiff-zero", "--cached", "--reverse", "-"]
    );
}

#[test]
fn test_discarding_a_hunk_reverses_it_out_of_the_working_tree() {
    let mut tool = tool_with_diff();
    cursor_on_hunk(&mut tool, 0);
    tool.on_key(&press(KeyCode::Char('x')));
    let command = command_of(&tool.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: ASK_DISCARD,
    }));
    assert_eq!(command.args, ["apply", "--unidiff-zero", "--reverse", "-"]);
}

#[test]
fn test_a_new_status_drops_the_diffs_it_invalidated() {
    // Staging a hunk changes the file's diff; applying a patch built from
    // the old one would hit the wrong lines.
    let mut tool = tool_with_diff();
    assert!(!tool.diffs.is_empty());
    tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
    assert!(tool.diffs.is_empty(), "the stale diff is gone");
}

#[test]
fn test_a_refresh_reads_everything_the_view_shows_in_one_chain() {
    // Each read asks for the next, so one refresh fills the whole view
    // rather than five jobs racing each other into it.
    let mut tool = tool_with_diff();
    tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
    let after_log = tool.on_job(reply(exec::TAG_LOG, ""));
    assert_eq!(command_of(&after_log).tag, exec::TAG_STASHES);
    let after_stashes = tool.on_job(reply(exec::TAG_STASHES, ""));
    assert_eq!(command_of(&after_stashes).tag, exec::TAG_UNPULLED);

    // The chain ends on the state files, which no command reports.
    let after_unpulled = tool.on_job(reply(exec::TAG_UNPULLED, ""));
    let ToolOutcome::Job(JobRequest::ReadFiles(paths)) = after_unpulled else {
        panic!("expected the state files, got {after_unpulled:?}");
    };
    assert!(paths.iter().any(|path| path.ends_with("MERGE_HEAD")));

    // And the open diff is what still needs reading.
    let stale = tool.request_stale_diff();
    assert_eq!(command_of(&stale).tag, exec::TAG_DIFF);
}

#[test]
fn test_a_file_the_status_no_longer_lists_stops_being_expanded() {
    let mut tool = tool_with_diff();
    tool.on_job(reply(
        exec::TAG_STATUS,
        "# branch.head main
",
    ));
    assert!(tool.expanded.is_empty());
}

#[test]
fn test_folding_an_expanded_file_hides_its_diff_without_re_reading() {
    let mut tool = tool_with_diff();
    cursor_on(&mut tool, "working.rs");
    let outcome = tool.on_key(&press(KeyCode::Tab));
    assert_eq!(outcome, ToolOutcome::Consumed, "nothing to run");
    assert!(!tool
        .rows
        .iter()
        .any(|row| matches!(row.item, Item::Hunk(_))));
}

fn shift(code: KeyCode) -> Key {
    Key {
        alt: false,
        code,
        ctrl: false,
        shift: true,
        sup: false,
    }
}

fn choose(tool: &mut GitTool, opener: char, choice: char) -> ToolOutcome {
    tool.on_key(&press_char(opener));
    tool.on_key(&press_char(choice))
}

fn press_char(c: char) -> Key {
    press(KeyCode::Char(c))
}

fn alt_char(c: char) -> Key {
    Key {
        alt: true,
        code: KeyCode::Char(c),
        ctrl: false,
        shift: false,
        sup: false,
    }
}

/// Ask for `text` through the search prompt, the way a key does.
fn search(tool: &mut GitTool, text: &str) -> ToolOutcome {
    let outcome = tool.on_key(&press_char('/'));
    let ToolOutcome::Prompt(request) = outcome else {
        panic!("expected a prompt, got {outcome:?}");
    };
    assert_eq!(request.tag, ASK_SEARCH);
    tool.on_prompt(PromptReply {
        answer: Some(text.to_string()),
        tag: request.tag,
    })
}

fn row_under_cursor(tool: &GitTool) -> String {
    tool.rows
        .get(tool.list.cursor())
        .map(|row| row_text(&row.spans))
        .unwrap_or_default()
}

#[test]
fn test_a_menu_key_opens_a_menu_and_hands_it_to_the_host_to_draw() {
    // The menu is the host's own key-hint card, centred over the window,
    // so the view keeps every row of the pane to itself.
    let mut tool = loaded_tool();
    tool.on_key(&press_char('z'));
    assert_eq!(tool.popup, Some(Popup::Stash));

    let hint = tool.hint().expect("a hint while the menu is open");
    assert_eq!(hint.title, "Stash");
    assert!(
        hint.items
            .iter()
            .any(|(key, what)| key == "p" && what == "pop the newest"),
        "got {:?}",
        hint.items
    );

    let painted = tool.content(80, 80, false);
    let text: Vec<String> = painted
        .rows
        .iter()
        .map(|row| row.iter().map(|span| span.text.clone()).collect())
        .collect();
    assert!(
        !text.iter().any(|line| line.contains("pop the newest")),
        "and nothing of it is drawn into the view, got {text:?}"
    );
}

#[test]
fn test_an_unknown_second_key_closes_the_menu_without_running_anything() {
    // A mistyped follow key must not fall through to the view's own keys,
    // where it could stage or discard something.
    let mut tool = loaded_tool();
    let outcome = choose(&mut tool, 'z', '@');
    assert_eq!(outcome, ToolOutcome::Consumed);
    assert_eq!(tool.popup, None);
}

#[test]
fn test_stash_choices_run_the_matching_command() {
    let mut tool = loaded_tool();
    assert_eq!(
        command_of(&choose(&mut tool, 'z', 'p')).args,
        ["stash", "pop"]
    );
    assert_eq!(
        command_of(&choose(&mut tool, 'z', 'd')).args,
        ["stash", "drop"]
    );
    assert_eq!(
        command_of(&choose(&mut tool, 'z', 'l')).args,
        ["stash", "list"]
    );
}

#[test]
fn test_a_branch_choice_asks_for_the_name_then_runs() {
    let mut tool = loaded_tool();
    let ToolOutcome::Prompt(request) = choose(&mut tool, 'b', 'c') else {
        panic!("expected a prompt");
    };
    let command = command_of(&tool.on_prompt(PromptReply {
        answer: Some("feature".to_string()),
        tag: request.tag,
    }));
    assert_eq!(command.args, ["checkout", "-b", "feature", "--"]);
}

#[test]
fn test_an_empty_answer_runs_nothing() {
    // `git checkout -b ""` fails with a confusing message; not running is
    // the better answer.
    let mut tool = loaded_tool();
    let ToolOutcome::Prompt(request) = choose(&mut tool, 'b', 'c') else {
        panic!("expected a prompt");
    };
    let outcome = tool.on_prompt(PromptReply {
        answer: Some("   ".to_string()),
        tag: request.tag,
    });
    assert_eq!(outcome, ToolOutcome::Consumed);
}

#[test]
fn test_a_stash_message_may_be_empty_because_it_is_optional() {
    let mut tool = loaded_tool();
    let ToolOutcome::Prompt(request) = choose(&mut tool, 'z', 'z') else {
        panic!("expected a prompt");
    };
    let command = command_of(&tool.on_prompt(PromptReply {
        answer: Some(String::new()),
        tag: request.tag,
    }));
    assert_eq!(command.args, ["stash", "push"], "no empty --message");
}

#[test]
fn test_a_file_changed_on_disk_makes_the_tool_read_the_status_again() {
    let dir = std::env::temp_dir().join(format!(
        "winter-gitpage-watch-{:?}",
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).expect("git dir");
    let mut tool = GitTool::new(dir.clone());
    let root = format!("{}\n{}\n", dir.display(), dir.join(".git").display());
    tool.on_job(reply(exec::TAG_ROOT, &root));
    tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
    // Set up off the UI thread, and taken over on a tick.
    let deadline = Instant::now() + Duration::from_secs(5);
    while tool.tree_watch.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        tool.adopt_started_watch();
    }
    assert!(tool.tree_watch.is_some(), "the platform can watch it");
    std::thread::sleep(Duration::from_millis(150));

    std::fs::write(dir.join("new.rs"), "fn main() {}").expect("write");
    std::thread::sleep(Duration::from_millis(300));

    let now = Instant::now();
    assert_eq!(
        tool.on_tick(now),
        ToolOutcome::Consumed,
        "waiting out the burst"
    );
    let asked = tool.on_tick(now + Duration::from_secs(4));
    assert_eq!(command_of(&asked).tag, exec::TAG_STATUS);
    assert_eq!(
        tool.on_tick(now + Duration::from_secs(8)),
        ToolOutcome::Consumed,
        "one read at a time"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_change_to_the_files_reads_the_status_and_one_in_git_reads_everything() {
    let mut tool = loaded_tool();
    let changed = STATUS_OUTPUT.replace("? new.rs", "? newer.rs");

    tool.watching = Some(super::watch::Read::Status);
    assert_eq!(
        tool.on_job(reply(exec::TAG_STATUS, &changed)),
        ToolOutcome::Consumed,
        "the files changed: no log, stash, or unpulled reads"
    );
    assert!(tool
        .rows
        .iter()
        .any(|row| matches!(&row.item, Item::File(file) if file.path == "newer.rs")));

    // The same status again, but the change was a commit or a fetch.
    tool.watching = Some(super::watch::Read::Full);
    let next = tool.on_job(reply(exec::TAG_STATUS, &changed));
    assert_eq!(command_of(&next).tag, exec::TAG_LOG, "everything is read");
}

#[test]
fn test_a_quiet_tree_is_read_again_without_redrawing_anything() {
    let mut tool = loaded_tool();
    tool.watching = Some(super::watch::Read::Status);

    // Nothing changed: no rows rebuilt, no log or stash reads chained on.
    let rows = tool.rows.len();
    assert_eq!(
        tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT)),
        ToolOutcome::Consumed
    );
    assert_eq!(tool.rows.len(), rows);
}

#[test]
fn test_the_watch_leaves_alone_a_view_the_user_is_reading() {
    let dir = std::env::temp_dir().join(format!(
        "winter-gitpage-reading-{:?}",
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".git")).expect("git dir");
    let mut tool = loaded_tool();
    let starting = super::watch::TreeWatch::start_in_background(&dir, None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !starting.is_ready() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    tool.tree_watch_starting = Some(starting);
    assert!(tool.next_tick().is_some(), "a watch to take over");
    tool.show_output("Log", "abc one\n", false);
    assert_eq!(tool.next_tick(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_repository_that_cannot_be_watched_is_not_read_on_a_timer() {
    // The view is passive: with no watch it waits to be asked, and says once
    // that it will.
    let mut tool = loaded_tool();
    let starting = super::watch::TreeWatch::start_in_background(Path::new("/no/such/repo"), None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !starting.is_ready() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    tool.tree_watch_starting = Some(starting);
    assert_eq!(tool.on_tick(Instant::now()), ToolOutcome::Consumed);
    assert!(tool.take_notice().is_some(), "said once");
    assert_eq!(tool.next_tick(), None, "and never polled");
    assert_eq!(
        tool.on_tick(Instant::now() + Duration::from_secs(60)),
        ToolOutcome::Consumed
    );
}

fn quick_push_key() -> Key {
    Key {
        alt: false,
        code: KeyCode::Char('p'),
        ctrl: true,
        shift: true,
        sup: false,
    }
}

#[test]
fn test_ctrl_shift_p_pushes_to_the_tracked_remote_and_keeps_tracking() {
    let mut tool = loaded_tool();
    let command = command_of(&tool.on_key(&quick_push_key()));
    assert_eq!(
        command.args,
        [
            "push",
            "--set-upstream",
            "--end-of-options",
            "origin",
            "main"
        ]
    );
}

#[test]
fn test_ctrl_shift_p_uses_the_remote_the_branch_tracks_not_always_origin() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(
        exec::TAG_STATUS,
        "# branch.head feature/x\n# branch.upstream fork/feature/x\n",
    ));
    let command = command_of(&tool.on_key(&quick_push_key()));
    assert_eq!(
        command.args,
        [
            "push",
            "--set-upstream",
            "--end-of-options",
            "fork",
            "feature/x"
        ]
    );
}

#[test]
fn test_ctrl_shift_p_creates_the_branch_on_origin_when_nothing_is_tracked() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(exec::TAG_STATUS, "# branch.head topic\n"));
    let command = command_of(&tool.on_key(&quick_push_key()));
    assert_eq!(
        command.args,
        [
            "push",
            "--set-upstream",
            "--end-of-options",
            "origin",
            "topic"
        ]
    );
}

#[test]
fn test_ctrl_shift_p_on_a_detached_head_says_so_and_runs_nothing() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(exec::TAG_STATUS, "# branch.head (detached)\n"));
    assert_eq!(tool.on_key(&quick_push_key()), ToolOutcome::Consumed);
    assert_eq!(notice_text(&mut tool).as_deref(), Some("no branch to push"));
}

fn quick_pull_key() -> Key {
    Key {
        alt: true,
        code: KeyCode::Char('f'),
        ctrl: true,
        shift: true,
        sup: false,
    }
}

#[test]
fn test_ctrl_shift_alt_f_pulls_the_tracked_branch_with_rebase() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(
        exec::TAG_STATUS,
        "# branch.head topic\n# branch.upstream fork/feature/x\n",
    ));
    let command = command_of(&tool.on_key(&quick_pull_key()));
    assert_eq!(
        command.args,
        ["pull", "--rebase", "--end-of-options", "fork", "feature/x"]
    );
}

#[test]
fn test_ctrl_shift_alt_f_pulls_the_same_name_from_origin_when_nothing_is_tracked() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(exec::TAG_STATUS, "# branch.head topic\n"));
    let command = command_of(&tool.on_key(&quick_pull_key()));
    assert_eq!(
        command.args,
        ["pull", "--rebase", "--end-of-options", "origin", "topic"]
    );
}

#[test]
fn test_ctrl_shift_alt_f_on_a_detached_head_says_so_and_runs_nothing() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(exec::TAG_STATUS, "# branch.head (detached)\n"));
    assert_eq!(tool.on_key(&quick_pull_key()), ToolOutcome::Consumed);
    assert_eq!(notice_text(&mut tool).as_deref(), Some("no branch to pull"));
}

#[test]
fn test_ctrl_shift_c_with_nothing_staged_says_so_rather_than_doing_nothing() {
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    tool.on_job(reply(exec::TAG_STATUS, "# branch.head main\n? new.rs\n"));
    let outcome = tool.on_key(&Key {
        alt: false,
        code: KeyCode::Char('c'),
        ctrl: true,
        shift: true,
        sup: false,
    });
    assert_eq!(outcome, ToolOutcome::Consumed);
    assert_eq!(
        notice_text(&mut tool).as_deref(),
        Some("nothing staged to commit")
    );
}

#[test]
fn test_ctrl_shift_c_commits_in_the_editor_when_something_is_staged() {
    let mut tool = loaded_tool();
    let outcome = tool.on_key(&Key {
        alt: false,
        code: KeyCode::Char('c'),
        ctrl: true,
        shift: true,
        sup: false,
    });
    let request = command_of(&outcome);
    assert_eq!(request.tag, exec::TAG_COMMIT);
    assert_eq!(request.args, ["commit"]);
}

#[test]
fn test_a_commit_runs_headless_with_winters_editor_and_no_terminal_tab() {
    // The editor is a tab of this window that git waits on, so the commit
    // needs no terminal of its own, and a spawn would leave an empty one
    // sitting beside the message.
    let mut tool = loaded_tool();
    let outcome = choose(&mut tool, 'c', 'c');
    assert!(matches!(outcome, ToolOutcome::Job(_)), "{outcome:?}");
    let request = command_of(&outcome);
    assert_eq!(request.args, ["commit"]);
    assert_eq!(request.cwd, PathBuf::from("/repo"));
    assert!(
        request.env.iter().any(|(name, _)| name == "GIT_EDITOR"),
        "git is pointed at Winter's editor: {:?}",
        request.env
    );
    let amend = command_of(&choose(&mut tool, 'c', 'a'));
    assert_eq!(amend.args, ["commit", "--amend"]);
}

#[test]
fn test_an_interactive_rebase_also_goes_to_a_pane() {
    let mut tool = loaded_tool();
    // The branch to rebase onto is chosen from a list, so the key asks
    // for the names first and the answer arrives under the same tag.
    let listing = choose(&mut tool, 'r', 'i');
    assert_eq!(command_of(&listing).tag, exec::TAG_CANDIDATES);
    let ToolOutcome::Pick(request) = tool.on_job(reply(exec::TAG_CANDIDATES, "main\nfeature\n"))
    else {
        panic!("expected a list to pick from");
    };
    let outcome = tool.on_prompt(PromptReply {
        answer: Some("HEAD~3".to_string()),
        tag: request.tag,
    });
    let ToolOutcome::Spawn(spawn) = outcome else {
        panic!("expected a spawn, got {outcome:?}");
    };
    assert_eq!(spawn.args, ["rebase", "--interactive", "HEAD~3"]);
}

#[test]
fn test_a_remote_add_answer_splits_into_name_and_url() {
    let mut tool = loaded_tool();
    let ToolOutcome::Prompt(request) = choose(&mut tool, 'M', 'a') else {
        panic!("expected a prompt");
    };
    let command = command_of(&tool.on_prompt(PromptReply {
        answer: Some("upstream git@example.com:o/r.git".to_string()),
        tag: request.tag,
    }));
    assert_eq!(
        command.args,
        [
            "remote",
            "add",
            "--end-of-options",
            "upstream",
            "git@example.com:o/r.git"
        ]
    );
}

#[test]
fn test_a_malformed_remote_add_reports_instead_of_running() {
    let mut tool = loaded_tool();
    let ToolOutcome::Prompt(request) = choose(&mut tool, 'M', 'a') else {
        panic!("expected a prompt");
    };
    let outcome = tool.on_prompt(PromptReply {
        answer: Some("justaname".to_string()),
        tag: request.tag,
    });
    assert_eq!(outcome, ToolOutcome::Consumed);
    assert!(notice_text(&mut tool)
        .unwrap_or_default()
        .starts_with("failed:"));
}

#[test]
fn test_enter_on_a_commit_reads_it() {
    // Enter opens the file under the cursor, and a commit row has no file:
    // it used to be swallowed, so a commit was the one row Enter did
    // nothing on.
    let mut tool = loaded_tool();
    let at = tool
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("a commit row");
    tool.list.set_cursor(at);
    let command = command_of(&tool.on_key(&press(KeyCode::Enter)));
    assert_eq!(command.tag, exec::TAG_SHOW);
    assert_eq!(command.args.first().map(String::as_str), Some("show"));
    assert!(command
        .args
        .ends_with(&["abc1234".to_string(), "--".to_string()]));
    assert!(
        !command.args.iter().any(|arg| arg == "--shortstat"),
        "no line totals: the view counts the commit's own hunks, {:?}",
        command.args
    );
}

#[test]
fn test_a_commit_read_fills_the_view_under_its_own_title() {
    let mut tool = loaded_tool();
    // Joined rather than written as one literal: `git show` output is
    // indentation-sensitive (the subject is the message block's four-space
    // indent), and a continued string literal cannot carry that faithfully.
    let shown = [
        "commit abc1234567890",
        "Author: Someone <a@b.c>",
        "Date:   2026-09-15 12:00:00 +0000",
        "",
        "    do the thing",
        "",
        " src/a.rs | 2 +-",
    ]
    .join("\n");
    assert_eq!(
        tool.on_job(reply(exec::TAG_SHOW, &shown)),
        ToolOutcome::Consumed
    );
    let output = tool
        .output
        .as_ref()
        .expect("the commit replaced the status view");
    assert_eq!(output.title, "Commit abc12345  do the thing");
    assert!(output.lines.iter().any(|line| line.contains("src/a.rs")));
}

#[test]
fn test_a_commit_content_view_bands_its_files() {
    // Enter on a commit shows its content the way the working tree reads:
    // the summary receded, then a band per file and decorated hunks
    // under it, no `diff --git` or `index` lines.
    let shown = [
        "commit abc1234567890",
        "Author: Someone <a@b.c>",
        "Date:   2026-09-15 12:00:00 +0000",
        "",
        "    do the thing",
        "",
        "diff --git a/src/a.rs b/src/a.rs",
        "index 111..222 100644",
        "--- a/src/a.rs",
        "+++ b/src/a.rs",
        "@@ -1,2 +1,2 @@",
        " fn main() {",
        "-    old();",
        "+    new();",
    ]
    .join("\n");
    let mut tool = loaded_tool();
    assert_eq!(
        tool.on_job(reply(exec::TAG_SHOW, &shown)),
        ToolOutcome::Consumed
    );
    // A commit opens shut, so open all of it to read what it paints.
    tool.on_key(&shift(KeyCode::Tab));
    let output = tool
        .output
        .as_ref()
        .expect("the commit replaced the status view");
    assert_eq!(
        output.rows[0].spans,
        vec![Span::new(Style::Header, "commit abc1234567890")]
    );
    let band = output
        .rows
        .iter()
        .find(|row| row_text(&row.spans).contains("src/a.rs"))
        .expect("the file band");
    assert!(
        band.spans.iter().any(|span| span.style == Style::Section),
        "got {band:?}"
    );
    assert!(
        output
            .rows
            .iter()
            .all(|row| !row_text(&row.spans).contains("diff --git")),
        "the patch's plumbing lines are gone"
    );
    let added = output
        .rows
        .iter()
        .find(|row| row_text(&row.spans).contains("new();"))
        .expect("the added line");
    assert!(added
        .spans
        .iter()
        .any(|span| span.style == Style::AddedEdit));
    assert_eq!(
        output.lines.len(),
        output.rows.len(),
        "the search text matches the painted rows"
    );
}

/// A `git show` of one file with two hunks, for the fold tests.
const SHOWN_TWO_HUNKS: &str = "commit abc1234567890\n\n    do the thing\ndiff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n-    old();\n+    new();\n@@ -9,2 +9,2 @@\n-    second();\n+    third();\n";

/// A commit touching two files, so folding every file at once is visible
/// as more than folding the one.
const SHOWN_TWO_FILES: &str = "commit abc1234567890\n\n    do the thing\ndiff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n-    old();\n+    new();\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1,2 +1,2 @@\n-    second();\n+    third();\n";

/// Put the cursor on the first row whose text contains `needle`.
fn output_cursor_on(tool: &mut GitTool, needle: &str) {
    let output = tool.output.as_mut().expect("an output view");
    let at = output
        .lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no output row for {needle}"));
    output.list.set_cursor(at);
}

fn output_has(tool: &GitTool, needle: &str) -> bool {
    tool.output
        .as_ref()
        .is_some_and(|output| output.lines.iter().any(|line| line.contains(needle)))
}

#[test]
fn test_tab_on_a_commits_heading_shuts_every_file_under_it() {
    // The heading is the commit's own section, the way the working tree's
    // sections are: one key takes the whole file list away and brings it
    // back, with the count left saying what is behind it.
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
    assert!(
        output_has(&tool, "src/a.rs"),
        "the files show to begin with"
    );

    output_cursor_on(&mut tool, "Changes");
    assert_eq!(tool.on_key(&press(KeyCode::Tab)), ToolOutcome::Consumed);
    assert!(
        !output_has(&tool, "src/a.rs") && !output_has(&tool, "src/b.rs"),
        "the file list is away"
    );
    assert!(
        output_has(&tool, "Changes (2)"),
        "the heading stays, still counting what it holds"
    );

    tool.on_key(&press(KeyCode::Tab));
    assert!(
        output_has(&tool, "src/a.rs") && output_has(&tool, "src/b.rs"),
        "and comes back"
    );
}

#[test]
fn test_a_commit_opens_shut_and_tab_opens_it_one_layer_at_a_time() {
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    assert!(
        output_has(&tool, "src/a.rs"),
        "the commit opens as the files it touched"
    );
    assert!(
        !output_has(&tool, "@@") && !output_has(&tool, "new();"),
        "with neither their hunks nor their diffs"
    );

    output_cursor_on(&mut tool, "src/a.rs");
    let at = tool.output.as_ref().unwrap().list.cursor();
    assert_eq!(tool.on_key(&press(KeyCode::Tab)), ToolOutcome::Consumed);
    assert!(
        output_has(&tool, "@@"),
        "the file opens to its hunk headers"
    );
    assert!(
        !output_has(&tool, "new();"),
        "which are themselves still shut"
    );
    assert_eq!(
        tool.output.as_ref().unwrap().list.cursor(),
        at,
        "the cursor stays on the band it acted on"
    );

    output_cursor_on(&mut tool, "@@ -1,2 +1,2 @@");
    tool.on_key(&press(KeyCode::Tab));
    assert!(output_has(&tool, "new();"), "the hunk opens to its body");

    output_cursor_on(&mut tool, "src/a.rs");
    tool.on_key(&press(KeyCode::Tab));
    assert!(
        !output_has(&tool, "@@"),
        "the file shuts all of it away again"
    );
    assert!(
        output_has(&tool, "src/a.rs"),
        "its band stays, so it can be opened again"
    );
}

#[test]
fn test_tab_on_a_commits_hunk_folds_only_that_hunk() {
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    tool.on_key(&shift(KeyCode::Tab));

    output_cursor_on(&mut tool, "@@ -1,2 +1,2 @@");
    tool.on_key(&press(KeyCode::Tab));

    assert!(
        output_has(&tool, "@@ -1,2 +1,2 @@"),
        "a folded hunk keeps its header"
    );
    assert!(!output_has(&tool, "new();"), "but drops its body");
    assert!(output_has(&tool, "third();"), "the other hunk is untouched");
}

#[test]
fn test_tab_on_a_commits_summary_does_nothing() {
    // The summary has nothing to fold, so Tab must not disturb the view.
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    output_cursor_on(&mut tool, "do the thing");
    let before = tool.output.as_ref().unwrap().lines.clone();

    assert_eq!(tool.on_key(&press(KeyCode::Tab)), ToolOutcome::Consumed);
    assert_eq!(
        tool.output.as_ref().unwrap().lines,
        before,
        "the view is unchanged"
    );
}

#[test]
fn test_shift_tab_folds_every_file_of_a_commit_and_opens_them_again() {
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
    assert!(
        !output_has(&tool, "new();") && !output_has(&tool, "third();"),
        "the commit opens shut"
    );

    assert_eq!(tool.on_key(&shift(KeyCode::Tab)), ToolOutcome::Consumed);
    assert!(
        output_has(&tool, "new();") && output_has(&tool, "third();"),
        "one key opens every file and hunk of it"
    );

    tool.on_key(&shift(KeyCode::Tab));
    assert!(
        !output_has(&tool, "new();") && !output_has(&tool, "third();"),
        "a second S-TAB shuts all of it again"
    );
    assert!(
        output_has(&tool, "src/a.rs") && output_has(&tool, "src/b.rs"),
        "the bands stay, so the commit still says what it touched"
    );
}

#[test]
fn test_shift_tab_opens_a_part_way_folded_commit_rather_than_shutting_it() {
    // One hunk folded by hand means the view is already part-way shut, so
    // the key that acts on all of it opens it rather than folding further.
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    tool.on_key(&shift(KeyCode::Tab));
    output_cursor_on(&mut tool, "@@ -1,2 +1,2 @@");
    tool.on_key(&press(KeyCode::Tab));
    assert!(!output_has(&tool, "new();"), "that hunk's body is away");

    tool.on_key(&shift(KeyCode::Tab));
    assert!(
        output_has(&tool, "new();") && output_has(&tool, "third();"),
        "every hunk is back"
    );
}

#[test]
fn test_shift_tab_leaves_an_output_view_with_nothing_to_fold_alone() {
    // A blame, a log, or a listing is the lines it came back as: there is
    // no commit under it to fold, and the key must not disturb the view.
    let mut tool = loaded_tool();
    tool.on_job(reply(
        exec::TAG_BLAME,
        "abc1234 (Someone 2026-09-18) fn main() {",
    ));
    let before = tool.output.as_ref().expect("the blame").lines.clone();

    assert_eq!(tool.on_key(&shift(KeyCode::Tab)), ToolOutcome::Consumed);
    assert_eq!(tool.output.as_ref().unwrap().lines, before);
}

#[test]
fn test_a_commits_file_band_carries_an_icon() {
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    let output = tool.output.as_ref().expect("the commit view");
    let band = output
        .rows
        .iter()
        .find(|row| row_text(&row.spans).contains("src/a.rs"))
        .expect("the file band");
    assert_eq!(
        band.icon.as_ref().map(|icon| &icon.kind),
        Some(&crate::model::paint::IconKind::File {
            name: "a.rs".to_string()
        }),
        "the band names its icon for the file's leaf"
    );
    assert!(
        output.rows.iter().filter(|row| row.icon.is_some()).count() == 1,
        "only the file band carries one; hunk lines do not"
    );
}

#[test]
fn test_folding_reaches_the_painted_content_the_pane_draws() {
    // The fold has to change what `content` returns, not just the row list,
    // or the pane keeps drawing the unfolded view.
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    tool.on_key(&shift(KeyCode::Tab));
    let drawn = |tool: &mut GitTool| -> String {
        tool.content(40, 100, false)
            .rows
            .iter()
            .map(row_text)
            .collect::<Vec<String>>()
            .join("\n")
    };
    assert!(drawn(&mut tool).contains("new();"));

    output_cursor_on(&mut tool, "src/a.rs");
    tool.on_key(&press(KeyCode::Tab));
    let after = drawn(&mut tool);
    assert!(
        !after.contains("new();"),
        "the pane draws the folded view, got {after:?}"
    );
    assert!(after.contains("src/a.rs"), "the band is still drawn");
}

#[test]
fn test_an_empty_commit_read_says_so_instead_of_blanking_the_view() {
    let mut tool = loaded_tool();
    tool.on_job(reply(
        exec::TAG_SHOW,
        "   
",
    ));
    assert!(tool.output.is_none(), "the status view stays");
    assert_eq!(notice_text(&mut tool).as_deref(), Some("nothing to show"));
}

#[test]
fn test_reset_on_a_commit_uses_that_commit_without_asking() {
    let mut tool = loaded_tool();
    let at = tool
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("a commit row");
    tool.list.set_cursor(at);
    let command = command_of(&tool.on_key(&press_char('o')));
    assert_eq!(command.args, ["reset", "--mixed", "abc1234", "--"]);
}

#[test]
fn test_reset_away_from_a_commit_asks_which_one() {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let ToolOutcome::Prompt(request) = tool.on_key(&press_char('o')) else {
        panic!("expected a prompt");
    };
    assert_eq!(request.tag, ASK_RESET_MIXED);
}

#[test]
fn test_a_hard_reset_says_what_it_costs() {
    let mut tool = loaded_tool();
    let ToolOutcome::Prompt(request) = choose(&mut tool, 'O', 'h') else {
        panic!("expected a prompt");
    };
    assert!(request.label.contains("HARD"), "got {:?}", request.label);
    assert!(
        request.label.to_lowercase().contains("losing"),
        "got {:?}",
        request.label
    );
}

#[test]
fn test_yanking_copies_the_hash_on_a_commit_and_the_path_on_a_file() {
    let mut tool = loaded_tool();
    let at = tool
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("a commit row");
    tool.list.set_cursor(at);
    assert_eq!(
        tool.on_key(&press_char('y')),
        ToolOutcome::Yank("abc1234".to_string())
    );

    cursor_on(&mut tool, "working.rs");
    assert_eq!(
        tool.on_key(&press_char('y')),
        ToolOutcome::Yank("working.rs".to_string())
    );
}

#[test]
fn test_ignoring_by_extension_writes_a_wildcard() {
    let tree = std::env::temp_dir().join(format!("winter-gitignore-{}", std::process::id()));
    std::fs::create_dir_all(&tree).expect("temp dir");
    let mut tool = GitTool::new(tree.clone());
    tool.on_job(reply(exec::TAG_ROOT, &format!("{}\n", tree.display())));
    tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
    cursor_on(&mut tool, "working.rs");

    choose(&mut tool, 'i', 'e');
    let written = std::fs::read_to_string(tree.join(".gitignore")).unwrap_or_default();
    assert_eq!(written.trim(), "*.rs");
    std::fs::remove_dir_all(&tree).ok();
}

/// `count` commits as `git log` writes them, newest first.
fn commits_output(count: usize) -> String {
    let commits: Vec<(String, String)> = (0..count)
        .map(|n| (format!("c{n:06}"), format!("commit {n}")))
        .collect();
    let refs: Vec<(&str, &str)> = commits
        .iter()
        .map(|(hash, subject)| (hash.as_str(), subject.as_str()))
        .collect();
    log_output(&refs)
}

/// Whether the view, as drawn, offers to load more commits.
fn offers_more(tool: &GitTool) -> bool {
    let rows = match &tool.output {
        Some(output) => output.lines.clone(),
        None => tool.rows.iter().map(|row| row_text(&row.spans)).collect(),
    };
    rows.iter().any(|row| row.contains(rows::LOG_MORE))
}

#[test]
fn test_a_log_fills_the_view_and_can_ask_for_more() {
    let mut tool = loaded_tool();
    choose(&mut tool, 'l', 'l');
    let page = tool.log_count;
    // One commit past the page came back: there are more to load.
    let outcome = tool.on_job(reply(exec::TAG_LOG_VIEW, &commits_output(page + 1)));
    assert_eq!(outcome, ToolOutcome::Consumed);
    assert!(tool.output.is_some(), "the log replaced the status view");
    assert!(offers_more(&tool));

    let more = tool.on_key(&press_char('+'));
    assert!(tool.log_count > page, "asked for more");
    assert_eq!(command_of(&more).tag, exec::TAG_LOG_VIEW);
}

#[test]
fn test_a_log_that_reaches_the_first_commit_offers_no_more() {
    // A history exactly a page long has its first commit on screen; the
    // old check took any full page for "there are more".
    let mut tool = loaded_tool();
    choose(&mut tool, 'l', 'l');
    let page = tool.log_count;
    tool.on_job(reply(exec::TAG_LOG_VIEW, &commits_output(page)));
    assert!(!offers_more(&tool));
    assert!(!matches!(
        tool.on_key(&press_char('+')),
        ToolOutcome::Job(_)
    ));
    assert_eq!(tool.log_count, page, "nothing asked for");
}

#[test]
fn test_q_leaves_the_log_for_the_status_view_rather_than_closing() {
    // Closing the whole tool on `q` from a log would lose the view the user
    // came from.
    let mut tool = loaded_tool();
    choose(&mut tool, 'l', 'l');
    tool.on_job(reply(exec::TAG_LOG_VIEW, &log_output(&[("aaa", "one")])));
    let outcome = tool.on_key(&press_char('q'));
    assert_eq!(
        command_of(&outcome).tag,
        exec::TAG_STATUS,
        "back at the status, it is read again at once"
    );
    assert!(tool.output.is_none());
    assert_ne!(tool.on_key(&press(KeyCode::Escape)), ToolOutcome::Close);
}

#[test]
fn test_r_i_in_the_log_rebases_from_the_commit_at_point() {
    let mut tool = loaded_tool();
    choose(&mut tool, 'l', 'l');
    tool.on_job(reply(
        exec::TAG_LOG_VIEW,
        &log_output(&[("abc1234", "one"), ("def5678", "two")]),
    ));
    while tool.revision_at_cursor().is_none() {
        assert_eq!(tool.on_key(&press_char('j')), ToolOutcome::Consumed);
    }
    let commit = tool.revision_at_cursor().expect("on a commit");
    let request = command_of(&choose(&mut tool, 'r', 'i'));
    assert_eq!(request.tag, exec::TAG_REBASE_FROM);
    assert!(request.args.contains(&commit), "{:?}", request.args);
    assert!(tool.output.is_some(), "the log stays up");
}

#[test]
fn test_c_w_in_the_log_rewords_the_commit_at_point() {
    let mut tool = loaded_tool();
    choose(&mut tool, 'l', 'l');
    tool.on_job(reply(
        exec::TAG_LOG_VIEW,
        &log_output(&[("abc1234", "one"), ("def5678", "two")]),
    ));
    while tool.revision_at_cursor().is_none() {
        tool.on_key(&press_char('j'));
    }
    tool.on_key(&press_char('j'));
    tool.on_key(&press_char('j'));
    let commit = tool.revision_at_cursor().expect("on the second commit");
    let request = command_of(&choose(&mut tool, 'c', 'w'));
    assert_eq!(request.tag, exec::TAG_REWORD_FROM);
    assert!(request.args.contains(&commit), "{:?}", request.args);
}

/// Open the log of the branch from the status, as `C-l l` does.
fn open_log(tool: &mut GitTool) -> ToolOutcome {
    tool.on_key(&ctrl(KeyCode::Char('l')));
    tool.on_key(&press_char('l'))
}

#[test]
fn test_a_command_run_from_the_log_reads_the_log_again_and_keeps_the_place() {
    // A reword or a rebase from the log rewrites the commits it lists, and a
    // log left as it was would offer the old ones to act on next.
    let mut tool = loaded_tool();
    let asked = command_of(&open_log(&mut tool));
    let log = log_output(&[("abc1234", "one"), ("def5678", "two"), ("0123abc", "three")]);
    tool.on_job(reply(exec::TAG_LOG_VIEW, &log));
    tool.on_key(&press_char('j'));
    tool.on_key(&press_char('j'));
    let place = tool.output.as_ref().expect("the log").list.cursor();
    assert!(place > 0);

    let reread = command_of(&tool.on_job(reply(exec::TAG_CHANGED, "")));
    assert_eq!(reread, asked, "the log, asked as it was");
    tool.on_job(reply(exec::TAG_LOG_VIEW, &log));
    assert_eq!(tool.output.as_ref().expect("the log").list.cursor(), place);

    // Back at the status, a change reads the status again.
    let back = command_of(&tool.on_key(&press_char('q')));
    assert_eq!(back.tag, exec::TAG_STATUS);
    let reread = command_of(&tool.on_job(reply(exec::TAG_CHANGED, "")));
    assert_eq!(reread.tag, exec::TAG_STATUS);
}

#[test]
fn test_a_view_asked_for_afresh_starts_at_its_top() {
    let mut tool = loaded_tool();
    open_log(&mut tool);
    let log = log_output(&[("abc1234", "one"), ("def5678", "two"), ("0123abc", "three")]);
    tool.on_job(reply(exec::TAG_LOG_VIEW, &log));
    tool.on_key(&press_char('j'));
    tool.on_key(&press_char('j'));
    tool.on_job(reply(exec::TAG_CHANGED, ""));
    tool.on_key(&press_char('q'));
    open_log(&mut tool);
    tool.on_job(reply(exec::TAG_LOG_VIEW, &log));
    assert_eq!(tool.output.as_ref().expect("the log").list.cursor(), 0);
}

#[test]
fn test_read_only_output_with_nothing_in_it_says_so() {
    let mut tool = loaded_tool();
    tool.on_job(reply(exec::TAG_READ, "\n"));
    assert!(tool.output.is_none());
    assert_eq!(notice_text(&mut tool).as_deref(), Some("nothing to show"));
}

#[test]
fn test_an_ssh_remote_becomes_an_https_url() {
    assert_eq!(
        browser_url("git@github.com:owner/repo.git").as_deref(),
        Some("https://github.com/owner/repo")
    );
    assert_eq!(
        browser_url("https://gitlab.com/owner/repo.git").as_deref(),
        Some("https://gitlab.com/owner/repo")
    );
    assert_eq!(browser_url("/srv/git/bare.git"), None);
}

#[test]
fn test_the_keymaps_apply_keys_act_on_a_hunk_only() {
    // `a` and `-` are hunk keys; on a file row there is no patch to apply,
    // and guessing one would stage the whole file by surprise.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    assert_eq!(tool.on_key(&press_char('a')), ToolOutcome::Consumed);
    assert_eq!(tool.on_key(&press_char('-')), ToolOutcome::Consumed);
}

#[test]
fn test_shift_tab_still_folds_while_a_menu_has_never_opened() {
    let mut tool = loaded_tool();
    tool.on_key(&shift(KeyCode::Tab));
    assert!(!tool
        .rows
        .iter()
        .any(|row| matches!(row.item, Item::File(_))));
}

#[test]
fn test_a_menu_shows_however_long_the_view_is() {
    // The card is drawn over the window rather than under the rows, so a
    // status longer than the pane cannot push it off the bottom, which is
    // where a menu drawn into the view ended up.
    let mut tool = loaded_tool();
    tool.on_key(&press_char('b'));
    let rows = 4;
    let painted = tool.content(rows, 80, false);

    assert!(tool.hint().is_some(), "the menu is there to draw");
    assert!(
        painted.rows.len() <= rows,
        "and the view still fits its pane, got {}",
        painted.rows.len()
    );
}

#[test]
fn test_the_jump_leader_says_where_it_can_go() {
    // `g` waits for a second key like every other prefix, so it says what
    // the second key may be the same way they do.
    let mut tool = loaded_tool();
    assert_eq!(tool.on_key(&press_char('g')), ToolOutcome::Consumed);
    let hint = tool.hint().expect("a hint while the leader is open");
    assert_eq!(hint.title, "Jump");
    assert!(
        hint.items
            .iter()
            .any(|(key, what)| key == "s" && what == "staged changes"),
        "got {:?}",
        hint.items
    );

    // And the second key still jumps.
    tool.on_key(&press_char('s'));
    assert_eq!(
        tool.selected().map(|row| row.item.clone()),
        Some(Item::Heading(Section::Staged))
    );
}

#[test]
fn test_switching_branch_offers_the_branches_rather_than_a_blank_line() {
    let mut tool = loaded_tool();
    let listing = choose(&mut tool, 'b', 'b');
    let command = command_of(&listing);
    assert_eq!(command.tag, exec::TAG_CANDIDATES);
    assert_eq!(command.args[0], "for-each-ref");
    assert!(
        command.args.iter().any(|arg| arg == "refs/remotes"),
        "a checkout takes a remote branch too, got {:?}",
        command.args
    );

    let outcome = tool.on_job(reply(
        exec::TAG_CANDIDATES,
        "main\nfeature\norigin/main\norigin/HEAD\n",
    ));
    let ToolOutcome::Pick(request) = outcome else {
        panic!("expected a list to pick from, got {outcome:?}");
    };
    assert_eq!(request.label, "Checkout");
    assert_eq!(request.items, ["main", "feature", "origin/main"]);

    // Choosing one answers the question the key asked.
    let ran = tool.on_prompt(PromptReply {
        answer: Some("feature".to_string()),
        tag: request.tag,
    });
    let command = command_of(&ran);
    assert_eq!(command.args, ["checkout", "feature", "--"]);
}

#[test]
fn test_deleting_a_branch_offers_only_the_ones_that_can_be_deleted() {
    // A branch on a remote is not this repository's to delete, so the
    // list stops at the local ones.
    let mut tool = loaded_tool();
    let command = command_of(&choose(&mut tool, 'b', 'd'));
    assert!(
        command.args.iter().any(|arg| arg == "refs/heads")
            && !command.args.iter().any(|arg| arg == "refs/remotes"),
        "got {:?}",
        command.args
    );
}

#[test]
fn test_a_question_with_nothing_to_choose_from_falls_back_to_typing() {
    // A repository with no tags still has a delete-tag key; it asks for
    // one to be spelled out rather than opening an empty list.
    let mut tool = loaded_tool();
    choose(&mut tool, 't', 'd');
    let outcome = tool.on_job(reply(exec::TAG_CANDIDATES, "\n"));
    let ToolOutcome::Prompt(request) = outcome else {
        panic!("expected a prompt, got {outcome:?}");
    };
    assert_eq!(request.label, "Delete tag: ");
}

#[test]
fn test_the_file_popup_acts_on_the_row_it_was_opened_over() {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    assert_eq!(tool.on_key(&press_char('.')), ToolOutcome::Consumed);
    assert_eq!(tool.popup, Some(Popup::File), "the menu is open");

    let command = command_of(&tool.on_key(&press_char('b')));
    assert_eq!(command.args[0], "blame");
    assert!(
        command.args.iter().any(|arg| arg == "working.rs"),
        "on the file the cursor was on, got {:?}",
        command.args
    );
}

#[test]
fn test_the_file_popup_stays_shut_where_there_is_no_file() {
    // On a heading or a commit there is nothing for it to act on, and a
    // menu whose every choice does nothing is worse than none.
    let mut tool = loaded_tool();
    tool.list.set_cursor(0);
    assert_eq!(tool.on_key(&press_char('.')), ToolOutcome::Consumed);
    assert_eq!(tool.popup, None);
    assert_eq!(notice_text(&mut tool).as_deref(), Some("no file here"));
}

#[test]
fn test_enter_on_a_stash_reads_it_as_a_diff() {
    let mut tool = loaded_tool();
    tool.on_job(reply(
        exec::TAG_STASHES,
        "stash@{0}\u{1f}WIP on main: a thing\n",
    ));
    let at = tool
        .rows
        .iter()
        .position(|row| row.item == Item::Stash(0))
        .expect("the stash row");
    tool.list.set_cursor(at);

    let command = command_of(&tool.on_key(&press(KeyCode::Enter)));
    assert_eq!(command.args[0], "stash");
    assert!(
        command.args.iter().any(|arg| arg == "stash@{0}"),
        "the stash under the cursor, got {:?}",
        command.args
    );
}

#[test]
fn test_the_emacs_buffer_ends_reach_both_ends_of_either_view() {
    // `M->` and `M-<` land on the last and first row, in the status view
    // and in an output view alike: both are read top to bottom, and the
    // Vim `G`/`gg` pair is otherwise the only way there.
    let mut tool = loaded_tool();
    tool.list.set_cursor(0);
    tool.on_key(&Key {
        shift: true,
        ..alt_char('>')
    });
    assert_eq!(tool.list.cursor(), tool.rows.len() - 1, "the last row");
    tool.on_key(&Key {
        shift: true,
        ..alt_char('<')
    });
    assert_eq!(tool.list.cursor(), 0, "and back to the first");

    tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
    tool.on_key(&Key {
        shift: true,
        ..alt_char('>')
    });
    let output = tool.output.as_ref().expect("the commit view");
    assert_eq!(output.list.cursor(), output.lines.len() - 1);
    tool.on_key(&Key {
        shift: true,
        ..alt_char('<')
    });
    assert_eq!(tool.output.as_ref().unwrap().list.cursor(), 0);
}

#[test]
fn test_the_shared_word_motions_step_between_entities() {
    // `w`/`b` land on the things the view rows stand for, stepping over
    // the blank separators, through the shared Vim layer.
    let mut tool = loaded_tool();
    tool.list.set_cursor(0);
    tool.on_key(&press(KeyCode::Char('w')));
    assert!(
        tool.selected().is_some_and(|row| row.item != Item::None),
        "never lands on a separator"
    );
    tool.on_key(&press(KeyCode::Char('b')));
    assert!(
        tool.selected().is_some_and(|row| row.item != Item::None),
        "and back, the same way"
    );
}

#[test]
fn test_the_braces_jump_between_section_headings() {
    let mut tool = loaded_tool();
    tool.on_key(&press(KeyCode::Char('}')));
    assert!(matches!(
        tool.selected().map(|row| &row.item),
        Some(Item::Heading(_) | Item::RecentHeading)
    ));
    let first = tool.list.cursor();
    tool.on_key(&press(KeyCode::Char('}')));
    assert!(
        tool.list.cursor() > first,
        "each brace reaches the next heading"
    );
    tool.on_key(&press(KeyCode::Char('{')));
    assert_eq!(tool.list.cursor(), first, "and back to it");
}

#[test]
fn test_gg_and_the_paging_motions_work_over_the_view() {
    let mut tool = loaded_tool();
    tool.on_key(&press(KeyCode::Char('g')));
    tool.on_key(&press(KeyCode::Char('g')));
    assert_eq!(tool.list.cursor(), 0, "gg through the view's own g leader");

    tool.on_key(&press(KeyCode::Char('$')));
    assert_eq!(tool.list.cursor(), tool.rows.len() - 1, "$ is the last row");

    tool.on_key(&press(KeyCode::Char('0')));
    assert_eq!(tool.list.cursor(), 0);
    tool.content(10, 80, false);
    tool.on_key(&ctrl(KeyCode::Char('d')));
    assert_eq!(tool.list.cursor(), 4, "half the viewport down");
    tool.on_key(&ctrl(KeyCode::Char('u')));
    assert_eq!(tool.list.cursor(), 0, "and back up");
}

#[test]
fn test_the_views_own_keys_still_win_over_the_shared_layer() {
    // The override contract: what the view claims never reaches the
    // shared layer. `G` refreshes rather than going to the bottom, and
    // `g` opens the section leader rather than arming `gg` there.
    let mut tool = loaded_tool();
    tool.on_key(&press(KeyCode::Char('$')));
    let before = tool.rows.len();
    tool.on_key(&press(KeyCode::Char('G')));
    assert!(tool.rows.len() >= before, "`G` refreshed, it did not move");
}

#[test]
fn test_the_g_keys_jump_between_sections() {
    let mut tool = loaded_tool();
    tool.on_key(&press(KeyCode::Char('g')));
    tool.on_key(&press(KeyCode::Char('s')));
    assert_eq!(
        tool.selected().map(|row| &row.item),
        Some(&Item::Heading(Section::Staged))
    );

    tool.on_key(&press(KeyCode::Char('g')));
    tool.on_key(&press(KeyCode::Char('t')));
    assert_eq!(
        tool.selected().map(|row| &row.item),
        Some(&Item::Heading(Section::Untracked))
    );
}

#[test]
fn test_entity_motion_skips_the_blank_lines_between_sections() {
    // Plain `j` walks through the gaps; `M-n` is what steps from one
    // thing to the next thing.
    let mut tool = loaded_tool();
    tool.list.set_cursor(0);
    tool.on_key(&Key {
        alt: true,
        code: KeyCode::Char('n'),
        ctrl: false,
        shift: false,
        sup: false,
    });
    assert_ne!(tool.selected().map(|row| &row.item), Some(&Item::None));
}

#[test]
fn test_folding_a_section_hides_its_files_but_not_the_others() {
    let mut tool = loaded_tool();
    let at = tool
        .rows
        .iter()
        .position(|row| row.item == Item::Heading(Section::Untracked))
        .expect("the untracked heading");
    tool.list.set_cursor(at);
    tool.on_key(&press(KeyCode::Tab));

    let listed: Vec<String> = tool
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
fn test_shift_tab_shuts_everything_then_opens_it_again() {
    let mut tool = loaded_tool();
    let shift_tab = Key {
        alt: false,
        code: KeyCode::Tab,
        ctrl: false,
        shift: true,
        sup: false,
    };
    tool.on_key(&shift_tab);
    assert!(!tool
        .rows
        .iter()
        .any(|row| matches!(row.item, Item::File(_))));
    tool.on_key(&shift_tab);
    assert!(tool
        .rows
        .iter()
        .any(|row| matches!(row.item, Item::File(_))));
}

/// The fixture's tool with the push menu open over `config`, what `git
/// config --get-regexp` printed.
fn push_menu(config: &str) -> GitTool {
    let mut tool = loaded_tool();
    let lookup = command_of(&tool.on_key(&press_char('P')));
    assert_eq!(lookup.tag, exec::TAG_PUSH_CONTEXT);
    assert_eq!(
        lookup.cwd,
        PathBuf::from("/repo"),
        "from the repository root"
    );
    tool.on_job(reply(exec::TAG_PUSH_CONTEXT, config));
    tool
}

const PUSH_CONFIG: &str = "remote.origin.url git@x:a.git\nremote.fork.url git@x:b.git\n";

#[test]
fn test_p_opens_the_push_menu_naming_where_each_push_goes() {
    let tool = push_menu(PUSH_CONFIG);
    let hint = tool.hint().expect("the menu");
    assert_eq!(hint.title, "Pushing");
    let said = |key: &str| {
        hint.items
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, what)| what.as_str())
    };
    assert_eq!(said("p"), Some("pushRemote, after setting that"));
    assert_eq!(said("u"), Some("origin/main"), "the upstream it tracks");
    assert_eq!(said("e"), Some("elsewhere"));
}

#[test]
fn test_switches_turned_on_in_the_options_go_with_the_push() {
    let mut tool = push_menu("remote.origin.url u\nbranch.main.pushremote origin\n");
    for key in ['-', 'f', 'd'] {
        tool.on_key(&press_char(key));
    }
    let options = tool.hint().expect("the options menu");
    assert_eq!(options.title, "Push options");
    assert!(options.items[0].1.starts_with("[x]"), "{:?}", options.items);
    tool.on_key(&press(KeyCode::Enter));
    assert!(tool
        .hint()
        .expect("back")
        .items
        .iter()
        .any(|(_, what)| what == "Options [--force-with-lease --dry-run]"));
    let push = command_of(&tool.on_key(&press_char('p')));
    assert_eq!(push.tag, exec::TAG_PUSH);
    assert_eq!(
        push.args,
        [
            "push",
            "--force-with-lease",
            "--dry-run",
            "--end-of-options",
            "origin",
            "main"
        ]
    );
}

#[test]
fn test_p_with_no_push_remote_sets_one_then_pushes_to_it() {
    let mut tool = push_menu(PUSH_CONFIG);
    let listing = command_of(&tool.on_key(&press_char('p')));
    assert_eq!(listing.tag, exec::TAG_CANDIDATES);
    let ToolOutcome::Pick(request) = tool.on_job(reply(exec::TAG_CANDIDATES, "origin\nfork\n"))
    else {
        panic!("a list of remotes");
    };
    let set = command_of(&tool.on_prompt(PromptReply {
        answer: Some("fork".to_string()),
        tag: request.tag,
    }));
    assert_eq!(set.tag, exec::TAG_PUSH_REMOTE_SET);
    assert!(set
        .args
        .ends_with(&["branch.main.pushRemote".to_string(), "fork".to_string()]));
    let push = command_of(&tool.on_job(reply(exec::TAG_PUSH_REMOTE_SET, "")));
    assert_eq!(push.args, ["push", "--end-of-options", "fork", "main"]);
}

#[test]
fn test_elsewhere_offers_a_new_branch_on_each_remote_first() {
    let mut tool = push_menu(PUSH_CONFIG);
    tool.on_key(&press_char('e'));
    let ToolOutcome::Pick(request) =
        tool.on_job(reply(exec::TAG_CANDIDATES, "origin/main\norigin/dev\n"))
    else {
        panic!("a list of destinations");
    };
    assert_eq!(request.items, ["fork/main", "origin/main", "origin/dev"]);
    let push = command_of(&tool.on_prompt(PromptReply {
        answer: Some("origin/dev".to_string()),
        tag: request.tag,
    }));
    assert_eq!(
        push.args,
        ["push", "--end-of-options", "origin", "main:dev"]
    );
}

#[test]
fn test_all_tags_go_to_the_upstream_s_remote() {
    let mut tool = push_menu(PUSH_CONFIG);
    let push = command_of(&tool.on_key(&press_char('t')));
    assert_eq!(push.args, ["push", "--tags", "--end-of-options", "origin"]);
}

#[test]
fn test_a_search_moves_the_cursor_to_the_row_holding_the_text() {
    let mut tool = loaded_tool();
    search(&mut tool, "working");
    assert!(row_under_cursor(&tool).contains("working.rs"));
}

#[test]
fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
    let mut tool = loaded_tool();
    search(&mut tool, ".rs");
    let first = row_under_cursor(&tool);

    tool.on_key(&press_char('n'));
    let second = row_under_cursor(&tool);
    assert_ne!(first, second, "the next match is a different row");

    tool.on_key(&press_char('N'));
    assert_eq!(row_under_cursor(&tool), first, "and back again");
}

#[test]
fn test_a_search_walks_the_output_view_while_one_is_up() {
    // The status rows are not on screen here, so searching them would move a
    // cursor nobody can see and leave the visible one where it was.
    let mut tool = loaded_tool();
    choose(&mut tool, 'l', 'l');
    tool.on_job(reply(
        exec::TAG_LOG_VIEW,
        &log_output(&[("aaa", "one"), ("bbb", "two"), ("ccc", "three")]),
    ));
    let before = tool.list.cursor();

    search(&mut tool, "ccc");
    // Located rather than hardcoded: the log view heads its list, and each
    // commit takes an identity row and an authorship row, so a literal index
    // here would only re-encode the current layout.
    let expected = tool
        .output
        .as_ref()
        .and_then(|output| output.lines.iter().position(|line| line.contains("ccc")));
    assert!(expected.is_some(), "the log lists the commit searched for");
    assert_eq!(
        tool.output.as_ref().map(|output| output.list.cursor()),
        expected
    );
    assert_eq!(tool.list.cursor(), before);
}

#[test]
fn test_a_search_that_finds_nothing_reports_it_instead_of_going_quiet() {
    let mut tool = loaded_tool();
    tool.on_key(&press_char('n'));
    assert_eq!(tool.take_notice(), Some(Notice::error("no search")));

    search(&mut tool, "absent");
    assert_eq!(tool.take_notice(), Some(Notice::error("not found: absent")));
}

#[test]
fn test_a_search_is_answered_before_git_has_said_where_the_root_is() {
    // Every other question needs a repository and is dropped without one,
    // which would swallow a search the user typed while the status loaded.
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    search(&mut tool, "anything");
    assert_eq!(
        tool.take_notice(),
        Some(Notice::error("not found: anything"))
    );
}

#[test]
fn test_blame_runs_against_the_file_under_the_cursor() {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let command = command_of(&tool.on_key(&alt_char('b')));
    assert_eq!(command.tag, exec::TAG_BLAME);
    assert_eq!(command.args.last().map(String::as_str), Some("working.rs"));
    assert_eq!(command.cwd, PathBuf::from("/repo"));
}

#[test]
fn test_blame_off_a_file_says_there_is_nothing_to_blame() {
    // A heading and a commit have no single path, and running blame on the
    // repository root would report a git error the user did not ask for.
    let mut tool = loaded_tool();
    tool.list.set_cursor(0);
    assert_eq!(tool.on_key(&alt_char('b')), ToolOutcome::Consumed);
    assert_eq!(
        notice_text(&mut tool).as_deref(),
        Some("no file to blame here")
    );
}

#[test]
fn test_a_blame_fills_the_view_under_its_own_title() {
    let mut tool = loaded_tool();
    tool.on_job(reply(
        exec::TAG_BLAME,
        "abc12345 (me 2026-01-01 1) fn main\n",
    ));
    assert_eq!(
        tool.output.as_ref().map(|output| output.title.as_str()),
        Some("Blame")
    );
}

#[test]
fn test_the_tab_says_which_view_it_shows_and_of_which_project() {
    assert_eq!(
        GitTool::new(PathBuf::from("/repo")).title(),
        "Git",
        "not read yet"
    );
    let mut tool = loaded_tool();
    assert_eq!(tool.title(), "Git Status: repo");

    tool.on_job(reply(
        exec::TAG_LOG_VIEW,
        &log_output(&[("abc1234", "one")]),
    ));
    assert_eq!(tool.title(), "Git Log: repo");

    let shown = ["commit abc1234567890", "", "    do the thing"].join("\n");
    tool.on_job(reply(exec::TAG_SHOW, &shown));
    assert!(
        tool.title().starts_with("Git Commit abc1234"),
        "{}",
        tool.title()
    );
    assert!(tool.title().ends_with(": repo"));

    tool.on_key(&press_char('q'));
    assert_eq!(tool.title(), "Git Status: repo", "back to the status");
}

#[test]
fn test_keys_before_the_root_is_known_do_nothing() {
    // The view is up as soon as the key is pressed, so every command has to
    // survive being asked before git has answered.
    let mut tool = GitTool::new(PathBuf::from("/repo"));
    for code in [
        KeyCode::Char('s'),
        KeyCode::Char('u'),
        KeyCode::Char('P'),
        KeyCode::Char('F'),
        KeyCode::Char('f'),
    ] {
        assert_eq!(tool.on_key(&press(code)), ToolOutcome::Consumed);
    }
}

#[test]
fn test_git_jump_status_rows_and_output_lines() {
    let mut tool = loaded_tool();
    let targets = tool.jump_targets().expect("status jump targets");
    assert!(!targets.is_empty());
    assert_eq!(tool.jump_cursor(), Some(0));

    // Jump preview on status view
    tool.list.view_mut().set_rows(10);
    tool.jump_to(1);
    assert_eq!(tool.list.cursor(), 1);
    assert_eq!(tool.list.view().scroll(), 1_usize.saturating_sub(10 / 2));
    tool.jump_cancel();
    assert_eq!(tool.list.cursor(), 0);
    assert_eq!(tool.list.view().scroll(), 0);

    // Jump preview and confirm on status view
    tool.jump_to(1);
    tool.jump_confirm(1);
    assert_eq!(tool.list.cursor(), 1);
    assert_eq!(tool.list.view().scroll(), 1_usize.saturating_sub(10 / 2));
    assert!(!tool.list.is_jumping());

    // Now with output (e.g. blame)
    tool.on_job(reply(exec::TAG_BLAME, "line 1\nline 2\nline 3\n"));
    let output_targets = tool.jump_targets().expect("output jump targets");
    assert_eq!(output_targets.len(), 3);
    assert_eq!(tool.jump_cursor(), Some(0));

    tool.jump_to(2);
    assert_eq!(tool.output.as_ref().unwrap().list.cursor(), 2);
    tool.jump_cancel();
    assert_eq!(tool.output.as_ref().unwrap().list.cursor(), 0);

    tool.jump_to(2);
    tool.jump_confirm(2);
    assert_eq!(tool.output.as_ref().unwrap().list.cursor(), 2);
}

fn yes(tag: &'static str) -> PromptReply {
    PromptReply {
        answer: Some("y".to_string()),
        tag,
    }
}

#[test]
fn test_the_file_menu_asks_before_discarding() {
    // `.` then `x` throws away as much as `x` does, so it asks the same.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let outcome = choose(&mut tool, '.', 'x');
    let ToolOutcome::Prompt(request) = outcome else {
        panic!("expected a confirmation, got {outcome:?}");
    };
    assert_eq!(request.mode, PromptMode::Confirm);
    assert!(request.label.contains("working.rs"), "{:?}", request.label);
}

#[test]
fn test_a_confirmed_discard_acts_on_what_the_question_named() {
    // The tree is read again while the question is open, and a file
    // appearing above moves every row down one: the yes is still about
    // the file it was asked about.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let ToolOutcome::Prompt(request) = tool.on_key(&press_char('x')) else {
        panic!("expected a confirmation");
    };
    let moved = STATUS_OUTPUT.replace(
        "1 .M N... 100644 100644 100644 aaa bbb working.rs",
        "1 .M N... 100644 100644 100644 aaa bbb aaa-first.rs\n\
         1 .M N... 100644 100644 100644 aaa bbb working.rs",
    );
    tool.on_job(reply(exec::TAG_STATUS, &moved));
    let command = command_of(&tool.on_prompt(yes(request.tag)));
    assert_eq!(command.args, ["restore", "--worktree", "--", "working.rs"]);
}

#[test]
fn test_a_discard_turned_down_is_forgotten() {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let ToolOutcome::Prompt(request) = tool.on_key(&press_char('x')) else {
        panic!("expected a confirmation");
    };
    tool.on_prompt(PromptReply {
        answer: None,
        tag: request.tag,
    });
    assert_eq!(
        tool.on_prompt(yes(request.tag)),
        ToolOutcome::Consumed,
        "a stray yes later discards nothing"
    );
}

#[test]
fn test_a_re_read_keeps_the_cursor_on_its_row() {
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "working.rs");
    let moved = STATUS_OUTPUT.replace("? new.rs", "? aaa.rs\n? new.rs");
    tool.on_job(reply(exec::TAG_STATUS, &moved));
    assert!(
        matches!(&tool.selected().expect("a row").item, Item::File(file) if file.path == "working.rs"),
        "still on working.rs"
    );
}

#[test]
fn test_an_untracked_files_diff_answers_with_exit_code_one_and_shows() {
    // `diff --no-index` says "they differ" with 1, which is the whole
    // point of asking; read as a failure, no untracked file could open.
    let mut tool = loaded_tool();
    cursor_on(&mut tool, "new.rs");
    let command = command_of(&tool.on_key(&press(KeyCode::Tab)));
    assert_eq!(command.tag, exec::TAG_DIFF_UNTRACKED);
    tool.on_job(JobReply::Command(CommandOutput {
        code: Some(1),
        stderr: String::new(),
        stdout: "diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n\
                 +++ b/new.rs\n@@ -0,0 +1 @@\n+fresh\n"
            .to_string(),
        tag: exec::TAG_DIFF_UNTRACKED,
    }));
    assert_eq!(notice_text(&mut tool), None, "no failure reported");
    assert!(
        tool.rows
            .iter()
            .any(|row| matches!(&row.item, Item::Hunk(hunk) if hunk.path == "new.rs")),
        "the file's hunk shows"
    );
}

#[test]
fn test_a_branch_tracking_nothing_is_behind_nothing_rather_than_failing() {
    let mut tool = loaded_tool();
    let next = tool.on_job(JobReply::Command(CommandOutput {
        code: Some(128),
        stderr: "fatal: no upstream configured for branch 'main'".to_string(),
        stdout: String::new(),
        tag: exec::TAG_UNPULLED,
    }));
    assert_eq!(notice_text(&mut tool), None, "nothing worth reporting");
    assert!(
        matches!(next, ToolOutcome::Job(JobRequest::ReadFiles(_))),
        "the refresh carries on: {next:?}"
    );
}

#[test]
fn test_a_revision_that_could_pass_for_an_option_is_refused() {
    let mut tool = loaded_tool();
    let outcome = tool.on_prompt(PromptReply {
        answer: Some("-f".to_string()),
        tag: ASK_BRANCH_CHECKOUT,
    });
    assert_eq!(outcome, ToolOutcome::Consumed, "nothing runs");
    assert!(notice_text(&mut tool).is_some_and(|text| text.contains("not a revision")));
}

#[test]
fn test_an_ignored_path_is_anchored_and_escaped() {
    assert_eq!(ignore_pattern("out.log", false), "/out.log");
    assert_eq!(ignore_pattern("a/[x]*.txt", false), "/a/\\[x]\\*.txt");
    assert_eq!(ignore_pattern("trailing ", false), "/trailing\\ ");
    assert_eq!(ignore_pattern("src/main.rs", true), "*.rs");
    // A dot in a directory is no extension of the file under it.
    assert_eq!(ignore_pattern("a.d/Makefile", true), "/a.d/Makefile");
}

#[test]
fn test_every_key_of_the_view_is_spelled_right() {
    use crate::model::input::table;
    for keys in [keys::STATUS_KEYS, keys::OUTPUT_KEYS, keys::SEARCH_KEYS] {
        assert_eq!(table::invalid_specs(keys), Vec::<&str>::new());
    }
}
