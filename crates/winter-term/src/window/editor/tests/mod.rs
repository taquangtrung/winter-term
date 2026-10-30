//! Tests for editor tabs: opening, closing and reopening, prompts, and keys.

use std::cell::RefCell;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crate::model::button::ButtonGroup;

use crate::model::editor::{
    CommandRequest, EditorOutcome, GitViewRequest, JobRequest, Notice, PromptMode, PromptReply, PromptRequest,
};

use crate::model::input::{Key, KeyCode};
use crate::model::layout::Direction;
use crate::model::mode::Mode;
use crate::model::paint::{Frame, IconKind, Span};
use crate::window::runtime::jobs::STOP_TASKS_KEY;
use crate::window::SPLIT_RATIO;

use super::prompt::input_dialog;
use super::*;

mod buffer_views;
mod builds;
mod git_views;
mod keyboard;
mod opening;
mod questions;

/// Far longer than an editor's tick takes to come due.
const TICK_WAIT: Duration = Duration::from_millis(500);

/// The longest a test loop sleeps between two passes.
const TICK_STEP: Duration = Duration::from_millis(5);

/// An editor that counts the keys it claims, so a test can tell a key the
/// editor spent from one it handed back.
#[derive(Default)]
struct CountingEditor {
    claimed: usize,
}

impl Editor for CountingEditor {
    /// The count is in the title so a test can tell one instance from
    /// another: an editor that comes back holding what it held is not the
    /// same thing as a fresh editor of its kind.
    fn title(&self) -> String {
        format!("Counting {}", self.claimed)
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("counting")]])
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        match key.glyph() {
            KeyCode::Char('x') => {
                self.claimed += 1;
                EditorOutcome::Consumed
            }
            // The chord the Git editor commits on, which the window also
            // binds to copy.
            // Matched on the lowercase letter alone: the window hands over
            // the capital, and the editor must not have to know.
            KeyCode::Char('c') if key.ctrl && key.shift => {
                self.claimed += 1;
                EditorOutcome::Consumed
            }
            KeyCode::Char('r') => EditorOutcome::RunAction("new_tab".to_string()),
            KeyCode::Char('q') => EditorOutcome::Close,
            _ => EditorOutcome::Ignored,
        }
    }
}

/// An editor that asks to be ticked `after` from whenever it is asked, as a
/// editor polling a background read does, and counts the ticks it gets.
/// `busy` is whether it has work under way, which a buried buffer is still
/// ticked for.
struct PollingEditor {
    after: Duration,
    busy: bool,
    ticks: Rc<RefCell<usize>>,
}

impl Editor for PollingEditor {
    fn title(&self) -> String {
        "Polling".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("polling")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn next_tick(&self) -> Option<Instant> {
        Some(Instant::now() + self.after)
    }

    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        *self.ticks.borrow_mut() += 1;
        EditorOutcome::Consumed
    }

    fn has_work_in_flight(&self) -> bool {
        self.busy
    }
}

/// An editor with one report queued for the status bar.
struct ReportingEditor(Option<Notice>);

impl Editor for ReportingEditor {
    fn title(&self) -> String {
        "Reporting".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("reporting")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.0.take()
    }
}

