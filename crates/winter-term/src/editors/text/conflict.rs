//! The conflicts a merge, a rebase, or a cherry-pick leaves in a file: the
//! `<<<<<<<` block git writes where two sides changed the same lines, each side
//! under its marker, and the common ancestor's version too where the file was
//! merged with `merge.conflictStyle diff3`.
//!
//! Resolving one is choosing what of it to keep, the way magic-vscode and
//! Emacs' smerge offer it: our side, their side, both in either order, the
//! ancestor's, or none of it. What is kept replaces the whole block, markers
//! and all.

// ========================================================================
// Constants
// ========================================================================

/// The markers git opens, splits, and closes a conflict with, each seven
/// characters long and followed by the side's name, if any.
const OURS_MARK: &str = "<<<<<<<";
const BASE_MARK: &str = "|||||||";
const SPLIT_MARK: &str = "=======";
const THEIRS_MARK: &str = ">>>>>>>";

/// Every way of resolving a conflict, as the menu offers them.
pub(super) const RESOLUTIONS: [Resolution; 6] = [
    Resolution::Ours,
    Resolution::Theirs,
    Resolution::Both,
    Resolution::BothReversed,
    Resolution::Base,
    Resolution::Neither,
];

// ========================================================================
// Data Structures
// ========================================================================

/// One conflict, as the rows of its markers: where it opens, where the
/// ancestor's version starts if it has one, where the two sides split, and
/// where it closes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Conflict {
    pub(super) base: Option<usize>,
    pub(super) end: usize,
    pub(super) split: usize,
    pub(super) start: usize,
}

/// What of a conflict to keep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Resolution {
    /// The ancestor's version, which only a diff3 conflict carries.
    Base,
    /// Ours, then theirs.
    Both,
    /// Theirs, then ours.
    BothReversed,
    /// None of it: the block goes.
    Neither,
    /// Our side, the one checked out (`HEAD`).
    Ours,
    /// Their side, the one coming in.
    Theirs,
}

// ========================================================================
// Conflict
// ========================================================================

impl Conflict {
    /// Whether `row` is one of the conflict's own rows, markers included.
    pub(super) fn holds(&self, row: usize) -> bool {
        (self.start..=self.end).contains(&row)
    }

    /// Whether `row` is one of the conflict's markers.
    pub(super) fn is_marker(&self, row: usize) -> bool {
        row == self.start || row == self.split || row == self.end || Some(row) == self.base
    }

    /// The lines `resolution` keeps of `lines`, the file the conflict is in,
    /// or `None` for the ancestor's version of a conflict that has none.
    pub(super) fn kept(&self, lines: &[String], resolution: Resolution) -> Option<Vec<String>> {
        let ours = &lines[self.start + 1..self.base.unwrap_or(self.split)];
        let theirs = &lines[self.split + 1..self.end];
        let kept = match resolution {
            Resolution::Base => {
                let base = self.base?;
                lines[base + 1..self.split].to_vec()
            }
            Resolution::Both => [ours, theirs].concat(),
            Resolution::BothReversed => [theirs, ours].concat(),
            Resolution::Neither => Vec::new(),
            Resolution::Ours => ours.to_vec(),
            Resolution::Theirs => theirs.to_vec(),
        };
        Some(kept)
    }

    /// How many rows the block covers, markers included.
    pub(super) fn len(&self) -> usize {
        self.end - self.start + 1
    }
}

// ========================================================================
// Resolution
// ========================================================================

