//! Bindings that apply only in a context: a `keybindings.kdl` line with a
//! `when` condition (`lsp_rename "F2" in="editor" when="lsp && normal"`),
//! looked up ahead of every other binding, an editor's own keys included.
//! Where several hold for a key, the most specific wins: the one with the
//! most conditions, then the user's over a built-in one, then the later line.
//!
//! A condition is terms joined by `&&`, each perhaps negated with `!`:
//! `focus == git` (or the focus alone, `git`), `mode == insert` (or the mode
//! alone, `insert`), `lang == rust`, `selection`, and `lsp`; `name != value`
//! is the negation of `name == value`. Either-or is written as two lines.

use kdl::{KdlDocument, KdlNode};

use super::chord::parse_chord_sequence;
use super::keyfile::{block_of, platform_of, COMMANDS_NODE};
use super::keymap::DEFAULT_KEYBINDINGS_KDL;
use super::Key;

// ========================================================================
// Constants
// ========================================================================

/// The property naming the condition a line binds under.
pub const WHEN_PROPERTY: &str = "when";

/// What joins a condition's terms, negates one, and compares a name.
const AND: &str = "&&";
const NOT: char = '!';
const EQUALS: &str = "==";
const NOT_EQUALS: &str = "!=";

/// The names a term compares, and the names that stand alone.
const FOCUS: &str = "focus";
const MODE: &str = "mode";
const LANG: &str = "lang";
const SELECTION: &str = "selection";
const LSP: &str = "lsp";

/// The modes a mode's name alone stands for.
const MODES: [&str; 4] = ["insert", "normal", "replace", "visual"];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key is pressed in, as conditions read it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyContext {
    /// What has the keys: `terminal`, `picker`, or an editor's block
    /// (`editor`, `dir`, `git`, ...).
    pub focus: String,
    /// The language of the file in front, as settings name it.
    pub lang: Option<String>,
    /// Whether a language server serves the file in front.
    pub lsp: bool,
    /// The mode of what has the keys: `insert`, `normal`, `replace`,
    /// `visual`.
    pub mode: Option<String>,
    /// Whether something is selected.
    pub selection: bool,
}

/// A condition: terms that must all hold.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Condition {
    terms: Vec<Term>,
}

/// One term of a condition, perhaps negated.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Term {
    negated: bool,
    test: Test,
}

/// What a term tests.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Test {
    Focus(String),
    Lang(String),
    Lsp,
    Mode(String),
    Selection,
}

/// A binding that applies only where its condition holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WhenBinding {
    /// The block it binds in: `window`, or an editor's.
    pub block: String,
    /// The command it runs.
    pub command: String,
    /// Where it applies.
    pub condition: Condition,
    /// The key it answers.
    pub key: Key,
    /// Whether it is the user's own rather than a built-in one.
    pub user: bool,
}

/// Every binding that applies only in a context, in the order read.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WhenMap {
    bindings: Vec<WhenBinding>,
}

// ========================================================================
// Condition
// ========================================================================

impl Condition {
    /// The condition `text` writes, or why it is none.
    pub fn parse(text: &str) -> Result<Self, String> {
        let terms = text
            .split(AND)
            .map(|term| parse_term(term.trim()))
            .collect::<Result<Vec<Term>, String>>()?;
        Ok(Self { terms })
    }

    /// Whether every term holds in `context`.
    pub fn holds(&self, context: &KeyContext) -> bool {
        self.terms.iter().all(|term| {
            let held = match &term.test {
                Test::Focus(focus) => context.focus == *focus,
                Test::Lang(lang) => context.lang.as_deref() == Some(lang.as_str()),
                Test::Lsp => context.lsp,
                Test::Mode(mode) => context.mode.as_deref() == Some(mode.as_str()),
                Test::Selection => context.selection,
            };
            held != term.negated
        })
    }

    /// How specific it is: how many terms it has.
    pub fn specificity(&self) -> usize {
        self.terms.len()
    }
}

// ========================================================================
// WhenMap
// ========================================================================

