//! A command palette and a quick pick over a long list, both built on
//! misty's one picker. Alt+X opens the palette of every command in the
//! registry with its keys; Ctrl+Alt+Z opens a picker over 100,000
//! generated items, matched on a worker thread. Type to filter, by words in
//! any order (matched characters are highlighted), Up and Down or Ctrl+P
//! and Ctrl+N move, Page Up and Page Down move a page, Enter chooses, F4
//! shows the row's actions, Esc closes; the
//! query line is a text field (Shift selects, Ctrl+C, Ctrl+X, and Ctrl+V
//! copy, cut, and paste, Ctrl+A, Ctrl+E, Ctrl+K edit). F5 counts, F6
//! switches between the grid style (square, flat boxes and monospace
//! labels) and the modern one (rounded boxes with soft shadows and
//! proportional menu labels), Esc with no picker open quits.
//!
//! ```sh
//! cargo run -p winter-cherry --example palette
//! ```

use std::time::{Duration, Instant};

use misty::text::{Span, StyledText};
use misty::theme::{ChromeText, Shape, Theme, ThemeToken};
use misty::{
    AppContext, Application, Command, CommandRegistry, Decorations, Error, EventCx, Frame,
    ImeFocus, Input, KeyScope, Point, QuickPick, Rect, Size, TitleBar, TitleBarResponse, Widget,
    WindowId, WindowOptions, command_palette, picker,
};
use winter_cherry::cherry::keys::{Keymap, Resolution, Resolver};
use winter_cherry::clock;
use winter_cherry::keymap;
use winter_cherry::keys::to_cherry;

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty palette";

/// The smallest size the window may be resized to, in logical pixels.
const MIN_SIZE: Size = Size::new(480, 320);

/// How long a half-typed key sequence waits for its next key.
const KEY_TIMEOUT: Duration = Duration::from_secs(1);

/// How many items the long picker offers.
const ITEM_COUNT: usize = 100_000;

/// The words generated item names are made of.
const WORDS: [&str; 8] = [
    "alpha", "bravo", "config", "delta", "editor", "files", "garden", "harbor",
];

/// The id of the command that opens the command palette.
const OPEN_PALETTE: &str = "palette.open";

/// The id of the command that opens the picker over the long list.
const OPEN_ITEMS: &str = "items.open";

/// The id of the command that adds one to the counter.
const INCREMENT: &str = "counter.increment";

/// The id of the command that sets the counter back to zero.
const RESET: &str = "counter.reset";

/// The id of the command that switches between the light and dark theme.
const TOGGLE_THEME: &str = "view.theme";

/// The id of the command that switches between the grid style and the
/// modern one.
const TOGGLE_SHAPE: &str = "view.shape";

/// The id of the command that quits.
const QUIT: &str = "app.quit";

/// Every command of the demo, each written once.
const COMMANDS: &[Command] = &[
    Command::new(OPEN_PALETTE, "Command Palette", "View").with_keys(&["M-x"]),
    Command::new(OPEN_ITEMS, "Go to Item", "View").with_keys(&["C-M-z"]),
    Command::new(TOGGLE_THEME, "Toggle Light Theme", "View"),
    Command::new(TOGGLE_SHAPE, "Toggle Modern Style", "View").with_keys(&["<f6>"]),
    Command::new(INCREMENT, "Increment", "Counter").with_keys(&["<f5>"]),
    Command::new(RESET, "Reset", "Counter").with_keys(&["C-c C-r"]),
    Command::new(QUIT, "Quit", "App").with_keys(&["<esc>"]),
];

/// The blank cells around the text, in columns and rows.
const MARGIN: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a title bar, a few lines of status, and at most one picker.
struct Palette {
    /// How many times Increment ran since the last Reset.
    counter: usize,
    /// Whether the input method is on: while a picker's query has focus.
    ime: ImeFocus,
    /// The names of the long picker's items.
    item_names: Vec<String>,
    /// The registry's keys: all of them, and those for while a picker's
    /// query line has focus.
    keymap: Keymap<Layer, &'static str>,
    /// What was last chosen, for the status line.
    last_choice: String,
    /// The open picker, if any.
    picker: Option<Picker>,
    /// Every command and its keys.
    registry: CommandRegistry,
    /// Turns keys into commands, one sequence at a time.
    resolver: Resolver,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The window's title bar.
    title_bar: TitleBar,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

/// The keymap layers of the demo.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Layer {
    /// Every command's keys.
    All,
    /// The keys that may run while a query line has focus.
    TextFocused,
}

