//! A tab an extension draws: its [`View`]'s lines as the tab's rows, and the
//! keys pressed in it matched against the actions its kind declared, the
//! view told only which action ran. A key it did not declare is Winter's.
//!
//! A view acts on Winter through a [`Host`] while it takes a key, but the
//! editor has no window to lend it: what it asks is kept, and the window
//! does it once the key is done (see [`Editor::take_extension_calls`]). A
//! view that panics is stopped there, its tab saying so, and Winter goes on.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use winter_ext_api::{
    Diagnostic, Document, Host, InlineAnswer, InlineOffer, Line, PickerEvents, PickerId, PickerRow,
    PickerSpec, Repaint, Reply, Tone, View, ViewKind, Work,
};

use winter_model::editor::{Appearance, EditorOutcome, Notice, Page, ScrollPosition};
use winter_model::input::table::{self, KeyHelp, KeyTable, TableMatch};
use winter_model::input::Key;
use winter_model::paint::{DiagnosticLevel, Frame, Point, Span, Style};
use winter_model::picture::Tile;

use winter_model::paint::Row;

use super::web_page::PageView;

// ========================================================================
// Constants
// ========================================================================

/// The keys of a view whose kind has no table: one Winter did not build,
/// which answers nothing.
static NO_KEYS: KeyTable<&'static str> = KeyTable {
    block: "",
    bindings: &[],
    configurable: false,
    title: "",
};

// ========================================================================
// Data Structures
// ========================================================================

/// A tab showing an extension's view.
pub struct ExtensionEditor {
    /// What the view asked of Winter while taking keys, not yet done.
    calls: Vec<Reply>,
    /// The extension the view is of.
    extension: &'static str,
    /// Whether the view panicked, and is no longer called.
    failed: bool,
    /// The actions the view's kind declared, and the keys they run on.
    keys: &'static KeyTable<&'static str>,
    /// What to tell the user, once the window asks.
    notice: Option<Notice>,
    /// What the tab is called.
    title: &'static str,
    /// The keys of a sequence typed so far, waiting for the rest.
    typed: Vec<Key>,
    /// The view laid out as a page, while it offers one.
    page: PageView,
    view: Box<dyn View>,
}

/// A [`Host`] that keeps what is asked of it, for the window to do later.
#[derive(Default)]
struct Deferred {
    calls: Vec<Reply>,
}

/// What stands in for a view whose constructor panicked: it shows nothing
/// and takes nothing, the tab saying it failed.
struct Unopened;

// ========================================================================
// ExtensionEditor
// ========================================================================

impl ExtensionEditor {
    /// A tab showing a fresh view of `kind`.
    pub fn new(kind: &'static ViewKind) -> Self {
        // Guarded like every other call into the view: a constructor's panic
        // took the window down with it.
        let opened = panic::catch_unwind(AssertUnwindSafe(|| (kind.open)()));
        let failed = opened.is_err();
        Self {
            calls: Vec::new(),
            extension: kind.extension,
            failed,
            keys: crate::extensions::keys::view_table(&kind.id).unwrap_or(&NO_KEYS),
            notice: failed.then(|| Notice::error(failure(kind.extension))),
            title: kind.title.as_str(),
            typed: Vec::new(),
            page: PageView::default(),
            view: opened.unwrap_or_else(|_| Box::new(Unopened)),
        }
    }

    /// Call into the view with `call`; a panic stops it, and is named in a
    /// notice. `None` when it panicked, or had before.
    fn guarded<T>(&mut self, call: impl FnOnce(&mut dyn View) -> T) -> Option<T> {
        if self.failed {
            return None;
        }
        let view = &mut *self.view;
        let ran = panic::catch_unwind(AssertUnwindSafe(|| call(view)));
        if ran.is_err() {
            self.failed = true;
            self.notice = Some(Notice::error(failure(self.extension)));
        }
        ran.ok()
    }
}

