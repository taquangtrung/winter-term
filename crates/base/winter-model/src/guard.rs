//! The guard every editor runs inside, so a bug in one editor stops that
//! editor and not the window. Each call the window makes into an editor runs
//! under [`std::panic::catch_unwind`]; when one panics, the editor is set
//! aside for good, a stopped editor takes its place in the tab, saying what
//! went wrong, and the failure is told once as an error notification. The
//! window, its terminals, and every other tab go on.
//!
//! An editor that panicked mid-call may be left half-changed, so it is never
//! called again: closing the tab and opening it anew starts a fresh one.

use std::any::Any;
use std::cell::{OnceCell, RefCell};
use std::ops::RangeInclusive;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, Instant};

use super::editor::{
    Appearance, Editor, EditorHint, EditorMenuItem, EditorOutcome, EditorRequest, EditorStatus,
    JobReply, JumpStep, JumpTarget, Notice, OpenTarget, Page, Popups, PromptReply, ScrollPosition,
    SearchReport, TargetStyles, TokenInspectData, Toolbar,
};
use super::input::table::KeyHelp;
use super::input::Key;
use super::paint::{
    Caret, ColumnName, DiagnosticLevel, Frame, GutterLane, IconKind, Point, Span, Style,
};
use super::picture::{Picture, Tile};

// The guard catches panics as they unwind; built to abort instead, it
// would catch none and every editor's panic would take the window down.
#[cfg(panic = "abort")]
compile_error!("the editor guard needs panics to unwind: keep `panic = \"unwind\"`");

// ========================================================================
// Constants
// ========================================================================

/// What a stopped editor's tab is called after the editor's own title.
const STOPPED_SUFFIX: &str = "(stopped)";

/// The title an editor is given when even its title cannot be read.
const UNNAMED: &str = "Editor";

/// What a panic is said to be when it carried no message.
const NO_MESSAGE: &str = "no message";

// ========================================================================
// Data Structures
// ========================================================================

/// A page or an editor run under the guard: itself until it panics, then
/// the stopped one that took its place.
pub struct Guarded<H: Held> {
    inner: H,
    /// Set once `inner` panics; from then on every call goes here.
    stopped: OnceCell<Stopped>,
}

