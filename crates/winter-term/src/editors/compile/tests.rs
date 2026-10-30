//! The Compile tab's tests: builds run for real, under `sh`, in scratch
//! directories.

use super::*;

use std::path::Path;
use std::thread;

use crate::model::editor::BuildStart;
use crate::model::input::KeyCode;

/// Long enough for a loaded machine to start and end a shell.
const PATIENCE: Duration = Duration::from_secs(10);

/// A fresh directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("winter-compile-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("creates the directory");
    dir
}

/// A tab over `dir` that builds with `command`, opening no error by
/// itself.
fn tab(dir: &Path, command: &str) -> CompileEditor {
    CompileEditor::new(
        dir.to_path_buf(),
        Task::shell(command, dir.to_path_buf()),
        false,
    )
}

/// Start `editor`'s build, which asks the host first, and run it to its end.
fn build(editor: &mut CompileEditor) {
    match editor.run_command(RERUN_COMMAND) {
        Some(EditorOutcome::Build(_)) => {}
        other => panic!("the build did not ask to start: {other:?}"),
    }
    run_to_end(editor);
}

/// Tick `editor` until its build and its log reading are done.
fn run_to_end(editor: &mut CompileEditor) {
    let deadline = Instant::now() + PATIENCE;
    while editor.next_tick().is_some() {
        assert!(Instant::now() < deadline, "the build did not end");
        thread::sleep(POLL);
        editor.on_tick(Instant::now());
    }
}

/// Every row's text, painted the way the pane would.
fn texts(editor: &mut CompileEditor) -> Vec<String> {
    let frame = editor.content(200, 120, false);
    frame.rows.iter().map(row_text).collect()
}

