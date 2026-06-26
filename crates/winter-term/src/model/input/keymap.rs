//! Configurable bindings: the chord tables a `keybindings.kdl` builds, how
//! its blocks merge over the defaults, and what is wrong with them.

use std::collections::{HashMap, HashSet};

use crate::model::tool::ToolCommand;

use super::actions::{EditBinding, IndexAction, WindowAction};
use super::chord::{find_chord, format_key, format_sequence, parse_chord_sequence};
use super::sequence::{SequenceMap, SequenceMatch, MAX_SEQUENCE_LEN};
use super::{Action, Key};

// ========================================================================
// Data Structures
// ========================================================================

/// The configurable key bindings: window commands, line edits, and each
/// tool's own commands. A binding is either a direct chord (e.g. `C-h` to
/// focus left) or a sequence of up to
/// [`MAX_SEQUENCE_LEN`](super::sequence::MAX_SEQUENCE_LEN) chords (e.g. `M-g`
/// then `M-h` to move a tab). Built from defaults and overlaid with config via
/// [`Keymap::from_blocks`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Keymap {
    /// Direct chords: the full key (with modifiers) triggers the action.
    pub(super) direct: Vec<(Key, WindowAction)>,
    /// Line-edit and prompt-history chords (e.g. `C-DEL` → delete word
    /// back, `C-/` → undo, `C-\` → redo).
    pub(super) editing: Vec<(Key, EditBinding)>,
    /// Chords for globally-intercepted actions (settings, font size, tab/pane
    /// management, palette toggles): looked up before the command palette, tab
    /// rename, and settings overlays get a chance to consume the key.
    pub(super) global: Vec<(Key, WindowAction)>,
    /// Chords bound to one specific pane/tab index (config actions
    /// `close_pane_N`/`focus_pane_N`/`goto_tab_N`, `N` = `1..9`).
    pub(super) index_specific: Vec<(Key, IndexAction)>,
    /// Direct chords bound to a command-palette action name with no
    /// `WindowAction` variant (`mux_new_session`, `export_block_svg`, …),
    /// dispatched through `run_command` instead of `to_action()`.
    pub(super) named: Vec<(Key, String)>,
    /// Window sequences, each naming a `WindowAction`, an index action, or a
    /// command-palette name, resolved the way a single chord's name is.
    pub(super) sequences: SequenceMap,
    /// Each tool's own bindings, by the `keybindings.kdl` block naming them
    /// (`git`, `dir`, ...): commands the tool runs by name, looked up only
    /// while that tool has focus and ahead of the window's own sequences.
    pub(super) tool_sequences: HashMap<String, SequenceMap>,
}

/// What the chords typed so far resolve to, across the window's sequences and
/// the focused tool's.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SequenceStep {
    /// No binding starts with them.
    Miss,
    /// They start a binding: wait for the next key.
    Pending,
    /// A window binding: dispatch this action.
    Run(Action),
    /// A binding of the focused tool's: run the tool command of this name.
    Tool(String),
}
/// The built-in default keybindings, embedded from the canonical file (also
/// installed as a reference copy at `~/.config/winter-term/keybindings.kdl`).
/// [`Keymap::default`] parses this through the same pipeline used for
/// the user's own config, so there is exactly one place defining what the
/// defaults are.
pub(super) const DEFAULT_KEYBINDINGS_KDL: &str = include_str!("../../../samples/keybindings.kdl");
/// The `keybindings.kdl` block of window commands.
const WINDOW_BLOCK: &str = "window";
/// The `keybindings.kdl` block of line edits.
const EDITING_BLOCK: &str = "editing";
/// The `keybindings.kdl` blocks that are not a tool's: every other block
/// names one tool's own commands.
const NON_TOOL_BLOCKS: [&str; 5] = [EDITING_BLOCK, "insert", "normal", "visual", WINDOW_BLOCK];
impl Keymap {
    /// An unbound keymap: no chords at all. Only ever used as the starting
    /// point for a merge, which immediately layers the built-in defaults onto
    /// it before anything else.
    pub(super) fn empty() -> Self {
        Self {
            direct: vec![],
            editing: vec![],
            global: vec![],
            index_specific: vec![],
            named: vec![],
            sequences: SequenceMap::default(),
            tool_sequences: HashMap::new(),
        }
    }

    /// Build a keymap from the `window` and `editing` blocks, layered onto
    /// the built-in defaults, trusting every command name. A configured
    /// binding replaces every default chord for that same action, so
    /// rebinding `split_vertical` drops its default chord; actions left
    /// unmentioned keep their defaults.
    pub fn from_config(
        window: Option<&HashMap<String, String>>,
        editing: Option<&HashMap<String, String>>,
    ) -> Self {
        let mut blocks = HashMap::new();
        for (block, bindings) in [(WINDOW_BLOCK, window), (EDITING_BLOCK, editing)] {
            if let Some(bindings) = bindings {
                blocks.insert(block.to_string(), bindings.clone());
            }
        }
        Self::from_blocks(&blocks)
    }

    /// Build a keymap from every block of a `keybindings.kdl` over the
    /// defaults, trusting every command name.
    pub fn from_blocks(blocks: &Blocks) -> Self {
        Self::merge(blocks, None).0
    }

    /// Build a keymap from every block of a `keybindings.kdl` over the
    /// defaults, and say what in `blocks` could never run, one line each: a
    /// spec that is no chord, a command `catalog` does not know, a block no
    /// tool has, bindings that overlap, and a tool's chord a global chord
    /// takes first. A binding reported is a binding left out, so what is
    /// reported is exactly what did not bind.
    pub fn load(blocks: &Blocks, catalog: Catalog) -> (Self, Vec<String>) {
        Self::merge(blocks, Some(catalog))
    }