/// The open picker.
enum Picker {
    /// The command palette, handing back command ids.
    Commands(QuickPick<&'static str>),
    /// The long list, handing back item indices.
    Items(QuickPick<usize>),
}

/// What a picker's response asks of the app.
enum PickerOutcome {
    /// Close the picker and run this command.
    Command(&'static str),
    /// Close the picker; this item was chosen.
    Item(usize),
    /// Close the picker.
    Close,
    /// Keep the picker open; the key or event was used.
    Kept,
    /// The key or event was not the picker's.
    Passed,
}

// ========================================================================
// Palette Implementations
// ========================================================================

impl Palette {
    /// The layers active now: fewer keys while a picker has focus.
    fn layers(&self) -> [Layer; 1] {
        match self.picker {
            Some(_) => [Layer::TextFocused],
            None => [Layer::All],
        }
    }

    /// Runs the command `id`.
    fn run_command(&mut self, cx: &mut AppContext<'_, ()>, id: &'static str) {
        match id {
            INCREMENT => self.counter += 1,
            OPEN_ITEMS => {
                if !matches!(self.picker, Some(Picker::Items(_))) {
                    let rows = self
                        .item_names
                        .iter()
                        .zip(0..)
                        .map(|(name, key)| picker::Row::new(picker::RowKey(key), name.as_str()))
                        .collect();
                    let source =
                        picker::ListSource::new("items", "Go to Item", rows, "Go", |key| {
                            usize::try_from(key.0).ok()
                        });
                    let session = picker::Picker::open(
                        Box::new(source),
                        picker::PickerMemory::default(),
                        cx.waker(),
                    );
                    self.picker = Some(Picker::Items(QuickPick::new(session)));
                }
            }
            OPEN_PALETTE => {
                // Opening it again while it is open keeps the query.
                if !matches!(self.picker, Some(Picker::Commands(_))) {
                    self.picker = Some(Picker::Commands(command_palette(
                        cx.waker(),
                        &self.registry,
                        &self.theme,
                    )));
                }
            }
            QUIT => cx.exit(),
            RESET => self.counter = 0,
            TOGGLE_THEME => {
                let is_dark = self.theme.palette == Theme::dark().palette;
                self.theme = Theme {
                    chrome_text: self.theme.chrome_text,
                    shape: self.theme.shape,
                    ..match is_dark {
                        true => Theme::light(),
                        false => Theme::dark(),
                    }
                };
            }
            TOGGLE_SHAPE => {
                (self.theme.shape, self.theme.chrome_text) = match self.theme.shape == Shape::GRID {
                    true => (Shape::MODERN, ChromeText::Proportional),
                    false => (Shape::GRID, ChromeText::Cells),
                };
            }
            // No other id is registered.
            _ => {}
        }
        if let Some(command) = self.registry.command(id) {
            self.last_choice = format!("ran {}", command.title);
        }
        if let Some(window) = self.window {
            cx.request_redraw(window);
        }
    }

    /// Runs what `resolutions` completed.
    fn run_resolutions(
        &mut self,
        cx: &mut AppContext<'_, ()>,
        resolutions: impl IntoIterator<Item = Resolution<&'static str>>,
    ) {
        for resolution in resolutions {
            match resolution {
                Resolution::Matched(found) => self.run_command(cx, found.command),
                Resolution::Pending | Resolution::Unmatched(_) => {}
            }
        }
    }

    /// Acts on what the picker made of a key or an event; whether the app
    /// should still handle it.
    fn settle(&mut self, cx: &mut AppContext<'_, ()>, outcome: PickerOutcome) -> bool {
        if let Some(window) = self.window {
            cx.request_redraw(window);
        }
        match outcome {
            PickerOutcome::Close => self.picker = None,
            PickerOutcome::Command(id) => {
                self.picker = None;
                self.run_command(cx, id);
            }
            PickerOutcome::Item(index) => {
                self.picker = None;
                self.last_choice = format!("picked {}", self.item_names[index]);
            }
            PickerOutcome::Kept => {}
            PickerOutcome::Passed => return true,
        }
        false
    }

