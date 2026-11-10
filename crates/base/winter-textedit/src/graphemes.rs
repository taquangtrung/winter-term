//! What a reader sees as one character, over text indexed by `char`.
//!
//! A buffer's columns count `char`s, but a reader sees grapheme clusters: an
//! accented letter written as a letter and a combining mark, a flag made of
//! two regional indicators, a family emoji joined from several. The cursor
//! steps over a cluster whole and a delete takes one whole, so neither leaves
//! half a character behind. And a cluster is not one cell wide on screen:
//! CJK and most emoji take two, combining marks none.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

// ========================================================================
// Clusters
// ========================================================================

/// The column, counted in `char`s, at which each grapheme cluster of `line`
/// starts, in order.
pub fn cluster_starts(line: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut at = 0;
    for cluster in line.graphemes(true) {
        starts.push(at);
        at += cluster.chars().count();
    }
    starts
}

/// The start of the cluster `col` falls inside, which is `col` itself when it
/// is already a cluster's start or at the end of the line.
pub fn snap_to_cluster(line: &str, col: usize) -> usize {
    if col >= line.chars().count() {
        return col;
    }
    cluster_starts(line)
        .into_iter()
        .take_while(|&start| start <= col)
        .last()
        .unwrap_or(col)
}

/// The column `count` clusters on from `col`, or the line's end if there are
/// not that many.
pub fn clusters_after(line: &str, col: usize, count: usize) -> usize {
    if count == 0 {
        return col;
    }
    let len = line.chars().count();
    cluster_starts(line)
        .into_iter()
        .chain(std::iter::once(len))
        .filter(|&start| start > col)
        .nth(count.saturating_sub(1))
        .unwrap_or(len)
}

/// The column `count` clusters back from `col`, or the line's start.
pub fn clusters_before(line: &str, col: usize, count: usize) -> usize {
    if count == 0 {
        return col;
    }
    let starts = cluster_starts(line);
    let before: Vec<usize> = starts.into_iter().filter(|&start| start < col).collect();
    before
        .len()
        .checked_sub(count)
        .map_or(0, |index| before[index])
}

/// The start of the last cluster of `line`, the last place Normal mode's
/// cursor may sit; zero for an empty line.
pub fn last_cluster(line: &str) -> usize {
    cluster_starts(line).last().copied().unwrap_or(0)
}

// ========================================================================
// Widths
// ========================================================================

/// How many cells `c` takes on screen: two for a wide character, none for a
/// combining mark or another zero-width one, one for the rest. A control
/// character, which the caller draws its own way, counts as one.
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(1)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// `e` and a combining acute accent: one cluster, two chars.
    const E_ACUTE: &str = "e\u{301}";

    #[test]
    fn test_a_combining_mark_is_one_cluster_with_its_letter() {
        let line = format!("a{E_ACUTE}b");
        assert_eq!(cluster_starts(&line), vec![0, 1, 3]);
        assert_eq!(snap_to_cluster(&line, 2), 1, "inside the cluster");
        assert_eq!(snap_to_cluster(&line, 3), 3);
        assert_eq!(snap_to_cluster(&line, 4), 4, "the line's end stays");
    }

    #[test]
    fn test_stepping_by_clusters_never_lands_inside_one() {
        let line = format!("a{E_ACUTE}b");
        assert_eq!(clusters_after(&line, 0, 1), 1);
        assert_eq!(clusters_after(&line, 1, 1), 3);
        assert_eq!(clusters_after(&line, 1, 5), 4);
        assert_eq!(clusters_after(&line, 1, 0), 1, "no steps, no move");
        assert_eq!(clusters_before(&line, 3, 1), 1);
        assert_eq!(clusters_before(&line, 3, 9), 0);
        assert_eq!(clusters_before(&line, 3, 0), 3);
        assert_eq!(last_cluster(&line), 3);
        assert_eq!(last_cluster(""), 0);
    }

    #[test]
    fn test_wide_and_zero_width_characters_take_their_cells() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('\u{4e2d}'), 2, "CJK is wide");
        assert_eq!(char_width('\u{301}'), 0, "a combining mark takes none");
    }
}
