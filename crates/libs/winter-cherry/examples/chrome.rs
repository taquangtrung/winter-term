//! A frameless window with misty's own chrome: a title bar (drag it to move
//! the window, double-click it to maximize, drag any edge to resize) with a
//! menu button, over a tab strip. Every command lives in one command
//! registry, which binds the keys and fills both menus: Ctrl+T opens a
//! tab, Ctrl+Shift+Q or a tab's close button closes one (the last tab is
//! replaced, never left empty), Ctrl+Tab and Ctrl+Shift+Tab cycle tabs.
//! The same commands sit under the prefix Ctrl+K (`Ctrl+K t`, `Ctrl+K n`,
//! and `Ctrl+K x q` to quit): rest after Ctrl+K and a which-key popup lists
//! what may follow. Right-click the content for a context menu; in an open menu, Up and
//! Down move, Enter chooses, and Esc closes. Esc or the close button
//! quits. Pass `--light` for the built-in light theme.
//!
//! The tab strip and the content share a focus tree: Tab and Shift+Tab
//! move between them with a focus ring, a click focuses one, and while
//! the tab strip has the focus Left and Right (or `h` and `l`) switch
//! tabs. Each open menu is a focus trap: while it is open no key reaches
//! the app's commands, and when it closes the focus goes back where it
//! was.
//!
//! Every widget describes itself to screen readers: the title bar and its
//! buttons, the tabs, the content's lines, and an open menu's items; a
//! screen reader can focus the tab strip or the content and click any
//! button, tab, or menu item.
//!
//! ```sh
//! cargo run -p winter-cherry --example chrome -- --light
//! ```

use std::env;
use std::time::{Duration, Instant};

use misty::accesskit::{Node, Role};
use misty::cell::{Cell, CellBuffer, Style};
use misty::key::{Key, KeyCode, Mods};
use misty::text::{Span, StyledText};
use misty::theme::{Theme, ThemeToken};
use misty::{
    AccessAction, AppContext, Application, Command, CommandRegistry, Decorations, Error, FocusId,
    FocusTree, Frame, Input, KeyScope, Menu, MenuResponse, MenuSlot, Outcome, Point, PointerButton,
    PopupSide, Rect, Route, Size, TabId, TabStrip, TabStripResponse, TitleBar, TitleBarResponse,
    WhichKey, Widget, WindowId, WindowOptions,
};
use winter_cherry::cherry_keys::{Keymap, Resolution, Resolver};
use winter_cherry::keys::to_cherry;
use winter_cherry::{clock, keymap};

// ========================================================================
// Constants
// ========================================================================

/// The window title.
const TITLE: &str = "misty chrome";

/// The accessibility id of the title bar, which never takes the focus.
const TITLE_BAR: FocusId = FocusId::new("title-bar");

/// The focus id of the tab strip.
const TAB_STRIP: FocusId = FocusId::new("tab-strip");

/// The focus id of the content under the tab strip.
const CONTENT: FocusId = FocusId::new("content");

/// The focus id of the menu under the menu button.
const APP_MENU_FOCUS: FocusId = FocusId::new("app-menu");

/// The focus id of the context menu.
const CONTEXT_MENU_FOCUS: FocusId = FocusId::new("context-menu");

/// The argument that picks the light theme.
const LIGHT_FLAG: &str = "--light";

/// The smallest size the window may be resized to, in logical pixels.
const MIN_SIZE: Size = Size::new(320, 200);

/// How many tabs the demo opens with.
const START_TABS: usize = 3;

/// A tab title in wide characters.
const WIDE_TAB_TITLE: &str = "設定ファイルの一覧と漢字のタイトル";

/// How long a half-typed key sequence waits for its next key: long enough
/// to read the which-key popup that shows after half a second.
const KEY_TIMEOUT: Duration = Duration::from_secs(4);

/// The id of the command that closes the active tab.
const CLOSE_TAB: &str = "tab.close";

/// The id of the command that copies, never enabled: it shows a disabled
/// item.
const COPY: &str = "edit.copy";

/// The id of the command that opens a tab.
const NEW_TAB: &str = "tab.new";

/// The id of the command that activates the next tab.
const NEXT_TAB: &str = "tab.next";

