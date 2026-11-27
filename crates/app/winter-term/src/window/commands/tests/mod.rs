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
    app.run_command("upcase_word", tab);
    let editor = app
        .open_tabs
        .page(&tab)
        .and_then(|slot| slot.page.as_editor());
    let text = editor
        .and_then(|editor| editor.document())
        .map(|(_, text)| text);
    assert_eq!(text.as_deref(), Some("TEXT"), "Cherry's upcase-word ran");
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
    app.recent.last_palette_command = Some("upcase_word".to_string());
    app.run_command("repeat_last_command", tab);

    let editor = app
        .open_tabs
        .page(&tab)
        .and_then(|slot| slot.page.as_editor());
    let text = editor
        .and_then(|editor| editor.document())
        .map(|(_, text)| text);
    assert_eq!(text.as_deref(), Some("FOO bar"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_command_answers_to_its_id_alone() {
    assert_eq!(
        window_action_by_name("close_tabs_to_left"),
        Some(Action::CloseTabsToLeft)
    );
    assert_eq!(
        find("close_tabs_to_left").map(|b| b.id),
        Some("close_tabs_to_left")
    );
    for retired in ["close_tabs_left", "live_search", "jump", "new_tab"] {
        assert_eq!(window_action_by_name(retired), None, "{retired}");
        assert!(find(retired).is_none(), "{retired}");
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
            | IconKind::Named { .. }
            | IconKind::Program { .. }
            | IconKind::Spinner
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
    assert!(app.parts.picker().is_none(), "no empty list");
    assert_eq!(app.parts.notice_text(), Some("nothing to outline here"));
    let _ = std::fs::remove_dir_all(&dir);
}
