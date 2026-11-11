//! Installing an extension from its component, from the palette: the file
//! browser chooses it, a picker shows what it asks to reach, and only once
//! that is allowed is it installed, as `winter extension install` does (see
//! `extensions::manage`). What is installed, and what each may reach, is the
//! Extensions page's (see `editors::extensions`). A change takes effect
//! when Winter next starts.

use std::path::{Path, PathBuf};

use misty::picker::{Row, RowKey};
use winter_model::path::expand_home;

use crate::editors::extensions::ExtensionsPage;
use crate::extensions::manage::{self, Places, Review};

use super::editor::EditorKind;
use super::picker::sources::{BrowserSource, Chooser, Entry, Listing};
use super::picker::WindowOutcome;
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// Said after an install, which takes effect when Winter next starts.
const RESTART: &str = "restart Winter for it to take effect";

/// Said where there is nowhere to keep approvals.
const NO_STATE_DIR: &str = "there is no state directory to keep approvals in";

/// The directory the installer starts in.
const INSTALL_FROM: &str = "~";

/// What the installer's browser is headed by.
const INSTALL_TITLE: &str = "Install an extension: choose its component";

// ========================================================================
// Data Structures
// ========================================================================

/// A step of installing an extension from its component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum ExtensionRequest {
    /// Install the component reviewed, granting what it asks anew.
    Install(Box<Review>),
    /// Leave the component reviewed uninstalled, by its id.
    Refuse(String),
    /// Read the component at a path, to show what it asks.
    Review(PathBuf),
}

// ========================================================================
// App: installing extensions
// ========================================================================

impl App {
    /// Open the file browser to choose a component to install.
    pub(in crate::window) fn open_extension_installer(&mut self) {
        let chooser = Chooser {
            file: Box::new(|path| request(ExtensionRequest::Review(path))),
            title: INSTALL_TITLE.to_string(),
        };
        let source = BrowserSource::choosing(expand_home(INSTALL_FROM), chooser);
        self.open_picker(Box::new(source));
    }

    /// Do what a step of the installer asked.
    pub(in crate::window) fn carry_out_extension(&mut self, asked: ExtensionRequest) {
        let Some(places) = Places::user() else {
            self.set_error(NO_STATE_DIR);
            return;
        };
        let done = match asked {
            ExtensionRequest::Install(review) => self.install_reviewed(*review, &places),
            ExtensionRequest::Refuse(id) => {
                self.set_notice(format!("{id} was not installed"));
                Ok(())
            }
            ExtensionRequest::Review(path) => self.review_component(&path, &places),
        };
        if let Err(why) = done {
            self.set_error(why);
        }
    }

    /// Show what the component at `path` asks, to approve, or install it at
    /// once where it asks nothing beyond what was approved before.
    fn review_component(&mut self, path: &Path, places: &Places) -> Result<(), String> {
        let review = manage::review(path, places)?;
        if review.new.is_empty() && review.previous.is_some() {
            return self.install_reviewed(review, places);
        }
        let heading = review.package.heading();
        let mut list = Listing::new("extension_review", format!("Install {heading}?"));
        if let Some(description) = &review.package.description {
            list.push(Entry::read_only(Row::new(RowKey(0), description.clone())));
        }
        let asks = match review.new.lines() {
            lines if lines.is_empty() => vec!["nothing beyond its own directory".to_string()],
            lines => lines,
        };
        for line in asks {
            list.push(Entry::read_only(
                Row::new(RowKey(0), line).with_hint("asked"),
            ));
        }
        let id = review.package.id.clone();
        let allow = Row::new(RowKey(0), "Allow: install it, granting these");
        list.push(Entry::new(
            allow,
            request(ExtensionRequest::Install(Box::new(review))),
        ));
        let refuse = Row::new(RowKey(0), "Refuse: leave it uninstalled");
        list.push(Entry::new(refuse, request(ExtensionRequest::Refuse(id))));
        self.open_picker(Box::new(list));
        Ok(())
    }

    fn install_reviewed(&mut self, review: Review, places: &Places) -> Result<(), String> {
        let id = review.package.id.clone();
        manage::install(review, places)?;
        self.set_notice(format!("installed {id}: {RESTART}"));
        // The Extensions page lists it at once.
        if let Some(tab) = self.page_tab(EditorKind::Extensions) {
            if let Some(slot) = self.open_tabs.page_mut(&tab) {
                slot.page.with_kind_mut(ExtensionsPage::reread);
            }
        }
        Ok(())
    }
}

// ========================================================================
// Functions
// ========================================================================

fn request(asked: ExtensionRequest) -> WindowOutcome {
    WindowOutcome::Extension(asked)
}
