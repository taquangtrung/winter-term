//! The PDF viewer's keys: what each one is bound to in each mode, and
//! carrying out what a key, a count, or the menu asks.

use std::collections::HashMap;

use crate::model::editor::{Editor, EditorOutcome, Notice, PromptMode, PromptRequest};
use crate::model::input::table::{Binding, TableMatch};
use crate::model::input::{format_key, table, Key, KeyCode, Mods};
use crate::model::textedit::modal::EditMode;
use crate::model::textedit::search::Search;

use super::caret::{Caret, Effect, FindKind, Motion, Operator, Pos, Visual};
use super::document::Document;
use super::outline_list::OutlineList;
use super::{
    find_key, layout, place_offset, Act, CaretFind, CaretMotion, CaretRepeat, CaretSearch,
    CaretSearchStep, Command, HintUse, Mode, PagePlace, PdfEditor, Pending, Prefix, Spelling,
    Thumbnails, Zoom, ASK_SEARCH, ASK_SEARCH_BACK, COLUMN_PX, FULL_SCREEN, HALF_SCREEN, LINE_PX,
    MAX_COUNT, MAX_SCALE, MIN_SCALE, ZOOM_STEP,
};

// ========================================================================
// Constants
// ========================================================================

/// The keys that mean the same whichever way the document is being read.
/// Searching is Winter's one convention that holds whichever mode the
/// document is read in, so these are answered before the mode says what a
/// key means. A search finds text, and text is where the cursor lives, so
/// the surface puts one there.
pub(super) const COMMON_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Jump(true),
        keys: &["C-o", "M-<left>", "M-S-b"],
        label: "Back to where the last jump left",
    },
    Binding {
        action: Act::Jump(false),
        keys: &["C-i", "M-<right>", "M-S-f"],
        label: "Forward again, after going back",
    },
    Binding {
        action: Act::ShowLinks,
        keys: &["M-S-o"],
        label: "Outline every link, or only the one under the pointer",
    },
    Binding {
        action: Act::Sentences,
        keys: &["M-S-s"],
        label: "Tint the sentences each their own color, or not",
    },
    Binding {
        action: Act::Spelling,
        keys: &["M-S-i"],
        label: "Underline the words spelled wrong, or stop",
    },
    Binding {
        action: Act::Misspelling(true),
        keys: &["C-M-n"],
        label: "Next word spelled wrong, and how to spell it",
    },
    Binding {
        action: Act::Misspelling(false),
        keys: &["C-M-p"],
        label: "Previous word spelled wrong",
    },
    Binding {
        action: Act::First,
        keys: &["g g"],
        label: "First page, or the page counted to",
    },
    Binding {
        action: Act::Place(PagePlace::Top),
        keys: &["z t"],
        label: "This page to the top",
    },
    Binding {
        action: Act::Place(PagePlace::Center),
        keys: &["z z"],
        label: "This page to the middle",
    },
    Binding {
        action: Act::Place(PagePlace::Bottom),
        keys: &["z b"],
        label: "This page to the bottom",
    },
    Binding {
        action: Act::AddWord,
        keys: &["z g"],
        label: "Take the word stepped to as spelled right from now on",
    },
    Binding {
        action: Act::Search(true),
        keys: &["/"],
        label: "Search down the document",
    },
    Binding {
        action: Act::Search(false),
        keys: &["?"],
        label: "Search back up the document",
    },
    Binding {
        action: Act::SearchStep(false),
        keys: &["n"],
        label: "Next match",
    },
    Binding {
        action: Act::SearchStep(true),
        keys: &["N"],
        label: "Previous match",
    },
    Binding {
        action: Act::Screens(-FULL_SCREEN),
        keys: &["C-b"],
        label: "A screen up",
    },
    Binding {
        action: Act::Screens(FULL_SCREEN),
        keys: &["C-f"],
        label: "A screen down",
    },
    Binding {
        action: Act::Screens(HALF_SCREEN),
        keys: &["C-d"],
        label: "Half a screen down",
    },
    Binding {
        action: Act::Screens(-HALF_SCREEN),
        keys: &["C-u"],
        label: "Half a screen up",
    },
    Binding {
        action: Act::Lines(1),
        keys: &["C-e"],
        label: "A line down",
    },
    Binding {
        action: Act::Lines(-1),
        keys: &["C-y"],
        label: "A line up",
    },
    Binding {
        action: Act::Position,
        keys: &["C-g"],
        label: "Say where in the document you are",
    },
    Binding {
        action: Act::Count,
        keys: &["g C-g"],
        label: "Count the words selected, or the whole document's",
    },
];

