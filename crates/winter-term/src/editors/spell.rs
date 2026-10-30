//! The spelling dictionary every editor checks against: the bundled English
//! one and the words the user has added, loaded the first time one is
//! checked and shared from then on. Over it lie each project's own words,
//! kept in the project's `.winter/words.txt`, and the words ignored until
//! Winter quits; [`accepts`] asks those.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, PoisonError, RwLock, RwLockReadGuard};

use winter_spell::{Dictionary, DictionaryId, UserWords};

use crate::config::config_dir;

// ========================================================================
// Constants
// ========================================================================

/// Where the user's own words are kept, under the config directory.
const WORDS_FILE: [&str; 2] = ["spelling", "words.txt"];

/// Where a project's own words are kept, under its root.
const PROJECT_WORDS_FILE: [&str; 2] = [".winter", "words.txt"];

/// The one dictionary, once it has been loaded.
static DICTIONARY: OnceLock<RwLock<Dictionary>> = OnceLock::new();

/// Each project's own words, by its root, read the first time a file of it
/// is checked, each word in lower case.
static PROJECT_WORDS: OnceLock<RwLock<HashMap<PathBuf, HashSet<String>>>> = OnceLock::new();

/// The words ignored until Winter quits, each in lower case.
static IGNORED: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

/// Moves whenever a word is taken as spelled right anywhere, so a check
/// made before it is made again.
static GENERATION: AtomicU64 = AtomicU64::new(0);

// ========================================================================
// Data Structures
// ========================================================================

/// Where a word is taken as spelled right.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WordList {
    /// In the project at this root, kept in its `.winter/words.txt`.
    Project(PathBuf),
    /// Until Winter quits, kept nowhere.
    Session,
    /// Everywhere, kept in the user's list beside `settings.kdl`.
    User,
}

// ========================================================================
// Functions
// ========================================================================

/// The dictionary, loaded on the first call: the bundled American English
/// one, and every word of the user's list. A list that cannot be read
/// costs its words, not the check.
pub fn dictionary() -> RwLockReadGuard<'static, Dictionary> {
    shared()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Whether `word`, which the dictionary does not know, is taken as spelled
/// right all the same: ignored this session, or one of the words of the
/// project at `project`.
pub fn accepts(word: &str, project: Option<&Path>) -> bool {
    let key = word.to_lowercase();
    let ignored = IGNORED
        .get_or_init(RwLock::default)
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&key);
    ignored || project.is_some_and(|root| with_project_words(root, |words| words.contains(&key)))
}

/// Add `word` to the user's list, and to the dictionary at once, so it is
/// taken as spelled right from here on, in every editor.
pub fn add_word(word: &str) -> io::Result<()> {
    accept(word, &WordList::User)
}

/// Take `word` as spelled right in `list`, from here on and in every
/// editor: written to the list's file where it has one.
pub fn accept(word: &str, list: &WordList) -> io::Result<()> {
    match list {
        WordList::User => {
            UserWords::load(words_path())?.add(word)?;
            shared()
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .add_word(word);
        }
        WordList::Project(root) => {
            UserWords::load(project_words_path(root))?.add(word)?;
            with_project_words(root, |words| words.insert(word.to_lowercase()));
        }
        WordList::Session => {
            IGNORED
                .get_or_init(RwLock::default)
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(word.to_lowercase());
        }
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// How many times a word has been taken as spelled right: a check made at
/// an earlier count is out of date.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The project at `root`'s words, read the first time they are asked for,
/// handed to `f`. A list that cannot be read is an empty one.
fn with_project_words<T>(root: &Path, f: impl FnOnce(&mut HashSet<String>) -> T) -> T {
    let mut all = PROJECT_WORDS
        .get_or_init(RwLock::default)
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    let words = all.entry(root.to_path_buf()).or_insert_with(|| {
        UserWords::load(project_words_path(root))
            .map(|list| list.words().map(str::to_lowercase).collect())
            .unwrap_or_default()
    });
    f(words)
}

/// The user's word list, beside `settings.kdl`.
fn words_path() -> PathBuf {
    WORDS_FILE.iter().fold(config_dir(), |path, part| path.join(part))
}

/// The word list of the project at `root`.
fn project_words_path(root: &Path) -> PathBuf {
    PROJECT_WORDS_FILE.iter().fold(root.to_path_buf(), |path, part| path.join(part))
}

/// The lock round the dictionary, loading it the first time.
fn shared() -> &'static RwLock<Dictionary> {
    DICTIONARY.get_or_init(|| {
        let mut dictionary = Dictionary::bundled(DictionaryId::default());
        let user = UserWords::load(words_path()).ok();
        for word in user.iter().flat_map(UserWords::words) {
            dictionary.add_word(word);
        }
        RwLock::new(dictionary)
    })
}
