//! Walking a directory tree on a worker thread: the size of everything under
//! it, and the lines matching a search, through ripgrep when it is installed
//! and a plain walk when it is not.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use regex::{Regex, RegexBuilder};

use crate::model::editor::{SearchHit, SearchOptions, SearchPattern, SearchRequest, SearchResult};

// ========================================================================
// Constants
// ========================================================================

/// Matches a search reports before it stops. An editor shows a screenful at a
/// time, and a query loose enough to pass this is one to narrow, not to hold
/// every answer to.
pub(super) const SEARCH_MAX_HITS: usize = 500;

/// Longest a hit's text is kept. One minified line can run to megabytes, which
/// no pane can show and nothing should hold five hundred of.
const SEARCH_MAX_LINE: usize = 300;

/// Largest file a search reads. Anything past this is generated, packed, or
/// both, and reading it costs more than the match is worth.
const SEARCH_MAX_FILE_BYTES: u64 = 1 << 20;

/// How deep a search walks, matching the size walk's own ceiling.
const SEARCH_MAX_DEPTH: usize = 32;

/// Directories a search never enters: version-control internals and build
/// output, which are derived rather than written and would swamp every result.
const SEARCH_SKIP_DIRS: [&str; 5] = [".git", ".hg", ".svn", "node_modules", "target"];

/// The program a search hands its walk to where it is installed: it reads the
/// tree's `.gitignore` files, and it is faster than any walk written here.
pub(super) const RIPGREP: &str = "rg";

/// The byte ripgrep is asked to end each path with, since a path may hold the
/// `:` that would otherwise separate it from the line number.
const PATH_END: u8 = 0;

// ========================================================================
// Data Structures
// ========================================================================

/// What each line of a search is tested against, prepared once for the walk,
/// with the options it was asked with, which ripgrep is handed as flags.
#[derive(Debug)]
struct LineMatcher {
    /// How the line is tested.
    test: LineTest,
    /// How a text query is read.
    options: SearchOptions,
    /// The query as it was asked, for ripgrep.
    query: String,
}

/// How a line is tested.
#[derive(Debug)]
enum LineTest {
    /// The query as a regular expression, or as text made into one, where the
    /// options ask for more than finding it as it is.
    Regex(Regex),
    /// The query, lowered, found anywhere in a lowered line.
    Text(String),
    /// Words, any one of which a line holds whole, in their own case.
    Words(Vec<String>),
}

// ========================================================================
// LineMatcher
// ========================================================================

impl LineMatcher {
    /// The matcher `request` asks for, `Ok(None)` when it asks for nothing
    /// (an empty query matches nothing rather than everything), or why its
    /// regular expression cannot be read.
    fn new(request: &SearchRequest) -> Result<Option<Self>, String> {
        let options = match request.pattern {
            SearchPattern::Text => request.options.clone(),
            // A keyword list is its own reading: whole words, in their case.
            SearchPattern::Words => SearchOptions::default(),
        };
        let test = match request.pattern {
            SearchPattern::Words => LineTest::Words(
                request
                    .split_words()
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            ),
            SearchPattern::Text if request.query.is_empty() => return Ok(None),
            SearchPattern::Text
                if options.regex || options.whole_word || options.case_sensitive =>
            {
                let pattern = match options.regex {
                    true => request.query.clone(),
                    false => regex::escape(&request.query),
                };
                let pattern = match options.whole_word {
                    true => format!(r"\b(?:{pattern})\b"),
                    false => pattern,
                };
                let built = RegexBuilder::new(&pattern)
                    .case_insensitive(!options.case_sensitive)
                    .build()
                    .map_err(|e| format!("not a regular expression: {e}"))?;
                LineTest::Regex(built)
            }
            SearchPattern::Text => LineTest::Text(request.query.to_lowercase()),
        };
        if matches!(&test, LineTest::Words(words) if words.is_empty()) {
            return Ok(None);
        }
        Ok(Some(Self {
            test,
            options,
            query: request.query.clone(),
        }))
    }

    /// Whether `line` is one the search reports.
    fn is_match(&self, line: &str) -> bool {
        match &self.test {
            LineTest::Regex(regex) => regex.is_match(line),
            LineTest::Text(needle) => line.to_lowercase().contains(needle.as_str()),
            LineTest::Words(words) => words.iter().any(|word| holds_word(line, word)),
        }
    }

