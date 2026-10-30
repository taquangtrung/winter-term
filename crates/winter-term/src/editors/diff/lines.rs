//! Where texts differ: the line diff of two (Myers' algorithm, the one git
//! uses by default), and the regions three texts differ in, against a base
//! the other two are compared to, as diff3 finds them.

use std::ops::Range;

// ========================================================================
// Data Structures
// ========================================================================

/// How lines are compared: as they are, or with whitespace or case not
/// counting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Compare {
    pub(super) ignore_case: bool,
    pub(super) ignore_whitespace: bool,
}

/// A stretch two texts differ over: the lines of the first it covers, and
/// of the second. Either may be empty, for lines only one text has.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Hunk {
    pub(super) a: Range<usize>,
    pub(super) b: Range<usize>,
}

// ========================================================================
// Functions
// ========================================================================

/// The stretches `a` and `b` differ over, in order, lines compared the way
/// `compare` says.
pub(super) fn diff(a: &[String], b: &[String], compare: Compare) -> Vec<Hunk> {
    let a: Vec<String> = a.iter().map(|line| normalized(line, compare)).collect();
    let b: Vec<String> = b.iter().map(|line| normalized(line, compare)).collect();
    // What the two start and end with alike is no part of any hunk, and
    // leaving it out keeps the search to where they differ.
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let room = a.len().min(b.len()) - head;
    let tail = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
    let (a_mid, b_mid) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut hunks = Vec::new();
    for (a_range, b_range) in edit_runs(a_mid, b_mid) {
        hunks.push(Hunk {
            a: a_range.start + head..a_range.end + head,
            b: b_range.start + head..b_range.end + head,
        });
    }
    hunks
}

/// `line` as `compare` compares it.
fn normalized(line: &str, compare: Compare) -> String {
    let line = match compare.ignore_whitespace {
        true => line.split_whitespace().collect::<String>(),
        false => line.to_string(),
    };
    match compare.ignore_case {
        true => line.to_lowercase(),
        false => line,
    }
}

/// The runs of lines `a` and `b` do not share, from the shortest edit
/// script between them: each a stretch of `a` removed and of `b` put in
/// its place.
fn edit_runs<T: Eq>(a: &[T], b: &[T]) -> Vec<(Range<usize>, Range<usize>)> {
    let common = common_lines(a, b);
    let mut runs = Vec::new();
    let (mut x, mut y) = (0, 0);
    for (cx, cy) in common.into_iter().chain(std::iter::once((a.len(), b.len()))) {
        if cx > x || cy > y {
            runs.push((x..cx, y..cy));
        }
        x = cx + 1;
        y = cy + 1;
    }
    runs
}

/// The pairs of lines `a` and `b` keep in common along a shortest edit
/// script, in order, found by Myers' greedy search forward and its path
/// read back.
fn common_lines<T: Eq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let max = (n + m) as usize;
    let offset = max as isize;
    let mut v = vec![0isize; 2 * max + 2];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'search: for d in 0..=max as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let at = (k + offset) as usize;
            let mut x = match k == -d || (k != d && v[at - 1] < v[at + 1]) {
                true => v[at + 1],
                false => v[at - 1] + 1,
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at] = x;
            if x >= n && y >= m {
                break 'search;
            }
            k += 2;
        }
    }
    // Read the path back from the end, step by step.
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let at = (k + offset) as usize;
        let previous_k = match k == -d || (k != d && v[at - 1] < v[at + 1]) {
            true => k + 1,
            false => k - 1,
        };
        let previous_x = if d == 0 { 0 } else { v[(previous_k + offset) as usize] };
        let previous_y = previous_x - previous_k;
        while x > previous_x.max(0) && y > previous_y.max(0) && x > 0 && y > 0 {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        if d == 0 {
            break;
        }
        x = previous_x;
        y = previous_y;
    }
    pairs.reverse();
    pairs
}

