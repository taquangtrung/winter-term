//! What pickers remember across restarts: each picker's earlier queries, and
//! how often and how lately each of its rows was chosen.

use std::collections::BTreeMap;

// ========================================================================
// Constants
// ========================================================================

/// How many queries each picker keeps.
const MAX_QUERIES: usize = 100;

/// The longest query kept, in bytes.
const MAX_QUERY_BYTES: usize = 1024;

/// How many rows each picker remembers the use of.
const MAX_USED: usize = 500;

/// What one more use of a row is worth, as seconds of lateness: a row chosen
/// often ranks with one chosen lately.
const USE_WEIGHT_SECS: u64 = 24 * 60 * 60;

// ========================================================================
// Data Structures
// ========================================================================

/// Each picker's earlier queries, newest first, by the picker's id.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct History(BTreeMap<String, Vec<String>>);

/// How often and how lately each picker's rows were chosen, by the picker's
/// id and then by a name the source gives the row: a command's id, a path.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct Recency(BTreeMap<String, BTreeMap<String, Use>>);

/// The uses of one row.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct Use {
    /// How many times it was chosen.
    pub count: u32,
    /// When it was last chosen, in seconds since the Unix epoch.
    pub last: u64,
}

// ========================================================================
// History
// ========================================================================

impl History {
    /// `picker`'s queries, newest first.
    pub fn of(&self, picker: &str) -> &[String] {
        self.0.get(picker).map(Vec::as_slice).unwrap_or_default()
    }

    /// Remember `query` as `picker`'s newest, once; an empty or overlong
    /// query is not kept.
    pub fn record(&mut self, picker: &str, query: &str) {
        let query = query.trim();
        if query.is_empty() || query.len() > MAX_QUERY_BYTES {
            return;
        }
        let queries = self.0.entry(picker.to_string()).or_default();
        queries.retain(|kept| kept != query);
        queries.insert(0, query.to_string());
        queries.truncate(MAX_QUERIES);
    }
}

// ========================================================================
// Recency
// ========================================================================

impl Recency {
    /// Remember that `picker`'s row `name` was chosen at `now`.
    pub fn record(&mut self, picker: &str, name: &str, now: u64) {
        let used = self.0.entry(picker.to_string()).or_default();
        let entry = used.entry(name.to_string()).or_default();
        entry.count = entry.count.saturating_add(1);
        entry.last = now;
        if used.len() > MAX_USED {
            let least = used
                .iter()
                .min_by_key(|(_, kept)| rank_of(kept))
                .map(|(name, _)| name.clone());
            if let Some(least) = least {
                used.remove(&least);
            }
        }
    }

    /// How highly `picker`'s row `name` ranks by its use: 0 for never chosen.
    pub fn rank(&self, picker: &str, name: &str) -> u64 {
        self.0
            .get(picker)
            .and_then(|used| used.get(name))
            .map(rank_of)
            .unwrap_or_default()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A use's rank: its lateness, raised by how often it was chosen.
fn rank_of(used: &Use) -> u64 {
    used.last
        .saturating_add(u64::from(used.count).saturating_mul(USE_WEIGHT_SECS))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_each_picker_recalls_only_its_own_queries_newest_first_once() {
        let mut history = History::default();
        history.record("commands", "split");
        history.record("search", "fn main");
        history.record("commands", "zoom");
        history.record("commands", "split");
        history.record("commands", "  ");
        assert_eq!(history.of("commands"), ["split", "zoom"]);
        assert_eq!(history.of("search"), ["fn main"]);
        assert!(history.of("files").is_empty());
    }

    #[test]
    fn test_a_row_chosen_often_outranks_one_chosen_once_a_little_later() {
        let mut recency = Recency::default();
        for at in 0..3 {
            recency.record("files", "often.rs", at);
        }
        recency.record("files", "once.rs", 10);
        assert!(recency.rank("files", "often.rs") > recency.rank("files", "once.rs"));
        assert_eq!(recency.rank("files", "never.rs"), 0);
        assert_eq!(recency.rank("commands", "often.rs"), 0, "kept per picker");
    }

    #[test]
    fn test_the_least_used_row_is_forgotten_past_the_limit() {
        let mut recency = Recency::default();
        for at in 0..=MAX_USED as u64 {
            recency.record("files", &format!("f{at}"), at);
        }
        assert_eq!(recency.rank("files", "f0"), 0, "the oldest goes");
        assert!(recency.rank("files", &format!("f{MAX_USED}")) > 0);
    }
}
