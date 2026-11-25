//! The named registers, `a` to `z`, that the terminal's Normal mode yanks
//! into and puts from, kept for the whole window: a yank into `"a` in one
//! pane puts in another. A window is one process with one thread driving
//! it, so they are that thread's.

use std::cell::RefCell;
use std::collections::HashMap;

// ========================================================================
// Constants
// ========================================================================

thread_local! {
    /// The window thread's named registers.
    static REGISTERS: RefCell<HashMap<char, String>> = RefCell::new(HashMap::new());
}

// ========================================================================
// Functions
// ========================================================================

/// The text of named register `name`, where something was put in it.
pub fn register_text(name: char) -> Option<String> {
    REGISTERS.with(|registers| registers.borrow().get(&name).cloned())
}

/// Puts `text` in named register `name`.
pub fn set_register_text(name: char, text: String) {
    REGISTERS.with(|registers| {
        registers.borrow_mut().insert(name, text);
    });
}
