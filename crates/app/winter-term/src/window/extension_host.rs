//! What an extension acts on Winter through: the window, seen as a [`Host`]
//! narrow enough to promise to extensions. A command, an event handler, and
//! the reply of slow work all run on the window's thread, with the tab in
//! front as the one they are about; slow work itself runs on a thread of
//! its own and hands its reply back through [`Replies`].
//!
//! An extension that panics is stopped there and named in a notice, rather
//! than taking the window down with it.
//!
//! A picker an extension opens is one of the window's own: its rows come
//! through a link the window pushes into, the queries it hears are taken from
//! that link each turn of the loop, and what is chosen in it is handed back
//! as a command's is run, with the tab in front as the picker closes.
//!
//! What extensions hear of the editors comes from one [`Watch`], which
//! compares what the tabs show with what it saw last: each pass of the event
//! loop, and at once as a tab opens or closes. However a tab came to show a
//! file, then, it is heard of.

use std::collections::{BTreeMap, HashMap};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use winter_ext_api::{
    Command, Diagnostic, Document, Event, Host, InlineAnswer, InlineOffer, PickerChoice,
    PickerEvent, PickerEvents, PickerId, PickerRow, PickerSpec, Repaint, Reply, Settings, Severity,
    Work, ID_SEPARATOR,
};

use crate::editors::extension::ExtensionEditor;
use winter_base::wake::wake_window;
use winter_lsp::diagnostics::{self, Backend};
use winter_lsp::protocol::{self, Position, Range};
use winter_model::editor::OpenTarget;
use winter_model::layout::TabId;

use super::editor::EditorKind;
use super::picker::sources::{ExtensionChoice, ExtensionSource, PickerLink};
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How long the typing in the file in front has to pause before
/// extensions hear it changed.
const CHANGE_SETTLE: Duration = Duration::from_millis(300);

/// What a page an extension opens must start with.
const HTTPS: &str = "https://";

// ========================================================================
// Data Structures
// ========================================================================

/// The window, lent to the extension `extension` for one call, made with
/// the tab `focused` in front.
struct ExtensionHost<'a> {
    app: &'a mut App,
    extension: &'static str,
    focused: TabId,
}

/// A picker an extension opened, while it is open.
struct ExtensionPicker {
    /// What it is told of what happens in it; taken while it is being told,
    /// so the picker stays findable for the rows pushed meanwhile.
    events: Option<PickerEvents>,
    /// The extension that opened it.
    extension: &'static str,
    /// Where its rows go, and its queries come from.
    link: PickerLink,
}

/// What the window keeps for its extensions.
#[derive(Default)]
pub(in crate::window) struct Extensions {
    /// The settings each extension was last told it has, by its id.
    configured: HashMap<&'static str, Settings>,
    /// The pickers they have open, by id.
    pickers: HashMap<PickerId, ExtensionPicker>,
    /// What their slow work has handed back, waiting to run on this thread.
    replies: Replies,
    /// What each extension status bar segment says, by its full id; one
    /// saying nothing is absent.
    segments: HashMap<&'static str, String>,
    /// What they were last told the editors show.
    watch: Watch,
}