impl Page for ExtensionEditor {
    fn title(&self) -> String {
        self.title.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        // A view that offers a page is drawn as one, over every row.
        match self.guarded(|view| view.page()).flatten() {
            Some(markup) => {
                self.page.show(markup, (rows, cols));
                return Frame::new(vec![Row::new()]);
            }
            None => self.page.hide(),
        }
        let lines = self.guarded(|view| (view.render(rows, cols), view.cursor_line()));
        let Some((lines, cursor_line)) = lines else {
            let said = vec![Span::new(Style::Dim, failure(self.extension))];
            return Frame {
                rows: vec![said],
                ..Frame::default()
            };
        };
        let rows = lines
            .into_iter()
            .take(rows)
            .map(|line| {
                line.into_iter()
                    .map(|run| Span::new(style_of(run.tone), run.text))
                    .collect()
            })
            .collect();
        Frame {
            cursor_line,
            rows,
            ..Frame::default()
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        // A view that panicked answers nothing: its keys are Winter's again.
        if self.failed {
            return EditorOutcome::Ignored;
        }
        self.typed.push(*key);
        let action = match table::lookup(self.keys, &self.typed) {
            TableMatch::Exact(action) => action,
            TableMatch::Prefix => return EditorOutcome::Consumed,
            // A key the view did not declare is Winter's; one that ends a
            // sequence the view started is spent with it.
            TableMatch::Miss => {
                let ended = self.typed.len() > 1;
                self.typed.clear();
                return match ended {
                    true => EditorOutcome::Consumed,
                    false => EditorOutcome::Ignored,
                };
            }
        };
        self.typed.clear();
        let mut deferred = Deferred::default();
        self.guarded(|view| view.on_action(&mut deferred, action));
        self.calls.append(&mut deferred.calls);
        EditorOutcome::Consumed
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(self.keys)
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        // A press, not the drag after it.
        if at.drag {
            return EditorOutcome::Ignored;
        }
        if self.page.is_shown() {
            // A page's click is the action its element names, if the view
            // declared it; nothing else of the pointer reaches the view.
            let named = self.page.action_at(at);
            let declared = named.and_then(|named| {
                self.keys
                    .bindings
                    .iter()
                    .find(|binding| binding.id == named)
                    .map(|binding| binding.action)
            });
            if let Some(action) = declared {
                let mut deferred = Deferred::default();
                self.guarded(|view| view.on_action(&mut deferred, action));
                self.calls.append(&mut deferred.calls);
            }
            return EditorOutcome::Consumed;
        }
        let mut deferred = Deferred::default();
        let taken = self.guarded(|view| view.on_click(&mut deferred, at.row, at.col));
        self.calls.append(&mut deferred.calls);
        outcome(taken)
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        if self.page.is_shown() {
            self.page.scroll_rows(lines);
            return EditorOutcome::Consumed;
        }
        outcome(self.guarded(|view| view.on_scroll(lines)))
    }

    fn scrolls_smoothly(&self) -> bool {
        self.page.is_shown()
    }

    fn on_scroll_pixels(&mut self, pixels: f32) -> EditorOutcome {
        if self.page.is_shown() {
            self.page.scroll_by(pixels);
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Ignored
    }

    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.page.scroll_position()
    }

    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        if self.page.is_shown() {
            self.page.scroll_to(offset as f32);
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Ignored
    }

    fn tiles(&self) -> Vec<Tile> {
        self.page.tiles()
    }

    fn set_appearance(&mut self, appearance: &Appearance) {
        self.page.set_appearance(appearance.theme, appearance.scale);
    }

    fn set_pane_size(&mut self, pane: (f32, f32)) {
        self.page.set_pane_size(pane);
    }

    fn take_extension_calls(&mut self) -> Vec<(&'static str, Reply)> {
        let extension = self.extension;
        self.calls.drain(..).map(|call| (extension, call)).collect()
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }
}

// ========================================================================
// Unopened
// ========================================================================

impl View for Unopened {
    fn render(&mut self, _rows: usize, _columns: usize) -> Vec<Line> {
        Vec::new()
    }

    fn on_action(&mut self, _host: &mut dyn Host, _action: &str) {}
}

// ========================================================================
// Deferred
// ========================================================================

impl Deferred {
    /// Keep `call`, to be made on the window's host.
    fn later(&mut self, call: impl FnOnce(&mut dyn Host) + Send + 'static) {
        self.calls.push(Box::new(call));
    }
}

impl Host for Deferred {
    fn notify(&mut self, message: &str) {
        let message = message.to_string();
        self.later(move |host| host.notify(&message));
    }

    fn open_file(&mut self, path: &Path, line: Option<usize>) {
        let path = path.to_path_buf();
        self.later(move |host| host.open_file(&path, line));
    }

    fn open_view(&mut self, name: &str) {
        let name = name.to_string();
        self.later(move |host| host.open_view(&name));
    }

    fn document(&self) -> Option<Document> {
        None
    }

    fn publish_diagnostics(&mut self, path: &Path, diagnostics: Vec<Diagnostic>) {
        let path = path.to_path_buf();
        self.later(move |host| host.publish_diagnostics(&path, diagnostics));
    }

    fn run_command(&mut self, id: &str) {
        let id = id.to_string();
        self.later(move |host| host.run_command(&id));
    }

    fn open_url(&mut self, url: &str) {
        let url = url.to_string();
        self.later(move |host| host.open_url(&url));
    }

    fn copy_text(&mut self, text: &str) {
        let text = text.to_string();
        self.later(move |host| host.copy_text(&text));
    }

    fn start_server(&mut self, name: &str, program: Option<PathBuf>) {
        let name = name.to_string();
        self.later(move |host| host.start_server(&name, program));
    }