    /// The defaults, then `blocks` over them, checking names against
    /// `catalog` when there is one. Only `blocks` is reported on: the
    /// defaults are checked by a test instead, and never blamed on the
    /// user's file.
    fn merge(blocks: &Blocks, catalog: Option<Catalog>) -> (Self, Vec<String>) {
        let mut keymap = Self::empty();
        let mut defaults_report = Report::new(catalog);
        keymap.apply_blocks(&default_bindings_maps(), &mut defaults_report);
        let mut report = Report::new(catalog);
        keymap.apply_blocks(blocks, &mut report);
        (keymap, report.problems)
    }

    /// Overlay every block of one file, in a fixed order: `window`, then
    /// `editing`, then each tool's.
    fn apply_blocks(&mut self, blocks: &Blocks, report: &mut Report) {
        let mut sorted: Vec<(&String, &HashMap<String, String>)> = blocks.iter().collect();
        sorted.sort_by_key(|(block, _)| match block.as_str() {
            WINDOW_BLOCK => (0, block.as_str()),
            EDITING_BLOCK => (1, block.as_str()),
            other => (2, other),
        });
        // Bindings answering the same keys, by namespace: `window` and
        // `editing` share one, and each tool's block is one of its own.
        let mut namespaces: Vec<(&str, Vec<Parsed>)> = Vec::new();
        let mut tool_bindings: Vec<Parsed> = Vec::new();
        for (block, bindings) in sorted {
            let block = block.as_str();
            match block {
                WINDOW_BLOCK => {
                    let parsed = parse_block(block, bindings, MAX_SEQUENCE_LEN, report);
                    self.apply_window_bindings(&parsed, report);
                    push_namespace(&mut namespaces, WINDOW_BLOCK, parsed);
                }
                EDITING_BLOCK => {
                    let parsed = parse_block(block, bindings, 1, report);
                    self.apply_editing_bindings(&parsed, report);
                    push_namespace(&mut namespaces, WINDOW_BLOCK, parsed);
                }
                _ if NON_TOOL_BLOCKS.contains(&block) => {
                    report.push(format!("`{block}` is not configurable yet"));
                }
                _ => {
                    let Some(commands) = report.tool_commands(block) else {
                        report.push(format!("`{block}` is no tool's block"));
                        continue;
                    };
                    let parsed = parse_block(block, bindings, MAX_SEQUENCE_LEN, report);
                    let parsed = self.apply_tool_bindings(block, parsed, commands, report);
                    tool_bindings.extend(parsed.iter().cloned());
                    push_namespace(&mut namespaces, block, parsed);
                }
            }
        }
        for (_, parsed) in &namespaces {
            report_overlaps(parsed, report);
        }
        // A global chord is taken before any tool sees the key, so a tool's
        // binding that opens with one is dead.
        for parsed in &tool_bindings {
            if find_chord(&self.global, &parsed.keys[0]).is_some() {
                report.push(format!(
                    "{} never runs: \"{}\" is a global chord, taken before any tool sees it",
                    parsed.at,
                    format_key(&parsed.keys[0])
                ));
            }
        }
    }

    /// Overlay one tool's block, leaving out any command the tool does not
    /// have. A configured binding replaces every prior binding of that
    /// command, and takes its chords from any other. Returns what it bound.
    fn apply_tool_bindings(
        &mut self,
        block: &str,
        parsed: Vec<Parsed>,
        commands: Option<&[ToolCommand]>,
        report: &mut Report,
    ) -> Vec<Parsed> {
        let parsed: Vec<Parsed> = parsed
            .into_iter()
            .filter(|binding| {
                let is_known =
                    commands.is_none_or(|commands| commands.iter().any(|c| c.name == binding.name));
                if !is_known {
                    report.unknown_command(binding);
                }
                is_known
            })
            .collect();
        let sequences = self.tool_sequences.entry(block.to_string()).or_default();
        for binding in &parsed {
            sequences.unbind_commands(|bound| bound == binding.name);
        }
        for binding in &parsed {
            sequences.bind(binding.keys.clone(), binding.name.clone());
        }
        parsed
    }

    /// Overlay the `window` block onto the window chords. A configured
    /// binding replaces every prior chord for that same action; a single
    /// chord bound to a palette-only name becomes a named binding, displacing
    /// any `WindowAction` on that same chord. A name nothing runs is left
    /// out.
    fn apply_window_bindings(&mut self, bindings: &[Parsed], report: &mut Report) {
        let mut parsed: Vec<(Key, WindowAction)> = Vec::new();
        let mut index: Vec<(Key, IndexAction)> = Vec::new();
        let mut named: Vec<(Key, String)> = Vec::new();
        let mut sequences: Vec<(Vec<Key>, String)> = Vec::new();
        for binding in bindings {
            let name = binding.name.as_str();
            if !report.knows_window_command(name) {
                report.unknown_command(binding);
                continue;
            }
            match (binding.keys.as_slice(), WindowAction::from_name(name)) {
                // A sequence resolves its name when it runs, so any command
                // a single chord can name, a sequence can too.
                ([_, _, ..], _) => sequences.push((binding.keys.clone(), binding.name.clone())),
                ([single], Some(action)) => parsed.push((*single, action)),
                ([single], None) => match IndexAction::from_name(name) {
                    Some(action) => index.push((*single, action)),
                    None => named.push((*single, binding.name.clone())),
                },
                ([], _) => {}
            }
        }
        self.apply_index_bindings(index);

        // A chord belongs to one command: the one configured last takes it
        // from whatever held it, in every table that answers a chord, and a
        // chord that runs a command cannot also open a sequence.
        for key in named
            .iter()
            .map(|(key, _)| key)
            .chain(parsed.iter().map(|(key, _)| key))
        {
            self.unbind_chord(key);
            self.sequences.unbind_leader(key);
        }
        self.named.extend(named);

        let rebound: HashSet<WindowAction> = parsed.iter().map(|(_, action)| *action).collect();
        self.direct.retain(|(_, action)| !rebound.contains(action));
        self.global.retain(|(_, action)| !rebound.contains(action));
        self.sequences.unbind_commands(|name| {
            WindowAction::from_name(name).is_some_and(|action| rebound.contains(&action))
        });
        for (key, action) in parsed {
            match action.is_global() {
                true => self.global.push((key, action)),
                false => self.direct.push((key, action)),
            }
        }

        // Last, so that where the block's own bindings overlap, the longer
        // one is kept: a sequence takes its leader from a chord of the block.
        self.apply_window_sequences(sequences);
    }