/// The replies of extensions' slow work, sent from the threads doing it to
/// the window's, each with the extension it is for.
struct Replies {
    received: Receiver<(&'static str, Reply)>,
    sender: Sender<(&'static str, Reply)>,
}

/// What the editors showed when extensions were last told, to tell them
/// what has changed since.
#[derive(Default)]
struct Watch {
    /// The file in front.
    front: Option<PathBuf>,
    /// Its text's revision.
    revision: Option<usize>,
    /// When the file in front's latest edit will have settled, and the file.
    settle: Option<(Instant, PathBuf)>,
    /// The file each editor tab in sight showed.
    shown: BTreeMap<TabId, PathBuf>,
}

/// What the editors show now, for a [`Watch`] to compare.
struct Seen {
    front: Option<PathBuf>,
    revision: Option<usize>,
    shown: BTreeMap<TabId, PathBuf>,
}

// ========================================================================
// App: extensions
// ========================================================================

impl App {
    /// Run the extension command `command`, with the tab `focused` in front.
    pub(in crate::window) fn run_extension_command(&mut self, command: &Command, focused: TabId) {
        self.as_extension(command.extension, focused, |host| (command.run)(host));
    }

    /// Tell every extension that asked that `event` has happened.
    pub(in crate::window) fn emit_extension_event(&mut self, event: &Event) {
        let focused = self.layout().focused();
        for subscription in crate::extensions::registry().subscriptions() {
            self.as_extension(subscription.extension, focused, |host| {
                (subscription.handle)(host, event)
            });
        }
    }

    /// Tell each extension its settings where they are not what it was last
    /// told: once as the window starts, and after any change to them.
    fn tell_extension_settings(&mut self) {
        let registry = crate::extensions::registry();
        let mut extensions: Vec<&'static str> = registry
            .settings()
            .iter()
            .map(|setting| setting.extension)
            .collect();
        extensions.dedup();
        for extension in extensions {
            let settings = self.config.extension_settings.of(registry, extension);
            if self.host.extensions.configured.get(extension) == Some(&settings) {
                continue;
            }
            self.host
                .extensions
                .configured
                .insert(extension, settings.clone());
            self.tell_extension(extension, &Event::Configured(settings));
        }
    }

    /// Tell `extension` alone that `event` has happened: what is its own,
    /// as its settings and its servers' words are.
    pub(in crate::window) fn tell_extension(&mut self, extension: &'static str, event: &Event) {
        let focused = self.layout().focused();
        let theirs = crate::extensions::registry()
            .subscriptions()
            .iter()
            .filter(|subscription| subscription.extension == extension);
        for subscription in theirs {
            self.as_extension(extension, focused, |host| {
                (subscription.handle)(host, event)
            });
        }
    }

    /// Tell the extensions what the editors show that they have not heard
    /// of, and their settings where they changed. The moment a pending edit
    /// settles, if one is.
    pub(in crate::window) fn watch_for_extensions(&mut self) -> Option<Instant> {
        self.watch_for_extensions_at(Instant::now())
    }

    /// The same, as of `now`.
    fn watch_for_extensions_at(&mut self, now: Instant) -> Option<Instant> {
        if crate::extensions::registry().subscriptions().is_empty() {
            return None;
        }
        self.tell_extension_settings();
        let buried = self.open_tabs.buried();
        let shown = self
            .open_tabs
            .pages()
            .filter(|(id, _)| !buried.contains(id))
            .filter_map(|(id, slot)| {
                Some((
                    *id,
                    slot.page
                        .as_editor()
                        .and_then(|editor| editor.document_path())?,
                ))
            })
            .collect();
        let front = self.open_tabs.page(&self.layout().focused());
        let seen = Seen {
            front: front.as_ref().and_then(|slot| {
                slot.page
                    .as_editor()
                    .and_then(|editor| editor.document_path())
            }),
            revision: front.and_then(|slot| {
                slot.page
                    .as_editor()
                    .and_then(|editor| editor.text_revision())
            }),
            shown,
        };
        // Taken in full before any is told, so a handler opening a file is
        // heard of on its own.
        let events = self.host.extensions.watch.update(seen, now);
        for event in &events {
            self.emit_extension_event(event);
        }
        self.host.extensions.watch.due()
    }

    /// Run the replies extensions' slow work has handed back since last
    /// asked. Says whether there were any.
    pub(in crate::window) fn run_extension_replies(&mut self) -> bool {
        let replies: Vec<_> = self.host.extensions.replies.received.try_iter().collect();
        let focused = self.layout().focused();
        let any = !replies.is_empty();
        for (extension, reply) in replies {
            self.as_extension(extension, focused, reply);
        }
        any
    }

    /// Tell the extension whose picker `choice` was made in what was chosen,
    /// with the tab `focused` in front. The picker has closed, so it is
    /// forgotten.
    pub(in crate::window) fn tell_extension_choice(
        &mut self,
        choice: ExtensionChoice,
        focused: TabId,
    ) {
        let Some(picker) = self.host.extensions.pickers.remove(&choice.picker) else {
            return;
        };
        let (extension, Some(mut events)) = (picker.extension, picker.events) else {
            return;
        };
        let chosen = PickerChoice::new(choice.action, choice.row);
        self.as_extension(extension, focused, |host| {
            events(host, choice.picker, PickerEvent::Chosen(chosen))
        });
    }

    /// Tell the extensions the queries their open pickers have settled on,
    /// and of the pickers put away, which are forgotten. Says whether any
    /// was told.
    pub(in crate::window) fn tell_extension_queries(&mut self) -> bool {
        let pickers = &mut self.host.extensions.pickers;
        let closed: Vec<PickerId> = pickers
            .iter()
            .filter(|(_, picker)| picker.link.is_closed())
            .map(|(id, _)| *id)
            .collect();
        let asked: Vec<(PickerId, String)> = pickers
            .iter()
            .filter_map(|(id, picker)| Some((*id, picker.link.take_query()?)))
            .collect();
        let focused = self.layout().focused();
        let any = !asked.is_empty() || !closed.is_empty();
        for id in closed {
            let Some(picker) = self.host.extensions.pickers.remove(&id) else {
                continue;
            };
            if let (extension, Some(mut events)) = (picker.extension, picker.events) {
                self.as_extension(extension, focused, |host| {
                    events(host, id, PickerEvent::Closed)
                });
            }
        }
        for (id, query) in asked {
            let Some(picker) = self.host.extensions.pickers.get_mut(&id) else {
                continue;
            };
            let (extension, Some(mut events)) = (picker.extension, picker.events.take()) else {
                continue;
            };
            self.as_extension(extension, focused, |host| {
                events(host, id, PickerEvent::Query(query))
            });
            if let Some(picker) = self.host.extensions.pickers.get_mut(&id) {
                picker.events = Some(events);
            }
        }
        any
    }

    /// Call into the extension `extension` with `call`, the tab `focused`
    /// in front: a panic in it is caught and named in a notice.
    pub(in crate::window) fn as_extension(
        &mut self,
        extension: &'static str,
        focused: TabId,
        call: impl FnOnce(&mut dyn Host),
    ) {
        let mut host = ExtensionHost {
            app: self,
            extension,
            focused,
        };
        let ran = panic::catch_unwind(AssertUnwindSafe(|| call(&mut host)));
        if ran.is_err() {
            self.set_notice(format!(
                "the {extension} extension failed and was stopped there"
            ));
        }
        self.dirty = true;
    }
}

// ========================================================================
// Extensions
// ========================================================================

impl Extensions {
    /// What each extension status bar segment says, by its full id.
    pub(in crate::window) fn segments(&self) -> &HashMap<&'static str, String> {
        &self.segments
    }
}

// ========================================================================
// Watch
// ========================================================================

impl Watch {
    /// The events between what was seen last and `seen`, as of `now`:
    /// tabs no longer showing a file, then tabs newly showing one, then
    /// the file in front's edits once settled, or once it is left, then
    /// another file in front.
    fn update(&mut self, seen: Seen, now: Instant) -> Vec<Event> {
        let mut events: Vec<Event> = self
            .shown
            .iter()
            .filter(|(tab, path)| seen.shown.get(tab) != Some(path))
            .map(|(_, path)| Event::Closed(path.clone()))
            .collect();
        events.extend(
            seen.shown
                .iter()
                .filter(|(tab, path)| self.shown.get(tab) != Some(path))
                .map(|(_, path)| Event::Opened(path.clone())),
        );
        if seen.front != self.front {
            // Leaving a file is a pause in its typing.
            if let Some((_, edited)) = self.settle.take() {
                events.push(Event::Changed(edited));
            }
            events.push(Event::Focused(seen.front.clone()));
        } else if let (Some(front), Some(_), Some(_)) = (&seen.front, self.revision, seen.revision)
        {
            if seen.revision != self.revision {
                self.settle = Some((now + CHANGE_SETTLE, front.clone()));
            }
        }
        if self.settle.as_ref().is_some_and(|(due, _)| now >= *due) {
            if let Some((_, edited)) = self.settle.take() {
                events.push(Event::Changed(edited));
            }
        }
        self.front = seen.front;
        self.revision = seen.revision;
        self.shown = seen.shown;
        events
    }

