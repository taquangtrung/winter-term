//! Configurable bindings: parsing chords and resolving them to actions.

use super::*;

// ========================================================================
// Items
// ========================================================================

/// A configurable Insert-mode line edit at the shell prompt. Each is realized by
/// sending the equivalent readline keystrokes (the app layer owns that mapping)
/// and folding the same change into the prompt undo shadow.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditAction {
    DeleteToLineEnd,
    DeleteToLineStart,
    DeleteWordBackward,
    DeleteWordForward,
}
impl EditAction {
    /// Parse a config action name (shared with the binding docs).
    pub(super) fn from_name(name: &str) -> Option<EditAction> {
        Some(match name {
            "delete_to_line_end" => EditAction::DeleteToLineEnd,
            "delete_to_line_start" => EditAction::DeleteToLineStart,
            "delete_word_backward" => EditAction::DeleteWordBackward,
            "delete_word_forward" => EditAction::DeleteWordForward,
            _ => return None,
        })
    }
}
/// A configurable binding in the `editing` block: either a line edit, or a
/// prompt-history command (undo/redo). Line edits apply only in Insert mode;
/// undo/redo apply in Insert, Normal, and the palette.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditBinding {
    Edit(EditAction),
    Redo,
    Undo,
}
impl EditBinding {
    /// Parse a config action name. Recognizes the line-edit names plus
    /// `prompt_undo`/`undo` and `prompt_redo`/`redo`.
    pub(super) fn from_name(name: &str) -> Option<EditBinding> {
        Some(match name {
            "prompt_redo" | "redo" => EditBinding::Redo,
            "prompt_undo" | "undo" => EditBinding::Undo,
            other => EditBinding::Edit(EditAction::from_name(other)?),
        })
    }

    /// The dispatched [`Action`] this binding produces.
    pub(super) fn to_action(self) -> Action {
        match self {
            EditBinding::Edit(edit) => Action::Edit(edit),
            EditBinding::Redo => Action::PromptRedo,
            EditBinding::Undo => Action::PromptUndo,
        }
    }

    /// Whether this binding is a prompt-history command, which (unlike line
    /// edits) is also active in Normal mode and the palette.
    pub(super) fn is_history(self) -> bool {
        matches!(self, EditBinding::Redo | EditBinding::Undo)
    }
}
/// A window-management command: the configurable subset of Normal-mode bindings.
/// Each maps to a layout-affecting [`Action`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WindowAction {
    Close,
    CloseOthers,
    FocusDown,
    FocusLeft,
    FocusRight,
    FocusUp,
    SplitHorizontal,
    SplitVertical,
    Zoom,
    ScrollPageUp,
    ScrollPageDown,
    ScrollLineUp,
    ScrollLineDown,
    ScrollToTop,
    ScrollToBottom,
    PrevTab,
    NextTab,
    Copy,
    Paste,
    NewTab,
    CloseTab,
    OpenSettings,
    FontIncrease,
    FontDecrease,
    FontReset,
    TogglePalette,
    ToggleHistoryPalette,
    TogglePaneSwitcher,
    ToggleSwoop,
    NextBlock,
    PrevBlock,
}
impl WindowAction {
    /// Whether this action scrolls Winter's own scrollback. These bindings are
    /// bypassed when a full-screen (alt-screen) app is running so the key
    /// reaches the app instead — scrolling a TUI app's non-existent scrollback
    /// is never useful, and intercepting the key would swallow app shortcuts
    /// like Claude Code's `Shift+Alt+E/H/L`.
    pub(super) fn is_scroll(self) -> bool {
        matches!(
            self,
            WindowAction::ScrollPageUp
                | WindowAction::ScrollPageDown
                | WindowAction::ScrollLineUp
                | WindowAction::ScrollLineDown
                | WindowAction::ScrollToTop
                | WindowAction::ScrollToBottom
        )
    }

    /// Whether this action must intercept the key before overlays (the command
    /// palette, tab rename, settings page) get a chance to consume it — e.g.
    /// `Ctrl-,` must open Settings even while the palette is open. Window
    /// layout actions (split/close/focus/scroll/zoom/tab-cycle) are
    /// deliberately excluded: they only apply once no overlay owns the key.
    pub(super) fn is_global(self) -> bool {
        matches!(
            self,
            WindowAction::Copy
                | WindowAction::Paste
                | WindowAction::NewTab
                | WindowAction::CloseTab
                | WindowAction::OpenSettings
                | WindowAction::FontIncrease
                | WindowAction::FontDecrease
                | WindowAction::FontReset
                | WindowAction::TogglePalette
                | WindowAction::ToggleHistoryPalette
                | WindowAction::TogglePaneSwitcher
                | WindowAction::ToggleSwoop
                | WindowAction::NextBlock
                | WindowAction::PrevBlock
        )
    }
}
/// Which digit-indexed pane/tab operation an [`IndexAction`] performs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum IndexKind {
    ClosePane,
    FocusPane,
    GotoTab,
}
/// A chord bound to one *specific* pane or tab index (e.g. config action
/// `focus_pane_3` always focuses pane 3, whichever key triggers it).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct IndexAction {
    pub(super) kind: IndexKind,
    pub(super) n: usize,
}
impl IndexAction {
    pub(super) fn to_action(self) -> Action {
        match self.kind {
            IndexKind::ClosePane => Action::ClosePaneByIndex(self.n),
            IndexKind::FocusPane => Action::FocusPaneByIndex(self.n),
            IndexKind::GotoTab => Action::GotoTab(self.n),
        }
    }

