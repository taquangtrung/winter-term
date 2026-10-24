//! The listing's tests.

use super::*;
use crate::model::editor::FileOps;
use crate::model::input::table;
use crate::model::input::KeyCode;
use crate::model::paint::IconKind;
use crate::model::textedit::modal::EditMode;
use keys::{Act, EDIT_KEYS, KEYS};
use std::fs;

/// A temporary tree that removes itself on drop.
struct TempTree(PathBuf);

impl TempTree {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("winter-dirpage-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).expect("temp subdir");
        path
    }

    fn touch(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, "x").expect("temp file");
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

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

fn alt_shift(code: KeyCode) -> Key {
    Key {
        shift: true,
        sup: false,
        ..alt(code)
    }
}

fn ctrl(code: KeyCode) -> Key {
    Key {
        alt: false,
        code,
        ctrl: true,
        shift: false,
        sup: false,
    }
}

/// Toggle the name editor with its `C-x C-q` chord.
fn toggle_edit(editor: &mut DirEditor) {
    editor.on_key(&ctrl(KeyCode::Char('x')));
    editor.on_key(&ctrl(KeyCode::Char('q')));
}

fn selected_name(editor: &DirEditor) -> String {
    editor
        .selected()
        .map(|row| row.entry.name.clone())
        .unwrap_or_default()
}

#[test]
fn test_every_menu_entry_runs_a_key_the_listing_binds() {
    // The menu offers keys rather than commands of its own, so an entry
    // naming a chord the editor does not match is an entry that silently
    // does nothing when it is chosen. Nothing else checks the two sides
    // against each other.
    let tree = TempTree::new("menu_keys");
    tree.touch("a.txt");
    tree.dir("sub");
    let editor = DirEditor::new(tree.0.clone());

    for item in editor
        .context_items()
        .into_iter()
        .filter(|item| !item.is_separator())
    {
        let mut probe = DirEditor::new(tree.0.clone());
        assert_ne!(
            probe.on_key(&item.keys[0]),
            EditorOutcome::Ignored,
            "the menu offers {:?}, which the listing does not bind",
            item.label
        );
    }
}

#[test]
fn test_the_menu_offers_a_directory_only_what_a_directory_can_do() {
    // Handing a directory to $EDITOR or to the system handler as if it
    // were a file is the mistake a menu built without looking at the row
    // makes.
    let tree = TempTree::new("menu_rows");
    tree.touch("a.txt");
    tree.dir("sub");
    let mut editor = DirEditor::new(tree.0.clone());

    let labels = |editor: &DirEditor| -> Vec<String> {
        editor
            .context_items()
            .into_iter()
            .map(|item| item.label)
            .collect()
    };

    while selected_name(&editor) != "sub" {
        editor.on_key(&press(KeyCode::Char('j')));
    }
    let open_in_editor = table::menu_entry(KEYS, Act::OpenInEditor)
        .map(|(_, label)| label.to_string())
        .unwrap_or_default();
    assert!(!labels(&editor).contains(&open_in_editor));

    while selected_name(&editor) != "a.txt" {
        editor.on_key(&press(KeyCode::Char('j')));
    }
    assert!(labels(&editor).contains(&open_in_editor));
}

#[test]
fn test_the_emacs_buffer_ends_reach_the_first_and_last_entry() {
    let tree = TempTree::new("buffer_ends");
    tree.touch("a.txt");
    tree.touch("b.txt");
    tree.touch("c.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&Key {
        shift: true,
        ..alt(KeyCode::Char('>'))
    });
    assert_eq!(selected_name(&editor), "c.txt");
    editor.on_key(&Key {
        shift: true,
        ..alt(KeyCode::Char('<'))
    });
    assert_eq!(selected_name(&editor), "a.txt");
}

#[test]
fn test_opening_a_directory_scrolls_its_children_into_a_short_pane() {
    // Unfolding at the bottom of the pane used to leave the children
    // below the fold: the cursor stayed on the directory, which was
    // already visible, so nothing scrolled.
    let tree = TempTree::new("reveal");
    tree.dir("a-first");
    tree.dir("b-second");
    let last = tree.dir("c-last");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(last.join(name), "x").expect("child");
    }
    let mut editor = DirEditor::new(tree.0.clone());

    // A pane with room for the three directories and nothing else, with
    // the cursor on the last of them.
    editor.on_key(&press(KeyCode::Char('j')));
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(
        selected_name(&editor),
        "c-last",
        "sorted after the other two"
    );
    editor.content(4, 80, false);
    editor.on_key(&press(KeyCode::Tab));
    let painted: Vec<String> = editor
        .content(4, 80, false)
        .rows
        .iter()
        .map(row_text)
        .collect();
    assert!(
        painted.iter().any(|row| row.contains("a.txt")),
        "the children the fold opened are on screen, got {painted:?}"
    );
}

#[test]
fn test_cursor_stays_inside_the_listing() {
    let tree = TempTree::new("clamp");
    tree.touch("a.txt");
    tree.touch("b.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    for _ in 0..5 {
        editor.on_key(&press(KeyCode::Char('j')));
    }
    assert_eq!(selected_name(&editor), "b.txt");
    for _ in 0..5 {
        editor.on_key(&press(KeyCode::Char('k')));
    }
    assert_eq!(selected_name(&editor), "a.txt");
}

#[test]
fn test_entering_a_directory_makes_it_the_root() {
    let tree = TempTree::new("descend");
    let nested = tree.dir("nested");
    fs::write(nested.join("inner.txt"), "x").expect("inner file");
    let mut editor = DirEditor::new(tree.0.clone());

    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::Consumed
    );
    assert_eq!(editor.root, nested);
    assert_eq!(selected_name(&editor), "inner.txt");
}

#[test]
fn test_root_dir_stays_constant_while_moving_cursor() {
    let tree = TempTree::new("root_dir_check");
    tree.dir("subfolder");
    tree.touch("readme.md");
    let mut editor = DirEditor::new(tree.0.clone());

    assert_eq!(editor.root_dir(), Some(tree.0.clone()));
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(editor.root_dir(), Some(tree.0.clone()));
}

#[test]
fn test_entering_a_file_asks_the_host_to_open_it() {
    let tree = TempTree::new("open");
    let file = tree.touch("readme.md");
    let mut editor = DirEditor::new(tree.0.clone());

    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::OpenPath(OpenTarget::file(file))
    );
}

#[test]
fn test_going_up_lands_on_the_directory_just_left() {
    // Stepping up used to reset to the first row, so `l` then `h` walked
    // away from where the user was instead of back to it.
    let tree = TempTree::new("ascend");
    tree.dir("aaa");
    let target = tree.dir("zzz");
    let mut editor = DirEditor::new(target.clone());

    editor.on_key(&press(KeyCode::Char('h')));
    assert_eq!(editor.root, tree.0);
    assert_eq!(selected_name(&editor), "zzz");
}

#[test]
fn test_going_up_from_the_filesystem_root_is_a_no_op() {
    let mut editor = DirEditor::new(PathBuf::from("/"));
    editor.on_key(&press(KeyCode::Char('h')));
    assert_eq!(editor.root, PathBuf::from("/"));
}

#[test]
fn test_showing_dotfiles_keeps_the_cursor_on_the_same_entry() {
    // A reload renumbers every row, so a naive toggle leaves the cursor on
    // whatever entry inherited its index.
    let tree = TempTree::new("dotfiles");
    tree.touch(".hidden");
    tree.touch("visible.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    assert_eq!(selected_name(&editor), "visible.txt");

    editor.on_key(&press(KeyCode::Char('.')));
    assert_eq!(editor.rows.len(), 2, "the dotfile joined the listing");
    assert_eq!(selected_name(&editor), "visible.txt");
}

#[test]
fn test_folding_a_directory_lists_its_children_in_place() {
    let tree = TempTree::new("fold");
    let nested = tree.dir("nested");
    fs::write(nested.join("inner.txt"), "x").expect("inner file");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Tab));
    assert_eq!(editor.rows.len(), 2);
    assert_eq!(
        editor.root, tree.0,
        "expanding in place never moves the root"
    );
    assert_eq!(selected_name(&editor), "nested");

    editor.on_key(&press(KeyCode::Tab));
    assert_eq!(editor.rows.len(), 1);
}

