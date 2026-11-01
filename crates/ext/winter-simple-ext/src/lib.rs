//! An example Winter extension, and a template for writing one: three
//! commands, one that reads the file in front and says something about it,
//! one that publishes problems for Winter to underline and step through, and
//! one that offers what it found in a picker of its own;
//! a linter that runs on every save, reading the file off Winter's thread so
//! a big one never holds up typing, and again as you type; a status bar
//! segment following the file in front; a completion back-end; and a view
//! of its own.
//!
//! - `example.word_count`: how many words the file in front holds, also
//!   copied where the `copy-count` setting is on
//!   (`example { copy-count #true }` in `settings.kdl`).
//! - `example.open_guide`: the guide to writing an extension, in the
//!   browser.
//! - `example.flag_todos`: each `TODO` in it, as a problem.
//! - `example.pick_todo`: each `TODO` in it, in a picker: `Enter` goes to
//!   it, and the action menu (`F4`) can flag it as a problem instead.
//! - On a save, and as the typing in the file in front pauses: each `TODO`
//!   in the file, as a problem.
//! - The `example.todos` segment: how many `TODO`s the file in front holds.
//! - `example.markers`: [`MARKERS`] offered for the word at the cursor.
//! - `example.todo`: ghost text finishing a `TODO` begun at the cursor,
//!   `Tab` taking it.
//! - `example.todo_board`: the `TODO`s in every file saved so far, in a tab
//!   of their own; Up and Down choose one, Enter opens its file there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use winter_ext_api::{
    open_picker, spawn, CompletionItem, CompletionRequest, Completions, Diagnostic, Event,
    Extension, Host, InlineAnswer, InlineEdit, InlineKind, InlineOffer, InlineProvider,
    InlineRequest, KeyName, KeyPress, Line, PickerEvent, PickerRow, PickerSpec, Registry,
    SettingValue, Severity, Styled, TextPosition, TextRange, Tone, View,
};

// ========================================================================
// Constants
// ========================================================================

/// The marker [`flag_todos`] looks for.
const TODO: &str = "TODO";

/// What a problem it publishes says.
const TODO_MESSAGE: &str = "work left to do";

/// The markers `example.markers` offers, with what each means.
pub const MARKERS: [(&str, &str); 3] = [
    ("FIXME", "broken, to mend"),
    ("NOTE", "worth knowing"),
    (TODO, TODO_MESSAGE),
];

/// The inline source finishing a `TODO`, and what it is called.
const TODO_SOURCE: &str = "todo";
const TODO_SOURCE_TITLE: &str = "Example";

/// How much of a `TODO` typed has the source finish it.
const TODO_BEGUN: &str = "TO";

/// The setting having `example.word_count` copy what it counts.
const COPY_COUNT: &str = "copy-count";

/// Where `example.open_guide` goes.
const GUIDE_URL: &str =
    "https://github.com/taquangtrung/winter-term/blob/main/docs/usage-guide.md#extensions";

/// What the `TODO` picker is titled, and its rows' actions.
const PICK_TITLE: &str = "TODOs in File";
const GO: &str = "go";
const FLAG: &str = "flag";

/// The board's heading, and what it says before anything is found.
const BOARD_TITLE: &str = "TODOs in saved files";
const BOARD_EMPTY: &str = "none yet: save a file holding a TODO";

/// The `TODO`s found in each file linted, kept by the linter and shown by
/// the board. A file with none is absent.
static FOUND: Mutex<BTreeMap<PathBuf, Vec<Todo>>> = Mutex::new(BTreeMap::new());

/// Whether the `copy-count` setting is on.
static COPIES_COUNT: AtomicBool = AtomicBool::new(false);

/// The file in front, whose count the `todos` segment says.
static FRONT: Mutex<Option<PathBuf>> = Mutex::new(None);

// ========================================================================
// Data Structures
// ========================================================================

/// The example extension.
pub struct Example;

#[cfg(feature = "wasm")]
winter_ext_guest::export!(Example);

/// The inline source finishing a `TODO` begun at the cursor.
struct FinishTodo;

/// One `TODO`: the line it is on, from zero, and that line's text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Todo {
    pub line: u32,
    pub text: String,
}

/// The board: every `TODO` found, by file, one of them chosen.
#[derive(Default)]
struct Board {
    /// The line of what was last drawn the chosen `TODO` is on.
    cursor: Option<usize>,
    /// Which `TODO` is chosen, counting across every file.
    selected: usize,
}