/// The keys with no cursor in the text, ahead of [`WINDOW_KEYS`].
pub(super) const NORMAL_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Cursor,
        keys: &["i"],
        label: "Put a cursor in the text",
    },
    Binding {
        action: Act::Hints(HintUse::Follow),
        keys: &["f"],
        label: "Label the links in view, to follow one by its label",
    },
    Binding {
        action: Act::Hints(HintUse::Preview),
        keys: &["F"],
        label: "Label the links in view, to preview where one leads",
    },
];

/// The keys that move the window rather than a cursor: reached with no
/// cursor, and with one for the keys [`CURSOR_KEYS`] leaves alone, so a
/// document can still be paged through with a cursor in it.
pub(super) const WINDOW_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::SyncBack,
        keys: &["S"],
        label: "Go to the LaTeX source of the cursor's spot (SyncTeX)",
    },
    Binding {
        action: Act::Hints(HintUse::Sync),
        keys: &["s"],
        label: "Label the lines in view, to go to one's LaTeX source",
    },
    Binding {
        action: Act::Lines(1),
        keys: &["j", "<down>"],
        label: "Scroll down",
    },
    Binding {
        action: Act::Lines(-1),
        keys: &["k", "<up>"],
        label: "Scroll up",
    },
    // Sideways, for a page zoomed in past the pane's width. Vim has no
    // vertical-only rule here and neither does the reader.
    Binding {
        action: Act::Columns(-1),
        keys: &["h", "H", "<left>"],
        label: "Scroll left",
    },
    Binding {
        action: Act::Columns(1),
        keys: &["l", "L", "<right>"],
        label: "Scroll right",
    },
    // The surface relays every key home rather than acting on it, so the
    // scrolling a web engine would have done itself is the page's to do:
    // without these, the keys a reader reaches for first would do nothing.
    Binding {
        action: Act::Screens(FULL_SCREEN),
        keys: &["SPC", "<next>"],
        label: "A screen down",
    },
    Binding {
        action: Act::Screens(-FULL_SCREEN),
        keys: &["b", "<prior>"],
        label: "A screen up",
    },
    Binding {
        action: Act::Last,
        keys: &["G", "<end>"],
        label: "Last page, or the page counted to",
    },
    Binding {
        action: Act::Home,
        keys: &["<home>"],
        label: "First page",
    },
    Binding {
        action: Act::Pages(1),
        keys: &["]"],
        label: "Next page",
    },
    Binding {
        action: Act::Pages(-1),
        keys: &["["],
        label: "Previous page",
    },
    Binding {
        action: Act::Zoom(1),
        keys: &["+", "S-="],
        label: "Zoom in",
    },
    Binding {
        action: Act::Zoom(-1),
        keys: &["-"],
        label: "Zoom out",
    },
    Binding {
        action: Act::FitWidth,
        keys: &["="],
        label: "Fit the width",
    },
    Binding {
        action: Act::FitPage,
        keys: &["P"],
        label: "Fit the whole page",
    },
    Binding {
        action: Act::FitContent,
        keys: &["T"],
        label: "Fit the text's width, the margins off the sides",
    },
    Binding {
        action: Act::Crop,
        keys: &["C"],
        label: "Crop the pages to their text, or show them whole",
    },
    Binding {
        action: Act::Center,
        keys: &["E"],
        label: "Center the pages across the pane",
    },
    Binding {
        action: Act::Thumbnails,
        keys: &["V"],
        label: "Show the pages small down the left, or hide them",
    },
    Binding {
        action: Act::AskPage,
        keys: &[":"],
        label: "Go to a page",
    },
    Binding {
        action: Act::Outline,
        keys: &["O"],
        label: "Open or close the outline",
    },
    Binding {
        action: Act::Invert,
        keys: &["I"],
        label: "Invert the pages' colors",
    },
    // Whatever was half-typed, abandoned, the way Escape does everywhere
    // else. Consumed even when nothing was pending, so Escape in a document
    // never falls through to the pane.
    Binding {
        action: Act::Abandon,
        keys: &["ESC"],
        label: "Abandon what is half-typed",
    },
];

