//! The completion popup over the buffer being typed into: opened by `C-SPC`
//! or by itself once a word is long enough and typing pauses, narrowed as
//! typing goes on, moved through with `C-n` and `C-p`, and taken with `Tab`.
//!
//! What it offers comes from [`complete`]'s back-ends, the ones the
//! language asks in the order settings give.
//!
//! [`complete`]: super::complete

use std::path::Path;
use std::time::Instant;

use crate::config::CompletionConfig;
use crate::model::editor::{CompletionItem, CompletionPopup, EditorOutcome, Notice};
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::textedit::modal::EditMode;

use super::complete::{self, Context, Kind, Offer, ServerKind};
use crate::editors::highlight::Grammar;
use super::lsp::CompletionAsk;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// Each file extension and the language settings name it by.
const LANGUAGES: &[(&str, &str)] = &[
    ("bash", "shell"), ("c", "c"), ("cc", "cpp"), ("cjs", "javascript"), ("cls", "latex"),
    ("cpp", "cpp"), ("cxx", "cpp"), ("go", "go"), ("h", "c"), ("hpp", "cpp"),
    ("js", "javascript"), ("json", "json"), ("jsx", "javascript"), ("kdl", "kdl"),
    ("ltx", "latex"), ("markdown", "markdown"), ("md", "markdown"), ("mjs", "javascript"),
    ("py", "python"), ("rs", "rust"), ("sh", "shell"), ("sty", "latex"), ("tex", "latex"),
    ("toml", "toml"), ("ts", "typescript"), ("tsx", "typescript"), ("yaml", "yaml"),
    ("yml", "yaml"), ("zsh", "shell"), ("ml", "ocaml"), ("mli", "ocaml"), ("lua", "lua"),
    ("hs", "haskell"), ("zig", "zig"), ("nix", "nix"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// The popup while it is open: what it offers, which entry is chosen, the
/// row it is on, and whether `RET` takes the entry, which it does once the
/// popup was asked for or moved through rather than only opened by typing.
#[derive(Debug)]
pub(super) struct Completion {
    offer: Offer,
    row: usize,
    selected: usize,
    takes_return: bool,
}

/// The popup waiting for typing to pause before it opens by itself: when it
/// opens, and the row it was typed toward, which the cursor must still be on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CompletionDue {
    at: Instant,
    row: usize,
}

// ========================================================================
// TextEditor: completion
// ========================================================================

impl TextEditor {
    /// The language the buffer is written in, as settings name it.
    pub(super) fn language(&self) -> String {
        language_of(&self.doc().path, self.doc().grammar)
    }

    /// Take settings for completion, for the popup opened from now on.
    pub(super) fn take_completion_config(&mut self, config: &CompletionConfig) {
        if self.completion_config != *config {
            self.completion_config = config.clone();
        }
    }

    /// A key while typing, for the popup: what it does to the popup, or
    /// `None` for a key it leaves to the text, after which the popup is
    /// narrowed to what the text now is (see [`Self::after_typed`]).
    pub(super) fn completion_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        if !self.modal.is_typing() {
            self.completion = None;
            self.cancel_completion_due();
            return None;
        }
        let chord = key.chord();
        let asks = matches!(chord, (Mods::CTRL, KeyCode::Space | KeyCode::Char('m')));
        if self.completion.is_none() {
            return asks.then(|| self.open_completion(true));
        }
        let step = match chord {
            (Mods::NONE, KeyCode::Down) | (Mods::CTRL, KeyCode::Char('n')) => Some(1),
            (Mods::NONE, KeyCode::Up) | (Mods::CTRL, KeyCode::Char('p')) => Some(-1),
            _ => None,
        };
        if let Some(step) = step {
            if let Some(open) = self.completion.as_mut() {
                let count = open.offer.candidates.len() as isize;
                open.selected = (open.selected as isize + step).rem_euclid(count) as usize;
                open.takes_return = true;
            }
            return Some(EditorOutcome::Consumed);
        }
        match chord {
            (Mods::NONE, KeyCode::Tab) => Some(self.accept_completion()),
            (Mods::NONE, KeyCode::Enter) if self.completion.as_ref().is_some_and(|open| open.takes_return) => {
                Some(self.accept_completion())
            }
            (Mods::NONE, KeyCode::Escape) | (Mods::CTRL, KeyCode::Char('g')) => {
                self.completion = None;
                Some(EditorOutcome::Consumed)
            }
            _ if asks => Some(self.open_completion(true)),
            _ => None,
        }
    }

    /// After a key the text took while typing: narrow the open popup to
    /// what is typed now, closing it where nothing is left, or open it by
    /// itself once a word typed is long enough.
    pub(super) fn after_typed(&mut self, key: &Key) {
        // Each key typed puts off the popup waiting to open, or calls it off.
        self.cancel_completion_due();
        if !self.modal.is_typing() || self.modal.mode() != EditMode::Insert {
            self.completion = None;
            return;
        }
        if let Some(open) = self.completion.as_ref() {
            let takes_return = open.takes_return;
            if open.row != self.buffer().row() {
                self.completion = None;
                return;
            }
            let selected_label = open.offer.candidates.get(open.selected).map(|c| c.label.clone());
            let incomplete = self.link.served.as_ref().is_some_and(|served| served.incomplete);
            if incomplete && self.uses_server() {
                self.ask_server_completion(takes_return, None);
            }
            let row = self.buffer().row();
            self.completion = self.offer_here().map(|offer| {
                let selected = selected_label
                    .and_then(|label| offer.candidates.iter().position(|c| c.label == label))
                    .unwrap_or(0);
                Completion {
                    offer,
                    row,
                    selected,
                    takes_return,
                }
            });
            return;
        }
        let typed = match key.chord() {
            (Mods::NONE, KeyCode::Char(c)) => Some(c),
            _ => None,
        };
        let typed_word =
            typed.is_some_and(|c| complete::is_word_char(c) || matches!(c, '\\' | '/' | '{' | '#'));
        let trigger = typed.filter(|c| self.uses_server() && self.is_trigger(*c));
        if trigger.is_some() && self.completion_config.auto {
            self.ask_server_completion(false, trigger);
            return;
        }
        if !self.completion_config.auto || !typed_word {
            return;
        }
        let line: Vec<char> = self.line_of(self.buffer().row()).chars().collect();
        let col = self.buffer().col().min(line.len());
        let mut start = col;
        while start > 0 && complete::is_word_char(line[start - 1]) {
            start -= 1;
        }
        let triggered = start > 0 && matches!(line[start - 1], '\\' | '/' | '{' | '#');
        if col - start >= self.completion_config.min_chars || triggered {
            match self.completion_config.delay.is_zero() {
                true => {
                    self.open_completion(false);
                }
                false => {
                    self.completion_due = Some(CompletionDue {
                        at: Instant::now() + self.completion_config.delay,
                        row: self.buffer().row(),
                    });
                }
            }
        }
    }

    /// When the popup waiting for typing to pause opens.
    pub(super) fn completion_next_tick(&self) -> Option<Instant> {
        self.completion_due.map(|due| due.at)
    }

    /// Call off the popup waiting for typing to pause, noting its deadline
    /// so the host stops waiting on it (see [`Self::take_cancelled_completion`]).
    fn cancel_completion_due(&mut self) {
        if let Some(due) = self.completion_due.take() {
            let earliest = self.cancelled_tick.map_or(due.at, |at| at.min(due.at));
            self.cancelled_tick = Some(earliest);
        }
    }

    /// The earliest deadline of a popup called off since last asked.
    pub(super) fn take_cancelled_completion(&mut self) -> Option<Instant> {
        self.cancelled_tick.take()
    }

    /// Open the popup waiting for typing to pause, once it is due, while
    /// the cursor is still typing on the row it was typed toward.
    pub(super) fn completion_tick(&mut self, now: Instant) {
        let Some(due) = self.completion_due.filter(|due| due.at <= now) else {
            return;
        };
        self.completion_due = None;
        let typing = self.modal.is_typing() && self.modal.mode() == EditMode::Insert;
        if typing && self.completion.is_none() && self.buffer().row() == due.row {
            self.open_completion(false);
        }
    }

    /// Whether the language's back-ends name the `lsp` one, so a server
    /// is asked for completions as the others are.
    fn uses_server(&self) -> bool {
        let language = self.language();
        let setting = self.completion_config.backends.get(&language).map(Vec::as_slice);
        complete::backends_for(&language, setting).contains(&"lsp")
    }

    /// A server's completions arrived for the row `ask` names: narrow the
    /// popup open there with them, or open it with them where none is.
    pub(super) fn server_offered(&mut self, ask: CompletionAsk) {
        if self.modal.mode() != EditMode::Insert {
            return;
        }
        let selected_label = self
            .completion
            .as_ref()
            .and_then(|open| open.offer.candidates.get(open.selected).map(|c| c.label.clone()));
        let takes_return = self.completion.as_ref().map_or(ask.asked, |open| open.takes_return);
        let row = self.buffer().row();
        self.completion = self.offer_here().map(|offer| {
            let selected = selected_label
                .and_then(|label| offer.candidates.iter().position(|c| c.label == label))
                .unwrap_or(0);
            Completion {
                offer,
                row,
                selected,
                takes_return,
            }
        });
        if self.completion.is_none() && ask.asked {
            self.modal.report(Notice::info("nothing to complete here"));
        }
    }

    /// Open the popup on what could go at the cursor; asked for with `C-SPC`
    /// when `asked`, which says so where nothing could.
    fn open_completion(&mut self, asked: bool) -> EditorOutcome {
        let waiting = self.uses_server() && self.ask_server_completion(asked, None);
        match self.offer_here() {
            Some(offer) => {
                self.completion = Some(Completion {
                    offer,
                    row: self.buffer().row(),
                    selected: 0,
                    takes_return: asked,
                });
            }
            None => {
                self.completion = None;
                // A server still to answer may yet have something.
                if asked && !waiting {
                    self.modal.report(Notice::info("nothing to complete here"));
                }
            }
        }
        EditorOutcome::Consumed
    }

    /// What the language's back-ends offer at the cursor.
    fn offer_here(&self) -> Option<Offer> {
        let row = self.buffer().row();
        let line = self.line_of(row);
        let others: Vec<&[String]> = self
            .docs
            .iter()
            .enumerate()
            .filter(|(at, _)| *at != self.at)
            .map(|(_, doc)| doc.buffer.lines())
            .collect();
        let language = self.language();
        let setting = self.completion_config.backends.get(&language).map(Vec::as_slice);
        let backends = complete::backends_for(&language, setting);
        let col = self.buffer().col().min(line.chars().count());
        // What the server offered stands while the cursor is on its row,
        // past where its text would start.
        let server = self
            .link
            .served
            .as_ref()
            .filter(|served| served.row == row && served.reply.start <= col)
            .map(|served| &served.reply);
        let context = Context {
            col,
            language: &language,
            line: &line,
            lines: self.buffer().lines(),
            others: &others,
            path: &self.doc().path,
            row,
            server,
        };
        complete::gather(&context, &backends)
    }

    /// Put the chosen entry in place of what it completes: a snippet
    /// expanded, its cursor where its first field is.
    fn accept_completion(&mut self) -> EditorOutcome {
        let Some(open) = self.completion.take() else {
            return EditorOutcome::Consumed;
        };
        let Some(chosen) = open.offer.candidates.get(open.selected).cloned() else {
            return EditorOutcome::Consumed;
        };
        let row = self.buffer().row();
        let line: Vec<char> = self.line_of(row).chars().collect();
        let col = self.buffer().col().min(line.len());
        let start = open.offer.start.min(col);
        let head: String = line[..start].iter().collect();
        let tail: String = line[col..].iter().collect();
        let indent: String = line.iter().take_while(|c| c.is_whitespace()).collect();
        let brought = match chosen.source {
            "lsp" => self.served_edits(&chosen.label),
            _ => Vec::new(),
        };
        let expansion = match chosen.kind {
            Kind::Snippet | Kind::Command | Kind::Server(ServerKind { snippet: true, .. }) => {
                complete::expand(&chosen.insert, &indent)
            }
            _ => complete::Expansion {
                cursor: (0, chosen.insert.chars().count()),
                lines: vec![chosen.insert.clone()],
            },
        };
        let mut lines = expansion.lines.clone();
        let last = lines.len() - 1;
        lines[0] = format!("{head}{}", lines[0]);
        lines[last].push_str(&tail);
        let (cursor_row, cursor_col) = expansion.cursor;
        let cursor_col = match cursor_row {
            0 => start + cursor_col,
            _ => cursor_col,
        };
        self.buffer_mut().replace_rows(row, 1, lines);
        self.buffer_mut().move_to_typing(row + cursor_row, cursor_col);
        self.link.served = None;
        // An import the entry needs goes in where the server says, which is
        // read against the text before the entry went in: above it, where
        // an import goes, nothing has moved.
        if brought.iter().all(|edit| (edit.range.end.line as usize) < row) {
            let (row, col) = (self.buffer().row(), self.buffer().col());
            let before = self.buffer().lines().len();
            self.apply_served_edits(&brought);
            let added = self.buffer().lines().len() as isize - before as isize;
            self.buffer_mut().move_to_typing((row as isize + added).max(0) as usize, col);
        }
        EditorOutcome::Consumed
    }

    /// The popup, for the window to draw under the cursor.
    pub(super) fn popup(&self) -> Option<CompletionPopup> {
        let open = self.completion.as_ref()?;
        let typed = self.buffer().col().saturating_sub(open.offer.start);
        Some(CompletionPopup {
            items: open
                .offer
                .candidates
                .iter()
                .map(|candidate| CompletionItem {
                    detail: candidate.detail.clone(),
                    kind: kind_mark(candidate.kind),
                    label: candidate.label.clone(),
                    source: candidate.source,
                })
                .collect(),
            selected: open.selected,
            typed,
        })
    }
}

/// The letter the popup marks a kind of entry with.
fn kind_mark(kind: Kind) -> char {
    match kind {
        Kind::Citation => 'c',
        Kind::Command => '\\',
        Kind::Directory => 'd',
        Kind::Environment => 'e',
        Kind::Extension => 'x',
        Kind::File => 'f',
        Kind::Heading => '#',
        Kind::Keyword => 'k',
        Kind::Label => 'l',
        Kind::Server(server) => server.mark,
        Kind::Snippet => 's',
        Kind::Word => 'w',
    }
}

/// The language a file at `path`, read with `grammar`, is written in, as
/// settings name it: by its extension, else by its grammar's name.
pub(super) fn language_of(path: &Path, grammar: Option<Grammar>) -> String {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    extension
        .and_then(|extension| {
            LANGUAGES
                .iter()
                .find(|(known, _)| *known == extension)
                .map(|(_, language)| language.to_string())
        })
        .or_else(|| grammar.map(|grammar| grammar.name().to_ascii_lowercase()))
        .unwrap_or_else(|| "text".to_string())
}