/// The id of the command that activates the previous tab.
const PREVIOUS_TAB: &str = "tab.previous";

/// The id of the command that quits.
const QUIT: &str = "app.quit";

/// Every command of the demo, each written once: the keymap and both menus
/// come from here.
const COMMANDS: &[Command] = &[
    Command::new(NEW_TAB, "New Tab", "Tabs").with_keys(&["C-t", "C-k t"]),
    Command::new(CLOSE_TAB, "Close Tab", "Tabs").with_keys(&["C-S-q", "C-k w"]),
    Command::new(NEXT_TAB, "Next Tab", "Tabs").with_keys(&["C-<tab>", "C-k n"]),
    Command::new(PREVIOUS_TAB, "Previous Tab", "Tabs").with_keys(&["C-S-<tab>", "C-k p"]),
    Command::new(COPY, "Copy", "Edit")
        .with_keys(&["C-c"])
        .local_only(),
    Command::new(QUIT, "Quit", "App").with_keys(&["<esc>", "C-k x q"]),
];

/// The menu under the title bar's menu button.
const APP_MENU: &[MenuSlot] = &[
    MenuSlot::Command(NEW_TAB),
    MenuSlot::Command(CLOSE_TAB),
    MenuSlot::Separator,
    MenuSlot::Command(NEXT_TAB),
    MenuSlot::Command(PREVIOUS_TAB),
    MenuSlot::Separator,
    MenuSlot::Command(QUIT),
];

/// The menu a right click opens.
const CONTEXT_MENU: &[MenuSlot] = &[
    MenuSlot::Command(COPY),
    MenuSlot::Separator,
    MenuSlot::Command(NEW_TAB),
    MenuSlot::Command(CLOSE_TAB),
];

/// The lines shown under the tab strip, after the status line.
const LINES: [&str; 6] = [
    "Drag the title bar to move the window; double-click it to maximize.",
    "Drag any edge or corner to resize. The button at the left opens a menu.",
    "Ctrl+T opens a tab, Ctrl+Shift+Q or a close button closes one.",
    "Ctrl+Tab and Ctrl+Shift+Tab cycle tabs; middle-click closes a tab.",
    "Right-click here for a context menu; Up, Down, Enter, Esc drive it.",
    "Esc quits.",
];

/// The blank cells around the text, in columns and rows.
const MARGIN: usize = 1;

// ========================================================================
// Data Structures
// ========================================================================

/// The demo: a title bar, a tab strip, and two menus over a few lines of
/// text, all driven by one command registry.
struct Chrome {
    /// The menu under the title bar's menu button.
    app_menu: Menu<&'static str>,
    /// The text under the tab strip.
    content: CellBuffer,
    /// The menu opened by a right click.
    context_menu: Menu<&'static str>,
    /// Which of the window's widgets has the keyboard.
    focus: FocusTree,
    /// The registry's keys, in the one layer the demo has.
    keymap: Keymap<Layer, &'static str>,
    /// The id of the last command run, shown in the status line.
    last_command: Option<&'static str>,
    /// How many tabs have been opened, for numbering new ones.
    opened: usize,
    /// Every command, its keys, and whether it can run.
    registry: CommandRegistry,
    /// Turns keys into commands, one sequence at a time.
    resolver: Resolver,
    /// The window's tabs.
    tabs: TabStrip,
    /// The colors the window is drawn in.
    theme: Theme,
    /// The window's title bar.
    title_bar: TitleBar,
    /// Lists the keys that may follow a pending prefix.
    which_key: WhichKey,
    /// The open window, once it is open.
    window: Option<WindowId>,
}

/// The keymap layers of the demo.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Layer {
    /// The app's commands.
    App,
}

// ========================================================================
// Chrome Implementations
// ========================================================================

impl Chrome {
    /// Opens a numbered tab and makes it active.
    fn open_tab(&mut self) {
        self.opened += 1;
        self.tabs.push(format!("Tab {}", self.opened));
    }

    /// Closes `id`, opening a fresh tab when it was the last.
    fn close_tab(&mut self, id: TabId) {
        self.tabs.remove(id);
        if self.tabs.is_empty() {
            self.open_tab();
        }
    }