impl WhenMap {
    /// The bindings `built_in` and then `user` give, in that order.
    pub fn new(built_in: Vec<WhenBinding>, user: Vec<WhenBinding>) -> Self {
        Self {
            bindings: built_in.into_iter().chain(user).collect(),
        }
    }

    /// Whether any binding answers `key`, so a context is worth working out.
    pub fn answers(&self, key: &Key) -> bool {
        self.bindings.iter().any(|binding| binding.key == *key)
    }

    /// The binding `key` runs in `context`: of those answering it whose
    /// condition holds, the most specific, the user's over a built-in one,
    /// then the later.
    pub fn lookup(&self, key: &Key, context: &KeyContext) -> Option<&WhenBinding> {
        self.bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| binding.key == *key && binding.condition.holds(context))
            .max_by_key(|(at, binding)| (binding.condition.specificity(), binding.user, *at))
            .map(|(_, binding)| binding)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The bindings with a condition that `document` gives on `platform`, the
/// user's own where `user`, and what is wrong with the lines that could not
/// be read, which are left out.
pub fn when_bindings(
    document: &KdlDocument,
    platform: &str,
    user: bool,
) -> (Vec<WhenBinding>, Vec<String>) {
    let mut bindings = Vec::new();
    let mut problems = Vec::new();
    let lines = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == COMMANDS_NODE)
        .flat_map(|node| node.children().map(KdlDocument::nodes).unwrap_or_default());
    for line in lines {
        let Some(when) = when_of(line) else {
            continue;
        };
        if platform_of(line).is_some_and(|only| only != platform) {
            continue;
        }
        let command = line.name().value();
        let condition = match Condition::parse(when) {
            Ok(condition) => condition,
            Err(why) => {
                problems.push(format!("{command}'s when: {why}"));
                continue;
            }
        };
        for spec in line
            .entries()
            .iter()
            .filter(|entry| entry.name().is_none())
            .filter_map(|entry| entry.value().as_string())
        {
            match parse_chord_sequence(spec).as_deref() {
                Some([key]) => bindings.push(WhenBinding {
                    block: block_of(line).to_string(),
                    command: command.to_string(),
                    condition: condition.clone(),
                    key: *key,
                    user,
                }),
                Some(_) => problems.push(format!(
                    "{command} \"{spec}\": a binding with a condition is one chord, not a sequence"
                )),
                None => problems.push(format!("{command} \"{spec}\" is no chord")),
            }
        }
    }
    (bindings, problems)
}

/// The bindings with a condition the built-in `keybindings.kdl` gives on
/// this platform.
pub fn default_when_bindings() -> Vec<WhenBinding> {
    let document: KdlDocument = DEFAULT_KEYBINDINGS_KDL
        .parse()
        .expect("the built-in keybindings parse");
    when_bindings(&document, std::env::consts::OS, false).0
}

/// The condition a line binds under, if it names one.
pub fn when_of(line: &KdlNode) -> Option<&str> {
    line.get(WHEN_PROPERTY).and_then(|value| value.as_string())
}

/// One term, `!` and all, or why it is none.
fn parse_term(text: &str) -> Result<Term, String> {
    if text.is_empty() {
        return Err("an empty term".to_string());
    }
    if let Some((name, value)) = text.split_once(NOT_EQUALS) {
        let test = compared(name.trim(), value.trim())?;
        return Ok(Term {
            negated: true,
            test,
        });
    }
    if let Some((name, value)) = text.split_once(EQUALS) {
        let test = compared(name.trim(), value.trim())?;
        return Ok(Term {
            negated: false,
            test,
        });
    }
    let (negated, name) = match text.strip_prefix(NOT) {
        Some(rest) => (true, rest.trim()),
        None => (false, text),
    };
    let test = match name {
        SELECTION => Test::Selection,
        LSP => Test::Lsp,
        mode if MODES.contains(&mode) => Test::Mode(mode.to_string()),
        FOCUS | MODE | LANG => return Err(format!("`{name}` is compared: `{name} == …`")),
        focus if is_word(focus) => Test::Focus(focus.to_string()),
        _ => return Err(format!("`{text}` is no term")),
    };
    Ok(Term { negated, test })
}