    /// Parse a config action name like `close_pane_3`, `focus_pane_3`, or
    /// `goto_tab_3` (`n` must be `1..=9`).
    pub(super) fn from_name(name: &str) -> Option<Self> {
        let (kind, rest) = if let Some(rest) = name.strip_prefix("close_pane_") {
            (IndexKind::ClosePane, rest)
        } else if let Some(rest) = name.strip_prefix("focus_pane_") {
            (IndexKind::FocusPane, rest)
        } else if let Some(rest) = name.strip_prefix("goto_tab_") {
            (IndexKind::GotoTab, rest)
        } else {
            return None;
        };
        let n: usize = rest.parse().ok()?;
        (1..=9).contains(&n).then_some(IndexAction { kind, n })
    }
}
/// User-configurable window-management key bindings. A binding is either a
/// direct chord (e.g. `Ctrl-h` to focus left) or a two-key sequence opened by
/// the `leader` (e.g. `Ctrl-w` then `v` to split). Built from defaults and
/// overlaid with config via [`WindowKeymap::from_config`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowKeymap {
    /// Direct chords: the full key (with modifiers) triggers the action.
    pub(super) direct: Vec<(Key, WindowAction)>,
    /// Line-edit and prompt-history chords (e.g. `Ctrl-Backspace` → delete word
    /// back, `Ctrl-/` → undo, `Ctrl-\` → redo).
    pub(super) editing: Vec<(Key, EditBinding)>,
    /// Chords for globally-intercepted actions (settings, font size, tab/pane
    /// management, palette toggles): looked up before the command palette, tab
    /// rename, and settings overlays get a chance to consume the key.
    pub(super) global: Vec<(Key, WindowAction)>,
    /// Chords bound to one specific pane/tab index (config actions
    /// `close_pane_N`/`focus_pane_N`/`goto_tab_N`, `N` = `1..9`).
    pub(super) index_specific: Vec<(Key, IndexAction)>,
    /// The chord that opens a two-key window-command sequence.
    pub(super) leader: Key,
    /// Direct chords bound to a command-palette action name with no
    /// `WindowAction` variant (`mux_new_session`, `export_block_svg`, …),
    /// dispatched through `run_command` instead of `to_action()`.
    pub(super) named: Vec<(Key, String)>,
    /// Keys that select an action when pressed after the `leader`, matched by
    /// key code alone (modifiers on the follow key are ignored, as in Vim).
    pub(super) sequence: Vec<(KeyCode, WindowAction)>,
}
/// The built-in default keybindings, embedded from the canonical file (also
/// installed as a reference copy at `~/.config/winter-term/keybindings.kdl`).
/// [`WindowKeymap::default`] parses this through the same pipeline used for
/// the user's own config, so there is exactly one place defining what the
/// defaults are.
pub(super) const DEFAULT_KEYBINDINGS_KDL: &str = include_str!("../../../samples/keybindings.kdl");
/// The leader key when no two-chord `window` binding (default or user) has
/// set one explicitly. There is no built-in two-chord default to parse this
/// from, so it stays a plain constant.
pub(super) const DEFAULT_LEADER: Key = Key {
    alt: false,
    code: KeyCode::Char('w'),
    ctrl: true,
    shift: false,
};
impl WindowAction {
    /// The dispatched [`Action`] this window command produces.
    pub(super) fn to_action(self) -> Action {
        match self {
            WindowAction::Close => Action::ClosePane,
            WindowAction::CloseOthers => Action::CloseOtherPanes,
            WindowAction::FocusDown => Action::FocusPane(FocusDir::Down),
            WindowAction::FocusLeft => Action::FocusPane(FocusDir::Left),
            WindowAction::FocusRight => Action::FocusPane(FocusDir::Right),
            WindowAction::FocusUp => Action::FocusPane(FocusDir::Up),
            WindowAction::SplitHorizontal => Action::SplitPane(Direction::Horizontal),
            WindowAction::SplitVertical => Action::SplitPane(Direction::Vertical),
            WindowAction::Zoom => Action::ZoomPane,
            WindowAction::ScrollPageUp => Action::ScrollPageUp,
            WindowAction::ScrollPageDown => Action::ScrollPageDown,
            WindowAction::ScrollLineUp => Action::ScrollLineUp,
            WindowAction::ScrollLineDown => Action::ScrollLineDown,
            WindowAction::ScrollToTop => Action::ScrollToTop,
            WindowAction::ScrollToBottom => Action::ScrollToBottom,
            WindowAction::PrevTab => Action::PrevTab,
            WindowAction::NextTab => Action::NextTab,
            WindowAction::Copy => Action::Copy,
            WindowAction::Paste => Action::Paste,
            WindowAction::NewTab => Action::NewTab,
            WindowAction::CloseTab => Action::CloseTab(None),
            WindowAction::OpenSettings => Action::OpenSettings,
            WindowAction::FontIncrease => Action::IncreaseFontSize,
            WindowAction::FontDecrease => Action::DecreaseFontSize,
            WindowAction::FontReset => Action::ResetFontSize,
            WindowAction::TogglePalette => Action::TogglePalette,
            WindowAction::ToggleHistoryPalette => Action::ToggleHistoryPalette,
            WindowAction::TogglePaneSwitcher => Action::TogglePaneSwitcher,
            WindowAction::ToggleSwoop => Action::ToggleSwoop,
            WindowAction::NextBlock => Action::FocusBlock(BlockNav::Next),
            WindowAction::PrevBlock => Action::FocusBlock(BlockNav::Previous),
        }
    }

