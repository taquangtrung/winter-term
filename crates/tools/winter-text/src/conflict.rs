//! The conflicts a merge, a rebase, or a cherry-pick leaves in a file: the
//! `<<<<<<<` block git writes where two sides changed the same lines, each
//! side under its marker, and the common ancestor's version too where the
//! file was merged with `merge.conflictStyle diff3`.
//!
//! A file holding any has keys of its own from then on, bound in its
//! document alone as Viento's host commands: `M-n` and `M-p` step between
//! the conflicts, and `C-c` then a letter keeps what of the one under the
//! cursor the letter names, the way Emacs' smerge offers it (`C-c m` lists
//! every way, and the ways of resolving the whole file). What is kept
//! replaces the whole block, markers and all. The markers are drawn on a
//! band, so each side's own lines keep their colors.

use misty_viento::HighlightStyle;
use viento::{Decoration, DecorationKind, DocumentId, HighlightId, Mode};
use winter_model::editor::{EditorOutcome, Notice, PickPlacement, PickRequest, PromptReply};

use super::session::{self, Session};
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The markers git opens, splits, and closes a conflict with, each seven
/// characters long and followed by the side's name, if any.
const OURS_MARK: &str = "<<<<<<<";
const BASE_MARK: &str = "|||||||";
const SPLIT_MARK: &str = "=======";
const THEIRS_MARK: &str = ">>>>>>>";

/// The owner of the bands the markers are drawn on.
const MARKERS: &str = "conflict-markers";

/// The host commands of a file holding conflicts, and their keys in every
/// mode.
const NEXT: &str = "conflict-next";
const PREVIOUS: &str = "conflict-previous";
const MENU: &str = "conflict-menu";
const KEEP: &str = "conflict-keep-";
const STEP_KEYS: [(&str, &str); 3] = [(NEXT, "M-n"), (PREVIOUS, "M-p"), (MENU, "C-c m")];
const KEY_MODES: [Mode; 3] = [Mode::Insert, Mode::Normal, Mode::Visual];

/// Every way of resolving a conflict, as the menu offers them, each with
/// the letter `C-c` takes before it.
const RESOLUTIONS: [(Resolution, char); 6] = [
    (Resolution::Ours, 'o'),
    (Resolution::Theirs, 't'),
    (Resolution::Both, 'b'),
    (Resolution::BothReversed, 'r'),
    (Resolution::Base, 'a'),
    (Resolution::Neither, 'd'),
];

/// The menu's question, and its choices of resolving every conflict.
const ASK_CONFLICT: &str = "text-conflict";
const ALL_OURS: &str = "Keep ours in every conflict";
const ALL_THEIRS: &str = "Keep theirs in every conflict";
const FILTER_HINT: &str = "Filter resolutions";

/// What is said when there is no conflict to act on.
const NO_CONFLICTS: &str = "no merge conflicts here";
const NOT_IN_CONFLICT: &str = "the cursor is not in a merge conflict";
const NO_ANCESTOR: &str = "this conflict carries no ancestor (merge with conflictStyle diff3)";

// ========================================================================
// Data Structures
// ========================================================================

/// The conflicts found in a document, and whether its keys are bound.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Conflicts {
    /// The conflicts, in order.
    found: Vec<Conflict>,
    /// Whether the document's conflict keys are bound, which they stay.
    keys_bound: bool,
    /// The text's version they were found in.
    version: Option<u64>,
}

/// One conflict, as the lines of its markers: where it opens, where the
/// ancestor's version starts if it has one, where the two sides split, and
/// where it closes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Conflict {
    base: Option<usize>,
    end: usize,
    split: usize,
    start: usize,
}

/// What of a conflict to keep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Resolution {
    /// The ancestor's version, which only a diff3 conflict carries.
    Base,
    /// Ours, then theirs.
    Both,
    /// Theirs, then ours.
    BothReversed,
    /// None of it: the block goes.
    Neither,
    /// Our side, the one checked out (`HEAD`).
    Ours,
    /// Their side, the one coming in.
    Theirs,
}

// ========================================================================
// Conflict Implementations
// ========================================================================