/// What a page or an editor is held as under the guard: a box of either,
/// each seen as the page it is.
pub trait Held: 'static {
    /// What is held, as a page.
    fn page(&self) -> &(dyn Page + 'static);
    /// The same, to change.
    fn page_mut(&mut self) -> &mut (dyn Page + 'static);

    /// The guard holding this, as the editor it guards: `None` for a page.
    fn guarded_editor(guarded: &Guarded<Self>) -> Option<&dyn Editor>
    where
        Self: Sized;

    /// The same, to change.
    fn guarded_editor_mut(guarded: &mut Guarded<Self>) -> Option<&mut dyn Editor>
    where
        Self: Sized;
}

/// What a tab shows in place of an editor that panicked: its title and
/// icon, and what went wrong.
struct Stopped {
    icon: Option<IconKind>,
    /// Where the panic was raised, as `file:line`, when it was known.
    location: Option<String>,
    message: String,
    /// The failure, told once as an error notification.
    notice: Option<Notice>,
    title: String,
}

// ========================================================================
// Guarded
// ========================================================================

impl<H: Held> Guarded<H> {
    /// `run` on the editor itself, for a caller that knows its kind, under
    /// the guard as every other call is: a panic stops the editor. `None`
    /// once it is stopped, so nothing reaches the broken one.
    pub(crate) fn with_inner<R>(&self, run: impl FnOnce(&dyn Page) -> Option<R>) -> Option<R> {
        if self.stopped.get().is_some() {
            return None;
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run(self.inner.page()))) {
            Ok(value) => value,
            Err(payload) => {
                self.stop(payload.as_ref());
                None
            }
        }
    }

    /// The same, to change.
    pub(crate) fn with_inner_mut<R>(
        &mut self,
        run: impl FnOnce(&mut dyn Page) -> Option<R>,
    ) -> Option<R> {
        if self.stopped.get().is_some() {
            return None;
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run(self.inner.page_mut()))) {
            Ok(value) => value,
            Err(payload) => {
                self.stop(payload.as_ref());
                None
            }
        }
    }

    /// The editor the guard runs, for a test to look inside: the stopped
    /// editor once it panicked.
    #[cfg(any(test, feature = "test-support"))]
    pub fn inner(&self) -> &(dyn Page + 'static) {
        match self.stopped.get() {
            Some(stopped) => stopped,
            None => self.inner.page(),
        }
    }

    /// The same, to change.
    #[cfg(any(test, feature = "test-support"))]
    pub fn inner_mut(&mut self) -> &mut (dyn Page + 'static) {
        match self.stopped.get_mut() {
            Some(stopped) => stopped,
            None => self.inner.page_mut(),
        }
    }

    /// The failure, the first time it is asked for after the editor panicked:
    /// for the window to tell at once, rather than at the next key the tab
    /// hears.
    pub fn take_failure(&mut self) -> Option<Notice> {
        self.stopped.get_mut()?.notice.take()
    }

    /// `run` on the editor, or on the stopped editor once it panicked: it
    /// panicking now stops it, and `run` is answered by the stopped editor.
    fn read<R>(&self, run: impl Fn(&dyn Page) -> R) -> R {
        if let Some(stopped) = self.stopped.get() {
            return run(stopped);
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run(self.inner.page()))) {
            Ok(value) => value,
            Err(payload) => run(self.stop(payload.as_ref())),
        }
    }

    /// [`Self::read`], for a call that changes the editor.
    fn write<R>(&mut self, mut run: impl FnMut(&mut dyn Page) -> R) -> R {
        if let Some(stopped) = self.stopped.get_mut() {
            return run(stopped);
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run(self.inner.page_mut()))) {
            Ok(value) => value,
            Err(payload) => {
                self.stop(payload.as_ref());
                match self.stopped.get_mut() {
                    Some(stopped) => run(stopped),
                    None => unreachable!("stopping sets the stopped editor"),
                }
            }
        }
    }

    /// [`Self::write`], for a call handing the editor something it takes:
    /// `fallback` answers it if the editor panics, the thing being gone.
    fn write_once<R>(&mut self, run: impl FnOnce(&mut dyn Page) -> R, fallback: R) -> R {
        if let Some(stopped) = self.stopped.get_mut() {
            return run(stopped);
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run(self.inner.page_mut()))) {
            Ok(value) => value,
            Err(payload) => {
                self.stop(payload.as_ref());
                fallback
            }
        }
    }

    /// Set the editor aside after it panicked with `payload`, and the
    /// stopped editor that takes its place. A panic seen first by another
    /// call keeps the stopped editor that call made.
    fn stop(&self, payload: &(dyn Any + Send)) -> &Stopped {
        self.stopped
            .get_or_init(|| Stopped::new(self.inner.page(), payload))
    }
}

impl Guarded<Box<dyn Editor>> {
    /// [`Self::read`], for an editor's own call.
    fn read_editor<R>(&self, run: impl Fn(&dyn Editor) -> R) -> R {
        if let Some(stopped) = self.stopped.get() {
            return run(stopped);
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run(self.inner.as_ref()))) {
            Ok(value) => value,
            Err(payload) => run(self.stop(payload.as_ref())),
        }
    }

    /// [`Self::write`], for an editor's own call.
    fn write_editor<R>(&mut self, mut run: impl FnMut(&mut dyn Editor) -> R) -> R {
        if let Some(stopped) = self.stopped.get_mut() {
            return run(stopped);
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run(self.inner.as_mut()))) {
            Ok(value) => value,
            Err(payload) => {
                self.stop(payload.as_ref());
                match self.stopped.get_mut() {
                    Some(stopped) => run(stopped),
                    None => unreachable!("stopping sets the stopped editor"),
                }
            }
        }
    }

    /// [`Self::write_once`], for an editor's own call.
    fn write_editor_once<R>(&mut self, run: impl FnOnce(&mut dyn Editor) -> R, fallback: R) -> R {
        if let Some(stopped) = self.stopped.get_mut() {
            return run(stopped);
        }
        match panic::catch_unwind(AssertUnwindSafe(|| run(self.inner.as_mut()))) {
            Ok(value) => value,
            Err(payload) => {
                self.stop(payload.as_ref());
                fallback
            }
        }
    }
}