    /// Bind the sequences of a `window` block. Each replaces every prior
    /// binding of its command, a sequence or a single chord, and its leader
    /// stops being a chord of its own, since it could then never open.
    fn apply_window_sequences(&mut self, sequences: Vec<(Vec<Key>, String)>) {
        for (_, name) in &sequences {
            let action = WindowAction::from_name(name);
            self.sequences.unbind_commands(|bound| {
                bound == name.as_str()
                    || (action.is_some() && WindowAction::from_name(bound) == action)
            });
            if let Some(action) = action {
                self.direct.retain(|(_, bound)| *bound != action);
                self.global.retain(|(_, bound)| *bound != action);
            }
        }
        for (keys, name) in sequences {
            self.unbind_chord(&keys[0]);
            self.sequences.bind(keys, name);
        }
    }

    /// Free `key` in every table that answers a single chord, so the binding
    /// added next is the only one on it.
    fn unbind_chord(&mut self, key: &Key) {
        self.direct.retain(|(k, _)| k != key);
        self.global.retain(|(k, _)| k != key);
        self.index_specific.retain(|(k, _)| k != key);
        self.named.retain(|(k, _)| k != key);
    }

    /// Overlay the `close_pane_N`/`focus_pane_N`/`goto_tab_N` bindings from
    /// the `window` block. A configured chord replaces every prior chord for
    /// that same specific index, including the built-in default.
    fn apply_index_bindings(&mut self, parsed: Vec<(Key, IndexAction)>) {
        let rebound: HashSet<IndexAction> = parsed.iter().map(|(_, action)| *action).collect();
        self.index_specific
            .retain(|(_, action)| !rebound.contains(action));
        for (key, _) in &parsed {
            self.unbind_chord(key);
            self.sequences.unbind_leader(key);
        }
        self.index_specific.extend(parsed);
    }

    /// Overlay the `editing` block onto the line-edit chords. A configured
    /// binding replaces every prior chord for that same action; a name that
    /// is no line edit is left out.
    fn apply_editing_bindings(&mut self, bindings: &[Parsed], report: &mut Report) {
        let mut parsed: Vec<(Key, EditBinding)> = Vec::new();
        for binding in bindings {
            match EditBinding::from_name(&binding.name) {
                Some(action) => parsed.push((binding.keys[0], action)),
                None => report.unknown_command(binding),
            }
        }
        let rebound: HashSet<EditBinding> = parsed.iter().map(|(_, action)| *action).collect();
        self.editing.retain(|(_, action)| !rebound.contains(action));
        // A line edit's chord would never reach it from under a sequence.
        for (key, _) in &parsed {
            self.sequences.unbind_leader(key);
        }
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
    /// (`close_pane_N`/`focus_pane_N`/`goto_tab_N`), if any, including the
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

    /// What the chords in `typed` resolve to. `tool` names the focused
    /// tool's block, whose bindings are looked up first: a tool may bind a
    /// sequence under a window leader (`M-g r` in Git beside `M-g M-h`).
    pub fn resolve_sequence(&self, typed: &[Key], tool: Option<&str>) -> SequenceStep {
        let tool_match = self.tool_map(tool).map(|map| map.lookup(typed));
        if let Some(SequenceMatch::Exact(name)) = tool_match {
            return SequenceStep::Tool(name.to_string());
        }
        // A single chord is a window sequence's leader at most: the window's
        // own single chords resolve by mode, after this.
        let window_match = match typed.len() > 1
            || typed
                .first()
                .is_some_and(|key| self.sequences.has_leader(key))
        {
            true => self.sequences.lookup(typed),
            false => SequenceMatch::Miss,
        };
        match (tool_match, window_match) {
            (_, SequenceMatch::Exact(name)) => SequenceStep::Run(sequence_action(name)),
            (Some(SequenceMatch::Prefix), _) | (_, SequenceMatch::Prefix) => SequenceStep::Pending,
            (Some(SequenceMatch::Exact(_) | SequenceMatch::Miss) | None, SequenceMatch::Miss) => {
                SequenceStep::Miss
            }
        }
    }

    /// The keys that can follow `typed`, each with what it does, for the
    /// which-key hint: the focused tool's first, then the window's. `label`
    /// words a command from the tool block naming it (`None` for the
    /// window's) and its name.
    pub fn sequence_hint(
        &self,
        typed: &[Key],
        tool: Option<&str>,
        label: impl Fn(Option<&str>, &str) -> String,
    ) -> Vec<(String, String)> {
        let mut items = match (tool, self.tool_map(tool)) {
            (Some(block), Some(map)) => map.continuations(typed, |name| label(Some(block), name)),
            (None, _) | (Some(_), None) => Vec::new(),
        };
        for item in self
            .sequences
            .continuations(typed, |name| label(None, name))
        {
            if !items.iter().any(|(key, _)| *key == item.0) {
                items.push(item);
            }
        }
        items
    }

    /// The focused tool's bindings, if it has a block.
    fn tool_map(&self, tool: Option<&str>) -> Option<&SequenceMap> {
        self.tool_sequences.get(tool?)
    }

    /// The chords bound to the tool command `name` in `block`, formatted, or
    /// an empty string when it is unbound.
    pub fn tool_chord_hint(&self, block: &str, name: &str) -> String {
        self.tool_map(Some(block))
            .and_then(|map| map.keys_for(name))
            .map(format_sequence)
            .unwrap_or_default()
    }
}

/// The action a window sequence bound to `name` runs: a window command, an
/// index command, or else the command-palette entry of that name.
fn sequence_action(name: &str) -> Action {
    WindowAction::from_name(name)
        .map(WindowAction::to_action)
        .or_else(|| IndexAction::from_name(name).map(IndexAction::to_action))
        .unwrap_or_else(|| Action::RunCommand(name.to_string()))
}
impl Default for Keymap {
    /// Built from the embedded default keybindings ([`DEFAULT_KEYBINDINGS_KDL`])
    /// through the same parsing pipeline as user config: see
    /// [`from_config`](Self::from_config).
    fn default() -> Self {
        Self::from_config(None, None)
    }
}
/// A `keybindings.kdl` as parsed: its blocks, by name, of spec to command.
pub type Blocks = HashMap<String, HashMap<String, String>>;

/// The commands that exist beyond the keymap's own, for telling a binding
/// that runs one from a typo. The app supplies it; the model knows neither
/// the palette's list nor the tools'.
#[derive(Clone, Copy)]
pub struct Catalog {
    /// Whether a name is a command-palette command.
    pub is_command: fn(&str) -> bool,
    /// The commands a tool's block may bind, or `None` for no tool's block.
    pub tool_commands: fn(&str) -> Option<&'static [ToolCommand]>,
}

/// One binding of a block as parsed: its keys, the command it names, and
/// where it was written, for a report.
#[derive(Clone, Debug)]
struct Parsed {
    at: String,
    keys: Vec<Key>,
    name: String,
}

/// What a merge found wrong, and what it checks names against: with no
/// catalog, every name is trusted.
struct Report {
    catalog: Option<Catalog>,
    problems: Vec<String>,
}

impl Report {
    fn new(catalog: Option<Catalog>) -> Self {
        Self {
            catalog,
            problems: Vec::new(),
        }
    }

