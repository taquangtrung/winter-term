//! The settings the text editor follows, handed to it before each paint by
//! whatever keeps them (the app's `settings.kdl`), so the editor needs
//! nothing of how they are read. Their defaults are the built-in settings',
//! written again here for an editor no host has handed settings to yet; the
//! app's tests hold the two to the same values.

use std::collections::BTreeMap;
use std::time::Duration;

use winter_ext_api::InlineOffer;
use winter_lsp::ServerId;
use winter_model::paint::{MarkupMarkers, WrapIndent};

// ========================================================================
// Constants
// ========================================================================

/// How long typing pauses before the completion popup opens by itself, and
/// how long a word is first, as the built-in settings have them.
const COMPLETION_DELAY: Duration = Duration::from_millis(300);
const COMPLETION_MIN_CHARS: usize = 3;

// ========================================================================
// Data Structures
// ========================================================================

/// Everything the text editor takes from the settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorSettings {
    /// How completion behaves.
    pub completion: CompletionConfig,
    /// Whether a faint line marks each level of a line's indentation.
    pub indent_guides: bool,
    /// The extensions' inline sources, in the order they are asked, each
    /// as it stands now.
    pub inline: Vec<InlineOffered>,
    /// When markup's markers hide.
    pub markup_markers: MarkupMarkers,
    /// How far the rows a wrapped line continues on are indented.
    pub wrap_indent: WrapIndent,
}

/// An extension's inline source as the window says it stands: what it
/// offers, and the servers of its extension, which are handed the
/// documents before it is asked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineOffered {
    /// Its full id.
    pub id: String,
    pub offer: InlineOffer,
    /// Its extension's servers running.
    pub servers: Vec<ServerId>,
    /// What the status bar and notices call it.
    pub title: String,
}

/// How the text editor completes what is typed.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionConfig {
    /// Whether the popup opens by itself while a word is typed, once it is
    /// [`Self::min_chars`] long, rather than only on `C-SPC`.
    pub auto: bool,
    /// The back-ends each language asks, in order, by the name settings give
    /// the language; a language not named asks its defaults.
    pub backends: BTreeMap<String, Vec<String>>,
    /// How long typing pauses before the popup opens by itself, so it does
    /// not flicker up under every word typed straight through. `C-SPC`
    /// opens it at once, and an open popup narrows at once.
    pub delay: Duration,
    /// How long a word is before the popup opens by itself.
    pub min_chars: usize,
}

// ========================================================================
// CompletionConfig
// ========================================================================

/// Completion as the built-in settings set it.
impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            auto: true,
            backends: BTreeMap::new(),
            delay: COMPLETION_DELAY,
            min_chars: COMPLETION_MIN_CHARS,
        }
    }
}
