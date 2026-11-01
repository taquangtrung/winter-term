//! The extensions compiled into Winter, and what they contribute.
//!
//! Each extension is a crate written against [`winter_ext_api`], linked
//! in behind a Cargo feature of its own and listed in [`installed`]. They
//! are activated once, the first time anything asks, into one registry the
//! whole process shares, as the spelling dictionary is: the palette lists
//! their commands, a key binding may name them, and running one hands it a
//! [`Host`](winter_ext_api::Host) to act through (see
//! `window::extension_host`). Their status bar segments join the built-in
//! ones in `status-bar { segments ... }`, their completion back-ends the
//! built-in ones a language asks, and their views open in tabs as Winter's
//! own editors do (see `editors::extension`). Extensions installed as
//! WebAssembly files join them, each standing in the registry as one
//! compiled in does (see `wasm`).

#[cfg(feature = "wasm-extensions")]
pub(crate) mod approvals;
#[cfg(feature = "wasm-extensions")]
pub mod cli;
#[cfg(feature = "wasm-extensions")]
pub(crate) mod manage;
#[cfg(feature = "wasm-extensions")]
pub(crate) mod package;
#[cfg(feature = "wasm-extensions")]
mod wasm;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use winter_ext_api::{Command, Extension, Registry, Repaint, ViewKind};

use winter_base::wake::wake_window;

// ========================================================================
// Constants
// ========================================================================

/// Every extension's contributions, gathered the first time they are asked
/// for.
static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// What went wrong loading the extensions installed as files, said once a
/// window is up to show it.
static LOAD_PROBLEMS: OnceLock<Vec<String>> = OnceLock::new();

/// Whether an extension has asked for a repaint since the window last
/// looked.
static REPAINT_ASKED: AtomicBool = AtomicBool::new(false);

// ========================================================================
// Functions
// ========================================================================

/// The extensions compiled in: one line each, behind its crate's feature.
fn compiled_in() -> Vec<Box<dyn Extension>> {
    vec![
        #[cfg(feature = "example-extension")]
        Box::new(winter_simple_ext::Example),
        #[cfg(feature = "magic-winter")]
        Box::new(magic_winter::MagicWinter),
        #[cfg(test)]
        Box::new(tests::Probe),
    ]
}

/// Every extension, those compiled in first, and what went wrong loading
/// those installed as files.
fn installed() -> (Vec<Box<dyn Extension>>, Vec<String>) {
    let compiled = compiled_in();
    #[cfg(feature = "wasm-extensions")]
    let (compiled, problems) = wasm::with_installed(compiled);
    #[cfg(not(feature = "wasm-extensions"))]
    let problems = Vec::new();
    (compiled, problems)
}

/// Every extension's contributions, activating them the first time.
pub(crate) fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| {
        let (installed, problems) = installed();
        let _ = LOAD_PROBLEMS.set(problems);
        Registry::activate_all(installed.iter().map(Box::as_ref))
    })
}

/// What went wrong loading the extensions installed as files.
pub(crate) fn load_problems() -> &'static [String] {
    registry();
    LOAD_PROBLEMS.get().map_or(&[], Vec::as_slice)
}

/// The extension command whose full id is `id`.
pub(crate) fn command(id: &str) -> Option<&'static Command> {
    registry().find(id)
}

/// The full id of the extension status bar segment `id` names, kept as
/// long as Winter runs.
pub(crate) fn segment(id: &str) -> Option<&'static str> {
    registry()
        .find_segment(id)
        .map(|segment| segment.id.as_str())
}

/// What an extension keeps to have the window paint again, from any thread.
pub(crate) fn repaint() -> Repaint {
    Repaint::new(|| {
        REPAINT_ASKED.store(true, Ordering::Relaxed);
        wake_window();
    })
}

/// Whether an extension has asked for a repaint since this was last asked.
pub(crate) fn take_repaint() -> bool {
    REPAINT_ASKED.swap(false, Ordering::Relaxed)
}

/// The kind of extension view whose full id is `id`.
pub(crate) fn view(id: &str) -> Option<&'static ViewKind> {
    registry().find_view(id)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
pub(crate) mod tests {
    use winter_ext_api::{
        open_picker, pick, spawn, CompletionItem, Completions, Diagnostic, Event, Host,
        InlineAnswer, InlineEdit, InlineOffer, InlineProvider, InlineRequest, KeyName, KeyPress,
        Line, PickItem, PickerEvent, PickerEvents, PickerRow, PickerSpec, ServerSpec, SettingValue,
        Severity, Styled, TextRange, Tone, View,
    };

    use super::*;

