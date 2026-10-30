//! The words of a file's prose that are spelled wrong, found again after
//! each edit and kept a line at a time: the spelling back-end, publishing
//! them to [`winter_lsp::diagnostics`] for the editor to underline and step
//! through with every other problem.
//!
//! Only the prose is checked, as magic-vscode checks it: a LaTeX file's
//! text but not its commands or math, a Markdown file's but not its code,
//! and a program's comments alone. LaTeX, BibTeX, and Markdown are checked
//! from the moment they open; any other file once checking is asked for.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use winter_spell::{prose_for_extension, Prose, DEFAULT_SUGGESTION_LIMIT};

use winter_lsp::diagnostics::{self, Backend};
use crate::editors::background::Background;
use crate::editors::spell::{self, WordList};
use crate::project;
use winter_lsp::protocol::{self, Diagnostic, Range, Severity};
use crate::model::editor::{EditorOutcome, Notice};
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::textedit::modal::{BracketWait, EditMode, Pending};

use super::fixes::{Accept, Apply, Fix, Replace};
use super::{Document, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// What a card names as having found a word spelled wrong.
const SOURCE: &str = "spelling";

/// How bad a word spelled wrong is: a remark, so a server's own warning or
/// error over it still shows.
const SEVERITY: Severity = Severity::Information;

/// Past this many bytes, a file's spelling is checked again only once
/// typing pauses: the whole text is read each time, and on every keystroke
/// that lags a long document.
const SETTLE_PAST_BYTES: usize = 64 * 1024;

/// How long typing pauses before a long file's spelling is checked again.
const SETTLE: Duration = Duration::from_millis(400);

/// The thread a long file's spelling is checked on.
const CHECK_THREAD: &str = "winter spelling";

// ========================================================================
// Data Structures
// ========================================================================

/// Where checking one file stands.
#[derive(Debug)]
pub(super) struct Spelling {
    /// Each line's words spelled wrong, as the columns each covers, its
    /// first and one past its last character.
    lines: Vec<Vec<(usize, usize)>>,
    /// The root of the project the file is in, whose own words are taken
    /// as spelled right in it; `None` outside any project.
    project: Option<PathBuf>,
    /// What kind of prose the file is, read off its name.
    prose: Prose,
    /// The buffer's revision and the dictionary's generation the words were
    /// found at, `None` before they first are.
    checked: Option<(usize, u64)>,
    /// When the words were last found, for a long file to wait for typing
    /// to pause before they are found again.
    checked_at: Option<Instant>,
    /// A long file's words being found off the window's thread, with the
    /// revision and generation they are found at: read through the speller
    /// on it, a long document froze the window each time typing paused.
    running: Option<Background<Found>>,
}

/// Each line's words spelled wrong, and the revision and dictionary
/// generation they were found at.
type Found = ((usize, u64), Vec<Vec<(usize, usize)>>);

// ========================================================================
// Spelling
// ========================================================================

impl Spelling {
    /// Checking for the file at `path` as its kind of prose, a file of no
    /// kind it knows read as plain text.
    pub(super) fn of(path: &Path) -> Self {
        Self {
            checked: None,
            checked_at: None,
            lines: Vec::new(),
            running: None,
            project: path.parent().and_then(project::marked_root),
            prose: prose_of(path).unwrap_or(Prose::PlainText),
        }
    }

    /// Checking for the file at `path` from the moment it opens: a LaTeX,
    /// BibTeX, or Markdown file's. `None` for any other.
    pub(super) fn from_the_start(path: &Path) -> Option<Self> {
        let prose = prose_of(path)?;
        matches!(prose, Prose::Latex | Prose::Bibtex | Prose::Markdown).then(|| Self::of(path))
    }

    /// Find the words spelled wrong in `lines` again, unless they were
    /// found at `revision` already and no word has been taken as spelled
    /// right since. Says whether they were.
    fn update(&mut self, lines: &[String], revision: usize) -> bool {
        let landed = self.take_found();
        let now = (revision, spell::generation());
        if self.checked == Some(now) || self.running.is_some() {
            return landed;
        }
        // A long file keeps the marks it had while typing goes on; they are
        // found again once it pauses (see [`Self::due`]), off the window's
        // thread.
        if is_long(lines) {
            if self.due(lines).is_some_and(|due| Instant::now() < due) {
                return landed;
            }
            self.checked_at = Some(Instant::now());
            let (lines, prose, project) = (lines.to_vec(), self.prose, self.project.clone());
            self.running = Some(Background::start(CHECK_THREAD, move |_| {
                (now, find(&lines, prose, project.as_deref()))
            }));
            return landed;
        }
        self.checked = Some(now);
        self.checked_at = Some(Instant::now());
        self.lines = find(lines, self.prose, self.project.as_deref());
        true
    }

    /// Take the words a check off the window's thread found, once it has.
    /// Says whether there were any to take.
    fn take_found(&mut self) -> bool {
        let Some(running) = self.running.take_if(|running| running.is_done()) else {
            return false;
        };
        let Some((checked, lines)) = running.poll() else {
            return false;
        };
        self.checked = Some(checked);
        self.lines = lines;
        true
    }

    /// Whether a check off the window's thread has words to take.
    fn has_found(&self) -> bool {
        self.running.as_ref().is_some_and(Background::is_done)
    }

    /// Whether the words have never been found, a first check still running.
    pub(super) fn is_first_check_running(&self) -> bool {
        self.checked.is_none() && self.running.is_some()
    }

    /// When a long file's words are next found, typing having paused;
    /// `None` for a short file, which is checked as it changes.
    fn due(&self, lines: &[String]) -> Option<Instant> {
        is_long(lines).then_some(self.checked_at? + SETTLE)
    }

    /// The words line `index` has spelled wrong.
    pub(super) fn on_line(&self, index: usize) -> &[(usize, usize)] {
        self.lines.get(index).map_or(&[], Vec::as_slice)
    }

    /// The word spelled wrong after the cursor at `row` and `col`
    /// (`forward`), or before it, round past the file's end, as its row and
    /// columns.
    pub(super) fn step(&self, row: usize, col: usize, forward: bool) -> Option<(usize, (usize, usize))> {
        let all = self
            .lines
            .iter()
            .enumerate()
            .flat_map(|(line, words)| words.iter().map(move |word| (line, *word)));
        match forward {
            true => {
                let mut all = all.peekable();
                let first = all.peek().copied();
                all.find(|(line, (from, _))| (*line, *from) > (row, col))
                    .or(first)
            }
            false => {
                let words: Vec<_> = all.collect();
                let last = words.last().copied();
                words
                    .into_iter()
                    .rev()
                    .find(|(line, (from, _))| (*line, *from) < (row, col))
                    .or(last)
            }
        }
    }

    /// The word spelled wrong that column `col` of line `row` is in.
    pub(super) fn at(&self, row: usize, col: usize) -> Option<(usize, usize)> {
        self.on_line(row)
            .iter()
            .find(|(from, to)| (*from..*to).contains(&col))
            .copied()
    }
}

// ========================================================================
// TextEditor: spelling
// ========================================================================

impl TextEditor {
    /// Check the file's words again if it has changed since they were,
    /// and publish what is found.
    pub(super) fn check_spelling(&mut self) {
        if publish_spelling(self.doc_mut()) {
            let path = self.doc().path.clone();
            self.refresh_diagnostics(Some(&path));
        }
    }

    /// Bring the spelling back-end up to date before problems are gathered:
    /// every open file whose spelling is checked, and with `project`, the
    /// prose files under it on disk (see [`super::spelling_scan`]).
    pub(super) fn refresh_spelling(&mut self, project: Option<&Path>) {
        self.check_open_spelling();
        if let Some(root) = project {
            self.scan_prose(root);
        }
    }

    /// Now, where a check of any open file's spelling has landed off the
    /// window's thread, to be published (see [`Self::take_landed_spelling`]).
    pub(super) fn spelling_landed_tick(&self) -> Option<Instant> {
        self.docs
            .iter()
            .any(|doc| doc.spelling.as_ref().is_some_and(Spelling::has_found))
            .then(Instant::now)
    }

    /// Publish what the checks of open files that have landed found.
    pub(super) fn take_landed_spelling(&mut self) {
        if self.spelling_landed_tick().is_some() {
            self.check_open_spelling();
        }
    }

    /// Check every open file whose spelling is checked, and publish what
    /// is found: the open files' part of a project's writing issues.
    fn check_open_spelling(&mut self) {
        let checked: Vec<_> = self
            .docs
            .iter_mut()
            .filter_map(|doc| publish_spelling(doc).then(|| doc.path.clone()))
            .collect();
        for path in checked {
            self.refresh_diagnostics(Some(&path));
        }
    }

    /// Check the file's spelling, or stop.
    pub(super) fn toggle_spelling(&mut self) -> EditorOutcome {
        let doc = self.doc_mut();
        let (spelling, said) = match doc.spelling.take() {
            Some(_) => (None, "spelling is not checked"),
            None => (Some(Spelling::of(&doc.path)), "spelling is checked"),
        };
        doc.spelling = spelling;
        if doc.spelling.is_none() {
            let path = doc.path.clone();
            diagnostics::publish(Backend::Spelling, &path, Vec::new());
            self.refresh_diagnostics(Some(&path));
        }
        self.modal.report(Notice::info(said).unlogged());
        EditorOutcome::Consumed
    }

    /// Vim's spelling keys in Normal mode: `]s` and `[s` to the next and
    /// previous word spelled wrong, `z=` for how the word under the cursor
    /// might be spelled, and `zg` to take it as right. `None` for any other
    /// key, or with nothing part-typed for one.
    pub(super) fn vim_spelling(&mut self, key: &Key) -> Option<EditorOutcome> {
        if self.modal.mode() != EditMode::Normal {
            return None;
        }
        let outcome = match (self.modal.pending(), key.chord()) {
            (Some(Pending::Bracket(BracketWait { forward })), (Mods::NONE, KeyCode::Char('s'))) => {
                self.step_misspelling(forward)
            }
            (Some(Pending::Scroll), (Mods::NONE, KeyCode::Char('='))) => self.suggest_spelling(),
            (Some(Pending::Scroll), (Mods::NONE, KeyCode::Char('g'))) => self.take_word(),
            _ => return None,
        };
        // The bracket or the `z` waiting on this key is done with.
        let at = self.at.min(self.docs.len() - 1);
        self.modal.reset(&self.docs[at].buffer);
        Some(outcome)
    }

    /// Put the cursor on the next word spelled wrong (`forward`) or the
    /// previous, round past the file's end, and say how it might be
    /// spelled; checking starts if it had not.
    fn step_misspelling(&mut self, forward: bool) -> EditorOutcome {
        if self.doc().spelling.is_none() {
            let path = self.doc().path.clone();
            self.doc_mut().spelling = Some(Spelling::of(&path));
        }
        self.check_spelling();
        if self.doc().spelling.as_ref().is_some_and(Spelling::is_first_check_running) {
            self.modal.report(Notice::info("spelling is still being checked").unlogged());
            return EditorOutcome::Consumed;
        }
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let found = self
            .doc()
            .spelling
            .as_ref()
            .and_then(|spelling| spelling.step(row, col, forward));
        let Some((row, (from, to))) = found else {
            self.modal.report(Notice::info("no words spelled wrong").unlogged());
            return EditorOutcome::Consumed;
        };
        self.buffer_mut().move_to(row, from);
        let word = self.word_between(row, from, to);
        let suggested = spell::dictionary().suggest(&word, DEFAULT_SUGGESTION_LIMIT);
        let said = match suggested.is_empty() {
            true => format!("{word}: no suggestions (z g to take it as right)"),
            false => format!("{word}: {} (z = to choose)", suggested.join(", ")),
        };
        self.modal.report(Notice::info(said).unlogged());
        EditorOutcome::Consumed
    }

    /// The word under the cursor, spelled wrong or not, as its row and
    /// columns.
    fn word_here(&self) -> Option<(usize, usize, usize)> {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let found = self.doc().spelling.as_ref().and_then(|spelling| spelling.at(row, col));
        let (from, to) = found.or_else(|| word_at(self.buffer().lines().get(row)?, col))?;
        Some((row, from, to))
    }

    /// The text of line `row` from column `from` to `to`.
    fn word_between(&self, row: usize, from: usize, to: usize) -> String {
        let line = self.buffer().lines().get(row).map_or("", String::as_str);
        line.chars().skip(from).take(to - from).collect()
    }

    /// The fixes for the word under the cursor while it is spelled wrong:
    /// the source [`super::fixes`] asks spelling for.
    pub(super) fn spelling_fixes(&self) -> Vec<Fix> {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let found = self.doc().spelling.as_ref().and_then(|spelling| spelling.at(row, col));
        match found {
            Some((from, to)) => self.fixes_for(row, from, to),
            None => Vec::new(),
        }
    }

    /// Offer how the word under the cursor might be spelled, spelled wrong
    /// or not, to put in its place, and taking it as it is.
    fn suggest_spelling(&mut self) -> EditorOutcome {
        let Some((row, from, to)) = self.word_here() else {
            self.modal.report(Notice::info("no word under the cursor").unlogged());
            return EditorOutcome::Consumed;
        };
        let label = format!("Spell \"{}\" as", self.word_between(row, from, to));
        let fixes = self.fixes_for(row, from, to);
        self.offer_fixes(label, fixes)
    }

    /// The word at columns `from` to `to` of line `row` put right: each of
    /// its suggestions, its first letter's case kept, then taking it as it
    /// is.
    fn fixes_for(&self, row: usize, from: usize, to: usize) -> Vec<Fix> {
        let word = self.word_between(row, from, to);
        let capital = word.chars().next().is_some_and(char::is_uppercase);
        let mut fixes: Vec<Fix> = spell::dictionary()
            .suggest(&word, DEFAULT_SUGGESTION_LIMIT)
            .into_iter()
            .map(|suggested| {
                let text = match capital {
                    true => capitalized(&suggested),
                    false => suggested,
                };
                Fix {
                    title: format!("Change to \"{text}\""),
                    apply: Apply::Replace(Replace {
                        from,
                        row,
                        text,
                        to,
                    }),
                }
            })
            .collect();
        let project = self.doc().path.parent().and_then(project::marked_root);
        let lists = [
            Some((WordList::User, "Add \"{word}\" to user dictionary")),
            project.map(|root| (WordList::Project(root), "Add \"{word}\" to project dictionary")),
            Some((WordList::Session, "Ignore \"{word}\" for this session")),
        ];
        fixes.extend(lists.into_iter().flatten().map(|(list, title)| Fix {
            title: title.replace("{word}", &word),
            apply: Apply::Accept(Accept {
                list,
                word: word.clone(),
            }),
        }));
        fixes
    }

    /// Take the word under the cursor as spelled right.
    fn take_word(&mut self) -> EditorOutcome {
        let Some((row, from, to)) = self.word_here() else {
            self.modal.report(Notice::info("no word under the cursor").unlogged());
            return EditorOutcome::Consumed;
        };
        let word = self.word_between(row, from, to);
        self.accept_word(&word, &WordList::User);
        EditorOutcome::Consumed
    }

    /// Take `word` as spelled right in `list`, from now on and in every
    /// editor, which each check their buffers again for it.
    pub(super) fn accept_word(&mut self, word: &str, list: &WordList) {
        let said = match list {
            WordList::User => format!("added \"{word}\" to the user dictionary"),
            WordList::Project(_) => format!("added \"{word}\" to the project dictionary"),
            WordList::Session => format!("ignoring \"{word}\" for this session"),
        };
        let notice = match spell::accept(word, list) {
            Ok(()) => Notice::info(said),
            Err(why) => Notice::error(format!("could not add \"{word}\": {why}")),
        };
        self.modal.report(notice);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Check `doc`'s words again if it has changed since they were, and
/// publish what is found. Says whether it was checked.
/// When the open file's spelling is next checked, a long file waiting for
/// typing to pause; `None` when it is up to date or checked as it changes.
pub(super) fn spelling_due(doc: &Document) -> Option<Instant> {
    let spelling = doc.spelling.as_ref()?;
    // A check running lands once it is done; none starts beside it.
    if spelling.running.is_some() {
        return spelling.has_found().then(Instant::now);
    }
    let current = spelling.checked == Some((doc.buffer.revision(), spell::generation()));
    match current {
        true => None,
        false => spelling.due(doc.buffer.lines()),
    }
}

fn publish_spelling(doc: &mut Document) -> bool {
    let Some(spelling) = doc.spelling.as_mut() else {
        return false;
    };
    if !spelling.update(doc.buffer.lines(), doc.buffer.revision()) {
        return false;
    }
    diagnostics::publish(Backend::Spelling, &doc.path, as_diagnostics(&spelling.lines, doc.buffer.lines()));
    true
}

/// Whether `lines` are long enough to be checked only once typing pauses,
/// and off the window's thread.
fn is_long(lines: &[String]) -> bool {
    lines.iter().map(|line| line.len() + 1).sum::<usize>() > SETTLE_PAST_BYTES
}

/// The words `found` spelled wrong in `lines`, a line at a time as
/// columns, as the problems the store keeps. Words found in an older text,
/// on a line it no longer has, are left out.
pub(super) fn as_diagnostics(found: &[Vec<(usize, usize)>], lines: &[String]) -> Vec<Diagnostic> {
    found
        .iter()
        .enumerate()
        .take(lines.len())
        .flat_map(|(row, words)| words.iter().map(move |&(from, to)| (row, from, to)))
        .map(|(row, from, to)| {
            let word: String = lines[row].chars().skip(from).take(to.saturating_sub(from)).collect();
            Diagnostic {
                code: None,
                message: format!("\"{word}\" is not in the dictionary"),
                range: Range {
                    end: protocol::position_in(lines, row, to),
                    start: protocol::position_in(lines, row, from),
                },
                severity: SEVERITY,
                source: Some(SOURCE.to_string()),
            }
        })
        .collect()
}

/// The kind of prose the file at `path` is, by its extension.
pub(super) fn prose_of(path: &Path) -> Option<Prose> {
    prose_for_extension(path.extension()?.to_str()?)
}

/// The words of `lines`' prose that the dictionary does not know, a line at
/// a time, as columns.
pub(super) fn find(lines: &[String], prose: Prose, project: Option<&Path>) -> Vec<Vec<(usize, usize)>> {
    let text = lines.join("\n");
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for line in lines {
        starts.push(at);
        at += line.len() + 1;
    }
    let mut found = vec![Vec::new(); lines.len()];
    let misspelled = spell::dictionary().misspellings_in(&text, prose);
    let unaccepted = misspelled
        .into_iter()
        .filter(|token| !text.get(token.start..token.end).is_some_and(|word| spell::accepts(word, project)));
    for token in unaccepted {
        let row = starts.partition_point(|start| *start <= token.start).saturating_sub(1);
        let (Some(line), Some(start)) = (lines.get(row), starts.get(row)) else {
            continue;
        };
        let column = |byte: usize| line[..(byte - start).min(line.len())].chars().count();
        found[row].push((column(token.start), column(token.end)));
    }
    found
}

/// `word` with its first letter a capital.
fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The word that column `col` of `line` is in, as its first and one past its
/// last column: a run of letters and the apostrophes inside it.
pub(super) fn word_at(line: &str, col: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let is_word = |at: usize| chars.get(at).is_some_and(|ch| ch.is_alphabetic() || *ch == '\'');
    if !is_word(col) {
        return None;
    }
    let from = (0..=col).rev().take_while(|at| is_word(*at)).last()?;
    let to = (col..chars.len()).take_while(|at| is_word(*at)).last()? + 1;
    Some((from, to))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    /// Wait for the check running off the window's thread to land, at
    /// `revision`.
    fn land(spelling: &mut Spelling, lines: &[String], revision: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !spelling.update(lines, revision) {
            assert!(Instant::now() < deadline, "the check lands");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(spelling.checked.map(|(at, _)| at), Some(revision));
        assert!(spelling.running.is_none());
    }

    #[test]
    fn test_a_long_file_is_checked_again_once_typing_pauses() {
        // Every keystroke read the whole text through the speller, which
        // lagged typing in a long document.
        let mut spelling = Spelling::of(Path::new("/spelling-test/long.md"));
        let long: Vec<String> = (0..SETTLE_PAST_BYTES / 5).map(|_| "some text".to_string()).collect();
        assert!(!spelling.update(&long, 1), "the first look starts at once, off the window's thread");
        assert!(spelling.is_first_check_running());
        land(&mut spelling, &long, 1);
        assert!(!spelling.update(&long, 2), "typing on waits");
        assert!(spelling.running.is_none(), "and starts nothing");
        assert!(spelling.due(&long).is_some());
        spelling.checked_at = Some(Instant::now() - SETTLE);
        assert!(!spelling.update(&long, 3), "a pause checks again");
        land(&mut spelling, &long, 3);

        let short = lines(&["a line"]);
        let mut spelling = Spelling::of(Path::new("/spelling-test/short.md"));
        assert!(spelling.update(&short, 1));
        assert!(spelling.update(&short, 2), "a short file is checked as it changes");
    }

    #[test]
    fn test_only_the_prose_is_checked_and_found_by_its_columns() {
        let text = lines(&["\\section{Teh start}", "We recieve $\\frac{a}{bx}$ here."]);
        let mut spelling = Spelling::of(Path::new("paper.tex"));
        spelling.update(&text, 1);
        assert_eq!(spelling.on_line(0), [(9, 12)], "the command is no word");
        assert_eq!(spelling.on_line(1), [(3, 10)], "nor is the math");
    }

    #[test]
    fn test_a_program_s_comments_alone_are_checked() {
        let text = lines(&["let recieve = 1; // we recieve it"]);
        let mut spelling = Spelling::of(Path::new("main.rs"));
        spelling.update(&text, 1);
        assert_eq!(spelling.on_line(0), [(23, 30)]);
    }

    #[test]
    fn test_prose_files_are_checked_from_the_start_and_code_only_when_asked() {
        assert!(Spelling::from_the_start(Path::new("a.md")).is_some());
        assert!(Spelling::from_the_start(Path::new("a.bib")).is_some());
        assert!(Spelling::from_the_start(Path::new("a.rs")).is_none());
    }

    #[test]
    fn test_stepping_goes_on_from_the_cursor_and_round_the_ends() {
        let text = lines(&["teh one", "fine", "and teh two"]);
        let mut spelling = Spelling::of(Path::new("notes.txt"));
        spelling.update(&text, 1);
        assert_eq!(spelling.step(0, 0, true), Some((2, (4, 7))), "not the one under it");
        assert_eq!(spelling.step(2, 4, true), Some((0, (0, 3))), "round the end");
        assert_eq!(spelling.step(0, 0, false), Some((2, (4, 7))), "round the start");
        assert_eq!(spelling.at(2, 5), Some((4, 7)));
    }

    #[test]
    fn test_the_word_at_a_column_is_its_run_of_letters() {
        assert_eq!(word_at("say don't go", 6), Some((4, 9)));
        assert_eq!(word_at("a, b", 1), None);
    }
}