#[test]
fn test_home_and_end_jump_to_the_ends() {
    let tree = TempTree::new("jumps");
    tree.touch("a.txt");
    tree.touch("b.txt");
    tree.touch("c.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::End));
    assert_eq!(selected_name(&editor), "c.txt");
    editor.on_key(&press(KeyCode::Home));
    assert_eq!(selected_name(&editor), "a.txt");
}

#[test]
fn test_a_stray_fold_sequence_is_abandoned_not_left_pending() {
    // A pending `z` that survived an unrelated key would swallow the next
    // keystroke as well.
    let tree = TempTree::new("pending");
    tree.touch("a.txt");
    tree.touch("b.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Char('z')));
    editor.on_key(&press(KeyCode::Char('!')));
    assert!(editor.pending.is_none());
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(selected_name(&editor), "b.txt");
}

fn tree_with_nested_children(tag: &str) -> (TempTree, PathBuf) {
    let tree = TempTree::new(tag);
    let nested = tree.dir("nested");
    fs::write(nested.join("child-a.txt"), "x").expect("child a");
    fs::write(nested.join("child-b.txt"), "x").expect("child b");
    tree.touch("zzz-sibling.txt");
    (tree, nested)
}

#[test]
fn test_siblings_stay_inside_the_subtree_the_cursor_is_in() {
    // A naive "next row at this depth" scan walks out of an expanded
    // directory and lands in the parent's next entry instead.
    let (tree, _) = tree_with_nested_children("siblings");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(selected_name(&editor), "child-a.txt");

    editor.on_key(&alt(KeyCode::Char('n')));
    assert_eq!(selected_name(&editor), "child-b.txt");
    editor.on_key(&alt(KeyCode::Char('n')));
    assert_eq!(selected_name(&editor), "child-b.txt", "no escape upward");
    editor.on_key(&alt(KeyCode::Char('p')));
    assert_eq!(selected_name(&editor), "child-a.txt");
}

#[test]
fn test_parent_and_first_child_walk_the_tree_vertically() {
    let (tree, _) = tree_with_nested_children("vertical");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));

    editor.on_key(&alt(KeyCode::Char('d')));
    assert_eq!(selected_name(&editor), "child-a.txt");
    editor.on_key(&alt(KeyCode::Char('u')));
    assert_eq!(selected_name(&editor), "nested");
    editor.on_key(&alt(KeyCode::Char('u')));
    assert_eq!(
        selected_name(&editor),
        "nested",
        "the top level has no parent"
    );
}

#[test]
fn test_h_collapses_then_steps_out_then_leaves_the_root() {
    // One key covering three cases: each must be tried in this order, or
    // collapsing a directory would also move the cursor off it.
    let (tree, _) = tree_with_nested_children("stepout");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(selected_name(&editor), "child-a.txt");

    editor.on_key(&press(KeyCode::Char('h')));
    assert_eq!(
        selected_name(&editor),
        "nested",
        "steps out to the parent row"
    );
    editor.on_key(&press(KeyCode::Char('h')));
    assert_eq!(selected_name(&editor), "nested", "collapses, staying put");
    assert_eq!(editor.rows.len(), 2, "the children are gone");

    editor.on_key(&press(KeyCode::Char('h')));
    assert_eq!(
        editor.root,
        tree.0.parent().expect("a parent").to_path_buf()
    );
}

#[test]
fn test_history_walks_back_and_forward_through_visited_directories() {
    let (tree, nested) = tree_with_nested_children("history");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Enter));
    assert_eq!(editor.root, nested);

    editor.on_key(&alt_shift(KeyCode::Char('b')));
    assert_eq!(editor.root, tree.0, "back returns to where it came from");
    editor.on_key(&alt_shift(KeyCode::Char('f')));
    assert_eq!(editor.root, nested, "forward undoes the step back");
}

#[test]
fn test_history_at_either_end_is_a_no_op() {
    let (tree, _) = tree_with_nested_children("history-ends");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&alt_shift(KeyCode::Char('b')));
    assert_eq!(editor.root, tree.0);
    editor.on_key(&alt_shift(KeyCode::Char('f')));
    assert_eq!(editor.root, tree.0);
}

#[test]
fn test_moving_somewhere_new_drops_the_forward_history() {
    // Otherwise `forward` points at a branch the user has left, and
    // C-i teleports somewhere unrelated.
    let (tree, nested) = tree_with_nested_children("history-branch");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Enter));
    editor.on_key(&alt_shift(KeyCode::Char('b')));
    assert!(!editor.forward.is_empty());

    editor.on_key(&press(KeyCode::Backspace));
    assert!(editor.forward.is_empty());
    editor.on_key(&alt_shift(KeyCode::Char('f')));
    assert_ne!(editor.root, nested);
}

#[test]
fn test_za_opens_one_level_then_closes_everything() {
    // One key that always does what the listing is not already showing:
    // open when shut, shut when anything is open.
    let tree = TempTree::new("fold-all");
    let nested = tree.dir("nested");
    fs::create_dir_all(nested.join("deeper")).expect("deeper dir");
    let mut editor = DirEditor::new(tree.0.clone());

    assert_eq!(editor.rows.len(), 1);
    editor.on_key(&press(KeyCode::Char('z')));
    editor.on_key(&press(KeyCode::Char('a')));
    assert_eq!(editor.rows.len(), 2, "nested is open, deeper is not");

    editor.on_key(&press(KeyCode::Char('z')));
    editor.on_key(&press(KeyCode::Char('a')));
    assert_eq!(editor.rows.len(), 1, "and closed again");
}

#[test]
fn test_the_depth_keys_open_exactly_that_many_levels() {
    // Reading the whole tree to show two levels is the mistake; each depth
    // must read only as far as it shows.
    let tree = TempTree::new("depth");
    let nested = tree.dir("nested");
    fs::create_dir_all(nested.join("deeper")).expect("deeper dir");
    fs::write(nested.join("deeper").join("leaf.txt"), "x").expect("leaf");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&ctrl(KeyCode::Char('c')));
    editor.on_key(&press(KeyCode::Char('1')));
    assert_eq!(editor.rows.len(), 2, "nested opens, deeper stays shut");

    editor.on_key(&ctrl(KeyCode::Char('c')));
    editor.on_key(&press(KeyCode::Char('2')));
    assert_eq!(editor.rows.len(), 3, "deeper opens, showing its leaf");

    editor.on_key(&ctrl(KeyCode::Char('c')));
    editor.on_key(&press(KeyCode::Char('0')));
    assert_eq!(editor.rows.len(), 1, "depth zero is everything closed");
}

#[test]
fn test_zu_opens_a_whole_subtree_and_zf_shuts_it() {
    let tree = TempTree::new("subtree");
    let nested = tree.dir("nested");
    fs::create_dir_all(nested.join("deeper")).expect("deeper dir");
    fs::write(nested.join("deeper").join("leaf.txt"), "x").expect("leaf");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Char('z')));
    editor.on_key(&press(KeyCode::Char('u')));
    assert_eq!(editor.rows.len(), 3, "the whole subtree, in one key");

    editor.on_key(&press(KeyCode::Char('z')));
    editor.on_key(&press(KeyCode::Char('f')));
    assert_eq!(editor.rows.len(), 1, "and shut, children included");
}