impl Held for Box<dyn Page> {
    fn page(&self) -> &(dyn Page + 'static) {
        self.as_ref()
    }

    fn page_mut(&mut self) -> &mut (dyn Page + 'static) {
        self.as_mut()
    }

    fn guarded_editor(_guarded: &Guarded<Self>) -> Option<&dyn Editor> {
        None
    }

    fn guarded_editor_mut(_guarded: &mut Guarded<Self>) -> Option<&mut dyn Editor> {
        None
    }
}

impl Held for Box<dyn Editor> {
    fn page(&self) -> &(dyn Page + 'static) {
        self.as_ref()
    }

    fn page_mut(&mut self) -> &mut (dyn Page + 'static) {
        self.as_mut()
    }

    // The guard, not the editor in it: an editor's own calls are guarded
    // as every other is.
    fn guarded_editor(guarded: &Guarded<Self>) -> Option<&dyn Editor> {
        Some(guarded)
    }

    fn guarded_editor_mut(guarded: &mut Guarded<Self>) -> Option<&mut dyn Editor> {
        Some(guarded)
    }
}

impl<H: Held> Page for Guarded<H> {
    fn as_editor(&self) -> Option<&dyn Editor> {
        H::guarded_editor(self)
    }

    fn as_editor_mut(&mut self) -> Option<&mut dyn Editor> {
        H::guarded_editor_mut(self)
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.read(|page| page.cwd())
    }

    fn title(&self) -> String {
        self.read(|editor| editor.title())
    }

    fn icon(&self) -> Option<IconKind> {
        self.read(|editor| editor.icon())
    }

    fn picks_rows(&self) -> bool {
        self.read(|editor| editor.picks_rows())
    }

    fn set_neighbors(&mut self, dirs: Vec<PathBuf>) {
        self.write(|editor| editor.set_neighbors(dirs.clone()))
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        self.write(|editor| editor.content(rows, cols, wrap))
    }

    fn gutter_lanes(&self) -> Vec<GutterLane> {
        self.read(|editor| editor.gutter_lanes())
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.write(|editor| editor.on_key(key))
    }

    fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
        self.write(|editor| editor.run_command(name))
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        self.write(|editor| editor.on_mouse(at))
    }

    fn on_double_click(&mut self, at: Point) -> EditorOutcome {
        self.write(|editor| editor.on_double_click(at))
    }

    fn scrolls_smoothly(&self) -> bool {
        self.read(|editor| editor.scrolls_smoothly())
    }

    fn on_scroll_pixels(&mut self, pixels: f32) -> EditorOutcome {
        self.write(|editor| editor.on_scroll_pixels(pixels))
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.write(|editor| editor.on_scroll(lines))
    }

    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.read(|editor| editor.scroll_position())
    }

    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        self.write(|editor| editor.scroll_to(offset))
    }

    fn set_scroll_margin(&mut self, rows: usize) {
        self.write(|editor| editor.set_scroll_margin(rows))
    }

    fn on_side_button(&mut self, back: bool) -> EditorOutcome {
        self.write(|editor| editor.on_side_button(back))
    }

    fn on_hover(&mut self, at: Option<Point>) -> EditorOutcome {
        self.write(|editor| editor.on_hover(at))
    }

    fn on_paste(&mut self, text: String) -> EditorOutcome {
        self.write_once(|editor| editor.on_paste(text), EditorOutcome::Consumed)
    }

    fn hint(&self) -> Option<EditorHint> {
        self.read(|editor| editor.hint())
    }

    fn transient(&self) -> Option<crate::view::TransientView> {
        self.read(|editor| editor.transient())
    }

    fn select_transient_row(&mut self, row: usize) {
        self.write(|editor| editor.select_transient_row(row))
    }

    fn keys(&self) -> Vec<KeyHelp> {
        self.read(|editor| editor.keys())
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.write_once(|editor| editor.on_prompt(reply), EditorOutcome::Consumed)
    }

    fn on_notice_action(&mut self, id: &'static str) -> EditorOutcome {
        self.write_once(
            |editor| editor.on_notice_action(id),
            EditorOutcome::Consumed,
        )
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        self.write(|editor| editor.on_prompt_input(query, tag))
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        self.write_once(|editor| editor.on_job(reply), EditorOutcome::Consumed)
    }

    fn next_tick(&self) -> Option<Instant> {
        self.read(|editor| editor.next_tick())
    }

    fn take_cancelled_tick(&mut self) -> Option<Instant> {
        self.write(|editor| editor.take_cancelled_tick())
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        self.write(|editor| editor.on_tick(now))
    }

    fn take_extension_calls(&mut self) -> Vec<(&'static str, winter_ext_api::Reply)> {
        self.write(|editor| editor.take_extension_calls())
    }

    fn take_requests(&mut self) -> Vec<EditorRequest> {
        self.write(|editor| editor.take_requests())
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.write(|editor| editor.take_notice())
    }

    fn word_at_cursor(&self) -> Option<String> {
        self.read(|editor| editor.word_at_cursor())
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.write(|editor| editor.take_search())
    }

    fn status(&self) -> EditorStatus {
        self.read(|editor| editor.status())
    }

    fn caret(&self) -> Option<Caret> {
        self.read(|editor| editor.caret())
    }

    fn picture(&self) -> Option<Picture> {
        self.read(|editor| editor.picture())
    }

    fn set_picture_geometry(&mut self, natural: (f32, f32), pane: (f32, f32)) {
        self.write(|editor| editor.set_picture_geometry(natural, pane));
    }

    fn tiles(&self) -> Vec<Tile> {
        self.read(|editor| editor.tiles())
    }

    fn header_rows(&self) -> usize {
        self.read(|editor| editor.header_rows())
    }

    fn set_appearance(&mut self, appearance: &Appearance) {
        self.write(|editor| editor.set_appearance(appearance));
    }

    fn set_pane_size(&mut self, pane: (f32, f32)) {
        self.write(|editor| editor.set_pane_size(pane));
    }

    fn is_busy(&self) -> bool {
        self.read(|editor| editor.is_busy())
    }

    fn on_selected_rows_key(
        &mut self,
        key: &Key,
        rows: RangeInclusive<usize>,
    ) -> Option<EditorOutcome> {
        self.write(|editor| editor.on_selected_rows_key(key, rows.clone()))
    }

    fn token_inspect(&mut self) -> Option<TokenInspectData> {
        self.write(|editor| editor.token_inspect())
    }

    fn token_inspect_at(&mut self, row: usize, col: usize) -> Option<TokenInspectData> {
        self.write(|editor| editor.token_inspect_at(row, col))
    }

    fn on_resume(&mut self) -> EditorOutcome {
        self.write(|editor| editor.on_resume())
    }

    fn on_window_focus(&mut self) {
        self.write(|editor| editor.on_window_focus())
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        self.read(|editor| editor.context_items())
    }

    fn on_context_mouse(&mut self, at: Point) -> EditorOutcome {
        self.write(|editor| editor.on_context_mouse(at))
    }

    fn popups(&self) -> Popups {
        self.read(|editor| editor.popups())
    }

    fn crumbs(&self) -> Vec<String> {
        self.read(|editor| editor.crumbs())
    }

    fn on_crumb(&mut self, index: usize) -> EditorOutcome {
        self.write(|editor| editor.on_crumb(index))
    }

    fn column_header(&self) -> Vec<ColumnName> {
        self.read(|editor| editor.column_header())
    }

    fn toolbar(&self) -> Option<Toolbar> {
        self.read(|editor| editor.toolbar())
    }

    fn on_toolbar(&mut self, button: u16) -> EditorOutcome {
        self.write(|editor| editor.on_toolbar(button))
    }

    fn toolbar_tooltip(&self, button: u16) -> Option<String> {
        self.read(|editor| editor.toolbar_tooltip(button))
    }

    fn outline_targets(&mut self) -> Option<Vec<JumpTarget>> {
        self.write(|editor| editor.outline_targets())
    }

    fn jump_targets(&self) -> Option<Vec<JumpTarget>> {
        self.read(|editor| editor.jump_targets())
    }

    fn jump_styles(&mut self, targets: &[usize], budget: Duration) -> TargetStyles {
        self.write(|editor| editor.jump_styles(targets, budget))
    }

    fn jump_cursor(&self) -> Option<usize> {
        self.read(|editor| editor.jump_cursor())
    }

    fn on_jump(&mut self, step: JumpStep) {
        self.write(|editor| editor.on_jump(step));
    }
}

