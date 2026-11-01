//! The command palette over the app's own commands, driven through the
//! window as `M-x` opens it: what it lists, how the query reads, what
//! choosing a command does, and its own history.

use winter_model::input::Action;

use crate::window::commands::COMMANDS_PICKER;
use crate::window::App;

fn palette() -> App {
    let mut app = App::new();
    let focused = app.layout().focused();
    app.handle_action(Action::TogglePalette, focused);
    app.settle_picker();
    app
}

#[test]
fn test_the_palette_lists_every_command_with_its_chord() {
    let app = palette();
    let view = app.chrome.picker().expect("the palette is open").view();
    assert_eq!(view.title, "Commands");
    assert!(view.total > 100, "every command: {}", view.total);
    assert!(
        view.rows.iter().any(|row| !row.row.hint.is_empty()),
        "a chord beside a command bound to one"
    );
}

#[test]
fn test_a_query_matches_a_commands_title_never_its_chord() {
    let mut app = palette();
    app.press_in_picker("C - S - n");
    let view = app.chrome.picker().expect("open").view();
    assert!(
        view.rows
            .iter()
            .all(|row| !row.row.hint.contains("C-S-n") || row.row.label.contains('-')),
        "no row matched only by its chord"
    );
}

#[test]
fn test_the_query_takes_emacs_edits_at_its_caret() {
    let mut app = palette();
    app.press_in_picker("n e w SPC t a b");
    app.press_in_picker("C-a M-d");
    assert_eq!(
        app.chrome.picker().expect("open").query(),
        " tab",
        "M-d killed the first word"
    );
    app.press_in_picker("C-y");
    assert_eq!(
        app.chrome.picker().expect("open").query(),
        "new tab",
        "C-y put it back"
    );
}

#[test]
fn test_choosing_a_command_runs_it_and_remembers_it_for_the_palette_alone() {
    let mut app = palette();
    let before = app.layout().members().len();
    app.press_in_picker("n e w SPC t a b RET");
    assert!(app.chrome.picker().is_none());
    assert_eq!(app.layout().members().len(), before + 1, "the tab opened");
    assert_eq!(app.recent.last_palette_command.as_deref(), Some("new_tab"));
    assert_eq!(app.pickers.history.of(COMMANDS_PICKER), ["new tab"]);
    assert!(
        app.pickers.history.of("project_files").is_empty(),
        "each picker its own"
    );
}

#[test]
fn test_m_p_brings_back_the_palettes_earlier_query() {
    let mut app = palette();
    app.press_in_picker("n e w SPC t a b RET");
    let focused = app.layout().focused();
    app.handle_action(Action::TogglePalette, focused);
    app.settle_picker();
    app.press_in_picker("M-p");
    assert_eq!(app.chrome.picker().expect("open").query(), "new tab");
}

#[test]
fn test_m_x_again_closes_the_palette() {
    let mut app = palette();
    let focused = app.layout().focused();
    app.handle_action(Action::TogglePalette, focused);
    assert!(app.chrome.picker().is_none());
}

#[test]
fn test_c_m_r_resumes_the_palette_as_it_was_closed() {
    let mut app = palette();
    app.press_in_picker("s p l i t");
    app.press_in_picker("ESC");
    assert!(app.chrome.picker().is_none());
    app.resume_picker();
    app.settle_picker();
    let picker = app.chrome.picker().expect("resumed");
    assert_eq!(picker.id(), COMMANDS_PICKER);
    assert_eq!(picker.query(), "split");
}