// ========================================================================
// Extension
// ========================================================================

impl Extension for Example {
    fn id(&self) -> &'static str {
        "example"
    }

    fn activate(&self, registry: &mut Registry) {
        registry.command(
            "example.word_count",
            "Example: Count Words in File",
            |host: &mut dyn Host| {
                let Some(document) = host.document() else {
                    host.notify("no file in front to count");
                    return;
                };
                let count = word_count(&document.text);
                if COPIES_COUNT.load(Ordering::Relaxed) {
                    host.copy_text(&count.to_string());
                }
                host.notify(&format!("{count} words in {}", name_of(&document.path)));
            },
        );
        registry.inline(TODO_SOURCE, TODO_SOURCE_TITLE, FinishTodo);
        registry.setting(
            COPY_COUNT,
            SettingValue::Switch(false),
            "Copy the count Example: Count Words in File says.",
        );
        registry.command(
            "example.open_guide",
            "Example: Open the Extension Guide",
            |host: &mut dyn Host| host.open_url(GUIDE_URL),
        );
        registry.command(
            "example.flag_todos",
            "Example: Flag TODOs as Problems",
            |host: &mut dyn Host| {
                let Some(document) = host.document() else {
                    host.notify("no file in front to look in");
                    return;
                };
                let found = flag_todos(&document.text);
                let said = format!("{} TODOs in {}", found.len(), name_of(&document.path));
                host.publish_diagnostics(&document.path, found);
                host.notify(&said);
            },
        );
        registry.command(
            "example.pick_todo",
            "Example: Pick a TODO in File",
            pick_todo,
        );
        registry.completion("markers", markers);
        registry.view("todo_board", "TODO Board", || Box::new(Board::default()));
        registry.command(
            "example.todo_board",
            "Example: Open TODO Board",
            |host: &mut dyn Host| host.open_view("todo_board"),
        );
        registry.status_segment("todos");
        // The linter: each file saved is read and looked through off
        // Winter's thread, and what is found published back on it, its
        // count in the segment.
        registry.on_event(|host, event| match event {
            Event::Configured(settings) => {
                let copies = settings.switch(COPY_COUNT).unwrap_or(false);
                COPIES_COUNT.store(copies, Ordering::Relaxed);
                host.offer_inline(TODO_SOURCE, InlineOffer::new(true, false));
            }
            // A file saved is read from disk.
            Event::Saved(path) => lint(host, path.clone(), |path| {
                std::fs::read_to_string(path).ok()
            }),
            // The file in front, as typed, once the typing pauses.
            Event::Changed(path) => {
                if let Some(document) = host.document().filter(|document| document.path == *path) {
                    lint(host, document.path, move |_| Some(document.text));
                }
            }
            Event::Focused(front) => {
                *FRONT.lock().unwrap_or_else(PoisonError::into_inner) = front.clone();
                host.set_status("todos", front.as_deref().and_then(todos_said));
            }
            _ => {}
        });
    }
}

// ========================================================================
// FinishTodo
// ========================================================================

impl InlineProvider for FinishTodo {
    fn ask(&self, host: &mut dyn Host, request: &InlineRequest) {
        let answer = match (request.kind, host.document()) {
            (InlineKind::Ghost, Some(document)) => finish_todo(&document.text, request.at),
            _ => InlineAnswer::Nothing,
        };
        host.answer_inline(request.ticket, answer);
    }
}

// ========================================================================
// Board
// ========================================================================

impl View for Board {
    fn render(&mut self, _rows: usize, _columns: usize) -> Vec<Line> {
        let found = FOUND.lock().unwrap_or_else(PoisonError::into_inner);
        let total = found.values().map(Vec::len).sum::<usize>();
        self.selected = self.selected.min(total.saturating_sub(1));
        let (lines, cursor) = board_lines(&found, self.selected);
        self.cursor = cursor;
        lines
    }

    fn on_key(&mut self, host: &mut dyn Host, key: &KeyPress) -> bool {
        match key.key {
            KeyName::Down => self.selected += 1,
            KeyName::Up => self.selected = self.selected.saturating_sub(1),
            KeyName::Enter => {
                let found = FOUND.lock().unwrap_or_else(PoisonError::into_inner);
                let chosen = found
                    .iter()
                    .flat_map(|(path, todos)| todos.iter().map(move |todo| (path, todo)))
                    .nth(self.selected);
                if let Some((path, todo)) = chosen {
                    host.open_file(path, Some(todo.line as usize + 1));
                }
            }
            _ => return false,
        }
        true
    }