impl Editor for Guarded<Box<dyn Editor>> {
    fn on_closed(&mut self) {
        self.write_editor(|editor| editor.on_closed());
    }

    fn save_view(&self) -> Option<Box<dyn Any>> {
        self.read_editor(|editor| editor.save_view())
    }

    fn restore_view(&mut self, state: Box<dyn Any>) {
        self.write_editor_once(|editor| editor.restore_view(state), ());
    }

    fn set_shown(&mut self, shown: bool) {
        self.write_editor(|editor| editor.set_shown(shown));
    }

    fn shed_caches(&mut self) {
        self.write_editor(|editor| editor.shed_caches());
    }

    fn is_waited_on(&self) -> bool {
        self.read_editor(|editor| editor.is_waited_on())
    }

    fn is_kept_when_closed(&self) -> bool {
        self.read_editor(|editor| editor.is_kept_when_closed())
    }

    fn returns_to_git(&self) -> Option<PathBuf> {
        self.read_editor(|editor| editor.returns_to_git())
    }

    fn address(&self) -> Option<String> {
        self.read_editor(|editor| editor.address())
    }

    fn open_file(&mut self, target: OpenTarget) -> bool {
        self.write_editor_once(|editor| editor.open_file(target), false)
    }

    fn snapshot(&self) -> Option<OpenTarget> {
        self.read_editor(|editor| editor.snapshot())
    }

    fn holds_file(&self, path: &Path) -> bool {
        self.read_editor(|editor| editor.holds_file(path))
    }

    fn root_dir(&self) -> Option<PathBuf> {
        self.read_editor(|editor| editor.root_dir())
    }

    fn file_reference(&self) -> Option<String> {
        self.read_editor(|editor| editor.file_reference())
    }

    fn has_work_in_flight(&self) -> bool {
        self.read_editor(|editor| editor.has_work_in_flight())
    }

    fn is_dirty(&self) -> bool {
        self.read_editor(|editor| editor.is_dirty())
    }

    fn document(&self) -> Option<(PathBuf, String)> {
        self.read_editor(|editor| editor.document())
    }

    fn text_revision(&self) -> Option<usize> {
        self.read_editor(|editor| editor.text_revision())
    }

    fn document_path(&self) -> Option<PathBuf> {
        self.read_editor(|editor| editor.document_path())
    }

    fn focus_lost(&mut self) -> Vec<PathBuf> {
        self.write_editor(|editor| editor.focus_lost())
    }
}

// ========================================================================
// Stopped
// ========================================================================

impl Stopped {
    /// What stands in for `editor` after it panicked with `payload`.
    fn new(editor: &dyn Page, payload: &(dyn Any + Send)) -> Self {
        // A broken editor may panic again when asked its name.
        let name = panic::catch_unwind(AssertUnwindSafe(|| editor.title()))
            .unwrap_or_else(|_| UNNAMED.to_string());
        let icon = panic::catch_unwind(AssertUnwindSafe(|| editor.icon())).unwrap_or_default();
        let message = panic_message(payload);
        let location = LAST_PANIC.with(|last| last.borrow_mut().take());
        let at = location
            .as_ref()
            .map(|at| format!(" ({at})"))
            .unwrap_or_default();
        Self {
            icon,
            notice: Some(Notice::error(format!(
                "{name} stopped after an internal error: {message}{at}"
            ))),
            location,
            message,
            title: format!("{name} {STOPPED_SUFFIX}"),
        }
    }
}

