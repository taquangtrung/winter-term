//! A Vim command line, `:` in Normal or Visual mode, for the commands that
//! act on many lines at once:
//!
//! - `:g/pattern/command` runs the command on every line matching the
//!   pattern, `:v/pattern/command` (or `:g!`) on every line not matching it.
//! - `:normal keys` types the keys on each line of a range, in Normal mode,
//!   `<esc>`, `<cr>`, `<tab>`, `<bs>`, and `<lt>` spelling those keys.
//! - `:d` deletes the range's lines, `:s/pattern/replacement/` replaces the
//!   first match on each (`g` after it, every match; `i`, any case).
//! - `:w` saves.
//!
//! A range is `%` (the file), `'<,'>` (the selection, as `:` from Visual
//! mode starts with), a line number or two (`3,8`), `.` (the cursor's line),
//! or `$` (the last); with none, the cursor's line, or the file for `:g`.
//! Patterns are regular expressions as Rust's `regex` reads them. Whatever a
//! command changes, one `u` takes it back.

use regex::Regex;
use winter_model::editor::{EditorOutcome, Notice, PromptMode, PromptRequest};
use winter_model::input::{Key, KeyCode};
use winter_model::textedit::modal::{EditMode, EditOutcome};

use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The question a command line answers.
pub(super) const ASK_EX: &str = "ex";

/// What the line starts with from Visual mode: the selection's lines.
const SELECTION_RANGE: &str = "'<,'>";

/// The keys `:normal` spells by name.
const NAMED_KEYS: [(&str, KeyCode); 5] = [
    ("<esc>", KeyCode::Escape),
    ("<cr>", KeyCode::Enter),
    ("<tab>", KeyCode::Tab),
    ("<bs>", KeyCode::Backspace),
    ("<lt>", KeyCode::Char('<')),
];

// ========================================================================
// Data Structures
// ========================================================================

/// A range read off a command line, as its first and last line, and what
/// follows it.
type Ranged<'a> = (Option<(usize, usize)>, &'a str);

/// What a command line asks for.
#[derive(Clone, Debug, PartialEq)]
enum Command {
    Delete,
    Global(Global),
    Normal(String),
    Substitute(Substitute),
    Write,
}

/// `:g`: the pattern, whether lines matching it or the rest are acted on,
/// and what is done on each.
#[derive(Clone, Debug, PartialEq)]
struct Global {
    command: Box<Command>,
    keep: bool,
    pattern: String,
}

/// `:s`: what is looked for, what goes in its place, and whether every
/// match on a line is replaced or the first.
#[derive(Clone, Debug, PartialEq)]
struct Substitute {
    every: bool,
    ignore_case: bool,
    pattern: String,
    replacement: String,
}

// ========================================================================
// TextEditor: the command line
// ========================================================================

impl TextEditor {
    /// Ask for a command line, the selection's lines its range where there
    /// is a selection.
    pub(super) fn ask_ex(&mut self) -> EditorOutcome {
        let initial = match self.selection() {
            Some(_) => SELECTION_RANGE.to_string(),
            None => String::new(),
        };
        EditorOutcome::Prompt(PromptRequest {
            initial,
            label: ":".to_string(),
            mode: PromptMode::Text,
            tag: ASK_EX,
            title: None,
        })
    }

    /// Run the command line `line`.
    pub(super) fn run_ex(&mut self, line: &str) -> EditorOutcome {
        if line.trim().is_empty() {
            return EditorOutcome::Consumed;
        }
        if let Some(refused) = self.refuse_edits() {
            return refused;
        }
        let lines = self.buffer().lines().len();
        let selected = self
            .selection()
            .map(|selected| (selected.start.0, selected.end.0));
        self.leave_to_normal();
        let cursor = self.buffer().row();
        let (range, rest) = match parse_range(line.trim_start(), cursor, lines, selected) {
            Ok(parsed) => parsed,
            Err(why) => return self.refuse_ex(why),
        };
        let command = match parse_command(rest.trim_start()) {
            Ok(command) => command,
            Err(why) => return self.refuse_ex(why),
        };
        let rows: Vec<usize> = match (&command, range) {
            (Command::Global(_), None) => (0..lines).collect(),
            (_, Some((first, last))) => (first..=last).collect(),
            (_, None) => vec![cursor],
        };
        if command == Command::Write {
            return self.carry_out(EditOutcome::Save);
        }
        let depth = self.buffer().undo_depth();
        let done = match self.run_on(&command, &rows) {
            Ok(done) => done,
            Err(why) => return self.refuse_ex(why),
        };
        self.buffer_mut().join_steps(depth);
        let said = match done {
            1 => "1 line".to_string(),
            n => format!("{n} lines"),
        };
        self.modal.report(Notice::info(said).unlogged());
        EditorOutcome::Consumed
    }

