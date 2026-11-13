//! A settings page over a small app: Ctrl+, opens it (it opens at start).
//! Move with Up and Down (or the Vim list keys: `j`, `k`, `C-d`, `g g`,
//! `G`, counts such as `3j`), Tab between the sidebar and the rows; Enter or
//! Space flips, steps, or edits a row, Left and Right step it, Backspace
//! resets it, and Esc closes the page. `[` and `]` switch between the
//! User and Project scopes; a change made in Project marks its row, and
//! Backspace there drops the Project value. The Keybindings section rebinds the
//! demo's commands: Enter on a command, then press the new keys. Changes
//! apply at once (the theme, the counter step) and are printed to
//! standard output, where a real app would save them. With the page
//! closed, F5 counts and Ctrl+Q quits.
//!
//! The counter line and the page share a focus tree: with the page closed
//! the counter has the focus (Enter or Space counts too, Tab shows its
//! focus ring); the page is a focus trap, so while it is open no key
//! reaches the app's commands behind it, and closing it gives the focus
//! back to the counter.
//!
//! A text or number being edited is a text field: Shift with the arrows
//! selects, Ctrl+C, Ctrl+X, and Ctrl+V copy, cut, and paste through the
//! system clipboard, and an input method composes in place.
//!
//! ```sh
//! cargo run -p winter-cherry --example settings
//! ```

use std::time::{Duration, Instant};

use misty::color::Rgb;
use misty::key::{Key, KeyCode, Mods};
use misty::text::{Span, StyledText};
use misty::theme::{Theme, ThemeToken};
use misty::{
    AppContext, Application, Command, CommandRegistry, Error, FocusId, FocusTree, Frame, ImeFocus,
    Input, KeyScope, NumberRange, Outcome, Point, Rect, Route, Setting, SettingValue, SettingsPage,
    SettingsResponse, SettingsSection, Widget, WindowId, WindowOptions,
};
use winter_cherry::cherry::keys::{Keymap, Resolution, Resolver};
use winter_cherry::clock;
use winter_cherry::keymap;
use winter_cherry::keys::to_cherry;

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty settings";

/// How long a half-typed key sequence waits for its next key.
const KEY_TIMEOUT: Duration = Duration::from_secs(1);

/// The id of the command that opens the settings page.
const OPEN_SETTINGS: &str = "settings.open";

/// The id of the command that counts.
const INCREMENT: &str = "counter.increment";

/// The id of the command that quits.
const QUIT: &str = "app.quit";

/// Every command of the demo, each written once.
const COMMANDS: &[Command] = &[
    Command::new(OPEN_SETTINGS, "Open Settings", "App").with_keys(&["C-,"]),
    Command::new(INCREMENT, "Increment", "Counter").with_keys(&["<f5>"]),
    Command::new(QUIT, "Quit", "App").with_keys(&["C-q"]),
];

/// The id of the theme setting.
const THEME: &str = "appearance.theme";

/// The id of the counter step setting.
const STEP: &str = "counter.step";

/// The blank cells around the text, in columns and rows.
const MARGIN: f32 = 1.0;

/// The focus id of the counter line.
const COUNTER: FocusId = FocusId::new("counter");

/// The focus id of the settings page.
const PAGE: FocusId = FocusId::new("settings");

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a counter, and the settings page while it is open.
struct Demo {
    /// The count so far.
    counter: f64,
    /// Which of the window's widgets has the keyboard.
    focus: FocusTree,
    /// Whether the input method is on: while the page edits a text.
    ime: ImeFocus,
    /// The registry's keys, rebuilt after every rebinding.
    keymap: Keymap<(), &'static str>,
    /// The settings, kept while the page is closed so it reopens as left.
    page: SettingsPage,
    /// Whether the page is open.
    page_open: bool,
    /// Every command and its keys.
    registry: CommandRegistry,
    /// Turns keys into commands.
    resolver: Resolver,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

// ========================================================================
// Demo Implementations
// ========================================================================

impl Demo {
    /// Runs the command `id`.
    fn run_command(&mut self, cx: &mut AppContext<'_, ()>, id: &'static str) {
        match id {
            INCREMENT => self.count(),
            OPEN_SETTINGS => self.open_page(),
            QUIT => cx.exit(),
            // No other id is registered.
            _ => {}
        }
    }

    /// Opens the settings page, trapping the focus in it.
    fn open_page(&mut self) {
        self.page_open = true;
        self.focus.open_trap(PAGE);
    }

    /// Adds the counter step to the count.
    fn count(&mut self) {
        if let Some(SettingValue::Number(step)) = self.page.value(STEP) {
            self.counter += step;
        }
    }