    fn push(&mut self, problem: String) {
        self.problems.push(problem);
    }

    fn unknown_command(&mut self, binding: &Parsed) {
        self.push(format!(
            "{} runs unknown command \"{}\"",
            binding.at, binding.name
        ));
    }

    /// Whether a `window` binding's name runs anything.
    fn knows_window_command(&self, name: &str) -> bool {
        WindowAction::from_name(name).is_some()
            || IndexAction::from_name(name).is_some()
            || self
                .catalog
                .is_none_or(|catalog| (catalog.is_command)(name))
    }

    /// The commands `block` may bind: `Some(None)` when every name is
    /// trusted, `None` when the block is no tool's.
    fn tool_commands(&self, block: &str) -> Option<Option<&'static [ToolCommand]>> {
        match self.catalog {
            Some(catalog) => (catalog.tool_commands)(block).map(Some),
            None => Some(None),
        }
    }
}

/// Parse a block's specs in spec order, so a block merges the same way
/// however the map holding it iterates, reporting each spec that is not up
/// to `max_len` chords.
fn parse_block(
    block: &str,
    bindings: &HashMap<String, String>,
    max_len: usize,
    report: &mut Report,
) -> Vec<Parsed> {
    let mut parsed = Vec::new();
    for (spec, name) in in_spec_order(bindings) {
        let at = format!("{block}: \"{spec}\"");
        match parse_chord_sequence(spec).filter(|keys| keys.len() <= max_len) {
            Some(keys) => parsed.push(Parsed {
                at,
                keys,
                name: name.clone(),
            }),
            None => report.push(format!("{at} is not a chord")),
        }
    }
    parsed
}

/// Add `parsed` to the bindings of `namespace`.
fn push_namespace<'a>(
    namespaces: &mut Vec<(&'a str, Vec<Parsed>)>,
    namespace: &'a str,
    parsed: Vec<Parsed>,
) {
    match namespaces.iter_mut().find(|(name, _)| *name == namespace) {
        Some((_, bound)) => bound.extend(parsed),
        None => namespaces.push((namespace, parsed)),
    }
}