/// The keys with a cursor in the text: vim's motions, meaning here what they
/// mean there. The three that cannot are the ones with no character to step
/// over: `h` and `l` still move the cursor, but `zt`, `zz`, and `zb` move the
/// window, and the Ctrl scrolls leave the cursor where it is.
pub(super) const CURSOR_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::FollowAtCaret,
        keys: &["RET"],
        label: "Follow the link under the cursor",
    },
    Binding {
        action: Act::PreviewAtCaret,
        keys: &["K"],
        label: "Preview where the link under the cursor leads",
    },
    Binding {
        action: Act::Caret(Motion::Left),
        keys: &["h", "<left>"],
        label: "Cursor left",
    },
    Binding {
        action: Act::Caret(Motion::Right),
        keys: &["l", "<right>"],
        label: "Cursor right",
    },
    Binding {
        action: Act::Caret(Motion::Down),
        keys: &["j", "<down>"],
        label: "Cursor down",
    },
    Binding {
        action: Act::Caret(Motion::Up),
        keys: &["k", "<up>"],
        label: "Cursor up",
    },
    Binding {
        action: Act::Caret(Motion::WordForward),
        keys: &["w"],
        label: "Next word",
    },
    Binding {
        action: Act::Caret(Motion::WordBackward),
        keys: &["b"],
        label: "Previous word",
    },
    Binding {
        action: Act::Caret(Motion::WordEnd),
        keys: &["e"],
        label: "End of the word",
    },
    Binding {
        action: Act::Caret(Motion::LineStart),
        keys: &["0", "<home>"],
        label: "Start of the line",
    },
    Binding {
        action: Act::Caret(Motion::LineFirstNonBlank),
        keys: &["^"],
        label: "First non-blank of the line",
    },
    Binding {
        action: Act::Caret(Motion::LineEnd),
        keys: &["$", "<end>"],
        label: "End of the line",
    },
    Binding {
        action: Act::Caret(Motion::ParagraphBackward),
        keys: &["{"],
        label: "Previous paragraph",
    },
    Binding {
        action: Act::Caret(Motion::ParagraphForward),
        keys: &["}"],
        label: "Next paragraph",
    },
    Binding {
        action: Act::Caret(Motion::SentenceBackward),
        keys: &["("],
        label: "Previous sentence",
    },
    Binding {
        action: Act::Caret(Motion::SentenceForward),
        keys: &[")"],
        label: "Next sentence",
    },
    // Uppercase, as vim has them: the line nearest an edge of what is on
    // screen, without scrolling to get there.
    Binding {
        action: Act::Caret(Motion::ViewTop),
        keys: &["H"],
        label: "Line nearest the top of the pane",
    },
    Binding {
        action: Act::Caret(Motion::ViewMiddle),
        keys: &["M"],
        label: "Line nearest the middle of the pane",
    },
    Binding {
        action: Act::Caret(Motion::ViewBottom),
        keys: &["L"],
        label: "Line nearest the bottom of the pane",
    },
    Binding {
        action: Act::CaretEnd,
        keys: &["G"],
        label: "End of the document, or the page counted to",
    },
    Binding {
        action: Act::Find(FindKind::Forward),
        keys: &["f"],
        label: "Find a character forward",
    },
    Binding {
        action: Act::Find(FindKind::Backward),
        keys: &["F"],
        label: "Find a character backward",
    },
    Binding {
        action: Act::Find(FindKind::ForwardTill),
        keys: &["t"],
        label: "Till a character forward",
    },
    Binding {
        action: Act::Find(FindKind::BackwardTill),
        keys: &["T"],
        label: "Till a character backward",
    },
    Binding {
        action: Act::RepeatFind(false),
        keys: &[";"],
        label: "Repeat the find",
    },
    Binding {
        action: Act::RepeatFind(true),
        keys: &[","],
        label: "Repeat the find the other way",
    },
    Binding {
        action: Act::Visual(Visual::Char),
        keys: &["v"],
        label: "Select characters",
    },
    Binding {
        action: Act::Visual(Visual::Line),
        keys: &["V"],
        label: "Select lines",
    },
    Binding {
        action: Act::SwapEnds,
        keys: &["o"],
        label: "Swap the selection's ends",
    },
    // `y` on its own is the operator; in visual mode it takes the selection
    // there and then, and `yy` takes whole lines.
    Binding {
        action: Act::Yank,
        keys: &["y"],
        label: "Copy: the selection, `yy` lines, or with a motion",
    },
    Binding {
        action: Act::YankLines,
        keys: &["Y"],
        label: "Copy lines",
    },
    Binding {
        action: Act::Leave,
        keys: &["ESC"],
        label: "Step back: what is half-typed, the selection, the cursor",
    },
];