    /// Runs the command `id` and redraws.
    fn run_command(&mut self, cx: &mut AppContext<'_, ()>, id: &'static str) {
        if !self.registry.is_enabled(id) {
            return;
        }
        self.last_command = Some(id);
        match id {
            CLOSE_TAB => {
                if let Some(tab) = self.tabs.active() {
                    self.close_tab(tab);
                }
            }
            NEW_TAB => self.open_tab(),
            NEXT_TAB => {
                self.tabs.cycle(1);
            }
            PREVIOUS_TAB => {
                self.tabs.cycle(-1);
            }
            QUIT => cx.exit(),
            // Copy is never enabled, and no other id is registered.
            _ => {}
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

    /// Tells the which-key popup what the resolver now waits on.
    fn follow_pending(&mut self, cx: &mut AppContext<'_, ()>) {
        let layers = [Layer::App];
        let prefix = keymap::to_keys(self.resolver.pending());
        let continuations = self
            .keymap
            .active(&layers)
            .continuations(self.resolver.pending());
        let registry = &self.registry;
        let entries = keymap::which_key_entries(&continuations, |id| {
            registry
                .command(id)
                .map_or(*id, |command| command.title)
                .to_owned()
        });
        if let (true, Some(window)) = (self.which_key.set_pending(&prefix, entries), self.window) {
            cx.request_redraw(window);
        }
    }

    /// Opens the menu under the title bar's menu button, as a focus trap.
    fn open_app_menu(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId) {
        if let Some(button) = self.title_bar.menu_button() {
            self.app_menu = self.fresh_menu(APP_MENU);
            self.app_menu.open(button, PopupSide::Below);
            self.focus.open_trap(APP_MENU_FOCUS);
            cx.request_redraw(window);
        }
    }

    /// Describes the window to screen readers: the title bar, the tab
    /// strip, the content (the `status` line, then the help lines, one
    /// label a line), and the open menu.
    fn describe(&self, frame: &mut Frame, area: Rect, status: &StyledText) {
        self.title_bar.access(frame, TITLE_BAR);
        self.tabs.access(frame, TAB_STRIP);
        frame.enter_access(CONTENT, area, Node::new(Role::Document));
        let status_text: String = status
            .spans()
            .iter()
            .map(|span| span.text.as_str())
            .collect();
        let lines = [status_text.as_str()].into_iter().chain(LINES);
        for (index, line) in lines.enumerate() {
            let mut label = Node::new(Role::Label);
            label.set_value(line);
            frame.access(CONTENT.child(index as u64), area, label);
        }
        frame.leave_access();
        self.app_menu.access(frame, APP_MENU_FOCUS);
        self.context_menu.access(frame, CONTEXT_MENU_FOCUS);
    }

    /// A closed menu of `slots`, its items made fresh from the registry so
    /// they show the keys and availability in force.
    fn fresh_menu(&self, slots: &[MenuSlot]) -> Menu<&'static str> {
        Menu::new(self.registry.menu(slots).expect("menu slots name commands"))
    }

    /// Opens the context menu at `at`, as a focus trap.
    fn open_context_menu(&mut self, at: Point) {
        self.context_menu = self.fresh_menu(CONTEXT_MENU);
        self.context_menu
            .open(Rect::new(at.x, at.y, 0.0, 0.0), PopupSide::Below);
        self.focus.open_trap(CONTEXT_MENU_FOCUS);
    }
}

impl Application for Chrome {
    type Message = ();