#[test]
fn test_l_expands_and_is_not_a_toggle() {
    // `l` on an open directory must leave it open: it opens, and `h` is
    // what closes, so holding either never flaps the tree.
    let (tree, _) = tree_with_nested_children("expand-key");
    let mut editor = DirEditor::new(tree.0.clone());

    for _ in 0..2 {
        editor.on_key(&press(KeyCode::Char('l')));
    }
    assert_eq!(editor.rows.len(), 4, "still expanded");

    editor.on_key(&press(KeyCode::Char('h')));
    assert_eq!(editor.rows.len(), 2, "closed, cursor still on it");
    assert_eq!(selected_name(&editor), "nested");
}

#[test]
fn test_zc_collapses_everything() {
    let (tree, _) = tree_with_nested_children("zm");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));
    assert_eq!(editor.rows.len(), 4);

    editor.on_key(&press(KeyCode::Char('z')));
    editor.on_key(&press(KeyCode::Char('c')));
    assert_eq!(editor.rows.len(), 2);
}

#[test]
fn test_o_hands_the_entry_to_the_system_handler() {
    let tree = TempTree::new("external");
    let file = tree.touch("photo.png");
    let mut editor = DirEditor::new(tree.0.clone());
    assert_eq!(
        editor.on_key(&press(KeyCode::Char('&'))),
        EditorOutcome::OpenExternal(file)
    );
}

/// Answer whichever way the question was asked: a destination is offered
/// as a list of the directories in reach, everything else as a prompt,
/// and both are answered under the same tag.
fn answer(editor: &mut DirEditor, outcome: EditorOutcome, text: &str) {
    let tag = match outcome {
        EditorOutcome::Prompt(request) => request.tag,
        EditorOutcome::Pick(request) => request.tag,
        other => panic!("expected a question, got {other:?}"),
    };
    let outcome = editor.on_prompt(PromptReply {
        answer: Some(text.to_string()),
        tag,
    });
    settle(editor, outcome);
}

/// Run the file job a copy, move, delete, or trash came back as, the way
/// the host would, and hand the editor its report.
fn settle(editor: &mut DirEditor, outcome: EditorOutcome) {
    if let EditorOutcome::Job(JobRequest::FileOps(work)) = outcome {
        let report = ops::run_file_ops(work, &std::sync::atomic::AtomicBool::new(false));
        editor.on_job(JobReply::FileOps(report));
    }
}