// ========================================================================
// PdfEditor
// ========================================================================

impl PdfEditor {
    /// Carry out a resolved command, clearing whatever was being typed: a
    /// count belongs to the motion it preceded and to nothing after it.
    pub(super) fn act(&mut self, command: Command) -> EditorOutcome {
        self.pending = Pending::default();
        self.execute(command)
    }

    /// The count typed before the pending motion, or one when none was.
    pub(super) fn count(&self) -> usize {
        self.pending.count.unwrap_or(1).min(MAX_COUNT)
    }

    /// The count as a signed repeat in `direction`.
    pub(super) fn steps(&self, direction: isize) -> isize {
        (self.count() as isize).saturating_mul(direction)
    }

    /// Take `digit` as part of the count being typed, or say it is not one:
    /// a leading `0` is a motion in vim, not a count.
    pub(super) fn take_count_digit(&mut self, digit: char) -> bool {
        let Some(value) = digit.to_digit(10).map(|value| value as usize) else {
            return false;
        };
        if value == 0 && self.pending.count.is_none() {
            return false;
        }
        let so_far = self.pending.count.unwrap_or(0);
        self.pending.count = Some(
            so_far
                .saturating_mul(10)
                .saturating_add(value)
                .min(MAX_COUNT),
        );
        true
    }

    /// How the document is being read: with a cursor in it or without.
    pub(super) fn mode(&self) -> Mode {
        match self.caret.shown() {
            Some(_) => Mode::Cursor,
            None => Mode::Normal,
        }
    }

    /// The mode the status bar shows: none while the view only scrolls.
    pub(super) fn edit_mode(&self) -> Option<EditMode> {
        match (self.mode(), self.caret.visual) {
            (Mode::Normal, _) => None,
            (Mode::Cursor, Visual::None) => Some(EditMode::Normal),
            (Mode::Cursor, Visual::Char | Visual::Line) => Some(EditMode::Visual),
        }
    }

    /// The keys typed so far towards a command that is not finished.
    pub(super) fn typed(&self) -> String {
        let count = match self.pending.count {
            Some(count) => count.to_string(),
            None => String::new(),
        };
        let operator = match self.pending.operator {
            Operator::Move => "",
            Operator::Yank => "y",
        };
        let prefix = match self.pending.prefix {
            Prefix::Find(kind) => find_key(kind).to_string(),
            Prefix::Keys(first) => format_key(&first),
            Prefix::None => String::new(),
        };
        format!("{count}{operator}{prefix}")
    }

    /// Act on `key` after the first key of a sequence: the sequence it
    /// completes, or the prefix abandoned, as vim does.
    pub(super) fn on_sequence_key(&mut self, first: Key, key: &Key) -> EditorOutcome {
        match table::lookup(COMMON_KEYS, &[first, *key]) {
            TableMatch::Exact(act) => self.on_act(act),
            TableMatch::Miss | TableMatch::Prefix => {
                self.pending = Pending::default();
                EditorOutcome::Consumed
            }
        }
    }

    /// Act on the character typed after `f`, `F`, `t`, or `T`.
    pub(super) fn on_find_key(&mut self, kind: FindKind, key: &Key) -> EditorOutcome {
        let pending = self.pending;
        self.pending = Pending::default();
        let (Mods::NONE, KeyCode::Char(ch)) = key.chord() else {
            // Anything but a character abandons the find, as vim does.
            return EditorOutcome::Consumed;
        };
        self.act(Command::CaretFind(CaretFind {
            ch,
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            kind,
            operator: pending.operator,
        }))
    }

