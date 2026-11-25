//! What the model's shared parts tell the user, kept typed until a page
//! takes it: a list's search going nowhere, and an editor stopped by a
//! panic.
//!
//! Each tool wraps a [`SearchMiss`] in its own notice enum, so the words
//! live here once and tests compare variants rather than sentences.

use crate::notify::Notice;

// ========================================================================
// Data Structures
// ========================================================================

/// Why a list's search moved nowhere.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchMiss {
    /// A search was repeated before any was made.
    NoSearch,
    /// No row holds the query.
    NotFound(String),
}

// ========================================================================
// SearchMiss
// ========================================================================

impl SearchMiss {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub fn into_notice(self) -> Notice {
        match self {
            Self::NoSearch => Notice::refusal("no search"),
            Self::NotFound(query) => Notice::refusal(format!("not found: {query}")),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The error an editor stopped by a panic reports: its `name`, what it
/// panicked with, and where, when that is known.
pub(crate) fn stopped(name: &str, message: &str, at: Option<&str>) -> Notice {
    let at = at.map(|at| format!(" ({at})")).unwrap_or_default();
    Notice::error(format!(
        "{name} stopped after an internal error: {message}{at}"
    ))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_miss_names_the_query() {
        let notice = SearchMiss::NotFound("alpha".to_string()).into_notice();
        assert_eq!(notice.text, "not found: alpha");
    }

    #[test]
    fn test_a_stop_says_where_when_it_is_known() {
        assert_eq!(
            stopped("Dir", "boom", Some("src/lib.rs:3")).text,
            "Dir stopped after an internal error: boom (src/lib.rs:3)"
        );
        assert_eq!(
            stopped("Dir", "boom", None).text,
            "Dir stopped after an internal error: boom"
        );
    }
}