/// The regions where `one` and `two` part from `base`, as diff3 finds them:
/// each a stretch of the base and the lines of each other text standing for
/// it, a region one text left as the base has it holding that text's lines
/// unchanged. Hunks of the two that touch or overlap on the base are one
/// region.
pub(super) fn three_way(base: &[String], one: &[String], two: &[String], compare: Compare) -> Vec<[Range<usize>; 3]> {
    let first = diff(base, one, compare);
    let second = diff(base, two, compare);
    let mut regions: Vec<[Range<usize>; 3]> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < first.len() || j < second.len() {
        // The hunk that starts first on the base opens the region, and every
        // hunk of either text that touches what it has covered joins it.
        let take_first = j >= second.len() || (i < first.len() && first[i].a.start <= second[j].a.start);
        let mut base_range = match take_first {
            true => first[i].a.clone(),
            false => second[j].a.clone(),
        };
        let (from_i, from_j) = (i, j);
        loop {
            let mut grew = false;
            while i < first.len() && touches(&first[i].a, &base_range) {
                base_range = union(&base_range, &first[i].a);
                i += 1;
                grew = true;
            }
            while j < second.len() && touches(&second[j].a, &base_range) {
                base_range = union(&base_range, &second[j].a);
                j += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        let one_range = mapped(&first[from_i..i], &base_range);
        let two_range = mapped(&second[from_j..j], &base_range);
        regions.push([base_range, one_range, two_range]);
    }
    regions
}

/// Whether `hunk` touches or overlaps `region` on the base: an insertion
/// just at its edge counts as touching.
fn touches(hunk: &Range<usize>, region: &Range<usize>) -> bool {
    hunk.start <= region.end && hunk.end >= region.start
}

/// The smallest range holding both.
fn union(a: &Range<usize>, b: &Range<usize>) -> Range<usize> {
    a.start.min(b.start)..a.end.max(b.end)
}

/// The lines of the other text standing for `region` of the base, given
/// the hunks of that text within it: the lines before and after the hunks
/// shift by what the hunks before them added or took away.
fn mapped(hunks: &[Hunk], region: &Range<usize>) -> Range<usize> {
    let (Some(first), Some(last)) = (hunks.first(), hunks.last()) else {
        // Untouched: the text has the base's own lines here, shifted by
        // nothing that falls inside the region.
        return region.clone();
    };
    let start = first.b.start - (first.a.start - region.start);
    let end = last.b.end + (region.end - last.a.end);
    start..end
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split(' ').filter(|line| !line.is_empty()).map(str::to_string).collect()
    }

    fn hunks(a: &str, b: &str) -> Vec<(Range<usize>, Range<usize>)> {
        diff(&lines(a), &lines(b), Compare::default()).into_iter().map(|hunk| (hunk.a, hunk.b)).collect()
    }

    #[test]
    fn test_a_diff_finds_changes_insertions_and_deletions() {
        assert_eq!(hunks("a b c d", "a x c d"), [(1..2, 1..2)]);
        assert_eq!(hunks("a b c", "a b x y c"), [(2..2, 2..4)]);
        assert_eq!(hunks("a b c d", "a d"), [(1..3, 1..1)]);
        assert_eq!(hunks("a b c", "a b c"), []);
        assert_eq!(hunks("", "a b"), [(0..0, 0..2)]);
        assert_eq!(hunks("a b c d e f", "b c x e f y"), [(0..1, 0..0), (3..4, 2..3), (6..6, 5..6)]);
    }

    #[test]
    fn test_whitespace_and_case_need_not_count() {
        let (a, b) = (vec!["let  x = 1;".to_string()], vec!["LET x=1;".to_string()]);
        assert_eq!(diff(&a, &b, Compare::default()).len(), 1);
        let loose = Compare {
            ignore_case: true,
            ignore_whitespace: true,
        };
        assert!(diff(&a, &b, loose).is_empty());
    }

    #[test]
    fn test_three_way_regions_join_overlapping_hunks_and_map_untouched_sides() {
        let base = lines("a b c d e");
        let one = lines("a B c d e");
        let two = lines("a b c D E x");
        let regions = three_way(&base, &one, &two, Compare::default());
        assert_eq!(regions, [[1..2, 1..2, 1..2], [3..5, 3..5, 3..6]]);
        let both = three_way(&lines("a b c"), &lines("a X c"), &lines("a Y Z c"), Compare::default());
        assert_eq!(both, [[1..2, 1..2, 1..3]], "both changed the same lines: one region");
    }
}