/// The test `name == value` makes.
fn compared(name: &str, value: &str) -> Result<Test, String> {
    if !is_word(value) {
        return Err(format!("`{value}` is no value"));
    }
    match name {
        FOCUS => Ok(Test::Focus(value.to_string())),
        LANG => Ok(Test::Lang(value.to_string())),
        MODE if MODES.contains(&value) => Ok(Test::Mode(value.to_string())),
        MODE => Err(format!("`{value}` is no mode")),
        _ => Err(format!("`{name}` is nothing a condition compares")),
    }
}

/// Whether `text` is one word a name or a value may be: letters, digits,
/// `.`, `-`, and `_`.
fn is_word(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::input::KeyCode;

    fn context(focus: &str, mode: &str) -> KeyContext {
        KeyContext {
            focus: focus.to_string(),
            mode: Some(mode.to_string()),
            ..KeyContext::default()
        }
    }

    #[test]
    fn test_a_condition_holds_only_where_every_term_does() {
        let condition = Condition::parse("editor && !insert && lang == rust").expect("parses");
        let mut here = context("editor", "normal");
        assert!(!condition.holds(&here), "no language yet");
        here.lang = Some("rust".to_string());
        assert!(condition.holds(&here));
        assert!(!condition.holds(&KeyContext {
            mode: Some("insert".into()),
            ..here.clone()
        }));
        assert!(!condition.holds(&KeyContext {
            focus: "git".into(),
            ..here
        }));
        assert_eq!(condition.specificity(), 3);
        let unlike = Condition::parse("focus != terminal && selection").expect("parses");
        let selected = KeyContext {
            selection: true,
            ..context("dir", "normal")
        };
        assert!(unlike.holds(&selected));
        assert!(!unlike.holds(&KeyContext {
            selection: true,
            ..context("terminal", "insert")
        }));
    }

    #[test]
    fn test_a_condition_that_cannot_be_read_says_why() {
        for bad in [
            "",
            "lsp &&",
            "mode == sleeping",
            "colour == red",
            "focus",
            "a b",
        ] {
            assert!(Condition::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn test_the_most_specific_binding_wins_then_the_user_s_then_the_later() {
        let key = Key::plain(KeyCode::F(2));
        let binding = |command: &str, when: &str, user: bool| WhenBinding {
            block: "window".to_string(),
            command: command.to_string(),
            condition: Condition::parse(when).expect("parses"),
            key,
            user,
        };
        let map = WhenMap::new(
            vec![
                binding("built_in", "editor", false),
                binding("narrow", "editor && lsp", false),
            ],
            vec![binding("mine", "editor", true)],
        );
        let plain = context("editor", "normal");
        assert_eq!(
            map.lookup(&key, &plain).map(|b| b.command.as_str()),
            Some("mine")
        );
        let served = KeyContext { lsp: true, ..plain };
        assert_eq!(
            map.lookup(&key, &served).map(|b| b.command.as_str()),
            Some("narrow")
        );
        assert_eq!(map.lookup(&key, &context("terminal", "insert")), None);
        assert!(!map.answers(&Key::plain(KeyCode::F(3))));
    }

    #[test]
    fn test_a_file_s_lines_with_a_condition_are_read_and_the_rest_left_to_the_blocks() {
        let document: KdlDocument = r#"commands {
    new_tab "C-t"
    lsp_rename "F2" in="editor" when="lsp"
    split "C-x" when="terminal" platform="macos"
    long "M-g x" when="terminal"
    broken "F3" when="mode == sleeping"
}"#
        .parse()
        .expect("kdl");
        let (bindings, problems) = when_bindings(&document, "linux", true);
        assert_eq!(bindings.len(), 1, "{bindings:?}");
        assert_eq!(
            (bindings[0].block.as_str(), bindings[0].command.as_str()),
            ("editor", "lsp_rename")
        );
        assert_eq!(problems.len(), 2, "{problems:?}");
        let (on_mac, _) = when_bindings(&document, "macos", true);
        assert_eq!(on_mac.len(), 2, "the macOS line binds there");
    }
}