    fn cursor_line(&self) -> Option<usize> {
        self.cursor
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Ghost text finishing a [`TODO`] whose [`TODO_BEGUN`] ends `text` at
/// `at`: the rest of the marker, put in at the cursor.
pub fn finish_todo(text: &str, at: TextPosition) -> InlineAnswer {
    let Some(line) = text.lines().nth(at.line as usize) else {
        return InlineAnswer::Nothing;
    };
    let before: Vec<u16> = line.encode_utf16().take(at.column as usize).collect();
    if !String::from_utf16_lossy(&before).ends_with(TODO_BEGUN) {
        return InlineAnswer::Nothing;
    }
    let range = TextRange::new(at, at);
    let rest = &TODO[TODO_BEGUN.len()..];
    InlineAnswer::Ghost(vec![InlineEdit::new(range, rest, TODO)])
}

/// Offer each `TODO` in the file in front in a picker: going to it, or
/// flagging it as a problem.
fn pick_todo(host: &mut dyn Host) {
    let Some(document) = host.document() else {
        host.notify("no file in front to look in");
        return;
    };
    let listed = todos(&document.text);
    let rows = listed
        .iter()
        .map(|todo| {
            PickerRow::new(todo.text.clone())
                .with_description(format!("line {}", todo.line + 1))
                .with_action(GO, "Go to It")
                .with_action(FLAG, "Flag as a Problem")
        })
        .collect();
    let path = document.path;
    let flagged = flag_todos(&document.text);
    let spec = PickerSpec::new(PICK_TITLE).with_rows(rows);
    open_picker(
        host,
        spec,
        Box::new(move |host, _, event| {
            let PickerEvent::Chosen(choice) = event else {
                return;
            };
            let Some(todo) = listed.get(choice.row) else {
                return;
            };
            match choice.action.as_str() {
                FLAG => {
                    let on_line = flagged.iter().filter(|found| found.line == todo.line);
                    host.publish_diagnostics(&path, on_line.cloned().collect());
                }
                _ => host.open_file(&path, Some(todo.line as usize + 1)),
            }
        }),
    );
}

/// Look through the text `read` gives of `path` off Winter's thread, then
/// publish what is found back on it: as problems, on the board, and in the
/// segment where the file is in front.
fn lint(
    host: &mut dyn Host,
    path: PathBuf,
    read: impl FnOnce(&Path) -> Option<String> + Send + 'static,
) {
    spawn(
        host,
        move || {
            let text = read(&path)?;
            Some((path, flag_todos(&text), todos(&text)))
        },
        |host, linted| {
            let Some((path, found, listed)) = linted else {
                return;
            };
            host.publish_diagnostics(&path, found);
            record(path.clone(), listed);
            let front = FRONT.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if front.as_deref() == Some(path.as_path()) {
                host.set_status("todos", todos_said(&path));
            }
        },
    );
}

/// What the `todos` segment says of the file at `path`: how many `TODO`s
/// it holds, or nothing for none.
fn todos_said(path: &Path) -> Option<String> {
    let found = FOUND.lock().unwrap_or_else(PoisonError::into_inner);
    let count = found.get(path).map_or(0, Vec::len);
    (count > 0).then(|| format!("{count} TODOs in {}", name_of(path)))
}

/// Keep `listed` as the `TODO`s in the file at `path`, for the board.
fn record(path: PathBuf, listed: Vec<Todo>) {
    let mut found = FOUND.lock().unwrap_or_else(PoisonError::into_inner);
    match listed.is_empty() {
        true => found.remove(&path),
        false => found.insert(path, listed),
    };
}

/// The board drawn for `found`, the `selected`th `TODO` across them chosen,
/// and the line it is drawn on.
pub fn board_lines(
    found: &BTreeMap<PathBuf, Vec<Todo>>,
    selected: usize,
) -> (Vec<Line>, Option<usize>) {
    let run = |tone, text: String| vec![Styled::new(tone, text)];
    let mut lines = vec![run(Tone::Heading, BOARD_TITLE.to_string()), Vec::new()];
    if found.is_empty() {
        lines.push(run(Tone::Dim, BOARD_EMPTY.to_string()));
        return (lines, None);
    }
    let mut cursor = None;
    let mut at = 0;
    for (path, todos) in found {
        lines.push(run(Tone::Accent, path.display().to_string()));
        for todo in todos {
            if at == selected {
                cursor = Some(lines.len());
            }
            lines.push(vec![
                Styled::new(Tone::Dim, format!("{:>6}  ", todo.line + 1)),
                Styled::new(Tone::Normal, todo.text.clone()),
            ]);
            at += 1;
        }
    }
    (lines, cursor)
}

/// Each line of `text` holding a [`TODO`], trimmed.
pub fn todos(text: &str) -> Vec<Todo> {
    text.lines()
        .enumerate()
        .filter(|(_, content)| content.contains(TODO))
        .map(|(line, content)| Todo {
            line: line as u32,
            text: content.trim().to_string(),
        })
        .collect()
}

/// How many words `text` holds: its runs of anything but whitespace.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Each [`TODO`] in `text`, as a problem covering it.
pub fn flag_todos(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .enumerate()
        .flat_map(|(line, content)| {
            content.match_indices(TODO).map(move |(byte, _)| {
                // Columns count in UTF-16 units, as the API asks.
                let start = content[..byte].encode_utf16().count() as u32;
                let end = start + TODO.len() as u32;
                Diagnostic::new(Severity::Info, TODO_MESSAGE, line as u32, start..end)
            })
        })
        .collect()
}

/// [`MARKERS`] for the word of capitals before the cursor `request` is
/// at: the popup narrows them to what is typed.
pub fn markers(request: &CompletionRequest) -> Option<Completions> {
    let before: Vec<u16> = request
        .line
        .encode_utf16()
        .take(request.column as usize)
        .collect();
    // A marker is capitals, all ASCII, so each is one unit.
    let typed = before
        .iter()
        .rev()
        .take_while(|unit| u8::try_from(**unit).is_ok_and(|byte| byte.is_ascii_uppercase()))
        .count();
    if typed == 0 {
        return None;
    }
    let items = MARKERS
        .iter()
        .map(|(marker, meaning)| CompletionItem::new(*marker, *marker).with_detail(*meaning))
        .collect();
    Some(Completions::new((before.len() - typed) as u32, items))
}

/// The file's own name, which is how a notice names it.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_words_are_runs_of_anything_but_whitespace() {
        assert_eq!(word_count("one  two\nthree\t— four"), 5);
        assert_eq!(word_count(" \n "), 0);
    }