    fn init(&mut self, _cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.window = Some(window);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId, input: &Input) {
        // The focus tree hands the input to the widget it is for: the
        // topmost one under the pointer, the focused one for a key, and
        // only the open menu while one is open (it is a trap).
        let (mut bar, mut strip, mut menu) = (None, None, None);
        let focused = self.focus.focused();
        let mut widget_cx = cx.event_cx(window);
        let (title_bar, tabs) = (&mut self.title_bar, &mut self.tabs);
        let open_menu = match self.app_menu.is_open() {
            true => &mut self.app_menu,
            false => &mut self.context_menu,
        };
        let route = self.focus.dispatch(input, |id, input| match id {
            APP_MENU_FOCUS | CONTEXT_MENU_FOCUS => {
                open_menu.handle(input, &mut widget_cx).keep(&mut menu)
            }
            CONTENT => input.press(PointerButton::Right).is_some(),
            TAB_STRIP => {
                let used = tabs.handle(input, &mut widget_cx).keep(&mut strip);
                used || input.key().and_then(tab_step).is_some()
            }
            TITLE_BAR => title_bar.handle(input, &mut widget_cx).keep(&mut bar),
            _ => false,
        });
        match bar {
            Some(TitleBarResponse::Close) => cx.exit(),
            Some(TitleBarResponse::OpenMenu) => self.open_app_menu(cx, window),
            _ => {}
        }
        match strip {
            Some(TabStripResponse::Close(tab)) => self.close_tab(tab),
            Some(TabStripResponse::NewTab) => self.open_tab(),
            _ => {}
        }
        if let Some(MenuResponse::Chosen(command)) = menu {
            self.run_command(cx, command);
        }
        match (route, input) {
            (Route::Delivered(CONTENT), Input::PointerButton(press)) => {
                self.open_context_menu(press.position);
            }
            (Route::Delivered(TAB_STRIP), Input::Key(key)) => {
                self.tabs.cycle(tab_step(*key).unwrap_or(0));
            }
            (Route::Unhandled, Input::CloseRequested) => cx.exit(),
            (Route::Unhandled, Input::Key(key)) => {
                let layers = [Layer::App];
                let keymap = self.keymap.active(&layers);
                let resolutions = self.resolver.feed(&keymap, to_cherry(*key), clock::now());
                self.run_resolutions(cx, resolutions);
                self.follow_pending(cx);
                return;
            }
            _ => {}
        }
        if route != Route::Unhandled || self.focus.focused() != focused {
            cx.request_redraw(window);
        }
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        let layers = [Layer::App];
        let settled = self
            .resolver
            .tick(&self.keymap.active(&layers), clock::now());
        self.run_resolutions(cx, settled);
        self.follow_pending(cx);
        let window = self.window?;
        let popup = self.which_key.about_to_wait(&mut cx.event_cx(window));
        [self.resolver.deadline().map(clock::instant), popup]
            .into_iter()
            .flatten()
            .min()
    }