    /// An extension the tests run through Winter: `probe.ping` says
    /// "pong", `probe.flag` flags the first character of the file in front,
    /// `probe.show` has its `probe.said` segment say [`SHOWN`], and
    /// `probe.panic` panics. It says which file it saw open at once, and
    /// which it saw saved from work done off the window's thread. As the
    /// `probe.words` back-end it offers [`OFFERED`] for the word at the
    /// cursor. `probe.board` opens its [`Board`] view. `probe.choose`
    /// offers [`CHOICES`] to pick from, titled [`CHOOSE`], the first
    /// standing for a file, and says the one chosen after [`CHOSE`].
    /// `probe.ask` opens a picker hearing its query, offering it shouted,
    /// with two actions, and says the action and row chosen after [`ASKED`],
    /// or `nothing` after it when the picker is put away.
    pub(crate) struct Probe;

    /// The probe's one setting.
    pub(crate) const LOUD: &str = "loud";

    /// The probe's inline source, and the ghost text it offers.
    pub(crate) const GHOST: &str = "ghost";
    pub(crate) const GHOSTED: &str = "boo";

    /// The probe's inline source: ghost text at the cursor, at once.
    struct Ghostly;

    impl InlineProvider for Ghostly {
        fn ask(&self, host: &mut dyn Host, request: &InlineRequest) {
            let at = TextRange::new(request.at, request.at);
            let ghost = InlineEdit::new(at, GHOSTED, "probe ghost");
            host.answer_inline(request.ticket, InlineAnswer::Ghost(vec![ghost]));
        }
    }

    /// The probe's server, whose program is nowhere.
    pub(crate) const PILOT: &str = "pilot";

    /// What `probe.ping` says.
    pub(crate) const PONG: &str = "pong";

    /// What the probe says of a file opened, and of one saved.
    pub(crate) const OPENED: &str = "probe opened";
    pub(crate) const SAVED: &str = "probe saved";

    /// What `probe.show` has the probe's segment say.
    pub(crate) const SHOWN: &str = "probe shown";

    /// What the probe's completion back-end offers.
    pub(crate) const OFFERED: &str = "valued";

    /// What the probe's view is called, and what it says when `n` is
    /// pressed in it.
    pub(crate) const BOARD: &str = "Probe Board";
    pub(crate) const PRESSED: &str = "probe pressed";

    /// What `probe.choose` titles its list, offers in it, and says before
    /// the one chosen.
    pub(crate) const CHOOSE: &str = "Probe: Choose";
    pub(crate) const CHOICES: [&str; 2] = ["first", "second"];
    pub(crate) const CHOSE: &str = "probe chose";

    /// What `probe.ask` titles its picker, and says before what was chosen.
    pub(crate) const ASK: &str = "Probe: Ask";
    pub(crate) const ASKED: &str = "probe asked";

    /// What the probe's view says of a click, before where it landed.
    pub(crate) const CLICKED: &str = "probe clicked";

    /// The probe's view: a count of the `n`s pressed in it, said on its
    /// second line, and of the rows scrolled, on its third. `p` panics, and
    /// no other key is taken; a click is said where it landed.
    #[derive(Default)]
    pub(crate) struct Board {
        pressed: usize,
        scrolled: isize,
    }

    impl View for Board {
        fn render(&mut self, _rows: usize, _columns: usize) -> Vec<Line> {
            let line = |tone, text: String| vec![Styled::new(tone, text)];
            vec![
                line(Tone::Heading, BOARD.to_string()),
                line(Tone::Normal, format!("pressed {}", self.pressed)),
                line(Tone::Dim, format!("scrolled {}", self.scrolled)),
            ]
        }

        fn on_click(&mut self, host: &mut dyn Host, line: usize, column: usize) -> bool {
            host.notify(&format!("{CLICKED} {line}:{column}"));
            true
        }

        fn on_scroll(&mut self, lines: isize) -> bool {
            self.scrolled += lines;
            true
        }

        fn on_key(&mut self, host: &mut dyn Host, key: &KeyPress) -> bool {
            match key.key {
                KeyName::Char('n') => {
                    self.pressed += 1;
                    host.notify(PRESSED);
                    true
                }
                KeyName::Char('p') => panic!("the probe's view panicked"),
                _ => false,
            }
        }
    }

    impl Extension for Probe {
        fn id(&self) -> &'static str {
            "probe"
        }