    /// Do `command` on the lines `rows`, the last first, so a change to one
    /// line leaves the ones above it where they were. How many it changed.
    fn run_on(&mut self, command: &Command, rows: &[usize]) -> Result<usize, String> {
        let mut done = 0;
        for row in rows.iter().rev().copied() {
            if row >= self.buffer().lines().len() {
                continue;
            }
            match command {
                Command::Delete => {
                    self.buffer_mut().replace_rows(row, 1, Vec::new());
                    done += 1;
                }
                Command::Global(global) => {
                    let pattern = Regex::new(&global.pattern).map_err(|error| error.to_string())?;
                    let matches = pattern.is_match(&self.buffer().lines()[row]);
                    if matches == global.keep {
                        done += self.run_on(&global.command, &[row])?;
                    }
                }
                Command::Normal(keys) => {
                    self.buffer_mut().move_to(row, 0);
                    for key in keys_of(keys) {
                        let _ = <Self as winter_model::editor::Page>::on_key(self, &key);
                    }
                    if self.modal.mode() != EditMode::Normal {
                        self.leave_to_normal();
                    }
                    done += 1;
                }
                Command::Substitute(substitute) => {
                    let pattern = substitute_pattern(substitute)?;
                    let line = &self.buffer().lines()[row];
                    let replaced = match substitute.every {
                        true => pattern.replace_all(line, substitute.replacement.as_str()),
                        false => pattern.replace(line, substitute.replacement.as_str()),
                    };
                    if replaced != *line {
                        let replaced = replaced.into_owned();
                        self.buffer_mut().replace_rows(row, 1, vec![replaced]);
                        done += 1;
                    }
                }
                Command::Write => {}
            }
        }
        Ok(done)
    }

