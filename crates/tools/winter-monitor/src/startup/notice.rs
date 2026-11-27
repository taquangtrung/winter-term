//! What the startup monitor tells the user, kept typed until the window
//! takes it, so its wording is decided in one `match`.

use winter_model::editor::Notice;
use winter_model::notice::SearchMiss;

// ========================================================================
// Data Structures
// ========================================================================

/// One message for the status bar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StartupNotice {
    /// The entry is set up by no file to open.
    NoFile,
    /// A service of the whole machine, which needs an administrator to
    /// turn `on` or off.
    NeedsRoot {
        /// The unit.
        name: String,
        /// Whether it would be turned on.
        on: bool,
    },
    /// An entry changed in its file, not here.
    NotHere,
    /// A search over the entries that moved nowhere.
    Search(SearchMiss),
    /// A service turned on or off, or why it could not be.
    Toggled {
        /// What the command said, where it failed.
        failure: Option<String>,
        /// The unit.
        name: String,
        /// Whether it was turned on.
        on: bool,
    },
}

// ========================================================================
// StartupNotice
// ========================================================================

impl StartupNotice {
    /// The notice the window shows.
    pub(crate) fn into_notice(self) -> Notice {
        let word = |on: bool| if on { "on" } else { "off" };
        match self {
            Self::NoFile => Notice::info("it is set up by no file of its own"),
            Self::NeedsRoot { name, on } => Notice::refusal(format!(
                "a service of the whole machine: run `sudo systemctl {} {name}` in a terminal",
                if on { "enable" } else { "disable" }
            )),
            Self::NotHere => Notice::refusal("change it in its file: Enter opens it"),
            Self::Search(miss) => miss.into_notice(),
            Self::Toggled {
                failure: None,
                name,
                on,
            } => Notice::info(format!("turned {} {name}", word(on))),
            Self::Toggled {
                failure: Some(detail),
                name,
                on,
            } => Notice::error(format!("could not turn {name} {}", word(on))).with_detail(detail),
        }
    }
}
