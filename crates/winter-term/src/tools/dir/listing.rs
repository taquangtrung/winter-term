//! Ordering and filtering a listing: what appears in it, and in what order.

use std::cmp::Ordering;

use super::entry::Entry;

// ========================================================================
// Data Structures
// ========================================================================

/// What a listing is ordered by.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortKey {
    /// Most recently modified first.
    Modified,
    /// Alphabetical, ignoring case.
    #[default]
    Name,
    /// Largest first.
    Size,
}

// ========================================================================
// SortKey
// ========================================================================

impl SortKey {
    /// The next key the sort command steps to.
    pub fn next(self) -> Self {
        match self {
            SortKey::Modified => SortKey::Size,
            SortKey::Name => SortKey::Modified,
            SortKey::Size => SortKey::Name,
        }
    }

    /// How the key names itself in the header.
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Modified => "time",
            SortKey::Name => "name",
            SortKey::Size => "size",
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Order two entries: directories first whatever the key, then the key itself,
/// then the name, so a listing never reshuffles between identical reloads.
pub fn compare_entries(a: &Entry, b: &Entry, key: SortKey) -> Ordering {
    b.is_dir()
        .cmp(&a.is_dir())
        .then_with(|| compare_by_key(a, b, key))
        .then_with(|| compare_by_name(a, b))
}

/// Sort `entries` in place.
pub fn sort_entries(entries: &mut [Entry], key: SortKey) {
    entries.sort_by(|a, b| compare_entries(a, b, key));
}

/// Whether `name` is a dotfile. The unix convention is applied on every
/// platform, since it is what the listing's own toggle means.
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn compare_by_key(a: &Entry, b: &Entry, key: SortKey) -> Ordering {
    match key {
        // Newest first, and an entry with no readable timestamp sorts last
        // rather than pretending to be from the epoch.
        SortKey::Modified => match (a.meta.modified, b.meta.modified) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        },
        SortKey::Name => Ordering::Equal,
        SortKey::Size => b.meta.len.cmp(&a.meta.len),
    }
}

/// Case-insensitive, falling back to the exact name so `Cargo.toml` and
/// `cargo.toml` land beside each other in a fixed order.
fn compare_by_name(a: &Entry, b: &Entry) -> Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::dir::entry::{EntryKind, Meta};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, kind: EntryKind, len: u64, age: Option<Duration>) -> Entry {
        Entry::new(
            kind,
            Meta {
                len,
                mode: None,
                modified: age.map(|d| SystemTime::UNIX_EPOCH + d),
            },
            name.to_string(),
            PathBuf::from(name),
        )
    }

    fn names(mut entries: Vec<Entry>, key: SortKey) -> Vec<String> {
        sort_entries(&mut entries, key);
        entries.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn test_directories_lead_whatever_the_sort_key_is() {
        let entries = vec![
            entry("big.txt", EntryKind::File, 900, None),
            entry("zzz", EntryKind::Dir, 0, None),
        ];
        for key in [SortKey::Modified, SortKey::Name, SortKey::Size] {
            assert_eq!(names(entries.clone(), key)[0], "zzz", "key {key:?}");
        }
    }

    #[test]
    fn test_name_sort_ignores_case() {
        let entries = vec![
            entry("banana", EntryKind::File, 0, None),
            entry("Apple", EntryKind::File, 0, None),
        ];
        assert_eq!(names(entries, SortKey::Name), ["Apple", "banana"]);
    }

    #[test]
    fn test_time_sort_puts_an_unreadable_timestamp_last() {
        // Treating a missing timestamp as the epoch would bury a live file
        // under every entry whose stat happened to fail.
        let entries = vec![
            entry("unknown", EntryKind::File, 0, None),
            entry("older", EntryKind::File, 0, Some(Duration::from_secs(10))),
            entry("newer", EntryKind::File, 0, Some(Duration::from_secs(99))),
        ];
        assert_eq!(
            names(entries, SortKey::Modified),
            ["newer", "older", "unknown"]
        );
    }

    #[test]
    fn test_size_sort_is_largest_first() {
        let entries = vec![
            entry("small", EntryKind::File, 1, None),
            entry("large", EntryKind::File, 4096, None),
        ];
        assert_eq!(names(entries, SortKey::Size), ["large", "small"]);
    }

    #[test]
    fn test_sort_key_cycle_returns_to_its_start() {
        let key = SortKey::Name;
        assert_eq!(key.next().next().next(), key);
    }
}