    /// Say `why`, and do nothing.
    fn refuse_ex(&mut self, why: String) -> EditorOutcome {
        self.modal.report(Notice::refusal(why));
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The range `line` starts with, as its first and last line counting from
/// zero, and the rest of it; `None` where it names no range.
fn parse_range(
    line: &str,
    cursor: usize,
    lines: usize,
    selected: Option<(usize, usize)>,
) -> Result<Ranged<'_>, String> {
    if let Some(rest) = line.strip_prefix('%') {
        return Ok((Some((0, lines - 1)), rest));
    }
    if let Some(rest) = line.strip_prefix(SELECTION_RANGE) {
        let Some(selected) = selected else {
            return Err("nothing is selected".to_string());
        };
        return Ok((Some(selected), rest));
    }
    let (first, rest) = match address(line, cursor, lines) {
        Some(parsed) => parsed,
        None => return Ok((None, line)),
    };
    let (last, rest) = match rest.strip_prefix(',') {
        Some(after) => {
            address(after, cursor, lines).ok_or("a range needs a line after the comma")?
        }
        None => (first, rest),
    };
    match first <= last {
        true => Ok((Some((first, last)), rest)),
        false => Ok((Some((last, first)), rest)),
    }
}

/// The line `text` starts by naming, counting from zero, and the rest.
fn address(text: &str, cursor: usize, lines: usize) -> Option<(usize, &str)> {
    if let Some(rest) = text.strip_prefix('.') {
        return Some((cursor, rest));
    }
    if let Some(rest) = text.strip_prefix('$') {
        return Some((lines - 1, rest));
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let number: usize = text[..digits].parse().ok()?;
    Some((number.clamp(1, lines) - 1, &text[digits..]))
}

/// The command `text` asks for, after its range.
fn parse_command(text: &str) -> Result<Command, String> {
    let (name, rest) = text.split_at(
        text.find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(text.len()),
    );
    match name {
        "d" | "delete" if rest.trim().is_empty() => Ok(Command::Delete),
        "w" | "write" if rest.trim().is_empty() => Ok(Command::Write),
        "norm" | "normal" => match rest.strip_prefix(' ').unwrap_or(rest) {
            "" => Err(":normal needs the keys to type".to_string()),
            keys => Ok(Command::Normal(keys.to_string())),
        },
        "s" | "substitute" => {
            let parts = split_delimited(rest, 3)?;
            let flags = parts.get(2).map_or("", String::as_str);
            Ok(Command::Substitute(Substitute {
                every: flags.contains('g'),
                ignore_case: flags.contains('i'),
                pattern: parts[0].clone(),
                replacement: parts.get(1).cloned().unwrap_or_default(),
            }))
        }
        "g" | "global" | "v" | "vglobal" => {
            let (keep, rest) = match (name, rest.strip_prefix('!')) {
                ("g" | "global", Some(after)) => (false, after),
                ("g" | "global", None) => (true, rest),
                _ => (false, rest),
            };
            let parts = split_delimited(rest, 2)?;
            let inner = parts.get(1).map_or("", String::as_str).trim();
            let command = match inner {
                "" => {
                    return Err(":g needs a command to run on each line: d, normal, s".to_string())
                }
                inner => parse_command(inner)?,
            };
            if matches!(command, Command::Global(_) | Command::Write) {
                return Err(":g runs d, normal, or s on each line".to_string());
            }
            Ok(Command::Global(Global {
                command: Box::new(command),
                keep,
                pattern: parts[0].clone(),
            }))
        }
        "" => Err("no command after the range".to_string()),
        other => Err(format!(
            ":{other} is not a command this line knows: g, v, normal, d, s, w"
        )),
    }
}

/// `text` cut where its first character, the delimiter, comes again (a
/// backslash keeping one), into at most `parts` parts, the last one the
/// rest; the first part must be there.
fn split_delimited(text: &str, parts: usize) -> Result<Vec<String>, String> {
    let mut chars = text.chars();
    let Some(delimiter) = chars
        .next()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
    else {
        return Err("a pattern is written between delimiters: /pattern/".to_string());
    };
    let mut cut = vec![String::new()];
    let mut escaped = false;
    for c in chars {
        let room = cut.len() < parts;
        let last = cut.last_mut().expect("a part");
        match (escaped, c) {
            (true, c) if c == delimiter => {
                last.push(c);
                escaped = false;
            }
            (true, c) => {
                last.push('\\');
                last.push(c);
                escaped = false;
            }
            (false, '\\') => escaped = true,
            (false, c) if c == delimiter && room => cut.push(String::new()),
            (false, c) => last.push(c),
        }
    }
    match cut[0].is_empty() {
        true => Err("the pattern is empty".to_string()),
        false => Ok(cut),
    }
}

/// The pattern `:s` looks for, any case where it says so.
fn substitute_pattern(substitute: &Substitute) -> Result<Regex, String> {
    let pattern = match substitute.ignore_case {
        true => format!("(?i){}", substitute.pattern),
        false => substitute.pattern.clone(),
    };
    Regex::new(&pattern).map_err(|error| error.to_string())
}

/// The keys `keys` spells, a named one in angle brackets.
fn keys_of(keys: &str) -> Vec<Key> {
    let key = |code| Key {
        alt: false,
        code,
        ctrl: false,
        shift: false,
        sup: false,
    };
    let mut found = Vec::new();
    let mut rest = keys;
    'next: while let Some(c) = rest.chars().next() {
        let lower = rest.to_ascii_lowercase();
        for (name, code) in NAMED_KEYS {
            if lower.starts_with(name) {
                found.push(key(code));
                rest = &rest[name.len()..];
                continue 'next;
            }
        }
        // The space bar is a key of its own, as the window sends it.
        let code = match c {
            ' ' => KeyCode::Space,
            c => KeyCode::Char(c),
        };
        found.push(key(code));
        rest = &rest[c.len_utf8()..];
    }
    found
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_range_is_read_as_vim_writes_it() {
        assert_eq!(parse_range("%d", 4, 10, None), Ok((Some((0, 9)), "d")));
        assert_eq!(parse_range("3,5d", 0, 10, None), Ok((Some((2, 4)), "d")));
        assert_eq!(parse_range(".,$d", 7, 10, None), Ok((Some((7, 9)), "d")));
        assert_eq!(
            parse_range("'<,'>norm A;", 0, 10, Some((1, 2))),
            Ok((Some((1, 2)), "norm A;"))
        );
        assert_eq!(parse_range("d", 3, 10, None), Ok((None, "d")));
    }

    #[test]
    fn test_a_global_command_holds_its_pattern_and_what_it_runs() {
        let parsed = parse_command("g/TODO/normal A!").expect("reads");
        assert_eq!(
            parsed,
            Command::Global(Global {
                command: Box::new(Command::Normal("A!".to_string())),
                keep: true,
                pattern: "TODO".to_string(),
            })
        );
        let Command::Global(inverted) = parse_command("v/a\\/b/d").expect("reads") else {
            panic!("a global");
        };
        assert!(!inverted.keep);
        assert_eq!(
            inverted.pattern, "a/b",
            "an escaped delimiter is the pattern's"
        );
        assert!(parse_command("g/x/").is_err(), "nothing to run");
    }

    #[test]
    fn test_named_keys_are_spelled_in_angle_brackets() {
        let codes: Vec<KeyCode> = keys_of("A;<Esc>x")
            .into_iter()
            .map(|key| key.code)
            .collect();
        assert_eq!(
            codes,
            [
                KeyCode::Char('A'),
                KeyCode::Char(';'),
                KeyCode::Escape,
                KeyCode::Char('x')
            ]
        );
    }
}