impl Page for Stopped {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon(&self) -> Option<IconKind> {
        self.icon.clone()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        let error = Style::Diagnostic(DiagnosticLevel::Error);
        let mut rows = vec![
            vec![Span::new(
                error,
                "This editor stopped after an internal error.",
            )],
            Vec::new(),
            vec![Span::new(Style::Normal, self.message.clone())],
        ];
        if let Some(at) = &self.location {
            rows.push(vec![Span::new(Style::Dim, format!("at {at}"))]);
        }
        rows.extend([
            Vec::new(),
            vec![Span::new(
                Style::Dim,
                "The window and its other tabs are unaffected. Close this tab and open it again to start afresh.",
            )],
        ]);
        Frame::new(rows)
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        // Let the window's own keys (closing the tab) through.
        EditorOutcome::Ignored
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn status(&self) -> EditorStatus {
        EditorStatus {
            mode: None,
            segments: vec!["stopped".to_string()],
        }
    }
}

impl Editor for Stopped {}

// ========================================================================
// Functions
// ========================================================================

/// `editor`, run under the guard.
pub fn guard_editor(editor: Box<dyn Editor>) -> Box<dyn Editor> {
    install_panic_hook();
    Box::new(Guarded {
        inner: editor,
        stopped: OnceCell::new(),
    })
}

/// `page`, run under the guard.
pub fn guard_page(page: Box<dyn Page>) -> Box<dyn Page> {
    install_panic_hook();
    Box::new(Guarded {
        inner: page,
        stopped: OnceCell::new(),
    })
}

// ========================================================================
// Panic location
// ========================================================================

thread_local! {
    /// Where the latest panic on this thread was raised, as `file:line`: a
    /// caught panic carries only its message, so the hook keeps the rest.
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Note where each panic is raised for a guard to tell, and write it to the
/// crash log, ahead of the hook already in place, which still prints the
/// panic as it did. Once a process; every guard calls it before it catches
/// anything.
pub fn install_panic_hook() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if let Some(at) = info.location() {
                let at = format!("{}:{}", at.file(), at.line());
                LAST_PANIC.with(|last| *last.borrow_mut() = Some(at));
            }
            // Not from tests: their deliberate panics are no crash to keep.
            if !cfg!(test) {
                winter_base::crash_log::append(info);
            }
            previous(info);
        }));
    });
}

/// What a panic caught on this thread said, and where it was raised when
/// that is known: `message (file:line)`.
pub fn describe_panic(payload: &(dyn Any + Send)) -> String {
    let message = panic_message(payload);
    match LAST_PANIC.with(|last| last.borrow_mut().take()) {
        Some(at) => format!("{message} ({at})"),
        None => message,
    }
}

