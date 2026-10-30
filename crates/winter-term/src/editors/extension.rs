//! A tab an extension draws: its [`View`]'s lines as the tab's rows, and the
//! keys pressed in it handed to the view.
//!
//! A view acts on Winter through a [`Host`] while it takes a key, but the
//! editor has no window to lend it: what it asks is kept, and the window
//! does it once the key is done (see [`Editor::take_extension_calls`]). A
//! view that panics is stopped there, its tab saying so, and Winter goes on.

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use winter_extension::{
    Diagnostic, Document, Host, KeyName, KeyPress, Line, Repaint, Reply, Tone, View, ViewKind, Work,
};

use crate::model::editor::{Editor, EditorOutcome, Notice};
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::paint::{DiagnosticLevel, Frame, Point, Span, Style};

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
    /// What to tell the user, once the window asks.
    notice: Option<Notice>,
    /// What the tab is called.
    title: &'static str,
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
            notice: failed.then(|| Notice::error(failure(kind.extension))),
            title: kind.title.as_str(),
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

impl Editor for ExtensionEditor {
    fn title(&self) -> String {
        self.title.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
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
        let Some(press) = key_press(key) else {
            return EditorOutcome::Ignored;
        };
        let mut deferred = Deferred::default();
        let taken = self.guarded(|view| view.on_key(&mut deferred, &press));
        self.calls.append(&mut deferred.calls);
        outcome(taken)
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        // A press, not the drag after it.
        if at.drag {
            return EditorOutcome::Ignored;
        }
        let mut deferred = Deferred::default();
        let taken = self.guarded(|view| view.on_click(&mut deferred, at.row, at.col));
        self.calls.append(&mut deferred.calls);
        outcome(taken)
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        outcome(self.guarded(|view| view.on_scroll(lines)))
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

    fn on_key(&mut self, _host: &mut dyn Host, _key: &KeyPress) -> bool {
        false
    }
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

    fn set_status(&mut self, name: &str, text: Option<String>) {
        let name = name.to_string();
        self.later(move |host| host.set_status(&name, text));
    }

    fn spawn_work(&mut self, work: Work) {
        self.later(move |host| host.spawn_work(work));
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

/// `key` as a view is told of it; `None` for one held with Super, or the
/// Insert key, which are Winter's own.
fn key_press(key: &Key) -> Option<KeyPress> {
    // Every modifier is passed on, so the chord is read whole.
    let (
        Mods {
            alt,
            ctrl,
            shift,
            sup,
        },
        code,
    ) = key.chord();
    if sup {
        return None;
    }
    let name = match code {
        KeyCode::Backspace => KeyName::Backspace,
        KeyCode::Char(c) => KeyName::Char(c),
        KeyCode::Delete => KeyName::Delete,
        KeyCode::Down => KeyName::Down,
        KeyCode::End => KeyName::End,
        KeyCode::Enter => KeyName::Enter,
        KeyCode::Escape => KeyName::Escape,
        KeyCode::F(number) => KeyName::F(number),
        KeyCode::Home => KeyName::Home,
        KeyCode::Insert => return None,
        KeyCode::Left => KeyName::Left,
        KeyCode::PageDown => KeyName::PageDown,
        KeyCode::PageUp => KeyName::PageUp,
        KeyCode::Right => KeyName::Right,
        KeyCode::Space => KeyName::Char(' '),
        KeyCode::Tab => KeyName::Tab,
        KeyCode::Up => KeyName::Up,
    };
    let mut press = KeyPress::new(name);
    (press.alt, press.ctrl, press.shift) = (alt, ctrl, shift); // key-exact: all handed on
    Some(press)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_extension::{Extension, Registry};

    use super::*;

    /// An extension adding a view that cannot be made.
    struct Broken;

    impl Extension for Broken {
        fn id(&self) -> &'static str {
            "probe"
        }

        fn activate(&self, registry: &mut Registry) {
            registry.view("broken", "Broken", || panic!("the view could not be made"));
        }
    }

    #[test]
    fn test_a_view_whose_constructor_panics_opens_as_a_failed_tab() {
        // Every call into a view was guarded but making it: a constructor's
        // panic brought the whole window down.
        let registry: &'static Registry = Box::leak(Box::new(Registry::activate_all([&Broken as &dyn Extension])));
        let kind = registry.find_view("probe.broken").expect("the view is added");
        let mut editor = ExtensionEditor::new(kind);
        assert!(editor.failed);
        assert!(editor.notice.is_some(), "the tab says so");
        let _ = editor.content(5, 20, false);
    }
}