impl Resolution {
    /// What the menu and the key help call it, naming the sides as `ours`
    /// and `theirs` say them in the file.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Base => "Keep the ancestor's version",
            Self::Both => "Keep both, ours first",
            Self::BothReversed => "Keep both, theirs first",
            Self::Neither => "Keep neither: delete the conflict",
            Self::Ours => "Keep ours (the checked-out side)",
            Self::Theirs => "Keep theirs (the incoming side)",
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

impl super::Document {
    /// Every whole conflict in the text, in order (see [`find`]), found
    /// again only once the text has changed.
    pub(super) fn conflicts(&self) -> Vec<Conflict> {
        let revision = self.buffer.revision();
        let mut cached = self.conflicts.borrow_mut();
        match &*cached {
            Some((at, found)) if *at == revision => found.clone(),
            _ => {
                let found = find(self.buffer.lines());
                *cached = Some((revision, found.clone()));
                found
            }
        }
    }
}

/// Every whole conflict in `lines`, in order. A block git did not finish
/// writing (no split, or no close) is not a conflict, so its markers are
/// left as text.
pub(super) fn find(lines: &[String]) -> Vec<Conflict> {
    let mut found = Vec::new();
    // Nearly every file has none, which one look for the opening marker settles.
    if !lines.iter().any(|line| line.starts_with(OURS_MARK)) {
        return found;
    }
    let mut open: Option<(usize, Option<usize>, Option<usize>)> = None;
    for (row, line) in lines.iter().enumerate() {
        match open {
            None if is_mark(line, OURS_MARK) => open = Some((row, None, None)),
            None => {}
            Some((start, base, None)) if is_mark(line, BASE_MARK) && base.is_none() => {
                open = Some((start, Some(row), None));
            }
            Some((start, base, None)) if line == SPLIT_MARK => {
                open = Some((start, base, Some(row)));
            }
            Some((start, base, Some(split))) if is_mark(line, THEIRS_MARK) => {
                found.push(Conflict {
                    base,
                    end: row,
                    split,
                    start,
                });
                open = None;
            }
            // A second opening marker before the first closed starts over
            // from it, as git never nests them.
            Some(_) if is_mark(line, OURS_MARK) => open = Some((row, None, None)),
            Some(_) => {}
        }
    }
    found
}

/// The conflict `row` sits in, if any.
pub(super) fn at(conflicts: &[Conflict], row: usize) -> Option<Conflict> {
    conflicts.iter().copied().find(|conflict| conflict.holds(row))
}

/// The conflict after `row`, or before it, wrapping round the file as Vim's
/// searches do; `None` where there are none.
pub(super) fn next(conflicts: &[Conflict], row: usize, forward: bool) -> Option<Conflict> {
    let found = match forward {
        true => conflicts.iter().find(|conflict| conflict.start > row),
        false => conflicts.iter().rev().find(|conflict| conflict.end < row),
    };
    let wrapped = match forward {
        true => conflicts.first(),
        false => conflicts.last(),
    };
    found.or(wrapped).copied()
}

/// Whether `line` is the marker `mark`, alone or followed by a name.
fn is_mark(line: &str, mark: &str) -> bool {
    line.strip_prefix(mark)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    const TWO_WAY: &str = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nz";
    const DIFF3: &str = "<<<<<<< HEAD\nours\n||||||| base\nold\n=======\ntheirs\n>>>>>>> x";

    #[test]
    fn test_each_way_of_resolving_keeps_its_lines() {
        let file = lines(TWO_WAY);
        let conflict = find(&file)[0];
        let keep = |resolution| conflict.kept(&file, resolution);
        assert_eq!(keep(Resolution::Ours), Some(lines("ours")));
        assert_eq!(keep(Resolution::Theirs), Some(lines("theirs")));
        assert_eq!(keep(Resolution::Both), Some(lines("ours\ntheirs")));
        assert_eq!(keep(Resolution::BothReversed), Some(lines("theirs\nours")));
        assert_eq!(keep(Resolution::Neither), Some(Vec::new()));
        assert_eq!(keep(Resolution::Base), None, "no ancestor to keep");
        assert_eq!(conflict.len(), 5);
    }

    #[test]
    fn test_a_diff3_conflict_keeps_its_ancestor_apart_from_ours() {
        let file = lines(DIFF3);
        let conflict = find(&file)[0];
        assert_eq!(conflict.base, Some(2));
        assert_eq!(conflict.kept(&file, Resolution::Ours), Some(lines("ours")));
        assert_eq!(conflict.kept(&file, Resolution::Base), Some(lines("old")));
        assert!(conflict.is_marker(2) && !conflict.is_marker(3));
    }

    #[test]
    fn test_only_whole_blocks_are_conflicts() {
        assert!(find(&lines("<<<<<<< HEAD\nours\n=======\ntheirs")).is_empty());
        assert!(find(&lines("Title\n=======\ntext")).is_empty(), "a setext heading");
        assert!(find(&lines("<<<<<<<< eight\n=======\n>>>>>>> x")).is_empty());
        let restarted = find(&lines(&format!("<<<<<<< stray\n{TWO_WAY}")));
        assert_eq!(restarted.len(), 1);
        assert_eq!(restarted[0].start, 2, "the stray opening is left as text");
    }

    #[test]
    fn test_stepping_between_conflicts_wraps_round_the_file() {
        let file = lines(&format!("{TWO_WAY}\n{TWO_WAY}"));
        let conflicts = find(&file);
        let starts = |found: Option<Conflict>| found.map(|conflict| conflict.start);
        assert_eq!(starts(next(&conflicts, 0, true)), Some(1));
        assert_eq!(starts(next(&conflicts, 1, true)), Some(8));
        assert_eq!(starts(next(&conflicts, 8, true)), Some(1), "wrapped");
        assert_eq!(starts(next(&conflicts, 8, false)), Some(1));
        assert_eq!(starts(next(&conflicts, 0, false)), Some(8), "wrapped");
        assert_eq!(starts(at(&conflicts, 3)), Some(1));
        assert_eq!(at(&conflicts, 0), None);
        assert_eq!(next(&[], 0, true), None);
    }
}
