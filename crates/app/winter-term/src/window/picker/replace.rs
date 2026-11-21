//! Replacing in every line a project search found: the replacement asked
//! for in a picker of its own, then written into each file, each read and
//! written once. A line that no longer holds what was searched for is left
//! alone and counted, since writing over it would throw away a change made
//! since the search.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use regex::{NoExpand, Regex};
use winter_model::editor::SearchHit;
use winter_model::text_file;

use super::outcome::WindowOutcome;
use super::sources::{Typed, pattern_regex};
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The picker that asks for the replacement.
const REPLACE_PICKER: &str = "replace_with";

// ========================================================================
// Data Structures
// ========================================================================

/// The lines a search found, to replace what it searched for in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Replace {
    /// The lines, each file's in any order.
    pub(in crate::window) hits: Vec<SearchHit>,
    /// What was searched for, as typed: case counting only where it has a
    /// capital, and a `*` standing for anything.
    pub(in crate::window) pattern: String,
}

/// What replacing came to.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Replaced {
    /// How many files were written.
    files: usize,
    /// How many lines no longer held the pattern, or were in a file that
    /// could not be read or written, and were left alone.
    refused: usize,
    /// How many lines were changed.
    written: usize,
}

// ========================================================================
// App: replacing
// ========================================================================

impl App {
    /// Ask what to replace `replace`'s pattern with in its lines.
    pub(in crate::window) fn ask_replace(&mut self, replace: Replace) {
        if replace.hits.is_empty() {
            return self.set_notice("nothing to replace".to_string());
        }
        let files: BTreeSet<&PathBuf> = replace.hits.iter().map(|hit| &hit.path).collect();
        let title = format!(
            "Replace {} in {} of {} with",
            replace.pattern,
            lines_count(replace.hits.len()),
            files_count(files.len())
        );
        self.pickers.replacing = Some(replace);
        let typed = Typed::new(REPLACE_PICKER, title, |with| {
            Some(WindowOutcome::ReplaceWith(with.to_string()))
        })
        .placeholder("Replacement");
        self.open_typed(typed, "");
    }

    /// Replace the pattern asked about with `with`, as typed, in its lines.
    pub(in crate::window) fn replace_with(&mut self, with: &str) {
        let Some(replace) = self.pickers.replacing.take() else {
            return;
        };
        let regex = match pattern_regex(&replace.pattern) {
            Ok(regex) => regex,
            Err(why) => return self.set_error(why),
        };
        let done = replace_in_files(&replace.hits, &regex, with);
        let mut said = format!(
            "replaced in {} of {}",
            lines_count(done.written),
            files_count(done.files)
        );
        if done.refused > 0 {
            said.push_str(&format!(
                ", {} changed since the search, left alone",
                lines_count(done.refused)
            ));
        }
        self.set_notice(said);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Replace what `regex` matches with `with`, taken as written, on each line
/// of `hits`, each file read and written once.
fn replace_in_files(hits: &[SearchHit], regex: &Regex, with: &str) -> Replaced {
    let mut by_file: BTreeMap<&PathBuf, BTreeSet<usize>> = BTreeMap::new();
    for hit in hits {
        by_file.entry(&hit.path).or_default().insert(hit.line);
    }
    let mut done = Replaced::default();
    for (path, lines) in by_file {
        let Ok(mut loaded) = text_file::load(path) else {
            done.refused += lines.len();
            continue;
        };
        let mut changed = 0;
        for line in &lines {
            let Some(text) = loaded.lines.get_mut(line.saturating_sub(1)) else {
                done.refused += 1;
                continue;
            };
            let replaced = regex.replace_all(text, NoExpand(with));
            if replaced == *text {
                done.refused += 1;
                continue;
            }
            *text = replaced.into_owned();
            changed += 1;
        }
        if changed == 0 {
            continue;
        }
        match text_file::save(path, &loaded.lines, loaded.shape) {
            Ok(_) => {
                done.written += changed;
                done.files += 1;
            }
            Err(_) => done.refused += changed,
        }
    }
    done
}

/// `n lines`, singular where it should be.
fn lines_count(count: usize) -> String {
    match count {
        1 => "1 line".to_string(),
        n => format!("{n} lines"),
    }
}

/// `n files`, singular where it should be.
fn files_count(count: usize) -> String {
    match count {
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &std::path::Path, line: usize) -> SearchHit {
        SearchHit {
            line,
            path: path.to_path_buf(),
            text: String::new(),
        }
    }

    #[test]
    fn test_a_pattern_is_read_as_the_search_read_it() {
        let lower = pattern_regex("old_name").expect("reads");
        assert!(lower.is_match("Old_Name"), "no capital: any case");
        let capital = pattern_regex("Old").expect("reads");
        assert!(!capital.is_match("old"), "a capital: its own case");
        let wild = pattern_regex("a*c").expect("reads");
        assert!(wild.is_match("abbc"));
        assert!(pattern_regex("a.c").expect("reads").is_match("a.c"));
        assert!(
            !pattern_regex("a.c").expect("reads").is_match("abc"),
            "a dot is a dot"
        );
    }

    #[test]
    fn test_each_line_found_is_replaced_and_a_line_changed_since_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("winter-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let a = dir.join("a.rs");
        std::fs::write(&a, "let old = 1;\nkeep old here\nfn old() {}\n").expect("a");
        let b = dir.join("b.rs");
        std::fs::write(&b, "use $old;\n").expect("b");
        // Line 2 of a.rs was found, but has been changed since, as line 3's
        // edit without the word shows by holding none.
        std::fs::write(&a, "let old = 1;\nkeep old here\nfn renamed() {}\n").expect("edit");
        let hits = [hit(&a, 1), hit(&a, 3), hit(&b, 1), hit(&a, 1)];
        let done = replace_in_files(&hits, &pattern_regex("old").expect("reads"), "$new");
        assert_eq!(
            done,
            Replaced {
                files: 2,
                refused: 1,
                written: 2,
            }
        );
        assert_eq!(
            std::fs::read_to_string(&a).expect("a"),
            "let $new = 1;\nkeep old here\nfn renamed() {}\n",
            "only the lines found, the replacement taken as written"
        );
        assert_eq!(std::fs::read_to_string(&b).expect("b"), "use $$new;\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