impl Conflict {
    /// Whether `line` is one of the conflict's own lines, markers included.
    fn holds(&self, line: usize) -> bool {
        (self.start..=self.end).contains(&line)
    }

    /// The conflict's marker lines.
    fn markers(&self) -> impl Iterator<Item = usize> {
        [
            Some(self.start),
            self.base,
            Some(self.split),
            Some(self.end),
        ]
        .into_iter()
        .flatten()
    }

    /// The lines `resolution` keeps of `lines`, the file the conflict is
    /// in, or `None` for the ancestor's version of a conflict that has none.
    fn kept(&self, lines: &[String], resolution: Resolution) -> Option<Vec<String>> {
        let ours = &lines[self.start + 1..self.base.unwrap_or(self.split)];
        let theirs = &lines[self.split + 1..self.end];
        Some(match resolution {
            Resolution::Base => lines[self.base? + 1..self.split].to_vec(),
            Resolution::Both => [ours, theirs].concat(),
            Resolution::BothReversed => [theirs, ours].concat(),
            Resolution::Neither => Vec::new(),
            Resolution::Ours => ours.to_vec(),
            Resolution::Theirs => theirs.to_vec(),
        })
    }
}

// ========================================================================
// Resolution Implementations
// ========================================================================

impl Resolution {
    /// What the menu calls it.
    fn label(self) -> &'static str {
        match self {
            Self::Base => "Keep the ancestor's version",
            Self::Both => "Keep both, ours first",
            Self::BothReversed => "Keep both, theirs first",
            Self::Neither => "Keep neither: delete the conflict",
            Self::Ours => "Keep ours (the checked-out side)",
            Self::Theirs => "Keep theirs (the incoming side)",
        }
    }

    /// The host command keeping it.
    fn command(self) -> String {
        let name = match self {
            Self::Base => "base",
            Self::Both => "both",
            Self::BothReversed => "both-reversed",
            Self::Neither => "neither",
            Self::Ours => "ours",
            Self::Theirs => "theirs",
        };
        format!("{KEEP}{name}")
    }
}

// ========================================================================
// Session: conflicts
// ========================================================================

