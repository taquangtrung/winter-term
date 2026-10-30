//! The status view's tests.

use super::reply::browser_url;
use super::*;
use crate::editors::git_parse;
use crate::editors::git_exec;
use crate::model::editor::{CommandOutput, CommandRequest, GitViewRequest, PromptMode};
use crate::model::notify::Severity;

mod commits;
mod diffs;
mod log;
mod menus;
mod navigation;
mod remotes;
mod staging;
mod stashes;
mod status;
mod watching;

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
        id: 0,
        code: Some(0),
        stderr: String::new(),
        stdout: stdout.to_string(),
        tag,
    })
}

/// Answer the oldest request in flight of the kind `tag` names with
/// `stdout`, by its id, as the job runner does: the reply finds what that
/// request was asked for.
fn answer_job(editor: &mut GitEditor, tag: &'static str, stdout: &str) -> EditorOutcome {
    let kind = |asked: &Asked| match tag {
        git_exec::TAG_DIFF | git_exec::TAG_DIFF_UNTRACKED => matches!(asked, Asked::Diff(_)),
        git_exec::TAG_BLAME => matches!(asked, Asked::Blame(_)),
        git_exec::TAG_RESOLVE => matches!(asked, Asked::Resolve(_)),
        git_exec::TAG_SHOW_HEAD => matches!(asked, Asked::Head(_)),
        git_exec::TAG_SPIN_OFF => matches!(asked, Asked::SpinOff(_)),
        git_exec::TAG_INSTANT_FIXUP => matches!(asked, Asked::Squash(_)),
        other => panic!("{other} keeps nothing in flight"),
    };
    let id = editor.in_flight.oldest(kind).expect("a request in flight to answer");
    editor.on_job(JobReply::Command(CommandOutput {
        code: Some(0),
        id,
        stderr: String::new(),
        stdout: stdout.to_string(),
        tag,
    }))
}

/// The command an outcome runs, here or as a view in a tab of its own, with
/// the settings every git request starts with (`-c core.quotePath=false`)
/// taken off its arguments, so a test reads the command itself.
fn command_of(outcome: &EditorOutcome) -> CommandRequest {
    match outcome {
        EditorOutcome::Job(JobRequest::Command(command))
        | EditorOutcome::GitView(GitViewRequest {
            request: JobRequest::Command(command),
            ..
        }) => {
            let mut command = command.clone();
            assert_eq!(command.args[..2], ["-c", "core.quotePath=false"]);
            command.args.drain(..2);
            command
        }
        other => panic!("expected a command, got {other:?}"),
    }
}

/// An editor that has heard back about its root, its status, and its log.
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
/// feeds the shape [`git_parse::parse_decorated_log`] actually reads.
fn log_output(commits: &[(&str, &str)]) -> String {
    let sep = git_parse::FIELD_SEP;
    commits
        .iter()
        .map(|(hash, subject)| format!("{hash}{sep}{sep}Someone{sep}1700000000{sep}{subject}\n"))
        .collect()
}

fn loaded_editor() -> GitEditor {
    let mut editor = GitEditor::new(PathBuf::from("/repo/sub"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT));
    editor.on_job(reply(
        git_exec::TAG_LOG,
        &log_output(&[("abc1234", "do the thing")]),
    ));
    editor
}

/// Everything the editor paints at a usual size, joined.
fn painted_text(editor: &mut GitEditor) -> String {
    editor
        .content(24, 80, false)
        .rows
        .iter()
        .flatten()
        .map(|span| span.text.as_str())
        .collect()
}

/// The text of what the editor has to report, taking it.
fn notice_text(editor: &mut GitEditor) -> Option<String> {
    editor.take_notice().map(|notice| notice.text)
}