    /// Parse a config action name (matching the command-palette names).
    pub(super) fn from_name(name: &str) -> Option<WindowAction> {
        Some(match name {
            "close_pane" => WindowAction::Close,
            "close_other_panes" => WindowAction::CloseOthers,
            "focus_down" => WindowAction::FocusDown,
            "focus_left" => WindowAction::FocusLeft,
            "focus_right" => WindowAction::FocusRight,
            "focus_up" => WindowAction::FocusUp,
            "split_horizontal" => WindowAction::SplitHorizontal,
            "split_vertical" => WindowAction::SplitVertical,
            "toggle_pane_zoom" | "zoom_pane" => WindowAction::Zoom,
            "scroll_page_up" => WindowAction::ScrollPageUp,
            "scroll_page_down" => WindowAction::ScrollPageDown,
            "scroll_line_up" => WindowAction::ScrollLineUp,
            "scroll_line_down" => WindowAction::ScrollLineDown,
            "scroll_to_top" => WindowAction::ScrollToTop,
            "scroll_to_bottom" => WindowAction::ScrollToBottom,
            "prev_tab" => WindowAction::PrevTab,
            "next_tab" => WindowAction::NextTab,
            "copy_selection" => WindowAction::Copy,
            "paste_from_clipboard" => WindowAction::Paste,
            "new_tab" => WindowAction::NewTab,
            "close_tab" => WindowAction::CloseTab,
            "open_settings" => WindowAction::OpenSettings,
            "font_increase" => WindowAction::FontIncrease,
            "font_decrease" => WindowAction::FontDecrease,
            "font_reset" => WindowAction::FontReset,
            "toggle_command_palette" => WindowAction::TogglePalette,
            "toggle_history_palette" => WindowAction::ToggleHistoryPalette,
            "select_pane" => WindowAction::TogglePaneSwitcher,
            "swoop" | "toggle_swoop" => WindowAction::ToggleSwoop,
            "next_block" => WindowAction::NextBlock,
            "prev_block" => WindowAction::PrevBlock,
            _ => return None,
        })
    }
}
/// The dispatched action for a config action name (`copy_selection`, …) that
/// mirrors a window command, or `None` when the name names none. Shared by
/// the keymap parser and the command palette so both dispatch identically.
pub(crate) fn window_action_by_name(name: &str) -> Option<Action> {
    WindowAction::from_name(name).map(WindowAction::to_action)
}
impl WindowKeymap {
    /// An unbound keymap: no chords at all. Only ever used as the starting
    /// point for [`from_config`](Self::from_config), which immediately layers
    /// the built-in defaults onto it before anything else.
    pub(super) fn empty() -> Self {
        Self {
            direct: vec![],
            editing: vec![],
            global: vec![],
            index_specific: vec![],
            leader: DEFAULT_LEADER,
            named: vec![],
            sequence: vec![],
        }
    }

    /// Build a keymap from the `window` keybindings block, layered onto the
    /// built-in defaults (parsed from [`DEFAULT_KEYBINDINGS_KDL`]). A
    /// configured binding replaces every default chord for that same action,
    /// so rebinding `split_vertical` drops the default `Ctrl-w v`; actions
    /// left unmentioned keep their defaults.
    pub fn from_config(
        window: Option<&HashMap<String, String>>,
        editing: Option<&HashMap<String, String>>,
    ) -> Self {
        let defaults = default_bindings_maps();
        let mut keymap = Self::empty();
        keymap.apply_window_bindings(defaults.get("window"));
        keymap.apply_editing_bindings(defaults.get("editing"));
        keymap.apply_window_bindings(window);
        keymap.apply_editing_bindings(editing);
        keymap
    }

    /// Overlay the `window` block onto the default window chords. A configured
    /// binding replaces every default chord for that same action; a
    /// single-key chord bound to an unrecognized name becomes a named
    /// binding instead, displacing any `WindowAction` on that same chord.
    pub(super) fn apply_window_bindings(&mut self, bindings: Option<&HashMap<String, String>>) {
        let Some(bindings) = bindings else {
            return;
        };
        self.apply_index_bindings(bindings);

        let mut parsed: Vec<(Vec<Key>, WindowAction)> = Vec::new();
        let mut named: Vec<(Key, String)> = Vec::new();
        for (spec, name) in bindings {
            let Some(keys) = parse_chord_sequence(spec) else {
                continue;
            };
            if let Some(action) = WindowAction::from_name(name) {
                parsed.push((keys, action));
            } else if IndexAction::from_name(name).is_none() {
                // `close_pane_N`/`focus_pane_N`/`goto_tab_N` already have a
                // home in `index_specific` via `apply_index_bindings` above;
                // only a name neither table recognizes becomes a named
                // command, or those chords would double-resolve.
                if let [single] = keys.as_slice() {
                    named.push((single.clone(), name.clone()));
                }
            }
        }

        if !named.is_empty() {
            self.direct
                .retain(|(key, _)| !named.iter().any(|(k, _)| k == key));
            self.global
                .retain(|(key, _)| !named.iter().any(|(k, _)| k == key));
            self.named.extend(named);
        }

        if parsed.is_empty() {
            return;
        }

        let rebound: HashSet<WindowAction> = parsed.iter().map(|(_, action)| *action).collect();
        self.direct.retain(|(_, action)| !rebound.contains(action));
        self.global.retain(|(_, action)| !rebound.contains(action));
        self.sequence
            .retain(|(_, action)| !rebound.contains(action));

        for (keys, action) in parsed {
            match keys.as_slice() {
                [single] if action.is_global() => self.global.push((single.clone(), action)),
                [single] => self.direct.push((single.clone(), action)),
                [leader, follow] => {
                    self.leader = leader.clone();
                    self.sequence.push((follow.code, action));
                }
                _ => {}
            }
        }
    }