#[test]
fn test_a_new_file_opens_for_writing_and_the_listing_lands_on_it() {
    // A file is created to be written in. Leaving it as one more row in
    // the listing means finding it again before anything can go in it.
    let tree = TempTree::new("new-file");
    tree.touch("other.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&press(KeyCode::Char('_')));
    let EditorOutcome::Prompt(request) = outcome else {
        panic!("expected a question, got {outcome:?}");
    };
    let opened = editor.on_prompt(PromptReply {
        answer: Some("notes.md".to_string()),
        tag: request.tag,
    });

    let path = tree.0.join("notes.md");
    assert!(path.exists(), "the file was created");
    assert_eq!(
        opened,
        EditorOutcome::OpenPath(OpenTarget::file(path.clone()))
    );
    assert_eq!(
        editor.selected().map(|row| row.entry.path.clone()),
        Some(path),
        "and the listing is on it for when the editor closes"
    );
}

#[test]
fn test_a_new_directory_is_not_opened_for_writing() {
    // Handing a directory to the editor would report a read failure at a
    // moment nothing went wrong.
    let tree = TempTree::new("new-dir");
    let mut editor = DirEditor::new(tree.0.clone());
    let outcome = editor.on_key(&press(KeyCode::Char('+')));
    let EditorOutcome::Prompt(request) = outcome else {
        panic!("expected a question, got {outcome:?}");
    };
    assert_eq!(
        editor.on_prompt(PromptReply {
            answer: Some("src".to_string()),
            tag: request.tag,
        }),
        EditorOutcome::Consumed
    );
    assert!(tree.0.join("src").is_dir());
}

#[test]
fn test_a_name_that_could_not_be_created_opens_nothing() {
    // The failure is reported in the header; opening the file that was
    // never made would report a second, more confusing one.
    let tree = TempTree::new("new-file-clash");
    tree.touch("taken.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    let outcome = editor.on_key(&press(KeyCode::Char('_')));
    let EditorOutcome::Prompt(request) = outcome else {
        panic!("expected a question, got {outcome:?}");
    };
    assert_eq!(
        editor.on_prompt(PromptReply {
            answer: Some("taken.txt".to_string()),
            tag: request.tag,
        }),
        EditorOutcome::Consumed
    );
}

#[test]
fn test_marking_moves_on_so_a_run_of_marks_is_one_key_each() {
    let tree = TempTree::new("mark-run");
    tree.touch("a.txt");
    tree.touch("b.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Char('m')));
    editor.on_key(&press(KeyCode::Char('m')));
    assert_eq!(editor.marks.len(), 2);
    assert_eq!(editor.targets().len(), 2);
}

#[test]
fn test_an_operation_acts_on_the_marks_not_the_cursor() {
    // The cursor sits on an unmarked entry: deleting must leave it alone.
    let tree = TempTree::new("mark-target");
    tree.touch("marked.txt");
    tree.touch("untouched.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Char('m')));
    assert_eq!(selected_name(&editor), "untouched.txt");
    let outcome = editor.on_key(&press(KeyCode::Char('X')));
    answer(&mut editor, outcome, "y");

    assert!(!tree.path("marked.txt").exists());
    assert!(tree.path("untouched.txt").exists());
}

#[test]
fn test_a_delete_prompt_says_how_many_it_will_take() {
    let tree = TempTree::new("delete-count");
    tree.touch("a.txt");
    tree.touch("b.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Char('m')));
    editor.on_key(&press(KeyCode::Char('m')));

    let EditorOutcome::Prompt(request) = editor.on_key(&press(KeyCode::Char('x'))) else {
        panic!("expected a prompt");
    };
    assert!(request.label.contains('2'), "got {:?}", request.label);
    assert_eq!(request.mode, PromptMode::Confirm);
}

#[test]
fn test_declining_a_delete_keeps_the_files() {
    let tree = TempTree::new("delete-no");
    tree.touch("keep.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    let EditorOutcome::Prompt(request) = editor.on_key(&press(KeyCode::Char('x'))) else {
        panic!("expected a prompt");
    };
    editor.on_prompt(PromptReply {
        answer: None,
        tag: request.tag,
    });
    assert!(tree.path("keep.txt").exists());
}

#[test]
fn test_create_rename_and_delete_walk_the_listing_along() {
    let tree = TempTree::new("crud");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&press(KeyCode::Char('_')));
    answer(&mut editor, outcome, "notes.txt");
    assert!(tree.path("notes.txt").exists());
    assert_eq!(selected_name(&editor), "notes.txt", "the cursor follows it");

    let outcome = editor.on_key(&press(KeyCode::Char('R')));
    answer(&mut editor, outcome, "renamed.md");
    assert!(tree.path("renamed.md").exists());
    assert!(!tree.path("notes.txt").exists());

    let outcome = editor.on_key(&press(KeyCode::Char('x')));
    answer(&mut editor, outcome, "y");
    assert!(!tree.path("renamed.md").exists(), "trashed");
    assert!(editor.rows.is_empty());
}

#[test]
fn test_a_new_directory_joins_the_listing() {
    let tree = TempTree::new("mkdir");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&press(KeyCode::Char('+')));
    answer(&mut editor, outcome, "sub");
    assert!(tree.path("sub").is_dir());
}

#[test]
fn test_marked_entries_are_transferred_into_a_directory_chosen_from_a_list() {
    // The same picker the Git view chooses a branch with, over what a
    // destination may be here: a directory of this listing, by name,
    // since a transfer refuses anything reaching outside it.
    let tree = TempTree::new("destinations");
    tree.touch("a.txt");
    tree.touch("b.txt");
    let nested = tree.dir("nested");
    let mut editor = DirEditor::new(tree.0.clone());
    // The directory sorts first, so mark the two files after it.
    editor.on_key(&press(KeyCode::Char('j')));
    editor.on_key(&press(KeyCode::Char('m')));
    editor.on_key(&press(KeyCode::Char('m')));

    let EditorOutcome::Pick(request) = editor.on_key(&press(KeyCode::Char('C'))) else {
        panic!("expected a list of directories");
    };
    assert_eq!(request.label, "Copy 2 entries into");
    assert_eq!(request.items, ["nested"], "by name, not by path");

    // The choice lands as the answer to the same question.
    let outcome = editor.on_prompt(PromptReply {
        answer: Some("nested".to_string()),
        tag: request.tag,
    });
    settle(&mut editor, outcome);
    assert!(nested.join("a.txt").exists(), "the copy went there");
}

#[test]
fn test_one_entry_is_still_asked_for_by_name() {
    // A single transfer takes the new name to give it, which is nothing
    // a list could offer.
    let tree = TempTree::new("single");
    tree.touch("a.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&press(KeyCode::Char('C')));
    assert!(
        matches!(outcome, EditorOutcome::Prompt(_)),
        "got {outcome:?}"
    );
}

#[test]
fn test_copy_and_move_of_a_single_entry_use_the_answer_as_its_name() {
    let tree = TempTree::new("transfer");
    tree.touch("original.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&press(KeyCode::Char('C')));
    answer(&mut editor, outcome, "copy.txt");
    assert!(tree.path("copy.txt").exists());
    assert!(
        tree.path("original.txt").exists(),
        "a copy leaves the source"
    );

    let outcome = editor.on_key(&alt(KeyCode::Char('m')));
    answer(&mut editor, outcome, "moved.txt");
    assert!(tree.path("moved.txt").exists());
}

#[test]
fn test_several_marked_entries_transfer_into_a_directory() {
    let tree = TempTree::new("transfer-many");
    tree.touch("one.txt");
    tree.touch("two.txt");
    tree.dir("target");
    let mut editor = DirEditor::new(tree.0.clone());

    // The directory sorts first, so mark the two files after it.
    editor.on_key(&press(KeyCode::Char('j')));
    editor.on_key(&press(KeyCode::Char('m')));
    editor.on_key(&press(KeyCode::Char('m')));
    assert_eq!(editor.marks.len(), 2);

    let outcome = editor.on_key(&alt(KeyCode::Char('m')));
    answer(&mut editor, outcome, "target");
    assert!(tree.path("target").join("one.txt").exists());
    assert!(tree.path("target").join("two.txt").exists());
}

#[test]
fn test_an_operation_reports_a_failure_instead_of_pretending() {
    // A name that is not a name must be refused visibly, and must not be
    // reported as a success.
    let tree = TempTree::new("bad-name");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&press(KeyCode::Char('_')));
    answer(&mut editor, outcome, "../escape.txt");
    let message = editor
        .take_notice()
        .map(|notice| notice.text)
        .unwrap_or_default();
    assert!(message.starts_with("failed:"), "got {message:?}");
    assert!(!tree.0.parent().expect("parent").join("escape.txt").exists());
}

#[test]
fn test_an_operation_clears_the_marks_it_consumed() {
    // Marks left behind would silently widen the next operation.
    let tree = TempTree::new("marks-cleared");
    tree.touch("a.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Char('m')));

    let outcome = editor.on_key(&press(KeyCode::Char('C')));
    answer(&mut editor, outcome, "b.txt");
    assert!(editor.marks.is_empty());
}

#[test]
fn test_moving_the_root_clears_the_marks() {
    // A mark that survived into a different listing would be counted by the
    // next operation without ever being visible in it.
    let (tree, nested) = tree_with_nested_children("marks-root");
    let mut editor = DirEditor::new(tree.0.clone());
    // Marking steps the cursor on, so come back to the directory to enter.
    editor.on_key(&press(KeyCode::Char('m')));
    assert_eq!(editor.marks.len(), 1);
    editor.on_key(&press(KeyCode::Char('k')));

    editor.on_key(&press(KeyCode::Enter));
    assert_eq!(editor.root, nested);
    assert!(editor.marks.is_empty());
}

#[test]
fn test_toggling_sizes_asks_for_one_directory_at_a_time() {
    // Asking for every directory at once starts as many walks as there are
    // rows; each answer is what asks for the next.
    let tree = TempTree::new("sizes");
    let first = tree.dir("aaa");
    let second = tree.dir("bbb");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&alt_shift(KeyCode::Char('s')));
    assert_eq!(
        outcome,
        EditorOutcome::Job(JobRequest::DirSize(first.clone()))
    );

    let next = editor.on_job(JobReply::DirSize {
        bytes: 10,
        path: first,
    });
    assert_eq!(
        next,
        EditorOutcome::Job(JobRequest::DirSize(second.clone()))
    );

    let done = editor.on_job(JobReply::DirSize {
        bytes: 20,
        path: second,
    });
    assert_eq!(done, EditorOutcome::Consumed, "nothing left to walk");
}

#[test]
fn test_turning_sizes_off_cancels_the_walks_still_running() {
    let tree = TempTree::new("sizes-off");
    tree.dir("aaa");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&alt_shift(KeyCode::Char('s')));
    assert_eq!(
        editor.on_key(&alt_shift(KeyCode::Char('s'))),
        EditorOutcome::CancelJobs
    );
}

#[test]
fn test_an_answer_arriving_after_sizes_were_turned_off_asks_for_nothing() {
    // The walk was cancelled, but one already in flight still reports back;
    // it must not restart the chain.
    let tree = TempTree::new("sizes-late");
    let dir = tree.dir("aaa");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&alt_shift(KeyCode::Char('s')));
    editor.on_key(&alt_shift(KeyCode::Char('s')));

    let outcome = editor.on_job(JobReply::DirSize {
        bytes: 1,
        path: dir,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
}

#[test]
fn test_a_walked_total_survives_a_reload_but_not_a_new_root() {
    // Re-walking on every keystroke is the expensive mistake; carrying a
    // total into a different directory is the wrong one.
    let (tree, nested) = tree_with_nested_children("sizes-cache");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&alt_shift(KeyCode::Char('s')));
    editor.on_job(JobReply::DirSize {
        bytes: 64,
        path: nested.clone(),
    });
    assert_eq!(editor.sizes.get(&nested).copied(), Some(64));

    editor.on_key(&ctrl(KeyCode::Char('r')));
    assert_eq!(editor.sizes.get(&nested).copied(), Some(64));

    editor.on_key(&press(KeyCode::Backspace));
    assert!(editor.sizes.is_empty(), "a new root re-walks");
}

#[test]
fn test_a_listing_longer_than_the_pane_scrolls_with_the_cursor() {
    // Without a window the rows past the pane's last line are painted and
    // clipped, so the cursor walks off into content nobody can see.
    let tree = TempTree::new("scroll");
    for i in 0..20 {
        tree.touch(&format!("file{i:02}.txt"));
    }
    let mut editor = DirEditor::new(tree.0.clone());
    let pane_rows = 6;

    let visible = editor.content(pane_rows, 80, false).rows.len();
    assert_eq!(visible, pane_rows, "the pane is filled, not overrun");

    for _ in 0..19 {
        editor.on_key(&press(KeyCode::Char('j')));
    }
    let content = editor.content(pane_rows, 80, false);
    assert_eq!(
        content.cursor_line,
        Some(pane_rows - 1),
        "the cursor rides the last visible row"
    );
    assert_eq!(content.rows.len(), pane_rows);

    editor.on_key(&press(KeyCode::Home));
    assert_eq!(
        editor.content(pane_rows, 80, false).cursor_line,
        Some(HEADER_ROWS),
        "and returns to the top when the cursor does"
    );
}

#[test]
fn test_an_empty_directory_has_no_cursor_line() {
    // With no rows there is nothing to band, and a cursor line of zero
    // would highlight the header.
    let tree = TempTree::new("empty");
    let mut editor = DirEditor::new(tree.0.clone());
    assert_eq!(editor.content(20, 80, false).cursor_line, None);
}

#[test]
fn test_a_search_moves_the_cursor_to_the_matching_entry() {
    let tree = TempTree::new("search");
    tree.touch("alpha.txt");
    tree.touch("zeta.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&press(KeyCode::Char('/')));
    answer(&mut editor, outcome, "zet");
    assert_eq!(selected_name(&editor), "zeta.txt");
}

#[test]
fn test_a_search_leaves_the_marks_and_the_listing_alone() {
    // Answering a search down the same path as an operation would clear the
    // marks, losing what the next operation was aimed at.
    let tree = TempTree::new("search-marks");
    tree.touch("alpha.txt");
    tree.touch("zeta.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Char('m')));
    let outcome = editor.on_key(&press(KeyCode::Char('/')));
    answer(&mut editor, outcome, "zeta");
    assert_eq!(editor.marks.len(), 1);
}

#[test]
fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
    let tree = TempTree::new("search-repeat");
    tree.touch("alpha.txt");
    tree.touch("also.txt");
    tree.touch("zeta.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    let outcome = editor.on_key(&press(KeyCode::Char('/')));
    answer(&mut editor, outcome, "al");
    assert_eq!(selected_name(&editor), "also.txt");

    editor.on_key(&press(KeyCode::Char('n')));
    assert_eq!(
        selected_name(&editor),
        "alpha.txt",
        "and wraps rather than stopping at the last match"
    );

    editor.on_key(&press(KeyCode::Char('N')));
    assert_eq!(selected_name(&editor), "also.txt");
}

#[test]
fn test_a_search_with_nothing_to_find_reports_it() {
    let tree = TempTree::new("search-miss");
    tree.touch("alpha.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Char('n')));
    assert_eq!(
        editor.take_notice().map(|notice| notice.text).as_deref(),
        Some("no search")
    );

    let outcome = editor.on_key(&press(KeyCode::Char('/')));
    answer(&mut editor, outcome, "absent");
    assert_eq!(
        editor.take_notice().map(|notice| notice.text).as_deref(),
        Some("not found: absent")
    );
    assert_eq!(selected_name(&editor), "alpha.txt");
}

#[test]
fn test_editing_a_name_and_pressing_enter_renames_the_file() {
    let tree = TempTree::new("edit-apply");
    tree.touch("notes.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('S')));
    for ch in "renamed.md".chars() {
        editor.on_key(&press(KeyCode::Char(ch)));
    }
    assert!(editor.edit.is_some(), "typing alone renames nothing");

    editor.on_key(&press(KeyCode::Enter));
    assert!(editor.edit.is_none());
    assert!(tree.path("renamed.md").exists());
    assert!(!tree.path("notes.txt").exists());
    assert_eq!(selected_name(&editor), "renamed.md");
}

#[test]
fn test_enter_with_nothing_changed_leaves_edit_mode_quietly() {
    let tree = TempTree::new("edit-none");
    tree.touch("same.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);

    editor.on_key(&press(KeyCode::Char('Z')));
    editor.on_key(&press(KeyCode::Char('Z')));
    assert!(editor.edit.is_none());
    assert_eq!(
        editor.take_notice().map(|notice| notice.text).as_deref(),
        Some("no changes")
    );
    assert!(tree.path("same.txt").exists());
}

#[test]
fn test_escape_throws_the_edits_away_without_touching_the_disk() {
    let tree = TempTree::new("edit-discard");
    tree.touch("keep.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('S')));
    editor.on_key(&press(KeyCode::Char('x')));

    // Escape only leaves Insert for Normal; q is what quits the edit.
    editor.on_key(&press(KeyCode::Escape));
    assert!(editor.edit.is_some(), "Escape returns to Normal mode");
    editor.on_key(&press(KeyCode::Char('q')));
    assert!(editor.edit.is_none());
    assert_eq!(
        editor.take_notice().map(|notice| notice.text).as_deref(),
        Some("discarded 1")
    );
    assert!(tree.path("keep.txt").exists());
    assert!(!tree.path("x").exists());
    assert_eq!(selected_name(&editor), "keep.txt");
}

#[test]
fn test_q_also_leaves_edit_mode_without_applying() {
    // `q` types into a name like any other letter: quitting the edit is
    // Escape's job, because a filename may hold a `q`.
    let tree = TempTree::new("edit-q");
    tree.touch("stay.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('S')));

    for ch in "quick.json".chars() {
        editor.on_key(&press(KeyCode::Char(ch)));
    }
    editor.on_key(&press(KeyCode::Enter));
    assert!(tree.path("quick.json").exists(), "every letter typed");
}

#[test]
fn test_a_name_emptied_by_editing_is_refused_at_apply() {
    // An empty name is not a name, and the refusal stays in edit mode so
    // the mistake can be fixed rather than started over.
    let tree = TempTree::new("edit-empty");
    tree.touch("gone.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('S')));

    editor.on_key(&press(KeyCode::Enter));
    assert!(editor.edit.is_some(), "still editing, so it can be fixed");
    let message = editor
        .take_notice()
        .map(|notice| notice.text)
        .unwrap_or_default();
    assert!(message.starts_with("failed:"), "got {message:?}");
    assert!(tree.path("gone.txt").exists());

    for ch in "back.txt".chars() {
        editor.on_key(&press(KeyCode::Char(ch)));
    }
    editor.on_key(&press(KeyCode::Enter));
    assert!(editor.edit.is_none());
    assert!(tree.path("back.txt").exists());
    assert!(!tree.path("gone.txt").exists());
}

#[test]
fn test_edits_apply_to_children_before_the_directory_that_holds_them() {
    // Renaming the directory first would strand the file's pending path:
    // both renames come from the snapshot, so the deeper one must run
    // first, exactly as wdir sorts by path depth before applying.
    let tree = TempTree::new("edit-order");
    let nested = tree.dir("nested");
    fs::write(nested.join("leaf.txt"), "x").expect("leaf");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));

    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('S')));
    for ch in "outer".chars() {
        editor.on_key(&press(KeyCode::Char(ch)));
    }
    editor.on_key(&press(KeyCode::Escape));
    editor.on_key(&press(KeyCode::Char('j')));
    editor.on_key(&press(KeyCode::Char('S')));
    for ch in "sprout.txt".chars() {
        editor.on_key(&press(KeyCode::Char(ch)));
    }
    // Two renames are a batch, which is listed for a yes first.
    let outcome = editor.on_key(&press(KeyCode::Enter));
    assert!(editor.edit.is_some(), "nothing renamed before the yes");
    answer(&mut editor, outcome, "y");

    assert!(tree.path("outer").join("sprout.txt").exists());
    assert!(!tree.path("nested").exists());
}

#[test]
fn test_editing_moves_the_caret_between_names() {
    let tree = TempTree::new("edit-caret");
    tree.touch("a.txt");
    tree.touch("bb.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::End));

    toggle_edit(&mut editor);
    assert_eq!(
        editor.edit.as_ref().expect("editing").col(),
        5,
        "on the last character, where `a` appends"
    );

    editor.on_key(&press(KeyCode::Char('k')));
    assert_eq!(selected_name(&editor), "a.txt");
    assert_eq!(
        editor.edit.as_ref().expect("editing").col(),
        4,
        "clipped to the shorter line's last character"
    );

    editor.on_key(&press(KeyCode::Char('0')));
    assert_eq!(editor.edit.as_ref().expect("editing").col(), 0);
    editor.on_key(&press(KeyCode::Char('l')));
    assert_eq!(editor.edit.as_ref().expect("editing").col(), 1);
    editor.on_key(&press(KeyCode::Char('$')));
    assert_eq!(
        editor.edit.as_ref().expect("editing").col(),
        4,
        "on the last character"
    );
}

#[test]
fn test_the_edited_row_paints_its_caret() {
    let tree = TempTree::new("edit-paint");
    tree.touch("name.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('0')));

    let content = editor.content(10, 80, false);
    let cursor = content.cursor_line.expect("a cursor row");
    let line = crate::model::paint::row_text(&content.rows[cursor]);
    assert!(line.contains('│'), "got {line:?}");
}

#[test]
fn test_applying_edits_clears_marks_pointing_at_old_paths() {
    // A mark aimed at a path the rename has just left behind would count
    // toward an operation aimed at nothing.
    let tree = TempTree::new("edit-marks");
    tree.touch("before.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Char('m')));

    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('S')));
    for ch in "after.txt".chars() {
        editor.on_key(&press(KeyCode::Char(ch)));
    }
    editor.on_key(&press(KeyCode::Enter));
    assert!(editor.marks.is_empty());
}

#[test]
fn test_the_toggle_chord_leaves_the_edit_and_discards() {
    // The same chord that opens the session closes it, from either mode,
    // keeping the disk as it is.
    let tree = TempTree::new("edit-toggle-off");
    tree.touch("keep.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('S')));
    editor.on_key(&press(KeyCode::Char('x')));
    assert!(editor.edit.is_some(), "fixture: an unsaved edit");

    toggle_edit(&mut editor);
    assert!(editor.edit.is_none());
    assert_eq!(
        editor.take_notice().map(|notice| notice.text).as_deref(),
        Some("discarded 1")
    );
    assert!(tree.path("keep.txt").exists());
    assert!(!tree.path("x").exists());
}

#[test]
fn test_a_stray_follow_key_abandons_the_chord_instead_of_leaking() {
    // `C-x` followed by anything but `C-q` must not leave the
    // leader pending, swallowing the next keystroke.
    let tree = TempTree::new("edit-stray-chord");
    tree.touch("a.txt");
    tree.touch("b.txt");
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&ctrl(KeyCode::Char('x')));
    editor.on_key(&press(KeyCode::Char('!')));
    assert!(editor.pending.is_none());
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(selected_name(&editor), "b.txt");
}

#[test]
fn test_escape_leaves_insert_for_normal_and_never_discards() {
    // A vim pair: Escape steps out of Insert; a second Escape is inert in
    // Normal, so mashing it cannot throw the edits away.
    let tree = TempTree::new("edit-escape-pair");
    tree.touch("base.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    editor.on_key(&press(KeyCode::Char('A')));
    editor.on_key(&press(KeyCode::Char('2')));
    assert_eq!(
        editor.edit.as_ref().expect("editing").modal().mode(),
        EditMode::Insert,
        "fixture: typing keeps Insert"
    );

    editor.on_key(&press(KeyCode::Escape));
    editor.on_key(&press(KeyCode::Escape));
    assert_eq!(
        editor.edit.as_ref().expect("editing").modal().mode(),
        EditMode::Normal,
        "the first Escape steps out"
    );
    editor.on_key(&press(KeyCode::Escape));
    assert!(editor.edit.is_some(), "the second is inert");
    assert_eq!(
        editor
            .edit
            .as_ref()
            .and_then(|e| e.lines().first())
            .map(String::as_str),
        Some("base.txt2")
    );
}

#[test]
fn test_a_copy_offers_the_listing_beside_it_and_lands_there() {
    let tree = TempTree::new("copy-beside");
    tree.touch("report.txt");
    let beside = tree.dir("beside");
    let mut editor = DirEditor::new(tree.0.clone());
    while selected_name(&editor) != "report.txt" {
        editor.on_key(&press(KeyCode::Char('j')));
    }
    editor.set_neighbors(vec![beside.clone(), tree.0.clone()]);

    let outcome = editor.on_key(&press(KeyCode::Char('C')));
    let EditorOutcome::Prompt(request) = &outcome else {
        panic!("expected a prompt, got {outcome:?}");
    };
    assert_eq!(
        request.initial,
        format!("{}/", beside.display()),
        "the pane beside, not this one"
    );
    answer(&mut editor, outcome.clone(), &request.initial.clone());
    assert!(
        beside.join("report.txt").exists(),
        "copied into it under its own name"
    );
    assert!(tree.path("report.txt").exists());
}

#[test]
fn test_swapping_two_names_in_the_edit_renames_both() {
    let tree = TempTree::new("edit-swap");
    fs::write(tree.path("left.txt"), "L").expect("write");
    fs::write(tree.path("right.txt"), "R").expect("write");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    for keys in ["Sright.txt", "jSleft.txt"] {
        for ch in keys.chars() {
            editor.on_key(&press(KeyCode::Char(ch)));
        }
        editor.on_key(&press(KeyCode::Escape));
    }
    let outcome = editor.on_key(&press(KeyCode::Char('Z')));
    assert_eq!(
        outcome,
        EditorOutcome::Consumed,
        "fixture: the first half of ZZ"
    );
    let outcome = editor.on_key(&press(KeyCode::Char('Z')));
    answer(&mut editor, outcome, "y");
    assert_eq!(
        fs::read_to_string(tree.path("left.txt")).expect("read"),
        "R"
    );
    assert_eq!(
        fs::read_to_string(tree.path("right.txt")).expect("read"),
        "L"
    );
}

#[test]
fn test_line_commands_never_shift_one_name_onto_another_entry() {
    // The names are lines of text, so a delete or a join that moved the
    // lines below up would rename every later entry to its neighbor's.
    let tree = TempTree::new("edit-fixed-rows");
    tree.touch("a.txt");
    tree.touch("b.txt");
    tree.touch("c.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    for c in "ddJjo".chars() {
        editor.on_key(&press(KeyCode::Char(c)));
    }
    editor.on_key(&press(KeyCode::Escape));
    let names = editor.edit.as_ref().expect("editing").lines().to_vec();
    assert_eq!(names, ["", "b.txt", "c.txt"]);
}

#[test]
fn test_the_status_names_the_mode_the_editor_is_in_and_what_is_marked() {
    let tree = TempTree::new("edit-header-mode");
    tree.touch("name.txt");
    tree.touch("other.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    assert_eq!(
        editor.status().mode,
        Some(EditMode::Normal),
        "a listing defaults to Normal mode"
    );

    editor.on_key(&press(KeyCode::Char('m')));
    assert_eq!(editor.status().segments, ["1 marked"]);

    toggle_edit(&mut editor);
    assert_eq!(editor.status().mode, Some(EditMode::Normal));
    editor.on_key(&press(KeyCode::Char('A')));
    assert_eq!(editor.status().mode, Some(EditMode::Insert));
    editor.on_key(&press(KeyCode::Escape));
    assert_eq!(editor.status().mode, Some(EditMode::Normal));

    // Leaving edit mode discards edits and returns to the listing in Normal mode
    editor.on_key(&press(KeyCode::Char('q')));
    assert_eq!(editor.status().mode, Some(EditMode::Normal));

    // Pressing `i` on the listing directly enters edit mode in Insert mode
    editor.on_key(&press(KeyCode::Char('i')));
    assert_eq!(editor.status().mode, Some(EditMode::Insert));
    editor.on_key(&press(KeyCode::Escape));
    assert_eq!(editor.status().mode, Some(EditMode::Normal));
}

/// A listing whose tree gives the word motions something to step over:
/// an expanded directory with two children, another directory, two files.
fn wordy_tree(tag: &str) -> (TempTree, PathBuf) {
    let tree = TempTree::new(tag);
    let nested = tree.dir("nested");
    fs::write(nested.join("child-a.txt"), "x").expect("child a");
    fs::write(nested.join("child-b.txt"), "x").expect("child b");
    tree.dir("other");
    tree.touch("file1.txt");
    tree.touch("file2.txt");
    (tree, nested)
}

#[test]
fn test_w_steps_over_the_subtree_under_the_cursor() {
    // The expanded subtree is the word the cursor sits on: `w` lands past
    // it, on the next entry at its own depth, not on its first child.
    let (tree, _) = wordy_tree("word-w");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));
    assert_eq!(
        editor.rows.len(),
        6,
        "fixture: nested open with two children"
    );

    editor.on_key(&press(KeyCode::Char('w')));
    assert_eq!(
        selected_name(&editor),
        "other",
        "the children were stepped over"
    );

    editor.on_key(&press(KeyCode::Char('w')));
    assert_eq!(
        selected_name(&editor),
        "file1.txt",
        "a leaf steps to the next entry"
    );
}

#[test]
fn test_b_steps_back_over_the_subtree_above_the_cursor() {
    let (tree, _) = wordy_tree("word-b");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));
    editor.on_key(&press(KeyCode::End));

    editor.on_key(&press(KeyCode::Char('b')));
    assert_eq!(selected_name(&editor), "file1.txt");
    editor.on_key(&press(KeyCode::Char('b')));
    assert_eq!(selected_name(&editor), "other");
    editor.on_key(&press(KeyCode::Char('b')));
    assert_eq!(
        selected_name(&editor),
        "nested",
        "the whole subtree above was stepped over, not descended into"
    );
}

#[test]
fn test_e_lands_at_the_end_of_the_word() {
    let (tree, _) = wordy_tree("word-e");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));

    // On the expanded directory: its subtree's last row is the word's end.
    editor.on_key(&press(KeyCode::Char('e')));
    assert_eq!(selected_name(&editor), "child-b.txt");

    // Already at a leaf's end: the next word's end, which for a collapsed
    // entry is the entry itself.
    editor.on_key(&press(KeyCode::Char('e')));
    assert_eq!(selected_name(&editor), "other");
}

#[test]
fn test_the_braces_jump_between_directories() {
    // Directories sort first and so head every group: they are the
    // paragraphs of a listing.
    let (tree, _) = wordy_tree("word-braces");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));

    editor.on_key(&press(KeyCode::Char('}')));
    assert_eq!(selected_name(&editor), "other");
    editor.on_key(&press(KeyCode::Char('}')));
    assert_eq!(
        selected_name(&editor),
        "other",
        "no directory after the last one"
    );

    editor.on_key(&press(KeyCode::Char('{')));
    assert_eq!(selected_name(&editor), "nested");
    editor.on_key(&press(KeyCode::Char('{')));
    assert_eq!(
        selected_name(&editor),
        "nested",
        "no directory before the first"
    );
}

#[test]
fn test_dollar_and_gg_reach_the_ends() {
    let (tree, _) = wordy_tree("word-ends");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.on_key(&press(KeyCode::Tab));

    editor.on_key(&press(KeyCode::Char('$')));
    assert_eq!(selected_name(&editor), "file2.txt");
    editor.on_key(&press(KeyCode::Char('g')));
    editor.on_key(&press(KeyCode::Char('g')));
    assert_eq!(selected_name(&editor), "nested");
}

#[test]
fn test_the_half_page_motions_move_half_the_pane() {
    let (tree, _) = wordy_tree("word-halfpage");
    let mut editor = DirEditor::new(tree.0.clone());
    for i in 0..20 {
        tree.touch(&format!("pad{i:02}.txt"));
    }
    editor.reload();
    editor.on_key(&press(KeyCode::Tab));
    editor.on_key(&press(KeyCode::Home));

    // The pane holds 10 rows, one of them the header: half of nine is four.
    editor.content(10, 80, false);
    editor.on_key(&ctrl(KeyCode::Char('d')));
    assert_eq!(editor.list.cursor(), 4, "half a viewport down");
    editor.on_key(&ctrl(KeyCode::Char('u')));
    assert_eq!(editor.list.cursor(), 0, "and back up, clamped at the top");
}

#[test]
fn test_editing_an_empty_listing_is_a_no_op() {
    let tree = TempTree::new("edit-empty-listing");
    let mut editor = DirEditor::new(tree.0.clone());
    toggle_edit(&mut editor);
    assert!(editor.edit.is_none());
}

#[test]
fn test_dir_jump_targets_preview_confirm_and_cancel() {
    let tree = TempTree::new("dir-jump");
    tree.touch("apple.txt");
    tree.touch("banana.txt");
    tree.touch("cherry.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    let targets = editor.jump_targets().expect("targets");
    assert_eq!(targets.len(), 3);
    assert_eq!(editor.jump_cursor(), Some(0));
    assert_eq!(targets[0].label, format!("{:>5}  apple.txt", 1));
    assert_eq!(
        targets[0].icon,
        Some(IconKind::File {
            name: "apple.txt".to_string()
        })
    );

    // Preview to row 2
    editor.list.view_mut().set_rows(10);
    editor.jump_to(2);
    assert_eq!(editor.list.cursor(), 2);
    assert_eq!(editor.list.view().scroll(), 2_usize.saturating_sub(10 / 2));

    // Cancel restores to 0 and original scroll
    editor.jump_cancel();
    assert_eq!(editor.list.cursor(), 0);
    assert_eq!(editor.list.view().scroll(), 0);

    // Preview to 1 and confirm
    editor.jump_to(1);
    editor.jump_confirm(1);
    assert_eq!(editor.list.cursor(), 1);
    assert_eq!(editor.list.view().scroll(), 1_usize.saturating_sub(10 / 2));
    assert!(!editor.list.is_jumping());
}

#[test]
fn test_dir_jump_targets_includes_icons_and_depth() {
    let tree = TempTree::new("dir-jump-icons");
    tree.dir("sub");
    tree.touch("sub/child.rs");
    tree.touch("main.rs");
    let mut editor = DirEditor::new(tree.0.clone());
    let sub_idx = editor
        .rows
        .iter()
        .position(|r| r.entry.name == "sub")
        .expect("sub folder");
    editor.list.set_cursor(sub_idx);
    editor.toggle_fold();

    let targets = editor.jump_targets().expect("targets");
    assert_eq!(targets.len(), 3);

    // The folder was just opened, so its icon is the open one, as the
    // listing draws it; a jump list that drew it shut would disagree with
    // the row it jumps to.
    let sub = targets
        .iter()
        .find(|target| target.target == sub_idx)
        .expect("sub is a target");
    assert_eq!(
        sub.icon,
        Some(IconKind::Dir {
            expanded: true,
            name: "sub".to_string()
        })
    );
    for target in &targets {
        let row = &editor.rows[target.target];
        if row.depth > 0 {
            let indent = "  ".repeat(row.depth);
            assert!(
                target
                    .label
                    .contains(&format!("  {indent}{}", row.entry.name)),
                "target label '{}' should indent the name by '{indent}'",
                target.label
            );
        }
    }
}

#[test]
fn test_dir_editor_header_displays_formatted_path() {
    let tree = TempTree::new("dir-header-path");
    let mut editor = DirEditor::new(tree.0.clone());
    let header = crate::model::paint::row_text(&editor.content(10, 80, false).rows[0]);
    let expected = format_full_path(&tree.0);
    assert!(
        header.starts_with(&expected),
        "expected header starting with '{expected}', got {header:?}"
    );
    assert!(
        !header.contains("sort"),
        "header should not contain sort information"
    );
}

#[test]
fn test_dir_editor_reads_entries_even_with_leading_slash_on_windows_path() {
    let tree = TempTree::new("dir-osc7-path");
    tree.touch("file.txt");
    let raw = tree.0.to_string_lossy();
    let uri_style_path = if raw.starts_with('/') {
        tree.0.clone()
    } else {
        PathBuf::from(format!("/{raw}"))
    };
    let editor = DirEditor::new(uri_style_path);
    assert_eq!(editor.rows.len(), 1);
    assert_eq!(editor.rows[0].entry.name, "file.txt");
}

#[test]
fn test_a_delete_runs_as_a_job_and_reports_when_it_lands() {
    // A large tree takes long to delete: the window hands it off rather
    // than doing it on its own thread.
    let tree = TempTree::new("delete-job");
    tree.touch("gone.txt");
    let mut editor = DirEditor::new(tree.0.clone());
    let EditorOutcome::Prompt(request) = editor.on_key(&press(KeyCode::Char('X'))) else {
        panic!("expected the delete question");
    };
    let outcome = editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: request.tag,
    });
    assert!(
        matches!(
            outcome,
            EditorOutcome::Job(JobRequest::FileOps(FileOps::Delete(_)))
        ),
        "{outcome:?}"
    );
    assert!(
        tree.path("gone.txt").exists(),
        "not done on the window's thread"
    );
    settle(&mut editor, outcome);
    assert!(!tree.path("gone.txt").exists());
    assert_eq!(
        editor.take_notice().map(|notice| notice.text).as_deref(),
        Some("deleted 1")
    );
}