    /// When the pending edit settles, if one is pending.
    fn due(&self) -> Option<Instant> {
        self.settle.as_ref().map(|(due, _)| *due)
    }
}

// ========================================================================
// Replies
// ========================================================================

impl Default for Replies {
    fn default() -> Self {
        let (sender, received) = mpsc::channel();
        Self { received, sender }
    }
}

// ========================================================================
// ExtensionHost
// ========================================================================

impl Host for ExtensionHost<'_> {
    fn notify(&mut self, message: &str) {
        self.app.set_notice(message);
    }

    fn open_file(&mut self, path: &Path, line: Option<usize>) {
        self.app.open_path_editor(OpenTarget {
            column: None,
            line,
            path: path.to_path_buf(),
        });
    }

    fn open_view(&mut self, name: &str) {
        let id = format!("{}{ID_SEPARATOR}{name}", self.extension);
        let Some(view) = crate::extensions::view(&id) else {
            return;
        };
        let kind = EditorKind::Extension(view.id.as_str());
        self.app
            .open_page(kind, |_| Box::new(ExtensionEditor::new(view)));
    }

    fn document(&self) -> Option<Document> {
        let slot = self.app.open_tabs.page(&self.focused)?;
        let (path, text) = slot.page.as_editor().and_then(|editor| editor.document())?;
        Some(Document::new(path, text))
    }