    /// Overlay the `close_pane_N`/`focus_pane_N`/`goto_tab_N` bindings from
    /// the `window` block. A configured chord replaces every prior chord for
    /// that same specific index, including the built-in default.
    pub(super) fn apply_index_bindings(&mut self, bindings: &HashMap<String, String>) {
        let parsed: Vec<(Key, IndexAction)> = bindings
            .iter()
            .filter_map(|(spec, name)| Some((parse_chord(spec)?, IndexAction::from_name(name)?)))
            .collect();
        if parsed.is_empty() {
            return;
        }
        let rebound: HashSet<IndexAction> = parsed.iter().map(|(_, action)| *action).collect();
        self.index_specific
            .retain(|(_, action)| !rebound.contains(action));
        self.index_specific.extend(parsed);
    }

    /// Overlay the `editing` block onto the default line-edit chords. A configured
    /// binding replaces every default chord for that same action.
    pub(super) fn apply_editing_bindings(&mut self, bindings: Option<&HashMap<String, String>>) {
        let Some(bindings) = bindings else {
            return;
        };
        let parsed: Vec<(Key, EditBinding)> = bindings
            .iter()
            .filter_map(|(spec, name)| Some((parse_chord(spec)?, EditBinding::from_name(name)?)))
            .collect();
        if parsed.is_empty() {
            return;
        }
        let rebound: HashSet<EditBinding> = parsed.iter().map(|(_, action)| *action).collect();
        self.editing.retain(|(_, action)| !rebound.contains(action));
        self.editing.extend(parsed);
    }

    /// The [`Action`] a direct chord triggers: a `WindowAction` chord first
    /// (skipped without falling through when it's a scroll binding and
    /// `is_alt_screen` is true), else a palette-only named command.
    pub(super) fn direct_action(&self, key: &Key, is_alt_screen: bool) -> Option<Action> {
        if let Some(action) = find_chord(&self.direct, key) {
            return (!(is_alt_screen && action.is_scroll())).then(|| action.to_action());
        }
        find_chord(&self.named, key).map(Action::RunCommand)
    }

    /// The globally-intercepted [`Action`] a chord triggers, if any (see
    /// [`WindowAction::is_global`]). Checked before the command palette, tab
    /// rename, and settings overlays get a chance to consume the key.
    pub(crate) fn global_action(&self, key: &Key) -> Option<Action> {
        find_chord(&self.global, key).map(WindowAction::to_action)
    }

    /// The pane/tab action a chord triggers via an index binding
    /// (`close_pane_N`/`focus_pane_N`/`goto_tab_N`), if any — including the
    /// built-in defaults.
    pub(super) fn specific_index_action(&self, key: &Key) -> Option<Action> {
        find_chord(&self.index_specific, key).map(IndexAction::to_action)
    }

    /// The line-edit or prompt-history binding a chord triggers, if any. Like
    /// [`direct_action`](Self::direct_action) it retries with the unshifted glyph
    /// so specs using the physical key label still match.
    pub(crate) fn edit_binding(&self, key: &Key) -> Option<EditBinding> {
        find_chord(&self.editing, key)
    }