fn select(editor: &mut DirEditor, name: &str) {
    let index = editor
        .rows
        .iter()
        .position(|row| row.entry.name == name)
        .unwrap_or_else(|| panic!("no row {name}"));
    editor.list.set_cursor(index);
}

#[test]
fn test_renaming_an_entry_inside_an_open_directory_keeps_it_there() {
    // The listing's root is not where a nested entry lives: renaming
    // `nested/inner.txt` must not move it up beside `nested`.
    let tree = TempTree::new("rename-nested");
    let nested = tree.dir("nested");
    fs::write(nested.join("inner.txt"), "x").expect("inner");
    let mut editor = DirEditor::new(tree.0.clone());
    select(&mut editor, "nested");
    editor.on_key(&press(KeyCode::Tab));
    select(&mut editor, "inner.txt");
    let outcome = editor.on_key(&press(KeyCode::Char('R')));
    answer(&mut editor, outcome, "renamed.txt");
    assert!(nested.join("renamed.txt").exists(), "renamed where it was");
    assert!(!tree.path("renamed.txt").exists(), "not moved to the root");
}

#[cfg(unix)]
#[test]
fn test_enter_on_a_link_to_a_directory_goes_into_it() {
    let tree = TempTree::new("enter-link");
    let real = tree.dir("real");
    fs::write(real.join("inside.txt"), "x").expect("inside");
    std::os::unix::fs::symlink(&real, tree.path("alias")).expect("link");
    let mut editor = DirEditor::new(tree.0.clone());
    select(&mut editor, "alias");
    let outcome = editor.on_key(&press(KeyCode::Enter));
    assert_eq!(outcome, EditorOutcome::Consumed, "not opened as a file");
    assert_eq!(editor.root, tree.path("alias"));
    assert_eq!(editor.rows[0].entry.name, "inside.txt");
}

