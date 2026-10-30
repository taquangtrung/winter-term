//! What modal editing keeps for the whole window rather than for one editor:
//! the named registers (shared with the terminals' Normal mode), the kill
//! ring, the last search, and the last change. Every text editor in the
//! window reads and writes the same ones, so a yank into `"a` in one file
//! pastes in another, and `n` and `.` repeat across files.
//!
//! A window is one process with one thread driving it, so the state is that
//! thread's.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::model::input::Key;
use crate::model::textedit::buffer::Register;
use crate::model::textedit::kill::KillRing;
use crate::model::textedit::search::Search;

// ========================================================================
// Data Structures
// ========================================================================

/// The window's modal state shared by every editor in it.
#[derive(Debug, Default)]
struct Shared {
    kills: KillRing,
    /// The keys of the last command that changed a text, for `.`.
    last_change: Vec<Key>,
    /// The named registers, `a` to `z`.
    registers: HashMap<char, Register>,
    /// What the last `/` or `?` looked for.
    search: Option<Search>,
}

thread_local! {
    static SHARED: RefCell<Shared> = RefCell::new(Shared::default());
}

// ========================================================================
// Free functions
// ========================================================================

/// Named register `name`, where something was put in it.
pub fn register(name: char) -> Option<Register> {
    with(|shared| shared.registers.get(&name).cloned())
}

/// Put `register` in named register `name`.
pub fn set_register(name: char, register: Register) {
    with(|shared| {
        shared.registers.insert(name, register);
    });
}

/// Named register `name` as plain text, whole lines ending in a newline: what
/// a terminal's Normal mode pastes.
pub fn register_text(name: char) -> Option<String> {
    register(name).map(|held| match held.linewise {
        true => format!("{}\n", held.text),
        false => held.text,
    })
}

/// Put plain `text` in named register `name`, as whole lines when it ends in
/// a newline: what a terminal's Normal mode yanks.
pub fn set_register_text(name: char, text: String) {
    let linewise = text.ends_with('\n');
    let text = text.strip_suffix('\n').map(str::to_string).unwrap_or(text);
    set_register(name, Register { linewise, text });
}

/// Act on the window's kill ring.
pub(super) fn kills<R>(act: impl FnOnce(&mut KillRing) -> R) -> R {
    with(|shared| act(&mut shared.kills))
}

/// The keys of the last command that changed a text.
pub(super) fn last_change() -> Vec<Key> {
    with(|shared| shared.last_change.clone())
}

pub(super) fn set_last_change(keys: Vec<Key>) {
    with(|shared| shared.last_change = keys);
}

/// What the last `/` or `?` looked for.
pub(super) fn search() -> Option<Search> {
    with(|shared| shared.search.clone())
}

pub(super) fn set_search(search: Search) {
    with(|shared| shared.search = Some(search));
}

/// Act on the shared state.
fn with<R>(act: impl FnOnce(&mut Shared) -> R) -> R {
    SHARED.with(|shared| act(&mut shared.borrow_mut()))
}
