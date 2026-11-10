mod keymap;
mod palette;

use super::*;

use crate::window::editor::EditorKind;
use winter_model::input::Action;

#[test]
fn test_each_command_is_listed_once_and_runs_as_its_kind_says() {
    let mut seen = std::collections::HashSet::new();
    for builtin in BUILTIN {
        assert!(seen.insert(builtin.id), "{} is listed twice", builtin.id);
        match builtin.run {
            Run::Action => assert!(
                window_action_by_name(builtin.id).is_some(),
                "{} is no key binding's action",
                builtin.id
            ),
            Run::Editor => assert!(
                is_editor_command(builtin.id),
                "{} is no editor's command",
                builtin.id
            ),
            Run::Window(_) => {}
        }
    }
}

#[test]
fn test_an_editor_s_command_chosen_in_the_palette_reaches_the_editor_in_front() {
    let dir = std::env::temp_dir().join(format!("winter-commands-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "text\n").expect("file");
    let mut app = App::new();
    let editor = winter_text::TextEditor::new(file, None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));
    assert!(app.chrome.overlay.is_none());
    app.run_command("query_replace", tab);
    assert!(app.chrome.overlay.is_some(), "it asks what to replace");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_repeat_last_command_replays_previous_action() {
    let dir = std::env::temp_dir().join(format!("winter-repeat-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("test.txt");
    std::fs::write(&file, "foo bar\n").expect("file");
    let mut app = App::new();
    let editor = winter_text::TextEditor::new(file, None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));

    // With no command to repeat, running it does not panic
    app.run_command("repeat_last_command", tab);

    // Record a command into palette history and repeat it
    app.recent.last_palette_command = Some("uppercase_word".to_string());
    app.run_command("repeat_last_command", tab);

    let editor = app.open_tabs.page(&tab).unwrap();
    let text_editor = editor
        .page
        .downcast_ref::<winter_text::TextEditor>()
        .unwrap();
    assert_eq!(text_editor.buffer().lines()[0], "FOO bar");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_an_alias_finds_the_command_it_names() {
    for (alias, id) in ALIASES {
        assert_eq!(find(alias).map(|builtin| builtin.id), Some(*id), "{alias}");
        assert!(find(alias).is_some_and(|builtin| builtin.id != *alias));
    }
    assert!(find("no_such_command").is_none());
}

#[test]
fn test_close_tabs_to_left_and_right_action_and_aliases() {
    for name in [
        "close_tabs_to_left",
        "close_tabs_left",
        "close_tabs_to_the_left",
    ] {
        assert_eq!(window_action_by_name(name), Some(Action::CloseTabsToLeft));
        assert_eq!(find(name).map(|b| b.id), Some("close_tabs_to_left"));
    }
    for name in [
        "close_tabs_to_right",
        "close_tabs_right",
        "close_tabs_to_the_right",
    ] {
        assert_eq!(window_action_by_name(name), Some(Action::CloseTabsToRight));
        assert_eq!(find(name).map(|b| b.id), Some("close_tabs_to_right"));
    }
}

#[test]
fn test_every_command_icon_names_an_editor_with_artwork() {
    // The command table names editors by string, apart from the kinds the
    // icons are keyed by: a misspelt one would draw the generic file icon
    // beside the command rather than fail anywhere.
    let editors: Vec<&str> = BUILTIN
        .iter()
        .filter_map(|builtin| match command_icon(builtin.id)? {
            IconKind::Editor { kind } => Some(kind),
            IconKind::Dir { .. }
            | IconKind::File { .. }
            | IconKind::Git { .. }
            | IconKind::Gutter { .. }
            | IconKind::Program { .. }
            | IconKind::Symbol { .. }
            | IconKind::Terminal => None,
        })
        .collect();
    assert!(!editors.is_empty());
    for kind in editors {
        assert_ne!(
            crate::icons::name_for(&IconKind::Editor { kind }),
            crate::icons::name_for(&IconKind::Editor { kind: "" }),
            "{kind} draws the generic file icon"
        );
    }
}

#[test]
fn test_an_outline_with_nothing_in_it_opens_no_picker_and_says_so() {
    let dir = std::env::temp_dir().join(format!("winter-outline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "plain text\n").expect("file");
    let mut app = App::new();
    let editor = winter_text::TextEditor::new(file, None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));
    app.run_command("outline", tab);
    assert!(app.chrome.picker().is_none(), "no empty list");
    assert_eq!(app.chrome.notice_text(), Some("nothing to outline here"));
    let _ = std::fs::remove_dir_all(&dir);
}