#[test]
fn test_a_delete_acts_on_what_its_question_named() {
    // The cursor may land elsewhere while the question is open (a re-read
    // moving the rows); the yes is about the entry it named.
    let tree = TempTree::new("delete-asked");
    tree.touch("a");
    tree.touch("b");
    let mut editor = DirEditor::new(tree.0.clone());
    select(&mut editor, "a");
    let EditorOutcome::Prompt(request) = editor.on_key(&press(KeyCode::Char('X'))) else {
        panic!("expected a confirmation");
    };
    select(&mut editor, "b");
    let outcome = editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: request.tag,
    });
    assert_eq!(
        outcome,
        EditorOutcome::Job(JobRequest::FileOps(FileOps::Delete(vec![tree.path("a")])))
    );
}

#[test]
fn test_a_slow_read_leaves_the_listing_up_and_lands_on_a_later_tick() {
    let tree = TempTree::new("slow-read");
    tree.touch("first");
    let mut editor = DirEditor::new(tree.0.clone());
    tree.touch("second");
    // A read that does not finish inside the wait leaves the rows as they
    // were and the window free; the tick takes it once it has finished.
    editor.listing_wait = Duration::ZERO;
    editor.reload();
    let deadline = Instant::now() + Duration::from_secs(10);
    while editor.rows.len() < 2 && Instant::now() < deadline {
        assert!(
            editor.next_tick().is_some(),
            "a tick is asked for while reading"
        );
        std::thread::sleep(Duration::from_millis(5));
        editor.on_tick(Instant::now());
    }
    let names: Vec<&str> = editor
        .rows
        .iter()
        .map(|row| row.entry.name.as_str())
        .collect();
    assert_eq!(names, ["first", "second"]);
    assert!(!editor.is_reading());
}