    /// Whether the file at `path` is one the search reads, by its name.
    fn reads_file(&self, path: &Path) -> bool {
        let Some(glob) = &self.options.glob else {
            return true;
        };
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| glob_matches(glob, name))
    }

    /// The ripgrep arguments that match the same lines as [`Self::is_match`].
    fn ripgrep_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let words: Vec<String> = match &self.test {
            LineTest::Words(words) => {
                args.extend(
                    ["--fixed-strings", "--case-sensitive", "--word-regexp"].map(str::to_string),
                );
                words.clone()
            }
            LineTest::Regex(_) | LineTest::Text(_) => {
                if !self.options.regex {
                    args.push("--fixed-strings".to_string());
                }
                args.push(match self.options.case_sensitive {
                    true => "--case-sensitive".to_string(),
                    false => "--ignore-case".to_string(),
                });
                if self.options.whole_word {
                    args.push("--word-regexp".to_string());
                }
                vec![self.query.clone()]
            }
        };
        if let Some(glob) = &self.options.glob {
            args.push(format!("--glob={glob}"));
        }
        for pattern in words {
            args.push("--regexp".to_string());
            args.push(pattern);
        }
        args
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Total bytes under `dir`, skipping what cannot be read and links rather than
/// following them, the way `du` does. Returns what it summed so far when
/// cancelled, which the caller discards.
pub(super) fn walk_size(dir: &Path, cancel: &AtomicBool, depth: usize) -> u64 {
    const MAX_DEPTH: usize = 64;
    if depth >= MAX_DEPTH || cancel.load(Ordering::Relaxed) {
        return 0;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in reader.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return total;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.is_dir() && !meta.file_type().is_symlink() {
            total += walk_size(&entry.path(), cancel, depth + 1);
        } else {
            total += meta.len();
        }
    }
    total
}

/// Every line under the requested root its query matches, read the way its
/// [`SearchPattern`] says. An empty query matches nothing rather than
/// everything.
pub(super) fn search_tree(request: &SearchRequest, cancel: &AtomicBool) -> SearchResult {
    let mut found = SearchResult {
        query: request.query.clone(),
        ..SearchResult::default()
    };
    let matcher = match LineMatcher::new(request) {
        Ok(Some(matcher)) => matcher,
        Ok(None) => return found,
        Err(error) => {
            found.error = Some(error);
            return found;
        }
    };
    if let Some(ripgrep) = search_with_ripgrep(request, cancel) {
        return ripgrep;
    }
    search_dir(&request.root, &matcher, cancel, 0, &mut found);
    found
}

/// The same search run by ripgrep, under the same rules as the walk below:
/// the query matched as [`LineMatcher`] matches it, hidden files read, the
/// same directories skipped, the same caps, files in sorted order. `None`
/// where ripgrep is not installed or there is nothing to look for, so the
/// walk runs instead.
pub(super) fn search_with_ripgrep(
    request: &SearchRequest,
    cancel: &AtomicBool,
) -> Option<SearchResult> {
    let matcher = LineMatcher::new(request).ok().flatten()?;
    let mut command = Command::new(RIPGREP);
    command
        .args(["--line-number", "--with-filename", "--no-heading", "--null"])
        .args(["--color", "never", "--hidden"])
        .args(["--sort", "path", "--no-require-git"])
        .arg(format!("--max-filesize={SEARCH_MAX_FILE_BYTES}"))
        .arg(format!("--max-depth={SEARCH_MAX_DEPTH}"));
    for skipped in SEARCH_SKIP_DIRS {
        command.arg(format!("--glob=!{skipped}"));
    }
    let mut child = command
        .args(matcher.ripgrep_args())
        .arg(&request.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut found = SearchResult {
        query: request.query.clone(),
        ..SearchResult::default()
    };
    let stdout = child.stdout.take()?;
    for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if found.hits.len() >= SEARCH_MAX_HITS {
            found.truncated = true;
            break;
        }
        if let Some(hit) = ripgrep_hit(&line) {
            found.hits.push(hit);
        }
    }
    // Whatever is left unread is not wanted: a capped or cancelled search
    // stops ripgrep rather than letting it walk the rest of the tree.
    let _ = child.kill();
    let _ = child.wait();
    Some(found)
}

/// One line of ripgrep's output, `path NUL line:text`, as a hit.
fn ripgrep_hit(line: &[u8]) -> Option<SearchHit> {
    let split = line.iter().position(|byte| *byte == PATH_END)?;
    let path = PathBuf::from(String::from_utf8_lossy(&line[..split]).to_string());
    let rest = String::from_utf8_lossy(&line[split + 1..]);
    let (number, text) = rest.split_once(':')?;
    Some(SearchHit {
        line: number.parse().ok()?,
        path,
        text: text.trim_end().chars().take(SEARCH_MAX_LINE).collect(),
    })
}

/// Walk `dir`, adding what its files hold. Entries are taken in sorted order,
/// so running the same search twice reads the same way.
fn search_dir(
    dir: &Path,
    matcher: &LineMatcher,
    cancel: &AtomicBool,
    depth: usize,
    found: &mut SearchResult,
) {
    if depth >= SEARCH_MAX_DEPTH || found.truncated || cancel.load(Ordering::Relaxed) {
        return;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = reader.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if found.truncated || cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // A link is never followed, so a cycle cannot make the walk endless.
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if !is_skipped_dir(&path) {
                search_dir(&path, matcher, cancel, depth + 1, found);
            }
        } else if meta.len() <= SEARCH_MAX_FILE_BYTES && matcher.reads_file(&path) {
            search_file(&path, matcher, found);
        }
    }
}

/// Whether a directory is one searches leave alone.
fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SEARCH_SKIP_DIRS.contains(&name))
}