#[cfg(unix)]
#[test]
fn test_a_failed_builds_errors_are_places_that_open_in_the_editor() {
    let dir = scratch("errors");
    fs::create_dir_all(dir.join("src")).expect("creates src");
    fs::write(dir.join("src/a.c"), "int main() {}\n").expect("writes the source");
    let mut editor = tab(
        &dir,
        "printf 'building\\n'; printf 'src/a.c:3:5: error: expected\\n'; exit 2",
    );
    build(&mut editor);
    let rows = texts(&mut editor);
    assert!(rows[0].starts_with("printf"), "{rows:?}");
    assert_eq!(rows[2], "src/a.c:3:5: error: expected");
    assert!(
        rows.iter()
            .any(|row| row.starts_with("Build failed with exit code 2")),
        "{rows:?}"
    );
    // The place is painted as one, in the hue of an error.
    let frame = editor.content(200, 120, false);
    assert_eq!(frame.rows[2][0].text, "src/a.c:3:5");
    assert_eq!(
        frame.rows[2][0].style,
        Style::Diagnostic(DiagnosticLevel::Error)
    );
    // M-n finds it from the top, and RET opens it.
    editor.set_cursor(0);
    editor.on_key(&Key::with_alt(KeyCode::Char('n')));
    assert_eq!(editor.cursor, 2);
    assert_eq!(
        editor.on_key(&Key::plain(KeyCode::Enter)),
        EditorOutcome::OpenBeside(OpenTarget::at(dir.join("src/a.c"), 3, Some(5)))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_next_error_walks_every_place_and_comes_round_again() {
    let dir = scratch("walk");
    fs::write(dir.join("a.c"), "").expect("writes a.c");
    fs::write(dir.join("b.c"), "").expect("writes b.c");
    let mut editor = tab(&dir, "printf 'a.c:1: error: x\\nb.c:2: warning: y\\n'");
    build(&mut editor);
    let opened = |editor: &mut CompileEditor, command| match editor.run_command(command) {
        Some(EditorOutcome::OpenBeside(target)) => target,
        other => panic!("opened nothing: {other:?}"),
    };
    assert_eq!(
        opened(&mut editor, NEXT_PLACE_COMMAND).path,
        dir.join("a.c")
    );
    assert_eq!(
        opened(&mut editor, NEXT_PLACE_COMMAND).path,
        dir.join("b.c")
    );
    assert_eq!(
        opened(&mut editor, NEXT_PLACE_COMMAND).path,
        dir.join("a.c")
    );
    assert_eq!(
        opened(&mut editor, PREVIOUS_PLACE_COMMAND).path,
        dir.join("b.c")
    );
    assert_eq!(editor.status().segments[0], "1 error, 1 warning");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_the_cursor_rides_the_end_until_it_is_moved_off_it() {
    let dir = scratch("follow");
    let mut editor = tab(&dir, "printf 'one\\ntwo\\nthree\\n'");
    build(&mut editor);
    let last = editor.total_rows() - 1;
    assert_eq!(editor.cursor, last);
    editor.on_key(&Key::plain(KeyCode::Char('k')));
    assert!(!editor.follow);
    editor.on_key(&Key::plain(KeyCode::Char('G')));
    assert!(editor.follow, "back at the end, it rides it again");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_stopping_a_build_says_it_was_stopped() {
    let dir = scratch("stop");
    let mut editor = tab(&dir, "sleep 30");
    editor.run_command(RERUN_COMMAND);
    editor.on_tick(Instant::now());
    assert!(editor.is_busy());
    editor.on_key(&Key::with_ctrl(KeyCode::Char('g')));
    run_to_end(&mut editor);
    let rows = texts(&mut editor);
    assert!(
        rows.iter()
            .any(|row| row.starts_with("Build stopped after")),
        "{rows:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_a_documents_log_adds_its_warnings_and_leaves_out_errors_already_shown() {
    let dir = scratch("latex");
    fs::write(dir.join("main.tex"), "\\documentclass{article}\n").expect("writes the root");
    fs::write(
        dir.join("main.log"),
        "(./main.tex\n./main.tex:5: Undefined control sequence.\n\
         LaTeX Warning: Reference `x' on page 1 undefined on input line 7.\n)\n",
    )
    .expect("writes the log");
    let task = Task {
        document: Some(dir.join("main.tex")),
        name: "LaTeX: main.tex".to_string(),
        ..Task::shell(
            "printf './main.tex:5: Undefined control sequence.\\nTranscript written on main.log.\\n'; exit 1",
            dir.clone(),
        )
    };
    let mut editor = CompileEditor::new(dir.clone(), task, false);
    build(&mut editor);
    let rows = texts(&mut editor);
    assert!(
        rows.contains(&"LaTeX log: 0 errors, 1 warning, 0 bad boxes".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(
            &"./main.tex:7: warning: Reference `x' on page 1 undefined on input line 7."
                .to_string()
        ),
        "{rows:?}"
    );
    // The error the output placed and the log's warning are both walked.
    assert_eq!(editor.found.len(), 2);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_a_place_splits_the_run_it_falls_in_and_keeps_the_rest_of_its_colors() {
    let red = TerminalStyle {
        foreground: Some(crate::model::paint::TerminalColor::Indexed(1)),
        ..TerminalStyle::default()
    };
    let runs = vec![
        (red, "error".to_string()),
        (TerminalStyle::default(), ": at a.rs:1:2 here".to_string()),
    ];
    let row = output_row(
        &runs,
        Some(Painted {
            current: false,
            level: DiagnosticLevel::Warning,
            span: &(10..18),
        }),
    );
    let pieces: Vec<(&str, Style)> = row
        .iter()
        .map(|span| (span.text.as_str(), span.style))
        .collect();
    assert_eq!(
        pieces,
        [
            ("error", Style::Terminal(red)),
            (": at ", Style::Normal),
            ("a.rs:1:2", Style::Diagnostic(DiagnosticLevel::Warning)),
            (" here", Style::Normal),
        ]
    );
}

#[cfg(unix)]
#[test]
fn test_a_build_asks_the_host_before_it_starts_and_runs_on_the_next_tick() {
    let dir = scratch("ask");
    let mut editor = tab(&dir, "true");
    assert_eq!(
        editor.run_command(RERUN_COMMAND),
        Some(EditorOutcome::Build(BuildStart {
            command: "true".to_string(),
            dir: dir.clone(),
            root: dir.clone(),
        }))
    );
    assert!(
        editor.runner.is_none(),
        "nothing runs until the host has saved"
    );
    assert!(editor.next_tick().is_some_and(|at| at <= Instant::now()));
    editor.on_tick(Instant::now());
    assert!(editor.runner.is_some() || matches!(editor.phase, Phase::Done(_)));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_errors_only_keeps_each_place_with_its_message_and_how_the_build_ended() {
    let dir = scratch("only");
    fs::write(dir.join("a.rs"), "").expect("writes a.rs");
    let mut editor = tab(
        &dir,
        "printf 'Compiling\\nerror[E0425]: cannot find x\\n --> a.rs:1:2\\nnoise\\n'; exit 1",
    );
    build(&mut editor);
    editor.on_key(&Key::plain(KeyCode::Char('e')));
    let rows = texts(&mut editor);
    assert_eq!(rows[1], "error[E0425]: cannot find x");
    assert_eq!(rows[2], " --> a.rs:1:2");
    assert!(
        !rows.iter().any(|row| row == "noise" || row == "Compiling"),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.starts_with("Build failed")),
        "{rows:?}"
    );
    // Back to everything, with the cursor where it was.
    editor.on_key(&Key::plain(KeyCode::Char('e')));
    assert!(texts(&mut editor).contains(&"noise".to_string()));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_a_failed_build_opens_its_first_error_once_when_asked_to() {
    let dir = scratch("jump");
    fs::write(dir.join("a.c"), "").expect("writes a.c");
    let mut editor = CompileEditor::new(
        dir.clone(),
        Task::shell(
            "printf 'a.c:1: warning: w\\na.c:2: error: e\\n'; exit 1",
            dir.clone(),
        ),
        true,
    );
    editor.run_command(RERUN_COMMAND);
    let deadline = Instant::now() + PATIENCE;
    let mut opened = Vec::new();
    while editor.next_tick().is_some() {
        assert!(Instant::now() < deadline, "the build did not end");
        thread::sleep(POLL);
        if let EditorOutcome::OpenBeside(target) = editor.on_tick(Instant::now()) {
            opened.push(target);
        }
    }
    opened.extend(match editor.on_tick(Instant::now()) {
        EditorOutcome::OpenBeside(target) => Some(target),
        _ => None,
    });
    assert_eq!(
        opened,
        [OpenTarget::at(dir.join("a.c"), 2, None)],
        "the error, not the warning, once"
    );
    assert!(
        editor
            .take_notice()
            .is_some_and(|notice| notice.text.contains("failed")),
        "the end is told"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_a_line_typed_to_a_running_build_reaches_it() {
    let dir = scratch("input");
    let mut editor = tab(&dir, "read answer; echo \"you said $answer\"");
    editor.run_command(RERUN_COMMAND);
    editor.on_tick(Instant::now());
    assert!(matches!(
        editor.on_key(&Key::plain(KeyCode::Char('i'))),
        EditorOutcome::Prompt(_)
    ));
    editor.on_prompt(PromptReply {
        answer: Some("yes".to_string()),
        tag: ASK_INPUT,
    });
    run_to_end(&mut editor);
    assert!(texts(&mut editor).iter().any(|row| row == "you said yes"));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_the_place_last_opened_is_marked_in_the_output() {
    let dir = scratch("mark");
    fs::write(dir.join("a.c"), "").expect("writes a.c");
    let mut editor = tab(&dir, "printf 'a.c:1: error: x\\na.c:2: error: y\\n'");
    build(&mut editor);
    editor.run_command(NEXT_PLACE_COMMAND);
    editor.run_command(NEXT_PLACE_COMMAND);
    let frame = editor.content(50, 120, false);
    let marked: Vec<&str> = frame
        .rows
        .iter()
        .flatten()
        .filter(|span| span.selected)
        .map(|span| span.text.as_str())
        .collect();
    assert_eq!(marked, ["a.c:2"]);
    let _ = fs::remove_dir_all(&dir);
}