    /// Offers `input` to the open picker, which is modal: it takes every
    /// input while it is open.
    fn picker_input(&mut self, cx: &mut EventCx<'_>, input: &Input) -> PickerOutcome {
        match &mut self.picker {
            Some(Picker::Commands(picker)) => {
                outcome(picker.handle(input, cx), PickerOutcome::Command)
            }
            Some(Picker::Items(picker)) => outcome(picker.handle(input, cx), PickerOutcome::Item),
            None => PickerOutcome::Passed,
        }
    }
}

impl Application for Palette {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        let picker_outcome = self.picker_input(&mut cx.event_cx(window), input);
        if !self.settle(cx, picker_outcome) {
            return;
        }
        match self.title_bar.handle(input, &mut cx.event_cx(window)) {
            TitleBarResponse::Close => {
                cx.exit();
                return;
            }
            // No menubar and no tabs in this bar.
            TitleBarResponse::Handled
            | TitleBarResponse::OpenMenu
            | TitleBarResponse::OpenMenuTitle(_)
            | TitleBarResponse::Tabs(_) => return,
            TitleBarResponse::Ignored => {}
        }
        if let Some(key) = input.key() {
            let layers = self.layers();
            let resolutions =
                self.resolver
                    .feed(&self.keymap.active(&layers), to_cherry(key), clock::now());
            self.run_resolutions(cx, resolutions);
            return;
        }
        if *input == Input::CloseRequested {
            cx.exit();
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let again = self.window.and_then(|window| {
            let cx = &mut cx.event_cx(window);
            match &mut self.picker {
                Some(Picker::Commands(picker)) => picker.tick(cx).again,
                Some(Picker::Items(picker)) => picker.tick(cx).again,
                None => None,
            }
        });
        let layers = self.layers();
        let settled = self
            .resolver
            .tick(&self.keymap.active(&layers), clock::now());
        self.run_resolutions(cx, settled);
        if let Some(window) = self.window {
            let caret = match &self.picker {
                Some(Picker::Commands(picker)) => picker.cursor_area(),
                Some(Picker::Items(picker)) => picker.cursor_area(),
                None => None,
            };
            self.ime.follow(&mut cx.event_cx(window), caret);
        }
        [self.resolver.deadline().map(clock::instant), again]
            .into_iter()
            .flatten()
            .min()
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.palette.background.into());
        let bounds = frame.bounds();
        let area = self.title_bar.draw(frame, bounds, &self.theme);
        let metrics = frame.cell_metrics();
        let x = area.x + MARGIN * metrics.width;
        let lines = [
            StyledText::new()
                .with(Span::new("Counter: ", ThemeToken::Muted.into()))
                .with(Span::new(self.counter.to_string(), ThemeToken::Accent.into()).bold()),
            StyledText::new()
                .with(Span::new("Last: ", ThemeToken::Muted.into()))
                .with(Span::new(
                    self.last_choice.as_str(),
                    ThemeToken::Foreground.into(),
                ))
                .with_ellipsis(),
            StyledText::plain(
                "Alt+X: command palette. Ctrl+Alt+Z: go to one of 100,000 items.",
                ThemeToken::Muted.into(),
            )
            .with_ellipsis(),
            StyledText::plain(
                "F5 counts, Ctrl+C Ctrl+R resets, Esc quits.",
                ThemeToken::Muted.into(),
            )
            .with_ellipsis(),
        ];
        for (row, line) in lines.iter().enumerate() {
            let y = area.y + (MARGIN + row as f32) * metrics.height;
            let clip = Rect::new(area.x, y, area.width, metrics.height);
            frame.draw_text(line, Point::new(x, y), clip, &self.theme);
        }
        match &mut self.picker {
            Some(Picker::Commands(picker)) => picker.draw(frame, bounds, &self.theme),
            Some(Picker::Items(picker)) => picker.draw(frame, bounds, &self.theme),
            None => {}
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What `response` from a picker handing back `T` asks of the app, a
/// chosen value turned into an outcome by `chosen`.
fn outcome<T>(response: picker::Reply<T>, chosen: impl Fn(T) -> PickerOutcome) -> PickerOutcome {
    match response {
        picker::Reply::Run(value) => chosen(value),
        picker::Reply::Close => PickerOutcome::Close,
        picker::Reply::Consumed | picker::Reply::NothingToSave => PickerOutcome::Kept,
        picker::Reply::Ignored => PickerOutcome::Passed,
    }
}

/// The name of generated item `index`: two words and a number.
fn item_name(index: usize) -> String {
    let first = WORDS[index % WORDS.len()];
    let second = WORDS[(index / WORDS.len()) % WORDS.len()];
    format!("{first}/{second}_{index}.txt")
}

fn main() -> Result<(), Error> {
    let registry = CommandRegistry::new(COMMANDS).expect("the command table is valid");
    let mut keymap = Keymap::new();
    for (layer, scope) in [
        (Layer::All, KeyScope::All),
        (Layer::TextFocused, KeyScope::TextFocused),
    ] {
        keymap::bind(&registry, &mut keymap, layer, scope).expect("every command has keys");
    }
    let app = Palette {
        counter: 0,
        ime: ImeFocus::new(),
        item_names: (0..ITEM_COUNT).map(item_name).collect(),
        keymap,
        last_choice: String::from("nothing yet"),
        picker: None,
        registry,
        resolver: Resolver::new(KEY_TIMEOUT),
        theme: Theme::dark(),
        title_bar: TitleBar::new(TITLE),
        window: None,
    };
    let options = WindowOptions::new(TITLE)
        .with_decorations(Decorations::Frameless)
        .with_min_size(MIN_SIZE);
    misty::run(app, options)
}