fn cursor_on(editor: &mut GitEditor, path: &str) {
    let index = editor
        .rows
        .iter()
        .position(|row| matches!(&row.item, Item::File(file) if file.path == path))
        .unwrap_or_else(|| panic!("no row for {path}"));
    editor.list.set_cursor(index);
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

fn cursor_on_hunk(editor: &mut GitEditor, index: usize) {
    let position = editor
        .rows
        .iter()
        .position(|row| matches!(&row.item, Item::Hunk(hunk) if hunk.index == index))
        .unwrap_or_else(|| panic!("no row for hunk {index}"));
    editor.list.set_cursor(position);
}

/// An editor with `working.rs` expanded and its diff read.
fn editor_with_diff() -> GitEditor {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    editor.on_key(&press(KeyCode::Tab));
    answer_job(&mut editor, git_exec::TAG_DIFF, DIFF_OUTPUT);
    editor
}

/// The row of `editor`'s status standing for `path`.
fn row_of(editor: &GitEditor, path: &str) -> usize {
    editor
        .rows
        .iter()
        .position(|row| matches!(&row.item, Item::File(file) if file.path == path))
        .unwrap_or_else(|| panic!("no row for {path}"))
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

fn choose(editor: &mut GitEditor, opener: char, choice: char) -> EditorOutcome {
    editor.on_key(&menu_key(opener));
    editor.on_key(&press_char(choice))
}

/// The key opening the menu magit opens on `opener`: its own letter, or
/// where evil-collection moved it.
fn menu_key(opener: char) -> Key {
    // Magit's letter, where the view moved it as evil-collection does.
    match opener {
        'V' => press_char('_'),
        'z' => press_char('Z'),
        'l' => ctrl(KeyCode::Char('l')),
        other => press_char(other),
    }
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
fn search(editor: &mut GitEditor, text: &str) -> EditorOutcome {
    let outcome = editor.on_key(&press_char('/'));
    let EditorOutcome::Prompt(request) = outcome else {
        panic!("expected a prompt, got {outcome:?}");
    };
    assert_eq!(request.tag, ASK_SEARCH);
    editor.on_prompt(PromptReply {
        answer: Some(text.to_string()),
        tag: request.tag,
    })
}

fn row_under_cursor(editor: &GitEditor) -> String {
    editor
        .rows
        .get(editor.list.cursor())
        .map(|row| row_text(&row.spans))
        .unwrap_or_default()
}

/// Answer the question a menu choice asked with `answer`.
fn answer(editor: &mut GitEditor, tag: &'static str, answer: &str) -> CommandRequest {
    command_of(&editor.on_prompt(PromptReply {
        answer: Some(answer.to_string()),
        tag,
    }))
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

fn quick_pull_key() -> Key {
    Key {
        alt: true,
        code: KeyCode::Char('f'),
        ctrl: true,
        shift: true,
        sup: false,
    }
}

/// A `git show` of one file with two hunks, for the fold tests.
const SHOWN_TWO_HUNKS: &str = "commit abc1234567890\n\n    do the thing\ndiff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n-    old();\n+    new();\n@@ -9,2 +9,2 @@\n-    second();\n+    third();\n";

/// A commit touching two files, so folding every file at once is visible
/// as more than folding the one.
const SHOWN_TWO_FILES: &str = "commit abc1234567890\n\n    do the thing\ndiff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n-    old();\n+    new();\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1,2 +1,2 @@\n-    second();\n+    third();\n";

/// Put the cursor on the first row whose text contains `needle`.
fn output_cursor_on(editor: &mut GitEditor, needle: &str) {
    let output = editor.output.as_mut().expect("an output view");
    let at = output
        .lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no output row for {needle}"));
    output.list.set_cursor(at);
}

fn output_has(editor: &GitEditor, needle: &str) -> bool {
    editor
        .output
        .as_ref()
        .is_some_and(|output| output.lines.iter().any(|line| line.contains(needle)))
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
fn offers_more(editor: &GitEditor) -> bool {
    let rows = match &editor.output {
        Some(output) => output.lines.clone(),
        None => editor.rows.iter().map(|row| row_text(&row.spans)).collect(),
    };
    rows.iter().any(|row| row.contains(rows::LOG_MORE))
}

/// Open the log of the branch from the status, as `C-l l` does.
fn open_log(editor: &mut GitEditor) -> EditorOutcome {
    editor.on_key(&ctrl(KeyCode::Char('l')));
    editor.on_key(&press_char('l'))
}

/// The tab of its own a view the status asked for opens as.
fn view_tab(outcome: EditorOutcome) -> GitEditor {
    match outcome {
        EditorOutcome::GitView(view) => GitEditor::viewing(view),
        other => panic!("expected a view in a tab of its own, got {other:?}"),
    }
}

/// The loaded editor with one stash listed, and the cursor on `item`.
fn stash_editor_on(item: Item) -> GitEditor {
    let mut editor = loaded_editor();
    editor.on_job(reply(
        git_exec::TAG_STASHES,
        "stash@{0}\u{1f}WIP on main: a thing\n",
    ));
    let at = editor
        .rows
        .iter()
        .position(|row| row.item == item)
        .expect("the row");
    editor.list.set_cursor(at);
    editor
}

/// The fixture's editor with the push menu open over `config`, what `git
/// config --get-regexp` printed.
fn push_menu(config: &str) -> GitEditor {
    let mut editor = loaded_editor();
    let lookup = command_of(&editor.on_key(&press_char('P')));
    assert_eq!(lookup.tag, git_exec::TAG_PUSH_CONTEXT);
    assert_eq!(
        lookup.cwd,
        PathBuf::from("/repo"),
        "from the repository root"
    );
    editor.on_job(reply(git_exec::TAG_PUSH_CONTEXT, config));
    editor
}

const PUSH_CONFIG: &str = "remote.origin.url git@x:a.git\nremote.fork.url git@x:b.git\n";

fn yes(tag: &'static str) -> PromptReply {
    PromptReply {
        answer: Some("y".to_string()),
        tag,
    }
}

/// What `git diff --no-index` says of a copy of `main.rs` against the file,
/// run from the filesystem's root.
const COMPARED: &str = "diff --git a/src/main.rs b/tmp/never-written-copy-main.rs
index de98044..a7bc997 100644
--- a/src/main.rs
+++ b/tmp/never-written-copy-main.rs
@@ -1,3 +1,3 @@
 a
-b
+B
 c
";

fn comparison() -> GitEditor {
    GitEditor::comparing(CompareRequest {
        copies: vec![(
            PathBuf::from("/tmp/never-written-copy-main.rs"),
            Some(PathBuf::from("/src/main.rs")),
        )],
        new: PathBuf::from("/tmp/never-written-copy-main.rs"),
        old: PathBuf::from("/src/main.rs"),
        title: "main.rs against the file on disk".to_string(),
    })
}