impl Session {
    /// Finds the conflicts in `document` once its text changed, draws their
    /// markers on a band of the style numbered `band`, and binds the
    /// conflict keys in it the first time it holds any.
    pub(crate) fn lay_conflicts(&mut self, document: DocumentId, band: usize) {
        let version = self.editor.document_version(document);
        let Some(file) = self.files.get(&document) else {
            return;
        };
        if file.conflicts.version == version {
            return;
        }
        let found = find(&self.lines(document));
        let bind = !found.is_empty() && !file.conflicts.keys_bound;
        if bind {
            bind_keys(self, document);
        }
        let bands = match u16::try_from(band) {
            Ok(id) => found
                .iter()
                .flat_map(Conflict::markers)
                .map(|line| {
                    let start = self.offset_of(document, line, 0);
                    let end = self.offset_of(document, line, usize::MAX);
                    Decoration::new(start..end, DecorationKind::Background(HighlightId::new(id)))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        self.editor.set_decorations(document, MARKERS, bands);
        if let Some(file) = self.files.get_mut(&document) {
            file.conflicts.keys_bound |= bind;
            file.conflicts.found = found;
            file.conflicts.version = version;
        }
    }
}

// ========================================================================
// TextEditor: conflicts
// ========================================================================

impl TextEditor {
    /// Runs the conflict command `id`: what the window is to do, or `None`
    /// for a command of another's.
    pub(super) fn run_conflict_command(&mut self, id: &str) -> Option<EditorOutcome> {
        Some(match id {
            NEXT => self.step_conflict(true),
            PREVIOUS => self.step_conflict(false),
            MENU => self.conflict_menu(),
            id => {
                let (resolution, _) = RESOLUTIONS
                    .iter()
                    .find(|(resolution, _)| resolution.command() == id)?;
                self.resolve_conflict(*resolution)
            }
        })
    }

    /// The choice the conflict menu came back with, carried out, where
    /// `reply` answers it.
    pub(super) fn on_conflict_prompt(&mut self, reply: &PromptReply) -> Option<EditorOutcome> {
        if reply.tag != ASK_CONFLICT {
            return None;
        }
        Some(match reply.answer.as_deref() {
            Some(ALL_OURS) => self.resolve_every_conflict(Resolution::Ours),
            Some(ALL_THEIRS) => self.resolve_every_conflict(Resolution::Theirs),
            Some(label) => match RESOLUTIONS.iter().find(|(r, _)| r.label() == label) {
                Some((resolution, _)) => self.resolve_conflict(*resolution),
                None => EditorOutcome::Consumed,
            },
            None => EditorOutcome::Consumed,
        })
    }

    /// Moves to the start of the next conflict, or the one before, round
    /// the file as Vim's searches go, saying which of how many it is.
    fn step_conflict(&mut self, forward: bool) -> EditorOutcome {
        let conflicts = find(&self.lines());
        let line = self.cursor_line_column().0;
        let found = match forward {
            true => conflicts.iter().find(|conflict| conflict.start > line),
            false => conflicts.iter().rev().find(|conflict| conflict.end < line),
        };
        let wrapped = match forward {
            true => conflicts.first(),
            false => conflicts.last(),
        };
        let Some(found) = found.or(wrapped).copied() else {
            self.notice = Some(Notice::info(NO_CONFLICTS));
            return EditorOutcome::Consumed;
        };
        self.leave_to_normal();
        self.move_to(found.start, 0);
        let index = conflicts.iter().position(|c| *c == found).unwrap_or(0);
        let said = format!("conflict {} of {}", index + 1, conflicts.len());
        self.notice = Some(Notice::info(said).unlogged());
        EditorOutcome::Consumed
    }

    /// The ways of resolving the conflict under the cursor, as a list to
    /// choose from, then the ways of resolving every conflict at once.
    fn conflict_menu(&mut self) -> EditorOutcome {
        let conflicts = find(&self.lines());
        if conflicts.is_empty() {
            self.notice = Some(Notice::info(NO_CONFLICTS));
            return EditorOutcome::Consumed;
        }
        let line = self.cursor_line_column().0;
        let here = conflicts.iter().find(|conflict| conflict.holds(line));
        let mut items: Vec<String> = RESOLUTIONS
            .iter()
            .filter(|(resolution, _)| {
                here.is_some_and(|c| *resolution != Resolution::Base || c.base.is_some())
            })
            .map(|(resolution, _)| resolution.label().to_string())
            .collect();
        items.extend([ALL_OURS.to_string(), ALL_THEIRS.to_string()]);
        EditorOutcome::Pick(PickRequest {
            icons: Vec::new(),
            items,
            label: format!("Merge conflicts: {} in this file", conflicts.len()),
            placeholder: FILTER_HINT.to_string(),
            placement: PickPlacement::Centered,
            tag: ASK_CONFLICT,
        })
    }

    /// Keeps `resolution` of the conflict under the cursor.
    fn resolve_conflict(&mut self, resolution: Resolution) -> EditorOutcome {
        let lines = self.lines();
        let conflicts = find(&lines);
        let line = self.cursor_line_column().0;
        let Some(here) = conflicts.iter().find(|conflict| conflict.holds(line)) else {
            self.notice = Some(Notice::error(NOT_IN_CONFLICT));
            return EditorOutcome::Consumed;
        };
        let Some(kept) = here.kept(&lines, resolution) else {
            self.notice = Some(Notice::error(NO_ANCESTOR));
            return EditorOutcome::Consumed;
        };
        let start = here.start;
        self.replace_whole_lines(start, here.end + 1 - start, &kept);
        self.leave_to_normal();
        self.move_to(start, 0);
        self.say_conflicts_left(conflicts.len() - 1);
        EditorOutcome::Consumed
    }

    /// Keeps `resolution` of every conflict in the file, as one change one
    /// undo takes back.
    fn resolve_every_conflict(&mut self, resolution: Resolution) -> EditorOutcome {
        let lines = self.lines();
        let conflicts = find(&lines);
        let mut kept = Vec::with_capacity(lines.len());
        let mut from = 0;
        for found in &conflicts {
            kept.extend_from_slice(&lines[from..found.start]);
            kept.extend(found.kept(&lines, resolution).unwrap_or_default());
            from = found.end + 1;
        }
        kept.extend_from_slice(&lines[from..]);
        self.replace_lines(0, usize::MAX, &kept.join("\n"));
        self.leave_to_normal();
        self.say_conflicts_left(0);
        EditorOutcome::Consumed
    }

    /// Replaces `count` lines from `first` with `kept`, which may be none.
    fn replace_whole_lines(&mut self, first: usize, count: usize, kept: &[String]) {
        if !kept.is_empty() {
            self.replace_lines(first, count, &kept.join("\n"));
            return;
        }
        // Nothing kept: the lines go with their breaks, the one before
        // them going instead where they end the file.
        let view = self.view_id();
        session::with(|session| {
            let Some(text) = session.editor.document(self.document) else {
                return;
            };
            let (total, length) = (text.len_lines(), text.len_chars());
            let start = session.offset_of(self.document, first, 0);
            let range = match first + count < total {
                true => start..session.offset_of(self.document, first + count, 0),
                false => start.saturating_sub(1)..length,
            };
            session
                .editor
                .apply_edits(view, vec![viento::Edit::new(range, "")]);
        });
    }

    /// Says how many conflicts are left, or that none are.
    fn say_conflicts_left(&mut self, left: usize) {
        let said = match left {
            0 => "every conflict resolved".to_string(),
            1 => "1 conflict left".to_string(),
            n => format!("{n} conflicts left"),
        };
        self.notice = Some(Notice::info(said).unlogged());
    }

    /// Leaves Insert or Visual mode for Normal, as resolving lands there.
    fn leave_to_normal(&mut self) {
        let view = self.view_id();
        session::with(|session| {
            let mode = session.editor.view(view).map(viento::View::mode);
            let command = match mode {
                Some(Mode::Insert) => "exit-insert",
                Some(Mode::Visual | Mode::VisualBlock | Mode::VisualLine) => "exit-visual",
                _ => return,
            };
            let _ = session.editor.run_command(view, command, None);
        });
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Binds the conflict keys in `document` alone.
fn bind_keys(session: &mut Session, document: DocumentId) {
    let mut keys: Vec<(String, String)> = STEP_KEYS
        .iter()
        .map(|(id, key)| (id.to_string(), key.to_string()))
        .collect();
    keys.extend(
        RESOLUTIONS
            .iter()
            .map(|(resolution, letter)| (resolution.command(), format!("C-c {letter}"))),
    );
    for (id, key) in &keys {
        for mode in KEY_MODES {
            let bound = session
                .editor
                .bind_in_document(document, mode, &[key.as_str()], id);
            debug_assert!(bound.is_ok(), "{id} takes {key}: {bound:?}");
        }
    }
}

/// Every conflict command's id, registered once with the editor.
pub(crate) fn host_commands() -> Vec<String> {
    let steps = STEP_KEYS.iter().map(|(id, _)| id.to_string());
    steps
        .chain(
            RESOLUTIONS
                .iter()
                .map(|(resolution, _)| resolution.command()),
        )
        .collect()
}

/// The style the markers' band is drawn in.
pub(crate) fn band_style(background: misty::color::Rgb) -> HighlightStyle {
    HighlightStyle {
        background: Some(background),
        ..HighlightStyle::default()
    }
}

/// Every whole conflict in `lines`, in order. A block git did not finish
/// writing (no split, or no close) is not a conflict, so its markers are
/// left as text.
fn find(lines: &[String]) -> Vec<Conflict> {
    let mut found = Vec::new();
    // Nearly every file has none, which one look for the opening marker
    // settles.
    if !lines.iter().any(|line| line.starts_with(OURS_MARK)) {
        return found;
    }
    let mut open: Option<(usize, Option<usize>, Option<usize>)> = None;
    for (line, text) in lines.iter().enumerate() {
        match open {
            None if is_mark(text, OURS_MARK) => open = Some((line, None, None)),
            None => {}
            Some((start, None, None)) if is_mark(text, BASE_MARK) => {
                open = Some((start, Some(line), None));
            }
            Some((start, base, None)) if text == SPLIT_MARK => {
                open = Some((start, base, Some(line)));
            }
            Some((start, base, Some(split))) if is_mark(text, THEIRS_MARK) => {
                found.push(Conflict {
                    base,
                    end: line,
                    split,
                    start,
                });
                open = None;
            }
            // A second opening marker before the first closed starts over
            // from it, as git never nests them.
            Some(_) if is_mark(text, OURS_MARK) => open = Some((line, None, None)),
            Some(_) => {}
        }
    }
    found
}

/// Whether `line` is the marker `mark`, alone or followed by a name.
fn is_mark(line: &str, mark: &str) -> bool {
    line.strip_prefix(mark)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::editor::Page;
    use winter_model::input::{Key, KeyCode};

    use super::*;

    /// A file with a plain conflict and a diff3 one.
    const MERGED: &str = "keep\n<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> topic\nmiddle\n\
                          <<<<<<< HEAD\nmine 2\n||||||| base\nold\n=======\nyours 2\n>>>>>>> topic\nend";

    /// `code` pressed, with Ctrl or Alt as asked.
    fn press(code: KeyCode, ctrl: bool, alt: bool) -> Key {
        Key {
            alt,
            code,
            ctrl,
            shift: false,
            sup: false,
        }
    }

    /// The lines of `text`.
    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn test_whole_conflicts_are_found_and_an_unfinished_block_is_not() {
        let found = find(&lines(MERGED));
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[0].split, found[0].end), (1, 3, 5));
        assert_eq!(found[1].base, Some(9), "diff3's ancestor");
        assert!(find(&lines("<<<<<<< HEAD\nmine\n=======\nyours")).is_empty());
        let restarted = find(&lines("<<<<<<< a\n<<<<<<< b\nx\n=======\ny\n>>>>>>> c"));
        assert_eq!(restarted[0].start, 1, "a second opening marker starts over");
        let ancestor = &found[1];
        assert_eq!(
            ancestor.kept(&lines(MERGED), Resolution::Base),
            Some(lines("old"))
        );
        assert_eq!(found[0].kept(&lines(MERGED), Resolution::Base), None);
    }

    #[test]
    fn test_conflicts_are_stepped_through_banded_and_resolved_by_their_keys() {
        let path = std::env::temp_dir().join(format!("winter-conflict-{}.txt", std::process::id()));
        std::fs::write(&path, MERGED).expect("written");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        session::with(|session| {
            session.lay_conflicts(editor.document, 7);
            let bands = session.editor.decorations(editor.document, MARKERS);
            assert_eq!(bands.len(), 7, "every marker line, the ancestor's too");
            assert_eq!(
                bands[0].kind,
                DecorationKind::Background(HighlightId::new(7))
            );
        });
        editor.on_key(&press(KeyCode::Char('n'), false, true));
        assert_eq!(editor.cursor_line_column().0, 1, "on the first conflict");
        editor.on_key(&press(KeyCode::Char('c'), true, false));
        editor.on_key(&press(KeyCode::Char('t'), false, false));
        assert_eq!(editor.lines()[..3], ["keep", "yours", "middle"]);
        let said = editor.take_notice().map(|notice| notice.text.clone());
        assert_eq!(said.as_deref(), Some("1 conflict left"));
        editor.on_key(&press(KeyCode::Char('c'), true, false));
        let menu = editor.on_key(&press(KeyCode::Char('m'), false, false));
        let EditorOutcome::Pick(pick) = menu else {
            panic!("a list of ways: {menu:?}");
        };
        assert_eq!(
            pick.items.len(),
            2,
            "the cursor in none: only the whole file's"
        );
        let reply = PromptReply {
            answer: Some(ALL_OURS.to_string()),
            tag: ASK_CONFLICT,
        };
        editor.on_prompt(reply);
        assert_eq!(editor.lines(), ["keep", "yours", "middle", "mine 2", "end"]);
        let _ = std::fs::remove_file(path);
    }
}