/// Add each line of `path` that `matcher` matches. A file that will not read
/// as text is skipped, since searching one prints a screen of control bytes.
fn search_file(path: &Path, matcher: &LineMatcher, found: &mut SearchResult) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (index, line) in text.lines().enumerate() {
        if !matcher.is_match(line) {
            continue;
        }
        if found.hits.len() >= SEARCH_MAX_HITS {
            found.truncated = true;
            return;
        }
        found.hits.push(SearchHit {
            line: index + 1,
            path: path.to_path_buf(),
            text: line.trim_end().chars().take(SEARCH_MAX_LINE).collect(),
        });
    }
}

/// Whether `name` matches `glob`, where `*` stands for any run of characters
/// and `?` for any one, as a shell matches a file name.
fn glob_matches(glob: &str, name: &str) -> bool {
    let (glob, name): (Vec<char>, Vec<char>) = (glob.chars().collect(), name.chars().collect());
    let (mut g, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match glob.get(g) {
            Some('*') => {
                star = Some((g, n));
                g += 1;
            }
            Some(c) if *c == '?' || *c == name[n] => {
                g += 1;
                n += 1;
            }
            _ => match star {
                Some((at, from)) => {
                    g = at + 1;
                    n = from + 1;
                    star = Some((at, from + 1));
                }
                None => return false,
            },
        }
    }
    glob[g..].iter().all(|c| *c == '*')
}

/// Whether `line` holds `word` with no word character on either side of it,
/// the way ripgrep's `--word-regexp` reads a word.
fn holds_word(line: &str, word: &str) -> bool {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(word).any(|(start, _)| {
        let before = line[..start].chars().next_back();
        let after = line[start + word.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_word_is_found_only_with_no_word_character_beside_it() {
        assert!(holds_word("// TODO: x", "TODO"));
        assert!(holds_word("TODO", "TODO"));
        assert!(holds_word("fn x() {} // TODO(me)", "TODO"));
        assert!(!holds_word("TODOS", "TODO"));
        assert!(!holds_word("MY_TODO", "TODO"));
        assert!(
            !holds_word("éTODO", "TODO"),
            "a letter outside ASCII is a word character"
        );
        // The first place it occurs is inside a word; the second is whole.
        assert!(holds_word("XTODO TODO", "TODO"));
    }

    #[test]
    fn test_the_walk_reads_a_words_query_as_ripgrep_does() {
        // The walk runs where ripgrep is not installed, and has to answer the
        // same question: whole words, in their own case.
        let request = SearchRequest::new(SearchPattern::Words, " TODO  FIXME ", PathBuf::new());
        let matcher = LineMatcher::new(&request).ok().flatten().expect("two words");
        assert!(matcher.is_match("// FIXME later"));
        assert!(!matcher.is_match("// fixme later"));
        assert!(!matcher.is_match("// TODOS"));
        let blank = SearchRequest::new(SearchPattern::Words, "   ", PathBuf::new());
        assert!(
            LineMatcher::new(&blank).ok().flatten().is_none(),
            "no words matches nothing"
        );
    }

    #[test]
    fn test_the_options_read_a_text_query_as_ripgrep_is_told_to() {
        let ask = |query: &str, options: SearchOptions| {
            let request = SearchRequest::new(SearchPattern::Text, query, PathBuf::new())
                .with_options(options);
            LineMatcher::new(&request)
        };
        let plain = ask("Foo", SearchOptions::default()).ok().flatten().expect("text");
        assert!(plain.is_match("a foo b"), "any case by default");
        assert!(plain.ripgrep_args().contains(&"--ignore-case".to_string()));
        let cased = SearchOptions {
            case_sensitive: true,
            whole_word: true,
            ..SearchOptions::default()
        };
        let matcher = ask("Foo", cased).ok().flatten().expect("text");
        assert!(matcher.is_match("a Foo b"));
        assert!(!matcher.is_match("a foo b"), "case counts");
        assert!(!matcher.is_match("a Foobar b"), "a whole word");
        let regex = SearchOptions {
            regex: true,
            glob: Some("*.rs".to_string()),
            ..SearchOptions::default()
        };
        let matcher = ask("fo+ba[rz]", regex.clone()).ok().flatten().expect("a regex");
        assert!(matcher.is_match("FOOOBAZ"));
        assert!(!matcher.ripgrep_args().contains(&"--fixed-strings".to_string()));
        assert!(matcher.ripgrep_args().contains(&"--glob=*.rs".to_string()));
        assert!(matcher.reads_file(Path::new("src/main.rs")));
        assert!(!matcher.reads_file(Path::new("README.md")));
        assert!(ask("(unclosed", regex).is_err(), "a regex that does not parse");
        assert!(ask("1+1", SearchOptions::default()).ok().flatten().expect("text").is_match("1+1"));
    }

    #[test]
    fn test_a_glob_matches_names_as_a_shell_does() {
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(glob_matches("test_?.py", "test_a.py"));
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("a*b*c", "aXXbYYc"));
        assert!(!glob_matches("*.rs", "main.rsx"));
        assert!(!glob_matches("test_?.py", "test_ab.py"));
    }
}