    /// Run `motion` with whatever count and operator were typed before it.
    pub(super) fn caret_motion(&mut self, motion: Motion) -> EditorOutcome {
        let pending = self.pending;
        self.act(Command::CaretMotion(CaretMotion {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            motion,
            operator: pending.operator,
        }))
    }

    /// `/` and `?`: ask the host for what to look for, keeping the count and
    /// operator typed until the answer comes back.
    pub(super) fn ask_search(&mut self, forward: bool) -> EditorOutcome {
        self.search_origin = Some((self.caret.pos, self.search.clone()));
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: match forward {
                true => "/".to_string(),
                false => "?".to_string(),
            },
            mode: PromptMode::Search { reverse: !forward },
            tag: match forward {
                true => ASK_SEARCH,
                false => ASK_SEARCH_BACK,
            },
        })
    }

    /// Put the cursor back where it was when the search prompt opened.
    pub(super) fn back_to_search_origin(&mut self) {
        if let Some((pos, _)) = &self.search_origin {
            self.caret.pos = *pos;
        }
        self.caret.found = None;
    }

    /// Look for what the prompt came back with, from where it was opened.
    pub(super) fn start_search(&mut self, pattern: String, forward: bool) -> EditorOutcome {
        self.back_to_search_origin();
        if pattern.is_empty() {
            self.pending = Pending::default();
            return EditorOutcome::Consumed;
        }
        let pending = self.pending;
        self.search = Some(Search::new(pattern.clone(), forward));
        self.searched = true;
        self.act(Command::CaretSearch(CaretSearch {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            forward,
            operator: pending.operator,
            pattern,
        }))
    }

    /// `n` and `N`, which need a search to repeat and say so when there has
    /// not been one.
    pub(super) fn search_step(&mut self, reverse: bool) -> EditorOutcome {
        if self.search.is_none() {
            self.pending = Pending::default();
            self.notice = Some(Notice::error("nothing has been searched for"));
            return EditorOutcome::Consumed;
        }
        self.searched = true;
        let pending = self.pending;
        self.act(Command::CaretSearchStep(CaretSearchStep {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            operator: pending.operator,
            reverse,
        }))
    }

    /// `;` and `,`, which repeat the last find.
    pub(super) fn repeat_find(&mut self, reverse: bool) -> EditorOutcome {
        let pending = self.pending;
        self.act(Command::CaretRepeatFind(CaretRepeat {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            operator: pending.operator,
            reverse,
        }))
    }

    /// `y`: the selection when there is one, `yy` when it follows itself,
    /// and otherwise the operator waiting for a motion.
    pub(super) fn on_yank_key(&mut self) -> EditorOutcome {
        if self.caret.visual != Visual::None {
            return self.act(Command::CaretYankSelection);
        }
        if self.pending.operator == Operator::Yank {
            let count = self.count();
            return self.act(Command::CaretYankLines(count));
        }
        self.pending.operator = Operator::Yank;
        EditorOutcome::Consumed
    }

    /// Escape with a cursor in the text: give up what is half-typed first,
    /// then the selection, then the cursor itself.
    pub(super) fn on_cursor_escape(&mut self) -> EditorOutcome {
        if self.pending != Pending::default() {
            self.pending = Pending::default();
            return EditorOutcome::Consumed;
        }
        if self.caret.visual != Visual::None {
            return self.act(Command::CaretVisual(Visual::None));
        }
        self.act(Command::CaretLeave)
    }

    /// Start a selection, swap which kind it is, or drop it: the same key
    /// again ends the selection it started.
    pub(super) fn set_visual(&mut self, kind: Visual) -> EditorOutcome {
        let visual = match self.caret.visual == kind {
            true => Visual::None,
            false => kind,
        };
        self.act(Command::CaretVisual(visual))
    }

    /// Do what a key of the viewer's own asks, with the count and the mode
    /// saying how.
    pub(super) fn on_act(&mut self, act: Act) -> EditorOutcome {
        let count = self.pending.count;
        match act {
            Act::Abandon => {
                self.pending = Pending::default();
                EditorOutcome::Consumed
            }
            Act::AskPage => self.ask_page(),
            Act::Caret(motion) => self.caret_motion(motion),
            Act::CaretEnd => match count {
                Some(_) => self.caret_motion(Motion::Page),
                None => self.caret_motion(Motion::DocEnd),
            },
            Act::Columns(direction) => {
                let steps = self.steps(direction);
                self.act(Command::ScrollColumns(steps))
            }
            Act::Cursor => self.act(Command::CaretEnter),
            Act::Find(kind) => {
                self.pending.prefix = Prefix::Find(kind);
                EditorOutcome::Consumed
            }
            // `gg` is the first page, or the page a count names: the one
            // place vim reads a count as an absolute. With a cursor, it is
            // where the text starts.
            Act::First => match (self.mode(), count) {
                (Mode::Cursor, Some(_)) => self.caret_motion(Motion::Page),
                (Mode::Cursor, None) => self.caret_motion(Motion::DocStart),
                (Mode::Normal, _) => self.act(Command::GoToPage(count.unwrap_or(1))),
            },
            Act::Center => self.act(Command::CenterPages),
            Act::Crop => self.act(Command::ToggleCrop),
            Act::FitContent => self.act(Command::FitContent),
            Act::FitPage => self.act(Command::SetZoom(Zoom::FitPage)),
            Act::FitWidth => self.act(Command::FitWidth),
            Act::Home => self.act(Command::GoToPage(1)),
            Act::Invert => self.act(Command::Invert),
            Act::ShowLinks => self.act(Command::ToggleLinks),
            Act::Sentences => self.act(Command::ToggleSentences),
            Act::Thumbnails => self.act(Command::ToggleThumbnails),
            Act::Spelling => self.act(Command::ToggleSpelling),
            Act::Misspelling(forward) => self.act(Command::StepMisspelling(forward)),
            Act::AddWord => self.act(Command::AddWord),
            Act::Last => match count {
                Some(page) => self.act(Command::GoToPage(page)),
                None => self.act(Command::GoToLastPage),
            },
            Act::Leave => self.on_cursor_escape(),
            Act::Lines(direction) => {
                let lines = self.steps(direction);
                self.act(Command::ScrollLines(lines))
            }
            Act::Outline => self.act(Command::ToggleOutline),
            Act::Pages(direction) => {
                let pages = self.steps(direction);
                self.act(Command::GoToPageBy(pages))
            }
            Act::Place(place) => match self.mode() {
                Mode::Cursor => self.act(Command::CaretPlaceLine(place)),
                Mode::Normal => self.act(Command::Place(place)),
            },
            Act::Position => self.act(Command::ReportPosition),
            Act::Count => self.act(Command::ReportCount),
            Act::RepeatFind(reverse) => self.repeat_find(reverse),
            Act::Screens(fraction) => self.act(Command::ScrollScreens(fraction)),
            Act::Search(forward) => self.ask_search(forward),
            Act::SearchStep(reverse) => self.search_step(reverse),
            Act::SwapEnds => self.act(Command::CaretSwapEnds),
            Act::FollowAtCaret => match self.link_at_caret() {
                Some((_, link)) => self.follow(link),
                None => EditorOutcome::Consumed,
            },
            Act::Hints(purpose) => self.show_hints(purpose),
            Act::Jump(back) => self.jump(back),
            Act::PreviewAtCaret => match self.link_at_caret() {
                Some((page, link)) => self.show_preview(link, page),
                None => EditorOutcome::Consumed,
            },
            Act::SyncBack => self.sync_back(None),
            Act::Visual(kind) => self.set_visual(kind),
            Act::Yank => self.on_yank_key(),
            Act::YankLines => {
                let count = self.count();
                self.act(Command::CaretYankLines(count))
            }
            Act::Zoom(steps) => self.act(Command::Zoom(steps)),
        }
    }

    /// The tables a key is looked up in, in order, for the mode the
    /// document is being read in.
    pub(super) fn mode_tables(&self) -> [&'static [Binding<Act>]; 2] {
        match self.mode() {
            Mode::Cursor => [CURSOR_KEYS, WINDOW_KEYS],
            Mode::Normal => [NORMAL_KEYS, WINDOW_KEYS],
        }
    }

    // --------------------------------------------------------------------
    // Carrying commands out
    // --------------------------------------------------------------------

    /// Carry out `command` on the document and the view.
    pub(super) fn execute(&mut self, command: Command) -> EditorOutcome {
        match command {
            Command::CaretEnter => {
                let at = self.screen_line(0.0);
                self.with_caret(|caret, doc| caret.enter(doc, at))
            }
            Command::CaretFind(find) => self.with_caret(|caret, doc| {
                caret.find(doc, find.kind, find.ch, find.count, find.operator)
            }),
            Command::CaretLeave => {
                self.caret.leave();
                EditorOutcome::Consumed
            }
            Command::CaretMotion(motion) => {
                let screen = match motion.motion {
                    Motion::ViewTop => self.screen_line(0.0),
                    Motion::ViewMiddle => self.screen_line(self.pane.1 / 2.0),
                    Motion::ViewBottom => self.screen_line(self.pane.1),
                    _ => None,
                };
                self.with_caret(|caret, doc| {
                    caret.motion(doc, motion.motion, motion.count, motion.operator, screen)
                })
            }
            Command::CaretPlaceLine(place) => {
                if let Some(rect) = self.caret_line_rect() {
                    let offset = place_offset(place, rect.3, self.pane.1, 0.0);
                    self.scroll_to((self.scroll.0, self.scroll.1 + rect.1 - offset));
                }
                EditorOutcome::Consumed
            }
            Command::CaretRepeatFind(repeat) => self.with_caret(|caret, doc| {
                caret.repeat_find(doc, repeat.reverse, repeat.count, repeat.operator)
            }),
            Command::CaretSearch(search) => {
                let from = self.screen_line(0.0);
                self.with_caret(|caret, doc| {
                    caret.search(
                        doc,
                        &search.pattern,
                        search.forward,
                        search.count,
                        search.operator,
                        from,
                    )
                })
            }
            Command::CaretSearchStep(step) => {
                let from = self.screen_line(0.0);
                self.with_caret(|caret, doc| {
                    caret.search_again(doc, step.reverse, step.count, step.operator, from)
                })
            }
            Command::CaretSwapEnds => self.with_caret(|caret, _| caret.swap_ends()),
            Command::CaretVisual(visual) => {
                self.caret.set_visual(visual);
                EditorOutcome::Consumed
            }
            Command::CaretYankLines(count) => {
                self.with_caret(|caret, doc| caret.yank_lines(doc, count))
            }
            Command::CaretYankSelection => self.with_caret(|caret, doc| caret.yank_selection(doc)),
            Command::CenterPages => {
                if let Some(column) = self.column() {
                    let x = (column.width - self.pane.0) / 2.0;
                    self.scroll_to((x, self.scroll.1));
                }
                EditorOutcome::Consumed
            }
            Command::FitContent => {
                if self.measure_text().is_none() {
                    self.notice = Some(Notice::info("no text to fit"));
                    return EditorOutcome::Consumed;
                }
                self.set_zoom(Zoom::FitContent);
                self.align_to_text();
                EditorOutcome::Consumed
            }
            Command::FitWidth => {
                self.set_zoom(Zoom::FitWidth);
                EditorOutcome::Consumed
            }
            Command::ToggleCrop => {
                if !self.cropped && self.measure_text().is_none() {
                    self.notice = Some(Notice::info("no text to crop to"));
                    return EditorOutcome::Consumed;
                }
                self.relayout(|pdf| pdf.cropped = !pdf.cropped);
                self.crop_drawn();
                EditorOutcome::Consumed
            }
            Command::ToggleSpelling => {
                self.spelling = match self.spelling {
                    Some(_) => None,
                    None => Some(Spelling::default()),
                };
                self.read_visible();
                EditorOutcome::Consumed
            }
            Command::StepMisspelling(forward) => self.step_misspelling(forward),
            Command::AddWord => self.add_word(),
            Command::ToggleThumbnails => {
                self.thumbnails = match self.thumbnails {
                    Some(_) => None,
                    None => Some(Thumbnails::default()),
                };
                // The pages are laid out again in what the strip leaves.
                self.set_pane_size(self.outer);
                self.request_visible();
                EditorOutcome::Consumed
            }
            Command::ToggleSentences => {
                self.sentences = match self.sentences {
                    Some(_) => None,
                    None => Some(HashMap::new()),
                };
                self.read_visible();
                EditorOutcome::Consumed
            }
            Command::ToggleLinks => {
                self.links_shown = !self.links_shown;
                self.read_visible();
                EditorOutcome::Consumed
            }
            Command::Invert => {
                // Drawn again, inverted or not: what is shown meanwhile is
                // the drawing it replaces, as at a zoom.
                self.inverted = !self.inverted;
                self.generation += 1;
                self.requested.clear();
                self.request_visible();
                EditorOutcome::Consumed
            }
            Command::SetZoom(zoom) => {
                self.set_zoom(zoom);
                EditorOutcome::Consumed
            }
            Command::ToggleOutline => {
                self.outline = match self.outline.take() {
                    Some(_) => None,
                    None => self.document.as_ref().and_then(|doc| {
                        let entries = doc.outline();
                        (!entries.is_empty())
                            .then(|| OutlineList::open(entries, self.current_section()))
                    }),
                };
                EditorOutcome::Consumed
            }
            Command::GoToLastPage => {
                self.remember_place();
                self.go_to_page(self.page_count().saturating_sub(1));
                EditorOutcome::Consumed
            }
            Command::GoToPage(number) => {
                self.remember_place();
                self.go_to_page(number.saturating_sub(1));
                EditorOutcome::Consumed
            }
            Command::GoToPageBy(delta) => {
                let page = self.current_page().saturating_add_signed(delta);
                self.go_to_page(page);
                EditorOutcome::Consumed
            }
            Command::Place(place) => {
                if let Some(column) = self.column() {
                    let rect = column.pages[self.current_page()];
                    let offset = place_offset(place, rect.height, self.pane.1, layout::GAP);
                    self.scroll_to((self.scroll.0, rect.y - offset));
                }
                EditorOutcome::Consumed
            }
            Command::ReportCount => {
                self.notice = self.word_count().map(Notice::info);
                EditorOutcome::Consumed
            }
            Command::ReportPosition => {
                self.notice = Some(Notice::info(self.position()));
                EditorOutcome::Consumed
            }
            Command::ScrollColumns(steps) => {
                let (x, y) = self.scroll_target();
                self.glide_to((x + steps as f32 * COLUMN_PX, y));
                EditorOutcome::Consumed
            }
            Command::ScrollLines(lines) => {
                let (x, y) = self.scroll_target();
                self.glide_to((x, y + lines as f32 * LINE_PX));
                EditorOutcome::Consumed
            }
            Command::ScrollScreens(fraction) => {
                let (x, y) = self.scroll_target();
                self.glide_to((x, y + fraction * self.pane.1));
                EditorOutcome::Consumed
            }
            Command::Zoom(steps) => {
                let steps = i32::try_from(steps).unwrap_or(i32::MAX);
                let scale = self.scale() * ZOOM_STEP.powi(steps);
                self.set_zoom(Zoom::Scale(scale.clamp(MIN_SCALE, MAX_SCALE)));
                EditorOutcome::Consumed
            }
        }
    }

    /// Run `act` on the cursor and the document, once there is one, and
    /// follow up what it did.
    pub(super) fn with_caret(
        &mut self,
        act: impl FnOnce(&mut Caret, &mut Document) -> Effect,
    ) -> EditorOutcome {
        let Some(doc) = self.document.as_mut() else {
            return EditorOutcome::Consumed;
        };
        match act(&mut self.caret, doc) {
            Effect::Moved => {
                self.scroll_caret_into_view();
                EditorOutcome::Consumed
            }
            Effect::Nothing => EditorOutcome::Consumed,
            Effect::Refused(why) => {
                self.notice = Some(Notice::error(why));
                EditorOutcome::Consumed
            }
            Effect::Yanked(text) => EditorOutcome::copied(text),
        }
    }

    // --------------------------------------------------------------------
    // Layout
    // --------------------------------------------------------------------

    /// Carry out `act` from a key, which a right-click's menu may have sent:
    /// going back to the source then starts from where the menu was opened.
    pub(super) fn on_key_act(&mut self, act: Act, menu_spot: Option<Pos>) -> EditorOutcome {
        match act {
            Act::SyncBack => self.sync_back(menu_spot),
            _ => self.on_act(act),
        }
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------
}
