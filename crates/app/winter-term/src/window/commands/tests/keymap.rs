//! The default and user keymaps checked against the commands the app runs:
//! every chord bound to a name must reach a command the palette or an editor
//! answers to.

use std::collections::HashMap;

use crate::window::commands::{command_label, window_commands};
use winter_model::input::test_support::key;
use winter_model::input::{
    default_bindings_maps, window_action_by_name, Action, Blocks, Catalog, Key, KeyCode, Keymap,
    SequenceStep,
};
use winter_model::layout::FocusDir;

#[test]
fn test_every_default_named_chord_dispatches_to_a_real_command() {
    // A chord bound to a name nothing answers to is a dead key: pressing it
    // does nothing at all, and a test of one binding at a time never sees
    // it. Every editor chord arrives this way.
    let keymap = Keymap::default();
    let commands: Vec<String> = window_commands(&keymap)
        .into_iter()
        .map(|entry| entry.action)
        .collect();
    for (_, name) in &keymap.named {
        assert!(
            commands.contains(name) || window_action_by_name(name).is_some(),
            "{name} is bound by default, but nothing dispatches it"
        );
    }
}

#[test]
fn test_a_sequence_may_name_a_palette_command_and_run_three_chords() {
    let mut bindings = HashMap::new();
    bindings.insert("C-x t n".to_string(), "mux_new_session".to_string());
    let keymap = Keymap::from_config(Some(&bindings), None);
    let ctrl_x = Key {
        ctrl: true,
        ..key(KeyCode::Char('x'))
    };
    let t = key(KeyCode::Char('t'));
    assert_eq!(
        keymap.resolve_sequence(&[ctrl_x, t], None),
        SequenceStep::Pending
    );
    assert_eq!(
        keymap.resolve_sequence(&[ctrl_x, t, key(KeyCode::Char('n'))], None),
        SequenceStep::Run(Action::RunCommand("mux_new_session".to_string()))
    );
    assert_eq!(keymap.chord_hint("mux_new_session"), "C-x t n");
}

/// The palette's and the editors' commands, as the app checks against.
const CATALOG: Catalog = Catalog {
    is_command: |name| command_label(name).is_some(),
    editor_commands: crate::editors::block_commands,
};

fn check(blocks: &Blocks) -> Vec<String> {
    Keymap::load(blocks, CATALOG).1
}

fn blocks_of(entries: &[(&str, &[(&str, &str)])]) -> HashMap<String, HashMap<String, String>> {
    entries
        .iter()
        .map(|(block, bindings)| {
            let bindings = bindings
                .iter()
                .map(|(spec, name)| (spec.to_string(), name.to_string()))
                .collect();
            (block.to_string(), bindings)
        })
        .collect()
}

#[test]
fn test_the_shipped_keybindings_have_no_problems() {
    assert_eq!(check(&default_bindings_maps()), Vec::<String>::new());
}

#[test]
fn test_a_reported_binding_is_left_out_and_the_default_on_its_chord_stays() {
    let blocks = blocks_of(&[("window", &[("M-h", "no_such_command")])]);
    let (keymap, problems) = Keymap::load(&blocks, CATALOG);
    assert_eq!(problems.len(), 1);
    let alt_h = Key::with_alt(KeyCode::Char('h'));
    assert_eq!(
        keymap.direct_action(&alt_h, false),
        Some(Action::FocusPane(FocusDir::Left)),
        "the default focus_left still answers M-h"
    );
}

#[test]
fn test_an_editor_chord_a_global_chord_takes_first_is_reported() {
    let blocks = blocks_of(&[("git", &[("C-S-n", "open_remote")])]);
    assert_eq!(
        check(&blocks),
        vec![
            r#"git: "C-S-n" never runs: "C-S-n" is a global chord, taken before any editor sees it"#
                .to_string()
        ]
    );
}

#[test]
fn test_check_blocks_reports_what_could_never_run() {
    let blocks = blocks_of(&[
        (
            "window",
            &[("M-q q", "split_vertical"), ("M-z", "no_such_command")],
        ),
        ("editing", &[("C-x C-y", "prompt_undo")]),
        ("git", &[("M-g r", "open_remot")]),
        ("gti", &[("M-o", "open_remote")]),
    ]);
    let problems = check(&blocks);
    for expected in [
        r#"window: "M-z" runs unknown command "no_such_command""#,
        r#"editing: "C-x C-y" is not a chord"#,
        r#"git: "M-g r" runs unknown command "open_remot""#,
        "`gti` is no editor's block",
    ] {
        assert!(
            problems.iter().any(|p| p == expected),
            "missing {expected:?} in {problems:?}"
        );
    }
    assert_eq!(problems.len(), 4, "{problems:?}");
}

#[test]
fn test_check_blocks_reports_bindings_that_overlap_across_window_and_editing() {
    let blocks = blocks_of(&[
        ("window", &[("M-g", "new_tab")]),
        ("editing", &[("M-g", "prompt_undo")]),
    ]);
    assert_eq!(
        check(&blocks),
        vec![r#"window: "M-g" and editing: "M-g" overlap: only one can run"#.to_string()]
    );
}