    #[test]
    fn test_markers_are_offered_from_where_the_capitals_before_the_cursor_start() {
        let request =
            |line, column| CompletionRequest::new(Path::new("/a.rs"), "rust", line, (0, column));
        let offered = markers(&request("// é FI", 7)).expect("capitals typed");
        assert_eq!(offered.start, 5, "past the é, one unit");
        assert_eq!(offered.items.len(), MARKERS.len());
        assert!(
            markers(&request("// fi", 5)).is_none(),
            "no capitals, no markers"
        );
    }

    #[test]
    fn test_the_board_lists_each_file_s_todos_and_bands_the_chosen_one() {
        let found = BTreeMap::from([
            (PathBuf::from("/a.rs"), todos("x\n  // TODO: one\n")),
            (PathBuf::from("/b.rs"), todos("// TODO: two")),
        ]);
        let (lines, cursor) = board_lines(&found, 1);
        let text: Vec<String> = lines
            .iter()
            .map(|line| line.iter().map(|run| run.text.as_str()).collect())
            .collect();
        assert_eq!(
            text,
            [
                BOARD_TITLE,
                "",
                "/a.rs",
                "     2  // TODO: one",
                "/b.rs",
                "     1  // TODO: two"
            ]
        );
        assert_eq!(cursor, Some(5), "the second TODO, under the second file");
        let (empty, none) = board_lines(&BTreeMap::new(), 0);
        assert_eq!(empty[2][0].text, BOARD_EMPTY);
        assert_eq!(none, None);
    }

    #[test]
    fn test_each_todo_is_a_problem_where_it_stands() {
        let found = flag_todos("fine\n// TODO: this, TODO: that\né TODO\n");
        let places: Vec<(u32, u32, u32)> = found.iter().map(|d| (d.line, d.start, d.end)).collect();
        assert_eq!(
            places,
            [(1, 3, 7), (1, 15, 19), (2, 2, 6)],
            "columns past an é count it once"
        );
    }
}