    fn access_action(
        &mut self,
        cx: &mut AppContext<'_, ()>,
        window: WindowId,
        action: AccessAction,
    ) {
        cx.request_redraw(window);
        if let AccessAction::Focus(id) = action {
            self.focus.focus(id);
            return;
        }
        let menus = [
            self.app_menu.access_action(APP_MENU_FOCUS, action),
            self.context_menu.access_action(CONTEXT_MENU_FOCUS, action),
        ];
        for response in menus {
            if let MenuResponse::Chosen(id) = response {
                self.run_command(cx, id);
                return;
            }
        }
        match self
            .title_bar
            .access_action(TITLE_BAR, action, &mut cx.event_cx(window))
        {
            TitleBarResponse::Close => cx.exit(),
            // The tabs are a strip of their own here, not the bar's, and
            // the bar has no menubar.
            TitleBarResponse::Handled
            | TitleBarResponse::Ignored
            | TitleBarResponse::OpenMenuTitle(_)
            | TitleBarResponse::Tabs(_) => {}
            TitleBarResponse::OpenMenu => self.open_app_menu(cx, window),
        }
        match self.tabs.access_action(TAB_STRIP, action) {
            TabStripResponse::NewTab => self.open_tab(),
            // One strip and no editor groups: a tab dropped off the strip
            // stays where it was.
            TabStripResponse::Activated(_)
            | TabStripResponse::Close(_)
            | TabStripResponse::Dragging(_)
            | TabStripResponse::DroppedOutside(_)
            | TabStripResponse::Handled
            | TabStripResponse::Ignored
            | TabStripResponse::Moved(_) => {}
        }
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        frame.clear(self.theme.palette.background.into());
        self.focus.begin_frame();
        let bounds = frame.bounds();
        let below_title = self.title_bar.draw(frame, bounds, &self.theme);
        let area = self.tabs.draw(frame, below_title, &self.theme);
        let strip = Rect::new(
            below_title.x,
            below_title.y,
            below_title.width,
            area.y - below_title.y,
        );
        self.focus.register(TAB_STRIP, strip);
        self.focus.register(CONTENT, area);
        // Above the rest for the pointer, so its resize border along every
        // edge wins; anywhere else it declines and the widget under it
        // takes the input.
        self.focus.register_hit(TITLE_BAR, bounds);
        let metrics = frame.cell_metrics();
        self.content
            .resize(metrics.cols_in(area.width), metrics.rows_in(area.height));
        self.content.clear();
        for (row, line) in LINES.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                self.content.set(
                    MARGIN + row + 1,
                    MARGIN + col,
                    Cell::new(ch, Style::default()),
                );
            }
        }
        frame.draw_cells(&self.content, Point::new(area.x, area.y), &self.theme);
        let last = self
            .last_command
            .and_then(|id| self.registry.command(id))
            .map_or("none", |command| command.title);
        let status = StyledText::new()
            .with(
                Span::new(
                    format!("{} tabs open", self.tabs.len()),
                    ThemeToken::Accent.into(),
                )
                .bold(),
            )
            .with(Span::new("; last command: ", ThemeToken::Muted.into()))
            .with(
                Span::new(last, ThemeToken::Foreground.into())
                    .italic()
                    .underlined(),
            )
            .with_ellipsis();
        let origin = Point::new(
            area.x + MARGIN as f32 * metrics.width,
            area.y + MARGIN as f32 * metrics.height,
        );
        frame.draw_text(&status, origin, area, &self.theme);
        self.describe(frame, area, &status);
        self.which_key.draw(frame, area, &self.theme);
        if let Some(rect) = self.app_menu.draw(frame, bounds, &self.theme) {
            self.focus.register(APP_MENU_FOCUS, rect);
        }
        if let Some(rect) = self.context_menu.draw(frame, bounds, &self.theme) {
            self.focus.register(CONTEXT_MENU_FOCUS, rect);
        }
        self.focus.end_frame();
        self.focus.draw(frame, &self.theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How many tabs `key` steps through while the tab strip has the focus:
/// Left or `h` one back, Right or `l` one on; `None` for other keys.
fn tab_step(key: Key) -> Option<isize> {
    if key.mods != Mods::NONE {
        return None;
    }
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Some(-1),
        KeyCode::Char('l') | KeyCode::Right => Some(1),
        _ => None,
    }
}

/// The registry of [`COMMANDS`], with Copy disabled.
fn registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new(COMMANDS).expect("the command table is valid");
    registry.set_enabled(COPY, false);
    registry
}

fn main() -> Result<(), Error> {
    let theme = match env::args().any(|arg| arg == LIGHT_FLAG) {
        true => Theme::light(),
        false => Theme::dark(),
    };
    let registry = registry();
    let mut keymap = Keymap::new();
    keymap::bind(&registry, &mut keymap, Layer::App, KeyScope::All)
        .expect("every command has keys");
    let mut app = Chrome {
        app_menu: Menu::new(Vec::new()),
        content: CellBuffer::new(0, 0),
        context_menu: Menu::new(Vec::new()),
        focus: FocusTree::new(),
        keymap,
        last_command: None,
        opened: 0,
        registry,
        resolver: Resolver::new(KEY_TIMEOUT),
        tabs: TabStrip::new(),
        theme,
        title_bar: TitleBar::new(TITLE).with_menu_button(),
        which_key: WhichKey::new(),
        window: None,
    };
    for _ in 0..START_TABS {
        app.open_tab();
    }
    // Wide characters take two cells each, in the tab and its title.
    app.tabs.push(WIDE_TAB_TITLE);
    let options = WindowOptions::new(TITLE)
        .with_decorations(Decorations::Frameless)
        .with_min_size(MIN_SIZE);
    misty::run(app, options)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_menus_and_the_registry_name_the_same_commands() {
        let registry = registry();
        for slots in [APP_MENU, CONTEXT_MENU] {
            assert!(registry.menu(slots).is_ok(), "{slots:?}");
        }
        let in_a_menu = |id: &'static str| {
            [APP_MENU, CONTEXT_MENU]
                .iter()
                .any(|slots| slots.contains(&MenuSlot::Command(id)))
        };
        for command in registry.commands() {
            assert!(in_a_menu(command.id), "{} is in no menu", command.id);
        }
        assert!(registry.conflicts(KeyScope::All).is_empty());
    }
}