/// What a panic said, from the payload it unwound with.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    match (
        payload.downcast_ref::<&str>(),
        payload.downcast_ref::<String>(),
    ) {
        (Some(text), _) => (*text).to_string(),
        (None, Some(text)) => text.clone(),
        (None, None) => NO_MESSAGE.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;
    use crate::input::KeyCode;
    use crate::paint::row_text;

    /// An editor that panics where it is told to, counting the calls that
    /// reach it.
    struct Faulty {
        calls: Rc<Cell<usize>>,
        panics_in_content: bool,
        panics_in_title: bool,
    }

    impl Page for Faulty {
        fn title(&self) -> String {
            self.calls.set(self.calls.get() + 1);
            assert!(!self.panics_in_title, "title gone wrong");
            "Faulty".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            self.calls.set(self.calls.get() + 1);
            let rows: Vec<usize> = Vec::new();
            if self.panics_in_content {
                let _ = rows[3];
            }
            Frame::new(vec![vec![Span::new(Style::Normal, "fine")]])
        }

        fn gutter_lanes(&self) -> Vec<GutterLane> {
            vec![GutterLane::Icon, GutterLane::Text { width: 3 }]
        }

        fn on_key(&mut self, _key: &Key) -> EditorOutcome {
            self.calls.set(self.calls.get() + 1);
            EditorOutcome::Consumed
        }
    }

    impl Editor for Faulty {}

    fn faulty(
        panics_in_content: bool,
        panics_in_title: bool,
    ) -> (Box<dyn Editor>, Rc<Cell<usize>>) {
        let calls = Rc::new(Cell::new(0));
        let editor = Faulty {
            calls: Rc::clone(&calls),
            panics_in_content,
            panics_in_title,
        };
        (guard_editor(Box::new(editor)), calls)
    }

    fn text(frame: &Frame) -> String {
        frame
            .rows
            .iter()
            .map(row_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_a_panic_stops_the_editor_tells_it_once_and_the_broken_editor_is_never_called_again() {
        let (mut editor, calls) = faulty(true, false);
        let shown = text(&editor.content(10, 40, false));
        assert!(shown.contains("stopped after an internal error"), "{shown}");
        assert!(
            shown.contains("index out of bounds"),
            "the panic's own message: {shown}"
        );
        assert!(
            shown.contains("guard.rs:"),
            "and where it was raised: {shown}"
        );
        assert_eq!(editor.title(), "Faulty (stopped)");

        let notice = editor.take_notice().expect("told as a notification");
        assert!(
            notice
                .text
                .contains("Faulty stopped after an internal error: index out of bounds"),
            "{}",
            notice.text
        );
        assert!(editor.take_notice().is_none(), "told once");

        let reached = calls.get();
        assert_eq!(
            editor.on_key(&Key::plain(KeyCode::Char('x'))),
            EditorOutcome::Ignored,
            "keys go to the window"
        );
        editor.content(10, 40, false);
        assert_eq!(
            calls.get(),
            reached,
            "the broken editor is not called again"
        );
    }

    #[test]
    fn test_a_panic_in_a_call_that_only_reads_stops_the_editor_too() {
        let (mut editor, _) = faulty(false, true);
        assert_eq!(
            editor.title(),
            "Editor (stopped)",
            "even its name could not be read"
        );
        assert!(text(&editor.content(10, 40, false)).contains("title gone wrong"));
    }

    #[test]
    fn test_the_editor_s_gutter_lanes_are_its_own_through_the_guard() {
        // Unforwarded, the guard answers with the default lane, and an
        // editor's own gutter (a listing's two, the line numbers) is lost.
        let (editor, _) = faulty(false, false);
        assert_eq!(
            editor.gutter_lanes(),
            [GutterLane::Icon, GutterLane::Text { width: 3 }]
        );
    }

    #[test]
    fn test_the_editor_s_kind_is_seen_through_the_guard_until_it_stops() {
        let (mut editor, _) = faulty(true, false);
        assert!(editor.downcast_ref::<Faulty>().is_some());
        assert!(editor.downcast_mut::<Faulty>().is_some());
        assert_eq!(
            text(&editor.content(10, 40, false)).lines().next(),
            Some("This editor stopped after an internal error.")
        );
        assert!(
            editor.downcast_ref::<Faulty>().is_none(),
            "nothing reaches the broken editor"
        );
        assert!(editor.downcast_mut::<Faulty>().is_none());
    }

    #[test]
    fn test_a_panic_in_a_kind_s_own_method_stops_the_editor_as_any_other_call_does() {
        let (mut editor, calls) = faulty(false, false);
        assert_eq!(
            editor.with_kind(|faulty: &Faulty| faulty.calls.get()),
            Some(0),
            "its own methods are reached"
        );
        let answer = editor.with_kind_mut(|faulty: &mut Faulty| {
            faulty.panics_in_content = true;
            faulty.content(10, 40, false)
        });
        assert!(answer.is_none(), "the call that panicked has no answer");
        assert!(editor
            .take_notice()
            .is_some_and(|notice| notice.text.contains("index out of bounds")));
        let reached = calls.get();
        assert_eq!(
            editor.with_kind(|faulty: &Faulty| faulty.calls.get()),
            None,
            "nothing reaches the broken editor"
        );
        assert_eq!(calls.get(), reached);
    }
}