    fn install_server(&mut self, name: &str) {
        let name = name.to_string();
        self.later(move |host| host.install_server(&name));
    }

    fn server_request(&mut self, name: &str, tag: u64, method: &str, params: &str) {
        let (name, method, params) = (name.to_string(), method.to_string(), params.to_string());
        self.later(move |host| host.server_request(&name, tag, &method, &params));
    }

    fn offer_inline(&mut self, name: &str, offer: InlineOffer) {
        let name = name.to_string();
        self.later(move |host| host.offer_inline(&name, offer));
    }

    fn answer_inline(&mut self, ticket: u64, answer: InlineAnswer) {
        self.later(move |host| host.answer_inline(ticket, answer));
    }

    fn server_notify(&mut self, name: &str, method: &str, params: &str) {
        let (name, method, params) = (name.to_string(), method.to_string(), params.to_string());
        self.later(move |host| host.server_notify(&name, &method, &params));
    }

    fn set_status(&mut self, name: &str, text: Option<String>) {
        let name = name.to_string();
        self.later(move |host| host.set_status(&name, text));
    }

    fn spawn_work(&mut self, work: Work) {
        self.later(move |host| host.spawn_work(work));
    }

    fn open_picker(&mut self, id: PickerId, spec: PickerSpec, events: PickerEvents) {
        self.later(move |host| host.open_picker(id, spec, events));
    }

    fn push_rows(&mut self, picker: PickerId, rows: Vec<PickerRow>, done: bool) {
        self.later(move |host| host.push_rows(picker, rows, done));
    }

    fn repaint(&self) -> Repaint {
        crate::extensions::repaint()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What a view's tab, and the notice, say once `extension` has panicked.
fn failure(extension: &str) -> String {
    format!("the {extension} extension failed and was stopped there")
}

/// What the window hears of a view taking a key, a click, or a turn of
/// the wheel (`Some(true)`), leaving it (`Some(false)`), or having failed.
fn outcome(taken: Option<bool>) -> EditorOutcome {
    match taken {
        Some(true) => EditorOutcome::Consumed,
        Some(false) | None => EditorOutcome::Ignored,
    }
}

/// The theme's style for `tone`.
fn style_of(tone: Tone) -> Style {
    match tone {
        Tone::Accent => Style::Accent,
        Tone::Dim => Style::Dim,
        Tone::Error => Style::Diagnostic(DiagnosticLevel::Error),
        Tone::Heading => Style::Header,
        Tone::Info => Style::Diagnostic(DiagnosticLevel::Info),
        Tone::Warning => Style::Diagnostic(DiagnosticLevel::Warning),
        _ => Style::Normal,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_ext_api::{Extension, Registry};

    use super::*;

    /// An extension adding a view that cannot be made.
    struct Broken;

    impl Extension for Broken {
        fn id(&self) -> &'static str {
            "probe"
        }

        fn activate(&self, registry: &mut Registry) {
            registry.view("broken", "Broken", Vec::new(), || {
                panic!("the view could not be made")
            });
        }
    }

    #[test]
    fn test_a_view_whose_constructor_panics_opens_as_a_failed_tab() {
        // Every call into a view was guarded but making it: a constructor's
        // panic brought the whole window down.
        let registry: &'static Registry =
            Box::leak(Box::new(Registry::activate_all(
                [&Broken as &dyn Extension],
            )));
        let kind = registry
            .find_view("probe.broken")
            .expect("the view is added");
        let mut editor = ExtensionEditor::new(kind);
        assert!(editor.failed);
        assert!(editor.notice.is_some(), "the tab says so");
        let _ = editor.content(5, 20, false);
    }

    #[test]
    fn test_a_view_offering_a_page_is_drawn_as_one_and_a_click_runs_its_action() {
        let kind = crate::extensions::registry()
            .find_view("probe.sheet")
            .expect("the probe's sheet");
        let mut editor = ExtensionEditor::new(kind);
        editor
            .page
            .set_appearance(&winter_render::Theme::dark(), 1.0);
        editor.set_pane_size((400.0, 300.0));
        editor.content(10, 40, false);
        assert_eq!(editor.tiles().len(), 1, "drawn as a picture over the pane");
        let on_the_block = Point {
            col: 20,
            drag: false,
            row: 5,
        };
        assert_eq!(editor.on_mouse(on_the_block), EditorOutcome::Consumed);
        assert_eq!(
            editor.take_extension_calls().len(),
            1,
            "the flip ran, asking the window to say so"
        );
        let painted = editor.tiles()[0].key;
        editor.content(10, 40, false);
        assert_ne!(
            editor.tiles()[0].key,
            painted,
            "the flipped page painted anew"
        );
    }
}