/// Report each pair of bindings in one namespace that share a prefix, of
/// which only one can run.
fn report_overlaps(bound: &[Parsed], report: &mut Report) {
    for (i, binding) in bound.iter().enumerate() {
        for other in &bound[i + 1..] {
            let shared = binding.keys.len().min(other.keys.len());
            if binding.keys[..shared] == other.keys[..shared] {
                report.push(format!(
                    "{} and {} overlap: only one can run",
                    binding.at, other.at
                ));
            }
        }
    }
}
/// A block's bindings sorted by their spec, so a block merges the same way
/// however the map holding it happens to iterate.
fn in_spec_order(bindings: &HashMap<String, String>) -> Vec<(&String, &String)> {
    let mut sorted: Vec<(&String, &String)> = bindings.iter().collect();
    sorted.sort();
    sorted
}
/// Parse [`DEFAULT_KEYBINDINGS_KDL`] into `window`/`editing` action maps, the
/// same shape a user's `keybindings.kdl` parses into.
pub(super) fn default_bindings_maps() -> HashMap<String, HashMap<String, String>> {
    kdl::de::from_str(DEFAULT_KEYBINDINGS_KDL).unwrap_or_default()
}
impl Keymap {
    /// Return the formatted shortcut hint for a palette command name
    /// (e.g. `"C-h"` for `"focus_left"`), or an empty string when unbound.
    pub fn chord_hint(&self, command: &str) -> String {
        let Some(action) = WindowAction::from_name(command) else {
            return self
                .named
                .iter()
                .find(|(_, name)| name == command)
                .map(|(key, _)| format_key(key))
                .unwrap_or_else(|| self.sequence_hint_for(command));
        };
        if let Some((key, _)) = self
            .direct
            .iter()
            .chain(self.global.iter())
            .find(|(_, a)| *a == action)
        {
            return format_key(key);
        }
        self.sequence_hint_for(command)
    }
}
impl Keymap {
    /// The window sequence bound to `command`, formatted (`"M-g M-h"`), or
    /// an empty string when none is.
    fn sequence_hint_for(&self, command: &str) -> String {
        self.sequences
            .keys_for(command)
            .map(format_sequence)
            .unwrap_or_default()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::actions::{window_action_by_name, IndexKind};
    use super::super::insert::encode;
    use super::super::{
        resolve_with, EditAction, KeyCode, Mode, PendingPrefix, TextObject, TextObjectSpec,
    };
    use super::*;
    use crate::model::input::test_support::*;
    use crate::model::layout::{Direction, FocusDir};

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
    fn test_every_default_named_chord_dispatches_to_a_real_command() {
        // A chord bound to a name nothing answers to is a dead key: pressing it
        // does nothing at all, and a test of one binding at a time never sees
        // it. Every tool chord arrives this way.
        let keymap = Keymap::default();
        let commands: Vec<String> = crate::model::palette::builtin_commands(&keymap)
            .into_iter()
            .map(|entry| entry.action)
            .collect();
        for (_, name) in &keymap.named {
            assert!(
                commands.contains(name) || window_action_by_name(name).is_some(),
                "{name} is bound by default, but nothing dispatches it"
            );
        }
    }
    #[test]
    fn test_new_tab_is_bound_to_ctrl_shift_n_and_not_ctrl_shift_t() {
        let keymap = Keymap::default();
        let ctrl_shift_n = Key {
            alt: false,
            code: KeyCode::Char('n'),
            ctrl: true,
            shift: true,
            sup: false,
        };
        let ctrl_shift_t = Key {
            alt: false,
            code: KeyCode::Char('t'),
            ctrl: true,
            shift: true,
            sup: false,
        };
        assert_eq!(keymap.global_action(&ctrl_shift_n), Some(Action::NewTab));
        assert_ne!(keymap.global_action(&ctrl_shift_t), Some(Action::NewTab));
        assert_eq!(keymap.chord_hint("new_tab"), "C-S-n");
    }
    #[test]
    fn test_a_configured_chord_matches_only_its_own_modifiers() {
        // For every key and every set of the four modifiers: bind that one
        // chord to a command of its own, then offer all sixteen sets of the
        // same key. Only the bound one may answer.
        let mut codes = vec![
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Down,
            KeyCode::End,
            KeyCode::Enter,
            KeyCode::Escape,
            KeyCode::Home,
            KeyCode::Insert,
            KeyCode::Left,
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::Right,
            KeyCode::Space,
            KeyCode::Tab,
            KeyCode::Up,
        ];
        codes.extend((1..=24).map(KeyCode::F));
        codes.extend("abz09,./;'[]\\-=`".chars().map(KeyCode::Char));
        let with = |code, bits: u8| Key {
            alt: bits & 1 != 0,
            code,
            ctrl: bits & 2 != 0,
            shift: bits & 4 != 0,
            sup: bits & 8 != 0,
        };
        let mut failures = Vec::new();
        for code in codes {
            for bound in 0..16u8 {
                let spec = format_key(&with(code, bound));
                let mut bindings = HashMap::new();
                bindings.insert(spec.clone(), "mux_probe_command".to_string());
                let keymap = Keymap::from_config(Some(&bindings), None);
                for offered in 0..16u8 {
                    let got = keymap.direct_action(&with(code, offered), false);
                    let hit = got == Some(Action::RunCommand("mux_probe_command".into()));
                    if hit != (offered == bound) {
                        failures.push(format!(
                            "bound {spec}, offered {}",
                            format_key(&with(code, offered))
                        ));
                    }
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} mismatches, first: {:?}",
            failures.len(),
            &failures[..failures.len().min(12)]
        );
    }

    #[test]
    fn test_a_configured_chord_takes_the_key_from_a_default_on_another_action() {
        // `M-h` is `focus_left` by default; binding it to something else must
        // move it, not leave the default answering first.
        let mut bindings = HashMap::new();
        bindings.insert("M-h".to_string(), "split_vertical".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);
        let alt_h = Key::with_alt(KeyCode::Char('h'));
        assert_eq!(
            keymap.direct_action(&alt_h, false),
            Some(WindowAction::SplitVertical.to_action())
        );
        // A second sequence on the `M-g` leader leaves the first alone.
        let mut bindings = HashMap::new();
        bindings.insert("M-g M-z".to_string(), "zoom_pane".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);
        let alt_g = Key::with_alt(KeyCode::Char('g'));
        assert_eq!(
            keymap.resolve_sequence(&[alt_g, Key::with_alt(KeyCode::Char('h'))], None),
            SequenceStep::Run(WindowAction::MoveTabLeft.to_action())
        );
    }

    #[test]
    fn test_shift_alt_slash_and_alt_slash_are_different_chords() {
        let mut bindings = HashMap::new();
        bindings.insert("M-/".to_string(), "mux_plain".to_string());
        bindings.insert("S-M-/".to_string(), "mux_shifted".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);
        let alt_slash = Key::with_alt(KeyCode::Char('/'));
        // Alt-Shift-/ reaches the window as the glyph it types, with Shift held.
        let alt_shift_slash = Key {
            shift: true,
            ..Key::with_alt(KeyCode::Char('?'))
        };
        let named = |name: &str| Some(Action::RunCommand(name.to_string()));
        assert_eq!(keymap.direct_action(&alt_slash, false), named("mux_plain"));
        assert_eq!(
            keymap.direct_action(&alt_shift_slash, false),
            named("mux_shifted")
        );
    }

    #[test]
    fn test_new_window_is_bound_to_ctrl_shift_alt_n() {
        let keymap = Keymap::default();
        let ctrl_shift_alt_n = Key {
            alt: true,
            code: KeyCode::Char('n'),
            ctrl: true,
            shift: true,
            sup: false,
        };
        assert_eq!(
            keymap.global_action(&ctrl_shift_alt_n),
            Some(Action::NewWindow)
        );
        assert_eq!(keymap.chord_hint("new_window"), "C-M-S-n");
    }
    #[test]
    fn test_config_rebinds_window_action_and_drops_default() {
        let mut bindings = HashMap::new();
        bindings.insert("C+w b".to_string(), "split_horizontal".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);

        // The rebound sequence now splits horizontally.
        let ctrl_w = Key {
            ctrl: true,
            ..key(KeyCode::Char('w'))
        };
        assert_eq!(
            keymap.resolve_sequence(&[ctrl_w, key(KeyCode::Char('b'))], None),
            SequenceStep::Run(Action::SplitPane(Direction::Horizontal))
        );
        // The default S+M+- is dropped because split_horizontal was rebound.
        let shift_alt_minus = Key {
            alt: true,
            code: KeyCode::Char('-'),
            ctrl: false,
            shift: true,
            sup: false,
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
        // An unmentioned action keeps its default (C-S-q still closes the tab).
        let ctrl_shift_q = Key {
            alt: false,
            code: KeyCode::Char('q'),
            ctrl: true,
            shift: true,
            sup: false,
        };
        assert_eq!(
            keymap.global_action(&ctrl_shift_q),
            Some(Action::CloseTab(None))
        );
    }
    #[test]
    fn test_shift_alt_q_and_w_close_other_tabs_and_the_pane_by_default() {
        let keymap = Keymap::default();
        let shift_alt = |ch: char| Key {
            alt: true,
            code: KeyCode::Char(ch),
            ctrl: false,
            shift: true,
            sup: false,
        };
        // Closing tabs is global, so it is resolved ahead of the overlays.
        assert_eq!(
            keymap.global_action(&shift_alt('q')),
            Some(Action::CloseOtherTabs)
        );
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &shift_alt('w'),
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
        let keymap = Keymap::default();
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
    fn test_ctrl_alt_z_finds_project_files_while_ctrl_z_still_reaches_the_shell() {
        let keymap = Keymap::default();
        let ctrl_z = Key {
            ctrl: true,
            ..key(KeyCode::Char('z'))
        };
        let ctrl_alt_z = Key {
            alt: true,
            ..ctrl_z
        };
        let named = |chord: &Key| {
            keymap
                .named
                .iter()
                .find(|(bound, _)| bound == chord)
                .map(|(_, name)| name.as_str())
        };
        assert_eq!(named(&ctrl_alt_z), Some("project_files"));
        assert_eq!(named(&ctrl_z), None, "the extra Alt is a different key");
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Insert, &ctrl_z, &mut pending, &keymap, 0, None, false),
            Action::SendBytes(encode(&ctrl_z, 0, None)),
            "C-z still suspends the job in the shell"
        );
    }

    #[test]
    fn test_config_rebinds_undo_redo() {
        let mut editing = HashMap::new();
        editing.insert("C+z".to_string(), "prompt_undo".to_string());
        editing.insert("C+y".to_string(), "prompt_redo".to_string());
        let keymap = Keymap::from_config(None, Some(&editing));

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
        // The default C-/ is dropped because undo was rebound.
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
        let keymap = Keymap::from_config(None, Some(&editing));

        let ctrl_u = Key {
            ctrl: true,
            ..key(KeyCode::Char('u'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Insert, &ctrl_u, &mut pending, &keymap, 0, None, false),
            Action::Edit(EditAction::DeleteToLineStart)
        );
        // The default C-DEL binding survives (a different action).
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
        let keymap = Keymap::from_config(Some(&bindings), None);

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

        // C+b opens a sequence; C+b then o closes other panes.
        let ctrl_b = Key {
            ctrl: true,
            ..key(KeyCode::Char('b'))
        };
        assert_eq!(
            keymap.resolve_sequence(&[ctrl_b], None),
            SequenceStep::Pending
        );
        assert_eq!(
            keymap.resolve_sequence(&[ctrl_b, key(KeyCode::Char('o'))], None),
            SequenceStep::Run(Action::CloseOtherPanes)
        );
    }
    #[test]
    fn test_global_action_open_settings_by_default() {
        let keymap = Keymap::default();
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
        let keymap = Keymap::from_config(Some(&bindings), None);

        let ctrl_shift_o = Key {
            ctrl: true,
            shift: true,
            sup: false,
            ..key(KeyCode::Char('o'))
        };
        assert_eq!(
            keymap.global_action(&ctrl_shift_o),
            Some(Action::OpenSettings)
        );

        // The default C-, no longer opens settings once rebound.
        let ctrl_comma = Key {
            ctrl: true,
            ..key(KeyCode::Char(','))
        };
        assert_eq!(keymap.global_action(&ctrl_comma), None);
    }
    #[test]
    fn test_config_rebinds_index_action_and_drops_default() {
        let mut bindings = HashMap::new();
        // Move focus_pane_2 off its default M-2 chord onto M-q.
        bindings.insert("M+q".to_string(), "focus_pane_2".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);

        let alt_q = Key {
            alt: true,
            ..key(KeyCode::Char('q'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_q, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(2)
        );

        // The default M-2 no longer focuses pane 2: it was dropped in favor
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

        // An unmentioned index (M-3) keeps its default.
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
    fn test_f8_walks_the_errors_by_default() {
        let keymap = Keymap::default();
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(
                Mode::Normal,
                &key(KeyCode::F(8)),
                &mut pending,
                &keymap,
                0,
                None,
                false
            ),
            Action::RunCommand("next_error".to_string())
        );
    }

    #[test]
    fn test_config_binds_a_chord_to_a_palette_only_command() {
        // `mux_new_session` has no `WindowAction` variant; the chord must
        // resolve to `RunCommand`, not silently drop like it used to.
        let mut bindings = HashMap::new();
        bindings.insert("F5".to_string(), "mux_new_session".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);

        let f5 = key(KeyCode::F(5));
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &f5, &mut pending, &keymap, 0, None, false),
            Action::RunCommand("mux_new_session".to_string())
        );
    }
    #[test]
    fn test_config_named_command_displaces_the_default_window_action_on_that_chord() {
        // Rebinding a chord that already has a default WindowAction (M-h /
        // focus_left) to a named command must actually take effect, not be
        // shadowed by the still-active default.
        let mut bindings = HashMap::new();
        bindings.insert("M+h".to_string(), "mux_new_session".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);

        let alt_h = key(KeyCode::Char('h'));
        let alt_h = Key { alt: true, ..alt_h };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_h, &mut pending, &keymap, 0, None, false),
            Action::RunCommand("mux_new_session".to_string())
        );
    }
    #[test]
    fn test_chord_hint_reports_a_bound_named_command() {
        let mut bindings = HashMap::new();
        bindings.insert("F5".to_string(), "mux_new_session".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);
        assert_eq!(keymap.chord_hint("mux_new_session"), "<f5>");
        assert_eq!(keymap.chord_hint("cd_recent"), "");
    }
    #[test]
    fn test_specific_index_binding_targets_a_fixed_pane() {
        let mut bindings = HashMap::new();
        bindings.insert("M+q".to_string(), "focus_pane_1".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);

        let alt_q = Key {
            alt: true,
            ..key(KeyCode::Char('q'))
        };
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve_with(Mode::Normal, &alt_q, &mut pending, &keymap, 0, None, false),
            Action::FocusPaneByIndex(1)
        );

        // An unrelated default (M-3) is untouched by the new binding.
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
        let win = Keymap::default();
        let mut pending = PendingPrefix::None;

        // Press `"`
        let a = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                sup: false,
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
                sup: false,
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
                sup: false,
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
        let win = Keymap::default();

        // `ds"`
        let mut pending = PendingPrefix::None;
        let _ = resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
        let win = Keymap::default();

        // `cw`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
        let win = Keymap::default();

        // `gn`
        let mut pending = PendingPrefix::None;
        resolve_with(
            Mode::Normal,
            &Key {
                alt: false,
                shift: false,
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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
                sup: false,
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

    #[test]
    fn test_move_tab_keybindings_and_hints() {
        let win = Keymap::default();
        assert_eq!(win.chord_hint("move_tab_left"), "M-g M-h");
        assert_eq!(win.chord_hint("move_tab_down"), "M-g M-j");
        assert_eq!(win.chord_hint("move_tab_up"), "M-g M-k");
        assert_eq!(win.chord_hint("move_tab_right"), "M-g M-l");

        let alt_g = Key::with_alt(KeyCode::Char('g'));
        assert_eq!(win.resolve_sequence(&[alt_g], None), SequenceStep::Pending);
        for (c, dir) in [
            ('h', FocusDir::Left),
            ('j', FocusDir::Down),
            ('k', FocusDir::Up),
            ('l', FocusDir::Right),
        ] {
            assert_eq!(
                win.resolve_sequence(&[alt_g, Key::with_alt(KeyCode::Char(c))], None),
                SequenceStep::Run(Action::MoveTabToPane(dir))
            );
        }
        // A bare h after M-g is not M-h: the follow key matches whole.
        assert_eq!(
            win.resolve_sequence(&[alt_g, Key::plain(KeyCode::Char('h'))], None),
            SequenceStep::Miss
        );
    }

    #[test]
    fn test_a_tool_sequence_shares_a_window_leader_and_wins_only_in_its_tool() {
        let alt_g = Key::with_alt(KeyCode::Char('g'));
        let r = Key::plain(KeyCode::Char('r'));
        let win = Keymap::default();
        assert_eq!(
            win.resolve_sequence(&[alt_g, r], Some("git")),
            SequenceStep::Tool("open_remote".to_string())
        );
        // Outside the Git tool the same keys are no binding at all.
        assert_eq!(win.resolve_sequence(&[alt_g, r], None), SequenceStep::Miss);
        // The window's own sequences under that leader still run in Git.
        assert_eq!(
            win.resolve_sequence(&[alt_g, Key::with_alt(KeyCode::Char('h'))], Some("git")),
            SequenceStep::Run(Action::MoveTabToPane(FocusDir::Left))
        );
        let hint = win.sequence_hint(&[alt_g], Some("git"), |_, name| name.replace('_', " "));
        assert!(hint.contains(&("r".to_string(), "open remote".to_string())));
        assert!(hint.contains(&("M-h".to_string(), "move tab left".to_string())));
    }

    #[test]
    fn test_a_user_tool_block_rebinds_a_tool_command() {
        let mut git = HashMap::new();
        git.insert("M-o".to_string(), "open_remote".to_string());
        let mut blocks = HashMap::new();
        blocks.insert("git".to_string(), git);
        let keymap = Keymap::from_blocks(&blocks);
        let alt_o = Key::with_alt(KeyCode::Char('o'));
        assert_eq!(
            keymap.resolve_sequence(&[alt_o], Some("git")),
            SequenceStep::Tool("open_remote".to_string())
        );
        // The default `M-g r` is dropped for the rebound command.
        let alt_g = Key::with_alt(KeyCode::Char('g'));
        assert_eq!(
            keymap.resolve_sequence(&[alt_g, Key::plain(KeyCode::Char('r'))], Some("git")),
            SequenceStep::Miss
        );
    }

    #[test]
    fn test_a_sequence_may_name_a_palette_command_and_run_three_chords() {
        let mut bindings = HashMap::new();
        bindings.insert("C-x t n".to_string(), "mux_new_session".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);
        let ctrl_x = Key {
            ctrl: true,
            ..key(KeyCode::Char('x'))
        };
        let t = key(KeyCode::Char('t'));
        assert_eq!(
            keymap.resolve_sequence(&[ctrl_x, t], None),
            SequenceStep::Pending
        );
        assert_eq!(
            keymap.resolve_sequence(&[ctrl_x, t, key(KeyCode::Char('n'))], None),
            SequenceStep::Run(Action::RunCommand("mux_new_session".to_string()))
        );
        assert_eq!(keymap.chord_hint("mux_new_session"), "C-x t n");
    }

    /// The palette's and the tools' commands, as the app checks against.
    const CATALOG: Catalog = Catalog {
        is_command: |name| crate::model::palette::command_label(name).is_some(),
        tool_commands: crate::tools::block_commands,
    };

    fn check(blocks: &Blocks) -> Vec<String> {
        Keymap::load(blocks, CATALOG).1
    }

    fn blocks_of(entries: &[(&str, &[(&str, &str)])]) -> HashMap<String, HashMap<String, String>> {
        entries
            .iter()
            .map(|(block, bindings)| {
                let bindings = bindings
                    .iter()
                    .map(|(spec, name)| (spec.to_string(), name.to_string()))
                    .collect();
                (block.to_string(), bindings)
            })
            .collect()
    }

    #[test]
    fn test_the_shipped_keybindings_have_no_problems() {
        assert_eq!(check(&default_bindings_maps()), Vec::<String>::new());
    }

    #[test]
    fn test_a_reported_binding_is_left_out_and_the_default_on_its_chord_stays() {
        let blocks = blocks_of(&[("window", &[("M-h", "no_such_command")])]);
        let (keymap, problems) = Keymap::load(&blocks, CATALOG);
        assert_eq!(problems.len(), 1);
        let alt_h = Key::with_alt(KeyCode::Char('h'));
        assert_eq!(
            keymap.direct_action(&alt_h, false),
            Some(Action::FocusPane(FocusDir::Left)),
            "the default focus_left still answers M-h"
        );
    }

    #[test]
    fn test_a_tool_chord_a_global_chord_takes_first_is_reported() {
        let blocks = blocks_of(&[("git", &[("C-S-n", "open_remote")])]);
        assert_eq!(
            check(&blocks),
            vec![
                r#"git: "C-S-n" never runs: "C-S-n" is a global chord, taken before any tool sees it"#
                    .to_string()
            ]
        );
    }

    #[test]
    fn test_check_blocks_reports_what_could_never_run() {
        let blocks = blocks_of(&[
            (
                "window",
                &[("M-q q", "split_vertical"), ("M-z", "no_such_command")],
            ),
            ("editing", &[("C-x C-y", "prompt_undo")]),
            ("git", &[("M-g r", "open_remot")]),
            ("gti", &[("M-o", "open_remote")]),
        ]);
        let problems = check(&blocks);
        for expected in [
            r#"window: "M-z" runs unknown command "no_such_command""#,
            r#"editing: "C-x C-y" is not a chord"#,
            r#"git: "M-g r" runs unknown command "open_remot""#,
            "`gti` is no tool's block",
        ] {
            assert!(
                problems.iter().any(|p| p == expected),
                "missing {expected:?} in {problems:?}"
            );
        }
        assert_eq!(problems.len(), 4, "{problems:?}");
    }

    #[test]
    fn test_check_blocks_reports_bindings_that_overlap_across_window_and_editing() {
        let blocks = blocks_of(&[
            ("window", &[("M-g", "new_tab")]),
            ("editing", &[("M-g", "prompt_undo")]),
        ]);
        assert_eq!(
            check(&blocks),
            vec![r#"window: "M-g" and editing: "M-g" overlap: only one can run"#.to_string()]
        );
    }

    #[test]
    fn test_where_a_block_overlaps_itself_the_longer_binding_is_kept() {
        let mut bindings = HashMap::new();
        bindings.insert("M-g".to_string(), "new_tab".to_string());
        bindings.insert("M-g x".to_string(), "close_tab".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);
        let alt_g = Key::with_alt(KeyCode::Char('g'));
        assert_eq!(keymap.global_action(&alt_g), None);
        assert_eq!(keymap.direct_action(&alt_g, false), None);
        assert_eq!(
            keymap.resolve_sequence(&[alt_g, key(KeyCode::Char('x'))], None),
            SequenceStep::Run(Action::CloseTab(None))
        );
    }

    #[test]
    fn test_an_editing_chord_takes_its_key_from_a_window_sequence_leader() {
        let mut editing = HashMap::new();
        editing.insert("M-g".to_string(), "prompt_undo".to_string());
        let keymap = Keymap::from_config(None, Some(&editing));
        let alt_g = Key::with_alt(KeyCode::Char('g'));
        assert_eq!(keymap.resolve_sequence(&[alt_g], None), SequenceStep::Miss);
        assert!(keymap.edit_binding(&alt_g).is_some());
    }
}