        fn activate(&self, registry: &mut Registry) {
            registry.command("probe.ping", "Probe: Ping", |host: &mut dyn Host| {
                host.notify(PONG)
            });
            registry.command(
                "probe.flag",
                "Probe: Flag First Character",
                |host: &mut dyn Host| {
                    if let Some(document) = host.document() {
                        let flagged = Diagnostic::new(Severity::Warning, "flagged", 0, 0..1);
                        host.publish_diagnostics(&document.path, vec![flagged]);
                    }
                },
            );
            registry.command("probe.panic", "Probe: Panic", |_: &mut dyn Host| {
                panic!("the probe panicked")
            });
            registry.view("board", BOARD, || Box::new(Board::default()));
            registry.command("probe.board", "Probe: Open Board", |host: &mut dyn Host| {
                host.open_view("board")
            });
            registry.command("probe.choose", CHOOSE, |host: &mut dyn Host| {
                // Only the first stands for a file.
                let [first, second] = CHOICES;
                let choices = vec![
                    (
                        PickItem::new(first)
                            .with_hint("probe")
                            .with_file(format!("/notes/{first}.md")),
                        first,
                    ),
                    (PickItem::new(second).with_hint("probe"), second),
                ];
                pick(host, CHOOSE, choices, |host, choice| {
                    host.notify(&format!("{CHOSE} {choice}"))
                });
            });
            registry.command("probe.ask", ASK, |host: &mut dyn Host| {
                let spec = PickerSpec::new(ASK).hearing_query();
                let events: PickerEvents = Box::new(|host, picker, event| match event {
                    PickerEvent::Query(query) => {
                        let row = PickerRow::new(format!("{query}!"))
                            .with_action("shout", "Shout")
                            .with_action("whisper", "Whisper")
                            .with_preview(format!("said {query}"));
                        host.push_rows(picker, vec![row], true);
                    }
                    PickerEvent::Chosen(choice) => {
                        host.notify(&format!("{ASKED} {} {}", choice.action, choice.row))
                    }
                    PickerEvent::Closed => host.notify(&format!("{ASKED} nothing")),
                    _ => {}
                });
                open_picker(host, spec, events);
            });
            registry.setting(LOUD, SettingValue::Switch(false), "Shout.");
            registry.server(ServerSpec::new(PILOT, "winter-no-such-program"));
            registry.inline(GHOST, "Probe", Ghostly);
            registry.status_segment("said");
            registry.command("probe.show", "Probe: Show", |host: &mut dyn Host| {
                host.set_status("said", Some(SHOWN.to_string()))
            });
            registry.completion("words", |request| {
                let before: Vec<u16> = request.line.encode_utf16().collect();
                let column = request.column as usize;
                let start = before[..column]
                    .iter()
                    .rposition(|unit| {
                        !char::from_u32(u32::from(*unit))
                            .is_some_and(|c| c.is_alphanumeric() || c == '_')
                    })
                    .map_or(0, |at| at + 1);
                Some(Completions::new(
                    start as u32,
                    vec![CompletionItem::new(OFFERED, OFFERED)],
                ))
            });
            registry.on_event(|host, event| {
                HEARD.with(|heard| heard.borrow_mut().push(event.clone()));
                hear(host, event)
            });
        }
    }

    thread_local! {
        /// Every event the probe has heard on this thread, which is the
        /// test's own: each test's window runs on it.
        static HEARD: std::cell::RefCell<Vec<Event>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// The events the probe has heard on this thread since last asked.
    pub(crate) fn heard() -> Vec<Event> {
        HEARD.with(|heard| heard.take())
    }

    /// What the probe does on hearing `event`.
    fn hear(host: &mut dyn Host, event: &Event) {
        match event {
            Event::Opened(path) => host.notify(&format!("{OPENED} {}", name(path))),
            Event::Saved(path) => {
                let name = name(path);
                spawn(
                    host,
                    move || format!("{SAVED} {name}"),
                    |host, said| host.notify(&said),
                );
            }
            Event::Configured(_) => host.offer_inline(GHOST, InlineOffer::new(true, false)),
            _ => {}
        }
    }

    /// `path`'s own name.
    fn name(path: &std::path::Path) -> String {
        path.file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    }

    #[test]
    fn test_a_repaint_asked_for_from_another_thread_is_taken_once() {
        let repaint = repaint();
        std::thread::spawn(move || repaint.request())
            .join()
            .expect("asked");
        assert!(take_repaint());
        assert!(!take_repaint(), "taken once");
    }

    #[test]
    fn test_the_extensions_compiled_in_are_activated_once_into_one_registry() {
        let ping = command("probe.ping").expect("the probe's command is registered");
        assert_eq!(ping.title, "Probe: Ping");
        assert!(std::ptr::eq(registry(), registry()), "one registry, shared");
        assert!(
            command("ping").is_none(),
            "commands are known by their full id"
        );
    }
}