    fn publish_diagnostics(&mut self, path: &Path, found: Vec<Diagnostic>) {
        let found = found
            .into_iter()
            .map(|d| as_problem(d, self.extension))
            .collect();
        diagnostics::publish(Backend::Extension(self.extension), path, found);
        self.app.dirty = true;
    }

    fn run_command(&mut self, id: &str) {
        self.app.run_command(id, self.focused);
    }

    fn open_url(&mut self, url: &str) {
        let extension = self.extension;
        if !url.starts_with(HTTPS) {
            self.app.set_error(format!(
                "the {extension} extension asked to open {url}, which is not an https page"
            ));
            return;
        }
        match ::open::that_detached(url) {
            Ok(()) => self.app.set_notice(format!("{extension}: opened {url}")),
            Err(e) => self
                .app
                .set_error(format!("{extension}: could not open {url}: {e}")),
        }
    }

    fn start_server(&mut self, name: &str, program: Option<PathBuf>) {
        self.app
            .start_extension_server(self.extension, name, program);
    }

    fn install_server(&mut self, name: &str) {
        self.app.install_extension_server(self.extension, name);
    }

    fn server_request(&mut self, name: &str, tag: u64, method: &str, params: &str) {
        self.app
            .extension_server_request(self.extension, name, tag, (method, params));
    }

    fn server_notify(&mut self, name: &str, method: &str, params: &str) {
        self.app
            .extension_server_notify(self.extension, name, (method, params));
    }

    fn offer_inline(&mut self, name: &str, offer: InlineOffer) {
        self.app.offer_extension_inline(self.extension, name, offer);
    }

    fn answer_inline(&mut self, ticket: u64, answer: InlineAnswer) {
        self.app.answer_extension_inline(ticket, &answer);
    }

    fn copy_text(&mut self, text: &str) {
        self.app.copy_quietly(text);
        let extension = self.extension;
        self.app
            .set_notice(format!("{extension}: copied to the clipboard"));
    }

    fn set_status(&mut self, name: &str, text: Option<String>) {
        let id = format!("{}{ID_SEPARATOR}{name}", self.extension);
        let Some(id) = crate::extensions::segment(&id) else {
            return;
        };
        let segments = &mut self.app.host.extensions.segments;
        match text {
            Some(text) => segments.insert(id, text),
            None => segments.remove(id),
        };
        self.app.dirty = true;
    }

    fn repaint(&self) -> Repaint {
        crate::extensions::repaint()
    }

    fn open_picker(&mut self, id: PickerId, spec: PickerSpec, events: PickerEvents) {
        let link = PickerLink::new(spec.rows, spec.streamed);
        let source = ExtensionSource::new(id, spec.title, spec.hears_query, link.clone());
        self.app.host.extensions.pickers.insert(
            id,
            ExtensionPicker {
                events: Some(events),
                extension: self.extension,
                link,
            },
        );
        self.app.open_picker(Box::new(source));
    }

    fn push_rows(&mut self, picker: PickerId, rows: Vec<PickerRow>, done: bool) {
        if let Some(open) = self.app.host.extensions.pickers.get(&picker) {
            open.link.push(rows, done);
            self.app.dirty = true;
        }
    }