#[test]
fn test_a_newer_read_replaces_one_still_running() {
    let tree = TempTree::new("newer-read");
    tree.touch("a");
    let mut editor = DirEditor::new(tree.0.clone());
    editor.listing_wait = Duration::ZERO;
    editor.reload();
    editor.listing_wait = Duration::from_secs(10);
    tree.touch("b");
    editor.reload();
    assert!(
        !editor.is_reading(),
        "the older read went with the newer one"
    );
    assert_eq!(editor.rows.len(), 2);
}

#[test]
fn test_the_cursor_stays_on_its_entry_when_one_appears_above_it() {
    let tree = TempTree::new("keep-cursor");
    tree.touch("m");
    tree.touch("z");
    let mut editor = DirEditor::new(tree.0.clone());
    select(&mut editor, "m");
    tree.touch("a");
    editor.on_resume();
    assert_eq!(
        editor.selected().map(|row| row.entry.name.as_str()),
        Some("m")
    );
}

#[test]
fn test_copying_a_reference_takes_the_marks_or_else_the_cursors_directory() {
    let tree = TempTree::new("copy-reference");
    let sub = tree.dir("sub");
    tree.touch("sub/child.rs");
    tree.touch("main.rs");
    let mut editor = DirEditor::new(tree.0.clone());
    let reference = |editor: &DirEditor| editor.file_reference().expect("a reference");
    let at = |editor: &DirEditor, name: &str| {
        editor
            .rows
            .iter()
            .position(|row| row.entry.name == name)
            .expect("listed")
    };

    // Nothing marked: the directory the entry is in, not the entry itself.
    editor.list.set_cursor(at(&editor, "main.rs"));
    assert_eq!(reference(&editor), tree.0.to_string_lossy());

    // Inside an expanded folder, that folder.
    editor.list.set_cursor(at(&editor, "sub"));
    editor.toggle_fold();
    editor.list.set_cursor(at(&editor, "child.rs"));
    assert_eq!(reference(&editor), sub.to_string_lossy());

    // Marked entries: every one of them, in listing order, whatever the
    // cursor is on.
    let main = tree.0.join("main.rs");
    let child = sub.join("child.rs");
    editor.marks.toggle(&main);
    editor.marks.toggle(&child);
    assert_eq!(
        reference(&editor),
        format!("{}\n{}", child.display(), main.display())
    );
}

#[test]
fn test_the_motion_keys_mean_their_vim_motions_in_the_listing() {
    // `G` used to reload and `M` to mark everything, so the bottom and the
    // middle of the screen were the two motions a listing could not reach.
    let tree = TempTree::new("vim-motions");
    for name in ["a.txt", "b.txt", "c.txt"] {
        tree.touch(name);
    }
    let mut editor = DirEditor::new(tree.0.clone());

    editor.on_key(&press(KeyCode::Char('G')));
    assert_eq!(selected_name(&editor), "c.txt", "G is the last entry");
    assert!(editor.marks.is_empty(), "and nothing was marked");

    editor.on_key(&press(KeyCode::Char('t')));
    assert_eq!(editor.marks.len(), 3, "t marks every entry");
}

#[test]
fn test_every_key_of_the_listing_is_spelled_right() {
    assert_eq!(table::invalid_specs(KEYS), Vec::<&str>::new());
    assert_eq!(table::invalid_specs(EDIT_KEYS), Vec::<&str>::new());
}