    /// Acts on what the page made of a key or an event: applies and
    /// "saves" changes, closes it when dismissed.
    fn settle(&mut self, response: SettingsResponse) {
        match response {
            SettingsResponse::Changed(id) => {
                let scope = self.page.scope();
                println!("save {id} = {:?} in scope {scope}", self.page.value(&id));
                if scope > 0 {
                    self.page.set_overridden(&id, true);
                }
                if id == THEME {
                    let is_light = self.page.value(THEME) == Some(&SettingValue::Choice(1));
                    self.theme = match is_light {
                        true => Theme::light(),
                        false => Theme::dark(),
                    };
                }
            }
            SettingsResponse::Cleared(id) => {
                // A real app would load the User value back in here.
                println!("drop the Project value of {id}");
                self.page.set_overridden(&id, false);
            }
            SettingsResponse::Dismissed => self.page_open = false,
            SettingsResponse::Handled | SettingsResponse::Ignored => {}
            SettingsResponse::ScopeChanged(scope) => {
                // A real app would load the scope's values in here.
                println!("show scope {scope}");
            }
            SettingsResponse::KeysChanged(id) => {
                let keys: Vec<String> = self
                    .registry
                    .keys(id)
                    .iter()
                    .map(|sequence| misty::keys::label(sequence))
                    .collect();
                println!("save keys of {id} = {keys:?}");
                self.keymap = bind(&self.registry);
            }
        }
    }
}

impl Application for Demo {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        cx.request_redraw(window);
        let (mut response, mut counted) = (None, false);
        let mut page_cx = cx.event_cx(window);
        let (page, registry) = (&mut self.page, &mut self.registry);
        let route = self.focus.dispatch(input, |id, input| match id {
            PAGE => page
                .lend(registry)
                .handle(input, &mut page_cx)
                .keep(&mut response),
            COUNTER if input.key().is_some_and(is_press) => {
                counted = true;
                true
            }
            _ => false,
        });
        if let Some(response) = response {
            self.settle(response);
        }
        if counted {
            self.count();
        }
        match (route, input) {
            (Route::Unhandled, Input::CloseRequested) => cx.exit(),
            (Route::Unhandled, Input::Key(key)) => {
                let keymap = self.keymap.active(&[()]);
                let resolutions = self.resolver.feed(&keymap, to_cherry(*key), clock::now());
                for resolution in resolutions {
                    if let Resolution::Matched(found) = resolution {
                        self.run_command(cx, found.command);
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let settled = self.resolver.tick(&self.keymap.active(&[()]), clock::now());
        if let Some(Resolution::Matched(found)) = settled {
            self.run_command(cx, found.command);
        }
        let glide = match (self.page_open, self.window) {
            (true, Some(window)) => self.page.about_to_wait(&mut cx.event_cx(window)),
            (false, _) | (true, None) => None,
        };
        if let Some(window) = self.window {
            let caret = self.page.cursor_area().filter(|_| self.page_open);
            self.ime.follow(&mut cx.event_cx(window), caret);
        }
        [self.resolver.deadline().map(clock::instant), glide]
            .into_iter()
            .flatten()
            .min()
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.palette.background.into());
        self.focus.begin_frame();
        let metrics = frame.cell_metrics();
        let area = frame.bounds();
        let hint = |id: &str| self.registry.key_hint(id).unwrap_or_default();
        let lines = [
            StyledText::new()
                .with(Span::new("Counter: ", ThemeToken::Muted.into()))
                .with(Span::new(self.counter.to_string(), ThemeToken::Accent.into()).bold()),
            StyledText::plain(
                format!(
                    "{} opens settings, {} counts, {} quits.",
                    hint(OPEN_SETTINGS),
                    hint(INCREMENT),
                    hint(QUIT)
                ),
                ThemeToken::Muted.into(),
            )
            .with_ellipsis(),
        ];
        for (row, line) in lines.iter().enumerate() {
            let y = area.y + (MARGIN + row as f32) * metrics.height;
            let clip = Rect::new(area.x, y, area.width, metrics.height);
            if row == 0 {
                let counter = Rect::new(
                    area.x + MARGIN * metrics.width,
                    y,
                    line.width() as f32 * metrics.width,
                    metrics.height,
                );
                self.focus.register(COUNTER, counter);
            }
            frame.draw_text(
                line,
                Point::new(area.x + MARGIN * metrics.width, y),
                clip,
                &self.theme,
            );
        }
        if self.page_open {
            let bounds = frame.bounds();
            let rect = self
                .page
                .lend(&mut self.registry)
                .draw(frame, bounds, &self.theme);
            self.focus.register(PAGE, rect);
        }
        self.focus.end_frame();
        self.focus.draw(frame, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `key` presses the focused widget: Enter or Space.
fn is_press(key: Key) -> bool {
    key.mods == Mods::NONE && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
}

/// The keymap of every command's keys in force in `registry`.
fn bind(registry: &CommandRegistry) -> Keymap<(), &'static str> {
    let mut keymap = Keymap::new();
    // A rebinding may leave a key that is a prefix of another; that
    // binding is refused and the rest stays bound.
    let _ = keymap::bind(registry, &mut keymap, (), KeyScope::All);
    keymap
}

/// The demo's settings page.
fn settings_page() -> SettingsPage {
    SettingsPage::new("Settings")
        .with_section(
            SettingsSection::new("Appearance")
                .with(
                    Setting::choice(THEME, "Theme", &["Dark", "Light"], 0)
                        .with_description("The colors of the whole window."),
                )
                .with(Setting::number(
                    "appearance.font-size",
                    "Font size",
                    14.0,
                    NumberRange::new(8.0, 32.0, 1.0),
                ))
                .with(Setting::color(
                    "appearance.accent",
                    "Accent color",
                    Rgb::new(0x89, 0xb4, 0xfa),
                ))
                .with(Setting::toggle("appearance.blink", "Blinking cursor", true)),
        )
        .with_section(
            SettingsSection::new("Counter")
                .with(
                    Setting::number(STEP, "Step", 1.0, NumberRange::new(0.5, 10.0, 0.5))
                        .with_description("How much each Increment adds."),
                )
                .with(Setting::text("counter.name", "Name", "clicks")),
        )
        .with_keybindings("Keybindings")
        .with_scopes(vec!["User".into(), "Project".into()])
}

fn main() -> Result<(), Error> {
    let registry = CommandRegistry::new(COMMANDS).expect("the command table is valid");
    let mut app = Demo {
        counter: 0.0,
        focus: FocusTree::new(),
        ime: ImeFocus::new(),
        keymap: bind(&registry),
        page: settings_page(),
        page_open: false,
        registry,
        resolver: Resolver::new(KEY_TIMEOUT),
        theme: Theme::dark(),
        window: None,
    };
    app.open_page();
    misty::run(app, WindowOptions::new(TITLE))
}