    fn spawn_work(&mut self, work: Work) {
        let (extension, sender) = (
            self.extension,
            self.app.host.extensions.replies.sender.clone(),
        );
        let started = thread::Builder::new()
            .name(format!("extension {extension}"))
            .spawn(move || {
                // A panic here ends the work and its thread, and no reply comes.
                let _ = sender.send((extension, work()));
                wake_window();
            });
        if started.is_err() {
            self.app.set_notice(format!(
                "the {extension} extension could not start its work"
            ));
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// An extension's diagnostic as the problems store keeps one, named as
/// found by `extension`.
fn as_problem(found: Diagnostic, extension: &str) -> protocol::Diagnostic {
    let at = |character| Position {
        character,
        line: found.line,
    };
    protocol::Diagnostic {
        code: None,
        message: found.message,
        range: Range {
            end: at(found.end.max(found.start)),
            start: at(found.start),
        },
        severity: match found.severity {
            Severity::Error => protocol::Severity::Error,
            Severity::Hint => protocol::Severity::Hint,
            Severity::Info => protocol::Severity::Information,
            Severity::Warning => protocol::Severity::Warning,
        },
        source: Some(extension.to_string()),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Config;
    use crate::config::StatusSegmentKind;
    use crate::extensions::tests::{
        heard, ASK, ASKED, BOARD, CHOICES, CHOOSE, CHOSE, CLICKED, LOUD, OPENED, PONG, PRESSED,
        SAVED, SHOWN,
    };
    use crate::window::commands::{command_label, window_commands};
    use crate::window::editor::EditorKind;
    use winter_ext_api::SettingValue;
    use winter_model::input::Keymap;
    use winter_model::input::{Key, KeyCode};
    use winter_model::paint::{IconKind, Point};

    #[test]
    fn test_an_extension_s_commands_are_listed_named_and_bindable() {
        let palette = window_commands(&Keymap::default());
        let ping = palette
            .iter()
            .find(|entry| entry.action == "probe.ping")
            .expect("listed");
        assert_eq!(ping.label, "Probe: Ping");
        // What a key binding is checked against, and its key hint's label.
        assert_eq!(command_label("probe.ping"), Some("Probe: Ping"));
    }

    #[test]
    fn test_an_extension_s_command_runs_through_the_window() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.ping", focused);
        assert_eq!(app.chrome.notice_text(), Some(PONG));
    }

    /// A text editor tab on a file holding "text", in a directory named
    /// for `name`, and the file.
    fn editing(app: &mut App, name: &str) -> (TabId, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("winter-ext-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "text\n").expect("file");
        let editor = winter_text::TextEditor::new(file.clone(), None).expect("opens");
        (app.show_editor(EditorKind::Text, Box::new(editor)), file)
    }

    #[test]
    fn test_an_extension_hears_its_settings_once_and_again_when_they_change() {
        let mut app = App::new();
        let loud = |on| {
            Event::Configured(Settings::new(
                [(LOUD.to_string(), SettingValue::Switch(on))].into(),
            ))
        };
        app.watch_for_extensions();
        let first: Vec<Event> = heard()
            .into_iter()
            .filter(|e| matches!(e, Event::Configured(_)))
            .collect();
        assert_eq!(first, [loud(false)], "its defaults, as the window starts");
        app.watch_for_extensions();
        assert!(
            !heard().iter().any(|e| matches!(e, Event::Configured(_))),
            "unchanged: not again"
        );
        app.config.extension_settings =
            Config::parse("probe {\n    loud #true\n}\n").extension_settings;
        app.watch_for_extensions();
        let changed: Vec<Event> = heard()
            .into_iter()
            .filter(|e| matches!(e, Event::Configured(_)))
            .collect();
        assert_eq!(changed, [loud(true)]);
    }

    #[test]
    fn test_an_extension_hears_a_file_open_at_once() {
        let mut app = App::new();
        let (_, file) = editing(&mut app, "opened");
        assert_eq!(
            app.chrome.notice_text(),
            Some(format!("{OPENED} notes.txt").as_str())
        );
        let _ = std::fs::remove_dir_all(file.parent().expect("dir"));
    }

    #[test]
    fn test_work_an_extension_starts_on_a_save_reports_back_on_the_window_s_thread() {
        let mut app = App::new();
        let (_, file) = editing(&mut app, "saved");
        app.on_file_written(&file);
        // The work runs on a thread of its own; its reply waits until the
        // window runs it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app.run_extension_replies() {
            assert!(
                std::time::Instant::now() < deadline,
                "the work reported back"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            app.chrome.notice_text(),
            Some(format!("{SAVED} notes.txt").as_str())
        );
        let _ = std::fs::remove_dir_all(file.parent().expect("dir"));
    }

    #[test]
    fn test_an_extension_s_segment_says_what_it_is_told_in_the_status_bar() {
        let mut app = App::new();
        let focused = app.layout().focused();
        assert!(
            StatusSegmentKind::defaults().contains(&StatusSegmentKind::Extension("probe.said")),
            "shown unless settings say otherwise"
        );
        assert_eq!(
            StatusSegmentKind::from_name("probe.said"),
            Some(StatusSegmentKind::Extension("probe.said"))
        );
        app.config.status_bar.segments = vec![StatusSegmentKind::Extension("probe.said")];
        app.run_command("probe.show", focused);
        assert_eq!(
            app.host
                .extensions
                .segments
                .get("probe.said")
                .map(String::as_str),
            Some(SHOWN)
        );
        assert_eq!(
            app.status_segment_tooltip(0).as_deref(),
            Some(SHOWN),
            "in the bar"
        );
    }

    /// The text of each row the editor in tab `tab` paints.
    fn painted(app: &mut App, tab: TabId) -> Vec<String> {
        let slot = app.open_tabs.page_mut(&tab).expect("an editor");
        let frame = slot.page.content(10, 40, false);
        frame
            .rows
            .iter()
            .map(|row| row.iter().map(|span| span.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn test_an_extension_s_view_opens_in_a_tab_of_its_own_once() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.board", focused);
        let board = app.layout().focused();
        assert_ne!(board, focused, "a tab of its own, in front");
        let slot = app.open_tabs.page(&board).expect("an editor");
        assert_eq!(slot.kind(), EditorKind::Extension("probe.board"));
        assert_eq!(slot.kind().title(), BOARD);
        assert_eq!(EditorKind::from_name("probe.board"), Some(slot.kind()));
        assert_eq!(painted(&mut app, board), [BOARD, "pressed 0", "scrolled 0"]);
        app.run_command("probe.board", board);
        assert_eq!(app.layout().focused(), board, "the open one, not another");
        assert_eq!(app.open_tabs.pages().count(), 1);
    }

    #[test]
    fn test_a_view_takes_its_keys_and_acts_through_the_window_once_done() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.board", focused);
        let board = app.layout().focused();
        assert!(app.offer_key_to_editor(board, &Key::plain(KeyCode::Char('n'))));
        assert_eq!(
            app.chrome.notice_text(),
            Some(PRESSED),
            "what it asked, done"
        );
        assert_eq!(painted(&mut app, board)[1], "pressed 1");
        assert!(
            !app.offer_key_to_editor(board, &Key::plain(KeyCode::Char('z'))),
            "a key it does not take is left to Winter"
        );
    }

    #[test]
    fn test_a_chord_a_view_did_not_declare_is_left_to_winter() {
        // `n` is the probe's; `M-n` is not, whatever the view would make of
        // an `n`, so the window's own bindings answer it.
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.board", focused);
        let board = app.layout().focused();
        let alt_n = Key {
            alt: true,
            ..Key::plain(KeyCode::Char('n'))
        };
        assert!(!app.offer_key_to_editor(board, &alt_n));
        assert_eq!(
            painted(&mut app, board)[1],
            "pressed 0",
            "and the view hears nothing"
        );
    }

    #[test]
    fn test_a_view_s_keys_are_listed_and_rebound_as_winter_s_own() {
        let table = crate::editors::key_table("probe.board").expect("listed as a key table");
        assert!(table.configurable(), "rebound in keybindings.kdl");
        let press = table
            .listed()
            .into_iter()
            .find(|binding| binding.id == "press")
            .expect("the probe's press");
        assert_eq!(press.keys, ["n"]);
    }

    #[test]
    fn test_a_view_that_panics_is_stopped_and_its_tab_says_so() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.board", focused);
        let board = app.layout().focused();
        app.offer_key_to_editor(board, &Key::plain(KeyCode::Char('p')));
        let failed = "the probe extension failed and was stopped there";
        assert_eq!(app.chrome.notice_text(), Some(failed));
        assert_eq!(painted(&mut app, board), [failed]);
        assert!(
            !app.offer_key_to_editor(board, &Key::plain(KeyCode::Char('n'))),
            "and is called no more"
        );
    }

    /// What the editors show: `front` in front at `revision`, and `shown`
    /// by tab.
    fn seen(front: Option<&str>, revision: Option<usize>, shown: &[(u64, &str)]) -> Seen {
        Seen {
            front: front.map(PathBuf::from),
            revision,
            shown: shown
                .iter()
                .map(|(tab, path)| (TabId(*tab), PathBuf::from(path)))
                .collect(),
        }
    }

    #[test]
    fn test_a_tab_turned_to_another_file_is_heard_closing_one_and_opening_the_other() {
        let mut watch = Watch::default();
        let now = Instant::now();
        let first = watch.update(seen(Some("/a"), Some(0), &[(1, "/a"), (2, "/b")]), now);
        let a = PathBuf::from("/a");
        let b = PathBuf::from("/b");
        let c = PathBuf::from("/c");
        assert_eq!(
            first,
            [
                Event::Opened(a.clone()),
                Event::Opened(b.clone()),
                Event::Focused(Some(a.clone()))
            ]
        );
        assert!(watch
            .update(seen(Some("/a"), Some(0), &[(1, "/a"), (2, "/b")]), now)
            .is_empty());
        let turned = watch.update(seen(Some("/c"), Some(0), &[(1, "/c")]), now);
        assert_eq!(
            turned,
            [
                Event::Closed(a),
                Event::Closed(b),
                Event::Opened(c.clone()),
                Event::Focused(Some(c))
            ]
        );
        let left = watch.update(seen(None, None, &[]), now);
        assert_eq!(left[1], Event::Focused(None), "a terminal in front");
    }

    #[test]
    fn test_an_edit_is_heard_once_the_typing_pauses_or_the_file_is_left() {
        let mut watch = Watch::default();
        let start = Instant::now();
        let a = PathBuf::from("/a");
        watch.update(seen(Some("/a"), Some(0), &[(1, "/a")]), start);
        assert!(watch
            .update(seen(Some("/a"), Some(1), &[(1, "/a")]), start)
            .is_empty());
        let typing = start + CHANGE_SETTLE / 2;
        assert!(
            watch
                .update(seen(Some("/a"), Some(2), &[(1, "/a")]), typing)
                .is_empty(),
            "still typing"
        );
        assert_eq!(
            watch.due(),
            Some(typing + CHANGE_SETTLE),
            "each key puts it off"
        );
        let paused = watch.update(
            seen(Some("/a"), Some(2), &[(1, "/a")]),
            typing + CHANGE_SETTLE,
        );
        assert_eq!(paused, [Event::Changed(a.clone())]);
        assert_eq!(watch.due(), None, "once");
        watch.update(seen(Some("/a"), Some(3), &[(1, "/a"), (2, "/b")]), start);
        let left = watch.update(seen(Some("/b"), Some(0), &[(1, "/a"), (2, "/b")]), start);
        assert_eq!(
            left,
            [Event::Changed(a), Event::Focused(Some(PathBuf::from("/b")))],
            "leaving the file is a pause"
        );
    }

    #[test]
    fn test_the_window_tells_extensions_of_files_shown_edited_and_closed() {
        let mut app = App::new();
        let (first, file) = editing(&mut app, "watched");
        let other = file.with_file_name("other.txt");
        std::fs::write(&other, "more\n").expect("file");
        heard();
        // A tab put in place without being opened, as a restored session's
        // are, is heard of on the loop's next pass.
        let restored = app.alloc_tab_id();
        let editor = winter_text::TextEditor::new(other.clone(), None).expect("opens");
        app.open_tabs
            .insert_editor(restored, EditorKind::Text, Box::new(editor));
        let now = Instant::now();
        app.watch_for_extensions_at(now);
        assert_eq!(heard(), [Event::Opened(other.clone())]);
        let slot = app.open_tabs.page_mut(&first).expect("an editor");
        slot.page.on_paste("typed ".to_string());
        app.watch_for_extensions_at(now);
        assert!(heard().is_empty(), "not while typing");
        app.watch_for_extensions_at(now + CHANGE_SETTLE);
        assert_eq!(heard(), [Event::Changed(file.clone())]);
        app.close_tab(first);
        let closed = heard();
        assert_eq!(closed[0], Event::Closed(file), "at once");
        assert!(
            matches!(closed.last(), Some(Event::Focused(_))),
            "and what is in front now"
        );
        let _ = std::fs::remove_dir_all(other.parent().expect("dir"));
    }

    #[test]
    fn test_a_view_takes_clicks_and_the_wheel() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.board", focused);
        let board = app.layout().focused();
        let press = Point {
            col: 4,
            drag: false,
            row: 1,
        };
        assert!(app.offer_mouse_to_editor(board, press));
        assert_eq!(
            app.chrome.notice_text(),
            Some(format!("{CLICKED} 1:4").as_str())
        );
        let drag = Point {
            drag: true,
            ..press
        };
        assert!(
            !app.offer_mouse_to_editor(board, drag),
            "a drag is no click"
        );
        app.offer_scroll_to_editor(board, -3);
        assert_eq!(painted(&mut app, board)[2], "scrolled -3");
    }

    #[test]
    fn test_an_extension_s_list_is_shown_in_the_picker_and_the_choice_handed_back() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.choose", focused);
        app.settle_picker();
        let view = app.chrome.picker().expect("the picker, open").view();
        assert_eq!(view.title, CHOOSE);
        assert_eq!(app.picker_labels(), CHOICES);
        assert_eq!(
            view.rows[0].row.description, "probe",
            "its hint, after its label"
        );
        assert!(
            view.rows[0].row.icon
                == crate::icons::name_for(&IconKind::File {
                    name: "first.md".to_string()
                }),
            "its file's icon"
        );
        assert!(view.rows[1].row.icon.is_none(), "no file, no icon");
        app.press_in_picker("<down> RET");
        assert_eq!(
            app.chrome.notice_text(),
            Some(format!("{CHOSE} {}", CHOICES[1]).as_str()),
            "the one selected, not the first"
        );
    }

    #[test]
    fn test_an_extension_s_picker_hears_its_query_and_is_told_the_action_chosen() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.ask", focused);
        app.settle_picker();
        assert_eq!(app.chrome.picker().expect("open").view().title, ASK);
        app.press_in_picker("h i");
        assert_eq!(
            app.picker_labels(),
            ["hi!"],
            "the rows it gave for the query"
        );
        app.press_in_picker("F4");
        let view = app.chrome.picker().expect("open").view();
        let menu = view.menu.expect("the action menu");
        let titles: Vec<&str> = menu
            .actions
            .iter()
            .map(|action| action.title.as_str())
            .collect();
        assert_eq!(titles, ["Shout", "Whisper"]);
        app.press_in_picker("C-n RET");
        assert!(app.chrome.picker().is_none());
        assert_eq!(
            app.chrome.notice_text(),
            Some(format!("{ASKED} whisper 0").as_str()),
            "the action chosen in the menu, on the row given"
        );
        assert!(
            app.host.extensions.pickers.is_empty(),
            "a picker chosen in is forgotten"
        );
    }

    #[test]
    fn test_an_extension_is_told_its_picker_was_put_away() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.ask", focused);
        app.settle_picker();
        app.press_in_picker("ESC");
        app.tell_extension_queries();
        assert_eq!(
            app.chrome.notice_text(),
            Some(format!("{ASKED} nothing").as_str())
        );
        assert!(app.host.extensions.pickers.is_empty());
    }

    #[test]
    fn test_an_extension_that_panics_is_stopped_and_named_not_the_window() {
        let mut app = App::new();
        let focused = app.layout().focused();
        app.run_command("probe.panic", focused);
        assert_eq!(
            app.chrome.notice_text(),
            Some("the probe extension failed and was stopped there")
        );
        app.run_command("probe.ping", focused);
        assert_eq!(
            app.chrome.notice_text(),
            Some(PONG),
            "and the window goes on"
        );
    }
}