/// An editor holding edits that are on no disk, which is the one thing
/// closing a pane cannot give back on its own.
struct UnsavedEditor(&'static str);

impl Editor for UnsavedEditor {
    fn title(&self) -> String {
        self.0.to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("unsaved")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn is_dirty(&self) -> bool {
        true
    }
}

/// An editor that reports `dir` as the place it is looking at.
struct LocatedEditor(PathBuf);

impl Editor for LocatedEditor {
    fn title(&self) -> String {
        "Located".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("located")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn cwd(&self) -> Option<PathBuf> {
        Some(self.0.clone())
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

fn app_with_open_editor() -> (App, TabId) {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    (app, tab)
}

/// Put `claimed` keys into the editor covering `pane`, which is what makes
/// one open editor tell itself apart from another.
fn tally(app: &mut App, tab: TabId, claimed: usize) {
    for _ in 0..claimed {
        if let Some(slot) = app.open_tabs.editor_mut(&tab) {
            slot.editor.on_key(&press(KeyCode::Char('x')));
        }
    }
}

/// What the focused pane's editor calls itself, or nothing when no editor
/// covers it.
fn showing(app: &App) -> Option<String> {
    app.open_tabs
        .editor(&app.layout().focused())
        .map(|slot| slot.editor.title())
}

/// An editor a program is blocked on, as `git commit` is on its message,
/// holding a message not yet written: closing its tab lets it go, so asks.
struct WaitedEditor;

impl Editor for WaitedEditor {
    fn title(&self) -> String {
        "COMMIT_EDITMSG".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("message")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn is_dirty(&self) -> bool {
        true
    }

    fn is_waited_on(&self) -> bool {
        true
    }
}

/// The actions of the open palette's rows, in order.
fn row_actions(app: &App) -> Vec<String> {
    app.chrome.picker()
        .expect("a list is open")
        .entries
        .iter()
        .map(|entry| entry.action.clone())
        .collect()
}

/// A window with a buffer that polls at once, ticked for `TICK_WAIT` after
/// `before` has had its way with it: the window, the buffer's tab, and the
/// ticks the buffer got.
fn ticks_after(busy: bool, before: impl FnOnce(&mut App, TabId)) -> (App, TabId, Rc<RefCell<usize>>) {
    let ticks = Rc::new(RefCell::new(0));
    let mut app = App::new();
    let editor = PollingEditor {
        after: Duration::ZERO,
        busy,
        ticks: Rc::clone(&ticks),
    };
    let tab = app.show_editor(EditorKind::Proc, Box::new(editor));
    before(&mut app, tab);
    let end = Instant::now() + TICK_WAIT;
    while Instant::now() < end {
        app.tick_editors(Instant::now());
        std::thread::sleep(TICK_STEP);
    }
    (app, tab, ticks)
}

/// An editor that notes what the window tells it about being shown.
struct ShownEditor(Rc<RefCell<Vec<&'static str>>>);

impl Editor for ShownEditor {
    fn title(&self) -> String {
        "Shown".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("shown")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn set_shown(&mut self, shown: bool) {
        self.0.borrow_mut().push(if shown { "shown" } else { "hidden" });
    }

    fn shed_caches(&mut self) {
        self.0.borrow_mut().push("shed");
    }
}

/// An editor whose view state is a cursor row, which `j` moves down.
#[derive(Default)]
struct ViewedEditor {
    row: usize,
}

impl Editor for ViewedEditor {
    fn title(&self) -> String {
        format!("Viewed {}", self.row)
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("viewed")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        self.row += 1;
        EditorOutcome::Consumed
    }

    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(self.row))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(row) = state.downcast::<usize>() {
            self.row = *row;
        }
    }
}

/// An app with an editor in one pane and a second, empty pane focused.
fn editor_and_another_pane(editor: Box<dyn Editor>) -> (App, TabId, TabId) {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Grep, editor);
    let sibling = app.alloc_tab_id();
    app.layout_mut()
        .split(Direction::Vertical, SPLIT_RATIO, sibling);
    app.layout_mut().focus(sibling);
    (app, tab, sibling)
}

/// What the editor of tab `tab` calls itself, with that tab's view loaded.
fn title_in(app: &mut App, tab: TabId) -> String {
    app.open_tabs.editor_mut(&tab).expect("an editor").editor.title()
}

/// A directory holding `names`, removed again when the test ends.
struct TempTree(PathBuf);

impl TempTree {
    fn new(tag: &str, names: &[&str]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("winter-browse-page-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        for name in names {
            match name.ends_with('/') {
                true => std::fs::create_dir_all(dir.join(name.trim_end_matches('/'))),
                false => std::fs::write(dir.join(name), "hello\n"),
            }
            .expect("temp entry");
        }
        Self(dir)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One key into the palette, the way the window loop delivers it.
fn browse_key(app: &mut App, named: winit::keyboard::NamedKey) {
    use winit::keyboard::{Key, PhysicalKey};

    let tab = app.layout().focused();
    let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    app.press_palette_key(&Key::Named(named), &phys, tab);
}

/// Put the selection on the row labelled `label`.
fn browse_to(app: &mut App, label: &str) {
    let palette = app.chrome.picker_mut().expect("the browser is up");
    let at = palette
        .filtered
        .iter()
        .position(|&i| palette.entries[i].label == label)
        .unwrap_or_else(|| panic!("no row labelled {label}"));
    palette.selected = at;
}

/// A terminal tab over `cat`, with a full-screen program on it when
/// `busy`, standing in for an agent or an editor that owns the pane.
fn terminal_tab(app: &mut App, busy: bool) -> TabId {
    let id = app.layout().focused();
    let mut terminal = crate::terminal::session::Terminal::with_command(
        20,
        4,
        portable_pty::CommandBuilder::new("cat"),
        winter_core::MAX_SCROLLBACK,
    )
    .expect("test pane spawn");
    if busy {
        terminal.feed_program_output(b"\x1b[?1049h");
    }
    app.open_tabs.insert_terminal(id, terminal);
    id
}

/// An app with two panes, focus on the first, and the second's id.
fn two_panes() -> (App, TabId, TabId) {
    let mut app = App::new();
    let first = app.layout().focused();
    let second = app.alloc_tab_id();
    app.layout_mut()
        .split(Direction::Vertical, SPLIT_RATIO, second);
    app.layout_mut().focus(first);
    (app, first, second)
}

/// An editor that asks one question, sharing the answer it hears with the
/// test that opened it.
struct AskingEditor {
    heard: Rc<RefCell<Vec<Option<String>>>>,
}

impl Editor for AskingEditor {
    fn title(&self) -> String {
        "Asking".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("asking")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: "seed".to_string(),
            label: "Name: ".to_string(),
            mode: PromptMode::Text,
            tag: "ask",
        })
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.heard.borrow_mut().push(reply.answer);
        EditorOutcome::Consumed
    }
}

fn app_asking() -> (App, TabId, Rc<RefCell<Vec<Option<String>>>>) {
    let mut app = App::new();
    let heard: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
    let tab = app.show_editor(
        EditorKind::Grep,
        Box::new(AskingEditor {
            heard: Rc::clone(&heard),
        }),
    );
    app.offer_key_to_editor(tab, &press(KeyCode::Char('x')));
    (app, tab, heard)
}

/// Type `text` into the open question and answer it with Enter.
fn answer(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_prompt_key(&press(KeyCode::Char(c)));
    }
    app.handle_prompt_key(&press(KeyCode::Enter));
}

/// A scratch LaTeX project: a root declaring its class, and a task list
/// whose first build prints an error at the root's line 2.
#[cfg(unix)]
fn latex_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("winter-compile-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".winter")).expect("temp dir");
    std::fs::write(dir.join("main.tex"), "\\documentclass{article}\nx\n").expect("root");
    std::fs::write(
        dir.join(".winter/tasks.kdl"),
        "task \"check\" \"printf 'main.tex:2: error: x\\\\n'\"\n",
    )
    .expect("task list");
    dir
}

/// The Compile tabs open.
fn compile_tabs(app: &App) -> Vec<TabId> {
    app.open_tabs
        .editors()
        .filter(|(_, slot)| slot.kind() == EditorKind::Compile)
        .map(|(tab, _)| *tab)
        .collect()
}

/// Tick the window until no editor wants another tick soon.
#[cfg(unix)]
fn tick_until_idle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        let busy = app.open_tabs.editors().any(|(_, slot)| {
            slot.editor.next_tick().is_some() && slot.kind() == EditorKind::Compile
        });
        if !busy {
            return;
        }
        app.tick_editors(Instant::now() + TICK_WAIT);
        std::thread::sleep(TICK_STEP);
    }
    panic!("the build did not end");
}

/// A scratch project whose task list names `tasks`, `(name, command)`.
#[cfg(unix)]
fn task_project(name: &str, tasks: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("winter-tasks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".winter")).expect("temp dir");
    let list: String = tasks
        .iter()
        .map(|(name, command)| format!("task \"{name}\" \"{command}\"\n"))
        .collect();
    std::fs::write(dir.join(".winter/tasks.kdl"), list).expect("task list");
    dir
}

/// Choose the open palette's entry at `at`, as `Enter` on it would.
#[cfg(unix)]
fn choose_palette_entry(app: &mut App, at: usize) {
    let mut palette = app.chrome.picker().cloned().expect("a palette is open");
    palette.selected = at;
    app.dismiss_overlay();
    let focused = app.layout().focused();
    app.confirm_palette_selection(&palette, None, focused);
}

/// An editor that notes the rows a key over a selection was pressed on.
struct SelectingEditor(Rc<RefCell<Vec<RangeInclusive<usize>>>>);

impl Editor for SelectingEditor {
    fn title(&self) -> String {
        "Selecting".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("a row")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn on_selected_rows_key(
        &mut self,
        key: &Key,
        rows: RangeInclusive<usize>,
    ) -> Option<EditorOutcome> {
        (key.code == KeyCode::Char('s')).then(|| {
            self.0.borrow_mut().push(rows);
            EditorOutcome::Consumed
        })
    }
}

/// An app with the window focused, showing a text editor on a file holding
/// "one\n" in a fresh directory named for `name`, beside a second text
/// editor in a split, the first holding the keyboard. The first's tab, the
/// second's, and the file.
fn app_editing_beside_another(name: &str) -> (App, TabId, TabId, PathBuf) {
    let dir = std::env::temp_dir().join(format!("winter-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "one\n").expect("file");
    let other = dir.join("other.txt");
    std::fs::write(&other, "other\n").expect("file");
    let mut app = App::new();
    app.input.focus.window = true;
    let editing = app.show_editor(
        EditorKind::Text,
        Box::new(crate::editors::text::TextEditor::new(file.clone(), None).expect("opens")),
    );
    let beside = app.show_editor_split(
        EditorKind::Text,
        Box::new(crate::editors::text::TextEditor::new(other, None).expect("opens")),
    );
    app.set_focus(editing);
    // Typing into the file: into Insert mode, then a word.
    let slot = app.open_tabs.editor_mut(&editing).expect("an editor");
    for code in [KeyCode::Char('i'), KeyCode::Char('z')] {
        slot.editor.on_key(&press(code));
    }
    (app, editing, beside, file)
}

/// The mode tab `tab`'s editor is in.
fn mode_of(app: &mut App, tab: TabId) -> Option<crate::model::textedit::modal::EditMode> {
    app.open_tabs.editor_mut(&tab).expect("an editor").editor.status().mode
}

/// What a [`DeferringEditor`] is waiting on, shared with the test: its
/// deadline, and the one it last called off.
#[derive(Default)]
struct Deadline {
    cancelled: Option<Instant>,
    due: Option<Instant>,
}

/// An editor waiting on one deadline, which the test puts off, calling the
/// old one off or not, as a timer reset while typing goes on is.
struct DeferringEditor(Rc<RefCell<Deadline>>);

impl Editor for DeferringEditor {
    fn title(&self) -> String {
        "Deferring".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("deferring")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn next_tick(&self) -> Option<Instant> {
        self.0.borrow().due
    }

    fn take_cancelled_tick(&mut self) -> Option<Instant> {
        self.0.borrow_mut().cancelled.take()
    }
}

/// The deadline the window waits on once an editor's deadline is put off by
/// a second, the old one called off first when `cancel`, and the deadline
/// first waited on and the one put off to.
fn waited_on_after_deferring(cancel: bool) -> (Option<Instant>, Instant, Instant) {
    let first = Instant::now() + Duration::from_secs(1);
    let shared = Rc::new(RefCell::new(Deadline {
        cancelled: None,
        due: Some(first),
    }));
    let mut app = App::new();
    app.show_editor(EditorKind::Grep, Box::new(DeferringEditor(Rc::clone(&shared))));
    let now = Instant::now();
    assert_eq!(app.tick_editors(now), Some(first));
    let later = first + Duration::from_secs(1);
    {
        let mut deadline = shared.borrow_mut();
        deadline.due = Some(later);
        if cancel {
            deadline.cancelled = Some(first);
        }
    }
    (app.tick_editors(now), first, later)
}