    /// The action the follow key selects after the leader, if any.
    pub(super) fn sequence_action(&self, code: KeyCode) -> Option<WindowAction> {
        self.sequence
            .iter()
            .find(|(follow, _)| *follow == code)
            .map(|(_, action)| *action)
    }
}
impl Default for WindowKeymap {
    /// Built from the embedded default keybindings ([`DEFAULT_KEYBINDINGS_KDL`])
    /// through the same parsing pipeline as user config — see
    /// [`from_config`](Self::from_config).
    fn default() -> Self {
        Self::from_config(None, None)
    }
}
/// Parse [`DEFAULT_KEYBINDINGS_KDL`] into `window`/`editing` action maps, the
/// same shape a user's `keybindings.kdl` parses into.
pub(super) fn default_bindings_maps() -> HashMap<String, HashMap<String, String>> {
    kdl::de::from_str(DEFAULT_KEYBINDINGS_KDL).unwrap_or_default()
}
impl WindowKeymap {
    /// Return the formatted shortcut hint for a palette command name
    /// (e.g. `"Ctrl-H"` for `"focus_left"`), or an empty string when unbound.
    pub fn chord_hint(&self, command: &str) -> String {
        let Some(action) = WindowAction::from_name(command) else {
            return self
                .named
                .iter()
                .find(|(_, name)| name == command)
                .map(|(key, _)| format_key(key))
                .unwrap_or_default();
        };
        self.direct
            .iter()
            .chain(self.global.iter())
            .find(|(_, a)| *a == action)
            .map(|(key, _)| format_key(key))
            .unwrap_or_default()
    }
}
/// Format a [`Key`] as a human-readable shortcut string (e.g. `"Ctrl-Shift-T"`).
pub fn format_key(key: &Key) -> String {
    let mut s = String::new();
    if key.ctrl {
        s.push_str("Ctrl-");
    }
    if key.shift {
        s.push_str("Shift-");
    }
    if key.alt {
        s.push_str("Alt-");
    }
    match key.code {
        KeyCode::Char(c) => s.push(c),
        KeyCode::Backspace => s.push_str("Backspace"),
        KeyCode::Delete => s.push_str("Del"),
        KeyCode::Down => s.push_str("Down"),
        KeyCode::End => s.push_str("End"),
        KeyCode::Enter => s.push_str("Enter"),
        KeyCode::Escape => s.push_str("Esc"),
        KeyCode::F(n) => {
            s.push('F');
            s.push_str(&n.to_string());
        }
        KeyCode::Home => s.push_str("Home"),
        KeyCode::Insert => s.push_str("Ins"),
        KeyCode::Left => s.push_str("Left"),
        KeyCode::PageDown => s.push_str("PgDn"),
        KeyCode::PageUp => s.push_str("PgUp"),
        KeyCode::Right => s.push_str("Right"),
        KeyCode::Space => s.push_str("Space"),
        KeyCode::Tab => s.push_str("Tab"),
        KeyCode::Up => s.push_str("Up"),
    }
    s
}
/// Parse a key-binding spec of one or two chords (e.g. `"C+h"` or `"C+w v"`)
/// into its keys. Returns `None` on any unrecognized chord or a length outside
/// 1..=2.
pub(super) fn parse_chord_sequence(spec: &str) -> Option<Vec<Key>> {
    let keys = spec
        .split_whitespace()
        .map(parse_chord)
        .collect::<Option<Vec<Key>>>()?;
    if (1..=2).contains(&keys.len()) {
        Some(keys)
    } else {
        None
    }
}
/// Look up `key` in a chord table, retrying with the unshifted glyph if the
/// first lookup misses. winit reports `logical_key`, which applies the Shift
/// character transformation (`Shift+\` -> `'|'`, `Shift+-` -> `'_'`,
/// `Shift+o` -> `'O'`, ...); binding specs use the physical key label, so a
/// held-Shift chord that misses on the transformed glyph retries on the
/// untransformed one.
pub(super) fn find_chord<T: Clone>(entries: &[(Key, T)], key: &Key) -> Option<T> {
    if let Some((_, action)) = entries.iter().find(|(chord, _)| chord == key) {
        return Some(action.clone());
    }
    if key.shift {
        if let KeyCode::Char(c) = key.code {
            if let Some(base) = unshift_char(c) {
                let base_key = Key {
                    code: KeyCode::Char(base),
                    ..*key
                };
                return entries
                    .iter()
                    .find(|(chord, _)| chord == &base_key)
                    .map(|(_, a)| a.clone());
            }
        }
    }
    None
}
/// Map a Shift-modified character back to its physical key for US QWERTY,
/// so binding specs like `"S+M+\\"` match the winit `logical_key` `'|'`.
pub(super) fn unshift_char(c: char) -> Option<char> {
    Some(match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return None,
    })
}
/// Parse a single chord like `"C+S+Space"` or `"C+w"` into a [`Key`].
/// Modifiers are `+`-separated and precede the key name. Accepted modifier
/// tokens (case-sensitive abbreviations take priority):
///   `C` or `ctrl`/`control` = Ctrl,
///   `S` or `shift` = Shift,
///   `M` or `alt`/`meta`/`option` = Alt.
///
/// A literal `+` key is written as a trailing `++` (e.g. `"C+S++"`), which
/// produces two consecutive empty segments when split on `+`.
pub(super) fn parse_chord(token: &str) -> Option<Key> {
    let parts: Vec<&str> = token.split('+').collect();
    let n = parts.len();

    // Two consecutive trailing empty segments mean the key literal is `+`
    // (e.g. "C+S++" splits into ["C", "S", "", ""]).
    let (mods, code) = if n >= 2 && parts[n - 1].is_empty() && parts[n - 2].is_empty() {
        (&parts[..n - 2], KeyCode::Char('+'))
    } else {
        let (m, name) = parts.split_at(n.checked_sub(1)?);
        (m, parse_key_code(name[0])?)
    };

    let mut key = Key {
        alt: false,
        code,
        ctrl: false,
        shift: false,
    };
    for modifier in mods {
        match *modifier {
            "C" => key.ctrl = true,
            "S" => key.shift = true,
            "M" => key.alt = true,
            _ => match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => key.ctrl = true,
                "shift" => key.shift = true,
                "alt" | "meta" | "option" => key.alt = true,
                _ => return None,
            },
        }
    }
    Some(key)
}
/// Parse a key name into a [`KeyCode`]: a single character is a `Char`, and
/// named keys (`Space`, `Enter`, `F1`, ...) map case-insensitively.
pub(super) fn parse_key_code(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(KeyCode::Char(first));
    }
    Some(match name.to_ascii_lowercase().as_str() {
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "down" => KeyCode::Down,
        "end" => KeyCode::End,
        "enter" | "return" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "home" => KeyCode::Home,
        "insert" => KeyCode::Insert,
        "left" => KeyCode::Left,
        "pagedown" => KeyCode::PageDown,
        "pageup" => KeyCode::PageUp,
        "right" => KeyCode::Right,
        "space" => KeyCode::Space,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        other => return parse_function_key(other),
    })
}
/// Parse an `f1`..`f12` function-key name into [`KeyCode::F`].
pub(super) fn parse_function_key(name: &str) -> Option<KeyCode> {
    let number: u8 = name.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&number).then_some(KeyCode::F(number))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::insert::encode;
    use super::*;
    use crate::model::input::test_support::*;

    #[test]
    fn test_palette_commands_dispatch_through_the_keymap_path() {
        // Regression: eleven palette entries (copy/paste, font size,
        // scrolling) had no dispatch arm of their own, so selecting them
        // from the palette silently did nothing even though their
        // keybindings worked.
        for name in [
            "copy_selection",
            "paste_from_clipboard",
            "font_decrease",
            "font_increase",
            "font_reset",
            "scroll_line_up",
            "scroll_line_down",
            "scroll_page_up",
            "scroll_page_down",
            "scroll_to_top",
            "scroll_to_bottom",
        ] {
            assert!(
                window_action_by_name(name).is_some(),
                "{name} must dispatch through the keymap path"
            );
        }
        assert!(window_action_by_name("not_a_command").is_none());
    }
    #[test]
    fn test_parse_chord_modifiers_and_named_keys() {
        // Abbreviated single-letter modifiers (case-sensitive).
        assert_eq!(
            parse_chord("C+w"),
            Some(Key {
                alt: false,
                code: KeyCode::Char('w'),
                ctrl: true,
                shift: false,
            })
        );
        assert_eq!(
            parse_chord("C+S+Space"),
            Some(Key {
                alt: false,
                code: KeyCode::Space,
                ctrl: true,
                shift: true,
            })
        );
        // Full names still accepted.
        assert_eq!(
            parse_chord("ctrl+shift+Space"),
            Some(Key {
                alt: false,
                code: KeyCode::Space,
                ctrl: true,
                shift: true,
            })
        );
        assert_eq!(parse_chord("F5").map(|k| k.code), Some(KeyCode::F(5)));
        assert_eq!(parse_chord("v").map(|k| k.code), Some(KeyCode::Char('v')));
        assert_eq!(parse_chord("Hyper+x"), None);
        // `-` key is just the last segment; no special escaping needed.
        assert_eq!(
            parse_chord("S+M+-"),
            Some(Key {
                alt: true,
                code: KeyCode::Char('-'),
                ctrl: false,
                shift: true,
            })
        );
        assert_eq!(
            parse_chord("C+-"),
            Some(Key {
                alt: false,
                code: KeyCode::Char('-'),
                ctrl: true,
                shift: false,
            })
        );
        // `+` key is written as a trailing `++`.
        assert_eq!(
            parse_chord("C+S++"),
            Some(Key {
                alt: false,
                code: KeyCode::Char('+'),
                ctrl: true,
                shift: true,
            })
        );
    }
    #[test]
    fn test_parse_chord_sequence_length_bounds() {
        assert_eq!(parse_chord_sequence("C+w v").map(|k| k.len()), Some(2));
        assert_eq!(parse_chord_sequence("C+h").map(|k| k.len()), Some(1));
        assert_eq!(parse_chord_sequence(""), None);
        assert_eq!(parse_chord_sequence("a b c"), None);
    }
    #[test]
    fn test_config_rebinds_window_action_and_drops_default() {
        let mut bindings = HashMap::new();
        bindings.insert("C+w b".to_string(), "split_horizontal".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        // The rebound sequence key now splits horizontally.
        let mut pending = PendingPrefix::CtrlW;
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &key(KeyCode::Char('b')),
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::SplitPane(Direction::Horizontal)
        );
        // The default S+M+- is dropped because split_horizontal was rebound.
        let shift_alt_minus = Key {
            alt: true,
            code: KeyCode::Char('-'),
            ctrl: false,
            shift: true,
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &shift_alt_minus,
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::Ignore
        );
        // An unmentioned action keeps its default (Ctrl-Shift-q still closes).
        let ctrl_shift_q = Key {
            alt: false,
            code: KeyCode::Char('q'),
            ctrl: true,
            shift: true,
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &ctrl_shift_q,
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::ClosePane
        );
    }
    #[test]
    fn test_ctrl_backspace_deletes_word_back_by_default() {
        let keymap = WindowKeymap::default();
        let ctrl_bsp = Key {
            ctrl: true,
            ..key(KeyCode::Backspace)
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Insert,
                &ctrl_bsp,
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::Edit(EditAction::DeleteWordBackward)
        );
    }
    #[test]
    fn test_config_rebinds_undo_redo() {
        let mut editing = HashMap::new();
        editing.insert("C+z".to_string(), "prompt_undo".to_string());
        editing.insert("C+y".to_string(), "prompt_redo".to_string());
        let keymap = WindowKeymap::from_config(None, Some(&editing));

        let ctrl_z = Key {
            ctrl: true,
            ..key(KeyCode::Char('z'))
        };
        let ctrl_y = Key {
            ctrl: true,
            ..key(KeyCode::Char('y'))
        };
        let resolve = |mode, k: &Key| {
            let mut pending = PendingPrefix::None;
            resolve_with(mode, k, &mut pending, &keymap, 0, None, false)
        };
        // The rebound chords drive undo/redo in both Insert and Normal mode.
        assert_eq!(resolve(Mode::Insert, &ctrl_z), Action::PromptUndo);
        assert_eq!(resolve(Mode::Normal, &ctrl_z), Action::PromptUndo);
        assert_eq!(resolve(Mode::Insert, &ctrl_y), Action::PromptRedo);
        assert_eq!(resolve(Mode::Normal, &ctrl_y), Action::PromptRedo);
        // The default Ctrl-/ is dropped because undo was rebound.
        let ctrl_slash = Key {
            ctrl: true,
            ..key(KeyCode::Char('/'))
        };
        assert_eq!(
            resolve(Mode::Insert, &ctrl_slash),
            Action::SendBytes(encode(&ctrl_slash, 0, None))
        );
    }
    #[test]
    fn test_config_rebinds_editing_action() {
        let mut editing = HashMap::new();
        editing.insert("C+u".to_string(), "delete_to_line_start".to_string());
        let keymap = WindowKeymap::from_config(None, Some(&editing));

        let ctrl_u = Key {
            ctrl: true,
            ..key(KeyCode::Char('u'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Insert, &ctrl_u, &mut pending, &keymap, 0, None, false),
            Action::Edit(EditAction::DeleteToLineStart)
        );
        // The default Ctrl-Backspace binding survives (a different action).
        let ctrl_bsp = Key {
            ctrl: true,
            ..key(KeyCode::Backspace)
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Insert,
                &ctrl_bsp,
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::Edit(EditAction::DeleteWordBackward)
        );
    }
    #[test]
    fn test_config_custom_leader_and_direct_focus() {
        let mut bindings = HashMap::new();
        bindings.insert("M+y".to_string(), "split_horizontal".to_string());
        bindings.insert("C+b o".to_string(), "close_other_panes".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        // A direct, non-default chord splits horizontally.
        let mut pending = PendingPrefix::None;
        let alt_y = Key {
            alt: true,
            ..key(KeyCode::Char('y'))
        };
        assert_eq!(
            resolve_with(Mode::Normal, &alt_y, &mut pending, &keymap, 0, None, false),
            Action::SplitPane(Direction::Horizontal)
        );

        // The leader is now C+b; C+b then o closes other panes.
        let mut pending = PendingPrefix::None;
        let ctrl_b = Key {
            ctrl: true,
            ..key(KeyCode::Char('b'))
        };
        assert_eq!(
            resolve_with(Mode::Normal, &ctrl_b, &mut pending, &keymap, 0, None, false),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::CtrlW);
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &key(KeyCode::Char('o')),
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::CloseOtherPanes
        );
    }
    #[test]
    fn test_global_action_open_settings_by_default() {
        let keymap = WindowKeymap::default();
        let ctrl_comma = Key {
            ctrl: true,
            ..key(KeyCode::Char(','))
        };
        assert_eq!(
            keymap.global_action(&ctrl_comma),
            Some(Action::OpenSettings)
        );
    }
    #[test]
    fn test_global_action_rebind_drops_default() {
        let mut bindings = HashMap::new();
        bindings.insert("C+S+o".to_string(), "open_settings".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let ctrl_shift_o = Key {
            ctrl: true,
            shift: true,
            ..key(KeyCode::Char('o'))
        };
        assert_eq!(
            keymap.global_action(&ctrl_shift_o),
            Some(Action::OpenSettings)
        );

        // The default Ctrl-, no longer opens settings once rebound.
        let ctrl_comma = Key {
            ctrl: true,
            ..key(KeyCode::Char(','))
        };
        assert_eq!(keymap.global_action(&ctrl_comma), None);
    }
    #[test]
    fn test_config_rebinds_index_action_and_drops_default() {
        let mut bindings = HashMap::new();
        // Move focus_pane_2 off its default Alt+2 chord onto Alt+Q.
        bindings.insert("M+q".to_string(), "focus_pane_2".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let alt_q = Key {
            alt: true,
            ..key(KeyCode::Char('q'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_q, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(2)
        );

        // The default Alt+2 no longer focuses pane 2 — it was dropped in favor
        // of the rebound chord.
        let alt_2 = Key {
            alt: true,
            ..key(KeyCode::Char('2'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_2, &mut pending, &keymap, 0, None, false),
            Action::Ignore
        );

        // An unmentioned index (Alt+3) keeps its default.
        let alt_3 = Key {
            alt: true,
            ..key(KeyCode::Char('3'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_3, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(3)
        );
    }
    #[test]
    fn test_config_binds_a_chord_to_a_palette_only_command() {
        // `mux_new_session` has no `WindowAction` variant; the chord must
        // resolve to `RunCommand`, not silently drop like it used to.
        let mut bindings = HashMap::new();
        bindings.insert("F5".to_string(), "mux_new_session".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let f5 = key(KeyCode::F(5));
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &f5, &mut pending, &keymap, 0, None, false),
            Action::RunCommand("mux_new_session".to_string())
        );
    }
    #[test]
    fn test_config_named_command_displaces_the_default_window_action_on_that_chord() {
        // Rebinding a chord that already has a default WindowAction (Alt+h /
        // focus_left) to a named command must actually take effect, not be
        // shadowed by the still-active default.
        let mut bindings = HashMap::new();
        bindings.insert("M+h".to_string(), "mux_new_session".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let alt_h = key(KeyCode::Char('h'));
        let alt_h = Key { alt: true, ..alt_h };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_h, &mut pending, &keymap, 0, None, false),
            Action::RunCommand("mux_new_session".to_string())
        );
    }
    #[test]
    fn test_config_ignores_a_two_key_sequence_bound_to_an_unrecognized_name() {
        // Named commands only support single-key chords in v1; a
        // leader-sequence spec paired with a palette-only name must be
        // dropped, not panic or half-bind.
        let mut bindings = HashMap::new();
        bindings.insert("C+w x".to_string(), "mux_new_session".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);
        assert!(keymap.named.is_empty());
    }
    #[test]
    fn test_chord_hint_reports_a_bound_named_command() {
        let mut bindings = HashMap::new();
        bindings.insert("F5".to_string(), "mux_new_session".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);
        assert_eq!(keymap.chord_hint("mux_new_session"), "F5");
        assert_eq!(keymap.chord_hint("cd_recent"), "");
    }
    #[test]
    fn test_specific_index_binding_targets_a_fixed_pane() {
        let mut bindings = HashMap::new();
        bindings.insert("M+q".to_string(), "focus_pane_1".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings), None);

        let alt_q = Key {
            alt: true,
            ..key(KeyCode::Char('q'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_q, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(1)
        );

        // An unrelated default (Alt+3) is untouched by the new binding.
        let alt_3 = Key {
            alt: true,
            ..key(KeyCode::Char('3'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_3, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(3)
        );
    }
    #[test]
    fn test_specific_index_binding_rejects_out_of_range_index() {
        assert_eq!(IndexAction::from_name("goto_tab_0"), None);
        assert_eq!(IndexAction::from_name("goto_tab_10"), None);
        assert_eq!(IndexAction::from_name("goto_tab_x"), None);
        assert_eq!(
            IndexAction::from_name("goto_tab_9"),
            Some(IndexAction {
                kind: IndexKind::GotoTab,
                n: 9
            })
        );
    }
    #[test]
    fn test_named_register_yank_and_paste_resolution() {
        let win = WindowKeymap::default();
        let mut pending = PendingPrefix::None;

        // Press `"`
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('"'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::Ignore);
        assert_eq!(pending, PendingPrefix::Register);

        // Press `a`
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('a'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::Ignore);
        assert_eq!(pending, PendingPrefix::WithRegister('a'));

        // Press `p`
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('p'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(
            a,
            Action::PasteRegister {
                register: 'a',
                after: true
            }
        );
        assert_eq!(pending, PendingPrefix::None);
    }
    #[test]
    fn test_change_surround_and_delete_surround_resolution() {
        let win = WindowKeymap::default();

        // `ds"`
        let mut pending = PendingPrefix::None;
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('d'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('s'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('"'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::DeleteSurround('"'));

        // `cs"'`
        let mut pending = PendingPrefix::None;
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('c'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('s'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('"'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('\''),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(
            a,
            Action::ChangeSurround {
                target: '"',
                replacement: '\''
            }
        );
    }
    #[test]
    fn test_change_and_replace_operators_resolution() {
        let win = WindowKeymap::default();

        // `cw`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('c'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::Change);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('w'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ChangeWordForward);

        // `ciw`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('c'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('i'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::ChangeObject { around: false });
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('w'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(
            a,
            Action::ChangeTextObject(TextObjectSpec::new(false, TextObject::Word))
        );

        // `C`
        let mut pending = PendingPrefix::None;
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('C'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ChangeToLineEnd);

        // `s`
        let mut pending = PendingPrefix::None;
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('s'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::SubstituteChar);

        // `S`
        let mut pending = PendingPrefix::None;
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('S'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ChangeLine);

        // `rx`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('r'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::ReplaceChar);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('x'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ReplaceChar('x'));

        // `~`
        let mut pending = PendingPrefix::None;
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('~'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ToggleCaseChar);
    }
    #[test]
    fn test_search_match_and_g_shortcuts_resolution() {
        let win = WindowKeymap::default();

        // `gn`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::G);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('n'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::SelectSearchMatch { forward: true });

        // `gN`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('N'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::SelectSearchMatch { forward: false });

        // `cgn`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('c'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::ChangeG);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('n'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ChangeSearchMatch { forward: true });

        // `dgn`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('d'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(pending, PendingPrefix::DeleteG);
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('n'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::DeleteSearchMatch { forward: true });

        // `gs` (Swoop)
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('s'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::ToggleSwoop);

        // `gp` (Prompt jump)
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('p'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::JumpToPrompt);

        // `gP` (Prev prompt jump)
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('g'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                ctrl: false,
                code: KeyCode::Char('P'),
            },
            &mut pending,
            &win,
            0,
            None,
            false,
        );
        assert_eq!(a, Action::JumpToPreviousPrompt);
    }
}
