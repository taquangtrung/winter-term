//! A diff session's texts and the regions they differ in: two texts
//! compared, three compared against the middle one, or two merged into a
//! third, with or without the ancestor they share. What a region holds in
//! each text, the rows that set the texts out side by side with filler
//! where one has fewer lines, copying a region from one text to another,
//! and choosing what a merge keeps.

use std::ops::Range;
use std::path::PathBuf;

use crate::editors::text_file::FileShape;
use crate::model::textedit::buffer::TextBuffer;

use super::lines::{self, Compare};

// ========================================================================
// Constants
// ========================================================================

/// The markers a merge region holding both sides is written between, as git
/// writes a conflict.
pub(super) const OURS_MARK: &str = "<<<<<<< variant A";
pub(super) const SPLIT_MARK: &str = "=======";
pub(super) const THEIRS_MARK: &str = ">>>>>>> variant B";

/// Which text is which: the first, the second, the third (a third file
/// compared, or the merge), and the ancestor a merge's texts share.
pub(super) const A: usize = 0;
pub(super) const B: usize = 1;
pub(super) const C: usize = 2;
pub(super) const ANCESTOR: usize = 3;

// ========================================================================
// Data Structures
// ========================================================================

/// What the session does with its texts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    /// Two texts, A and B.
    Compare,
    /// Three texts, A and C each compared against B.
    Compare3,
    /// A and B merged into C, against their ancestor where there is one.
    Merge,
}

/// One text of the session: what it reads, what it is called, and where it
/// is written back to, for one that can be.
#[derive(Debug)]
pub(super) struct Side {
    pub(super) buffer: TextBuffer,
    pub(super) label: String,
    pub(super) path: Option<PathBuf>,
    pub(super) shape: FileShape,
}

/// What a merge region holds now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Variant {
    A,
    Ancestor,
    B,
    /// Both, between conflict markers, A's first.
    Combined,
    /// Typed over in the merge itself.
    Edited,
}

/// One region the texts differ in: the lines it covers in each text (an
/// empty range where a text has none), and for a merge, what the merge
/// holds and whether A and B both changed it from the ancestor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Region {
    pub(super) conflict: bool,
    /// What the region held in each text before anything was copied into
    /// it, for putting back.
    pub(super) original: [Vec<String>; 3],
    pub(super) lines: [Range<usize>; 4],
    pub(super) variant: Option<Variant>,
}

/// One row of the texts set side by side: the line of each text shown on
/// it, `None` for filler, and the region it is in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Aligned {
    pub(super) lines: [Option<usize>; 4],
    pub(super) region: Option<usize>,
}

/// The texts and their regions.
#[derive(Debug)]
pub(super) struct Session {
    pub(super) ancestor: Option<Side>,
    pub(super) compare: Compare,
    pub(super) kind: Kind,
    pub(super) regions: Vec<Region>,
    pub(super) sides: Vec<Side>,
}

// ========================================================================
// Session
// ========================================================================

impl Session {
    /// A session of `kind` over `sides` (two, or three for a three-way
    /// comparison), merged into a new third text for a merge, against
    /// `ancestor` where one is given.
    pub(super) fn new(kind: Kind, mut sides: Vec<Side>, ancestor: Option<Side>, output: Option<PathBuf>) -> Self {
        if kind == Kind::Merge {
            sides.truncate(2);
            sides.push(Side {
                buffer: TextBuffer::new(vec![String::new()]),
                label: output.as_ref().map_or_else(|| "merge".to_string(), |path| path.display().to_string()),
                shape: sides[A].shape,
                path: output,
            });
        }
        let mut session = Self {
            ancestor,
            compare: Compare::default(),
            kind,
            regions: Vec::new(),
            sides,
        };
        session.recompute();
        if kind == Kind::Merge {
            session.build_merge();
        }
        session
    }

    /// The lines of text `side`.
    pub(super) fn lines(&self, side: usize) -> &[String] {
        match side {
            ANCESTOR => self.ancestor.as_ref().map_or(&[][..], |side| side.buffer.lines()),
            side => self.sides.get(side).map_or(&[][..], |side| side.buffer.lines()),
        }
    }

    /// The texts shown, in order: A and B, the third, and the ancestor
    /// where `with_ancestor` asks for it and there is one.
    pub(super) fn shown(&self, with_ancestor: bool) -> Vec<usize> {
        let mut shown: Vec<usize> = (0..self.sides.len()).collect();
        if with_ancestor && self.ancestor.is_some() {
            shown.push(ANCESTOR);
        }
        shown
    }

    /// Work the regions out again from the texts as they now read. A
    /// merge's regions are kept: its texts are not edited, and the merge's
    /// own lines are followed as it is (see [`Self::merge_edited`]).
    pub(super) fn recompute(&mut self) {
        let compare = self.compare;
        let regions: Vec<[Range<usize>; 4]> = match self.kind {
            Kind::Compare => lines::diff(self.lines(A), self.lines(B), compare)
                .into_iter()
                .map(|hunk| [hunk.a, hunk.b, 0..0, 0..0])
                .collect(),
            Kind::Compare3 => lines::three_way(self.lines(B), self.lines(A), self.lines(C), compare)
                .into_iter()
                .map(|[base, one, two]| [one, base, two, 0..0])
                .collect(),
            Kind::Merge if !self.regions.is_empty() => return,
            Kind::Merge => match &self.ancestor {
                Some(ancestor) => lines::three_way(ancestor.buffer.lines(), self.lines(A), self.lines(B), compare)
                    .into_iter()
                    .map(|[base, one, two]| [one, two, 0..0, base])
                    .collect(),
                None => lines::diff(self.lines(A), self.lines(B), compare)
                    .into_iter()
                    .map(|hunk| [hunk.a, hunk.b, 0..0, 0..0])
                    .collect(),
            },
        };
        self.regions = regions
            .into_iter()
            .map(|lines| {
                let original = [A, B, C].map(|side| self.slice(side, &lines[side]));
                Region {
                    conflict: false,
                    original,
                    lines,
                    variant: None,
                }
            })
            .collect();
    }

    /// The lines `range` covers in text `side`.
    fn slice(&self, side: usize, range: &Range<usize>) -> Vec<String> {
        let lines = self.lines(side);
        lines[range.start.min(lines.len())..range.end.min(lines.len())].to_vec()
    }

    /// Write the merge out from A, B, and the ancestor: what they share as
    /// it is, a region only one side changed as that side has it, one both
    /// changed alike as either, and one they changed apart (or every
    /// region, with no ancestor to tell) as both between conflict markers.
    fn build_merge(&mut self) {
        let mut merged = Vec::new();
        let mut a_at = 0;
        let has_ancestor = self.ancestor.is_some();
        for index in 0..self.regions.len() {
            let region = self.regions[index].clone();
            merged.extend_from_slice(&self.lines(A)[a_at..region.lines[A].start]);
            a_at = region.lines[A].end;
            let a = self.slice(A, &region.lines[A]);
            let b = self.slice(B, &region.lines[B]);
            let base = self.slice(ANCESTOR, &region.lines[ANCESTOR]);
            let (variant, conflict) = match (has_ancestor, a == base, b == base) {
                (true, true, _) => (Variant::B, false),
                (true, false, true) => (Variant::A, false),
                (true, false, false) if a == b => (Variant::A, false),
                (true, false, false) => (Variant::Combined, true),
                (false, _, _) => (Variant::Combined, true),
            };
            let text = self.variant_text(index, variant);
            let start = merged.len();
            merged.extend(text.iter().cloned());
            let region = &mut self.regions[index];
            region.lines[C] = start..merged.len();
            region.variant = Some(variant);
            region.conflict = conflict;
            region.original[C] = text;
        }
        merged.extend_from_slice(&self.lines(A)[a_at.min(self.lines(A).len())..]);
        if merged.is_empty() {
            merged.push(String::new());
        }
        self.sides[C].buffer = TextBuffer::new(merged);
    }

    /// The lines merge region `index` holds when it keeps `variant`.
    pub(super) fn variant_text(&self, index: usize, variant: Variant) -> Vec<String> {
        let region = &self.regions[index];
        match variant {
            Variant::A => self.slice(A, &region.lines[A]),
            Variant::B => self.slice(B, &region.lines[B]),
            Variant::Ancestor => self.slice(ANCESTOR, &region.lines[ANCESTOR]),
            Variant::Combined => {
                let mut text = vec![OURS_MARK.to_string()];
                text.extend(self.slice(A, &region.lines[A]));
                text.push(SPLIT_MARK.to_string());
                text.extend(self.slice(B, &region.lines[B]));
                text.push(THEIRS_MARK.to_string());
                text
            }
            Variant::Edited => self.slice(C, &region.lines[C]),
        }
    }

    /// Put `text` in place of what region `index` holds in text `side`, and
    /// move every later region of that text by what it added or took away.
    fn replace_region(&mut self, index: usize, side: usize, text: Vec<String>) {
        let range = self.regions[index].lines[side].clone();
        let delta = text.len() as isize - range.len() as isize;
        let buffer = &mut self.sides[side].buffer;
        let (row, col) = (buffer.row(), buffer.col());
        // An empty buffer is one empty line, which a region taking its
        // first lines replaces rather than pushes down.
        let empty = buffer.lines().len() == 1 && buffer.lines()[0].is_empty() && range.is_empty();
        let count = if empty { 1 } else { range.len() };
        if text.is_empty() && buffer.lines().len() == range.len() {
            buffer.replace_rows(range.start, count, vec![String::new()]);
        } else {
            buffer.replace_rows(range.start, count, text);
        }
        buffer.move_to(row.min(buffer.lines().len() - 1), col);
        let region = &mut self.regions[index];
        region.lines[side] = range.start..(range.end as isize + delta) as usize;
        for later in self.regions.iter_mut().skip(index + 1) {
            let lines = &mut later.lines[side];
            *lines = (lines.start as isize + delta) as usize..(lines.end as isize + delta) as usize;
        }
    }

    /// Copy region `index` of text `from` over the same region of `to`.
    pub(super) fn copy_region(&mut self, index: usize, from: usize, to: usize) {
        let text = self.slice(from, &self.regions[index].lines[from]);
        self.replace_region(index, to, text);
        if self.kind == Kind::Merge && to == C {
            self.regions[index].variant = Some(match from {
                A => Variant::A,
                B => Variant::B,
                _ => Variant::Edited,
            });
        }
    }

    /// Have merge region `index` keep `variant`.
    pub(super) fn choose(&mut self, index: usize, variant: Variant) {
        let text = self.variant_text(index, variant);
        self.replace_region(index, C, text);
        self.regions[index].variant = Some(variant);
    }

    /// Put region `index` of text `side` back as it was before anything was
    /// copied into it.
    pub(super) fn restore(&mut self, index: usize, side: usize) {
        let text = self.regions[index].original[side].clone();
        self.replace_region(index, side, text);
        if self.kind == Kind::Merge && side == C {
            let first = self.regions[index].variant;
            self.regions[index].variant = first.map(|_| self.initial_variant(index));
        }
    }

    /// What merge region `index` held when the merge was first written.
    fn initial_variant(&self, index: usize) -> Variant {
        let region = &self.regions[index];
        [Variant::A, Variant::B, Variant::Combined]
            .into_iter()
            .find(|variant| self.variant_text(index, *variant) == region.original[C])
            .unwrap_or(Variant::Edited)
    }

    /// Follow an edit typed into the merge: its first changed line was `row`
    /// and it added `delta` lines. The region holding the line grows, every
    /// later one moves, and a region typed into is the merge's own.
    pub(super) fn merge_edited(&mut self, row: usize, delta: isize) {
        for region in &mut self.regions {
            let lines = &mut region.lines[C];
            if lines.start > row {
                *lines = (lines.start as isize + delta).max(0) as usize..(lines.end as isize + delta).max(0) as usize;
            } else if row < lines.end || (row == lines.end && lines.start == lines.end) {
                lines.end = (lines.end as isize + delta).max(lines.start as isize) as usize;
                region.variant = Some(Variant::Edited);
            }
        }
    }

    /// The texts set out row by row: what the texts share, line for line,
    /// and each region as many rows as its longest text has lines, the
    /// others padded with filler.
    pub(super) fn aligned(&self, shown: &[usize]) -> Vec<Aligned> {
        let mut rows = Vec::new();
        let mut at = [0usize; 4];
        let regions = self.regions.iter().enumerate().map(|(index, region)| (Some(index), region.lines.clone()));
        let end = [A, B, C, ANCESTOR].map(|side| self.lines(side).len());
        let tail = std::iter::once((None, [end[A]..end[A], end[B]..end[B], end[C]..end[C], end[ANCESTOR]..end[ANCESTOR]]));
        for (region, lines) in regions.chain(tail) {
            // What comes before the region is shared, line for line.
            let common = shown.iter().map(|&side| lines[side].start.saturating_sub(at[side])).max().unwrap_or(0);
            for offset in 0..common {
                let mut row = Aligned {
                    lines: [None; 4],
                    region: None,
                };
                for &side in shown {
                    let line = at[side] + offset;
                    row.lines[side] = (line < lines[side].start).then_some(line);
                }
                rows.push(row);
            }
            for &side in shown {
                at[side] = lines[side].start;
            }
            let Some(index) = region else {
                break;
            };
            let height = shown.iter().map(|&side| lines[side].len()).max().unwrap_or(0).max(1);
            for offset in 0..height {
                let mut row = Aligned {
                    lines: [None; 4],
                    region: Some(index),
                };
                for &side in shown {
                    let line = lines[side].start + offset;
                    row.lines[side] = (line < lines[side].end).then_some(line);
                }
                rows.push(row);
            }
            for &side in shown {
                at[side] = lines[side].end;
            }
        }
        rows
    }

    /// Whether text `side` has edits not yet written.
    pub(super) fn is_dirty(&self, side: usize) -> bool {
        self.sides.get(side).is_some_and(|side| side.buffer.is_dirty())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn side(text: &str) -> Side {
        Side {
            buffer: TextBuffer::new(text.split(' ').map(str::to_string).collect()),
            label: String::new(),
            path: None,
            shape: FileShape::default(),
        }
    }

    fn text(session: &Session, side: usize) -> String {
        session.lines(side).join(" ")
    }

    #[test]
    fn test_rows_pad_the_shorter_text_of_a_region_with_filler() {
        let session = Session::new(Kind::Compare, vec![side("a b c"), side("a x y c")], None, None);
        let rows = session.aligned(&[A, B]);
        let shown: Vec<(Option<usize>, Option<usize>, Option<usize>)> =
            rows.iter().map(|row| (row.lines[A], row.lines[B], row.region)).collect();
        assert_eq!(
            shown,
            [(Some(0), Some(0), None), (Some(1), Some(1), Some(0)), (None, Some(2), Some(0)), (Some(2), Some(3), None)]
        );
    }

    #[test]
    fn test_copying_a_region_moves_the_later_ones_and_restoring_puts_it_back() {
        let mut session = Session::new(Kind::Compare, vec![side("a b c d"), side("a x y c e")], None, None);
        assert_eq!(session.regions.len(), 2);
        session.copy_region(0, A, B);
        assert_eq!(text(&session, B), "a b c e");
        assert_eq!(session.regions[1].lines[B], 3..4, "the later region moved up a line");
        session.copy_region(1, B, A);
        assert_eq!(text(&session, A), "a b c e");
        session.restore(0, B);
        assert_eq!(text(&session, B), "a x y c e");
        session.recompute();
        assert_eq!(session.regions.len(), 1, "worked out again from the texts");
    }

    #[test]
    fn test_a_merge_takes_one_sided_changes_and_marks_both_sided_ones() {
        let ancestor = side("a b c d");
        let session = Session::new(Kind::Merge, vec![side("a B c d"), side("a b c D")], Some(ancestor), None);
        assert_eq!(text(&session, C), "a B c D", "each side's own change is taken");
        assert!(session.regions.iter().all(|region| !region.conflict));
        let clash = Session::new(Kind::Merge, vec![side("a X c"), side("a Y c")], Some(side("a b c")), None);
        assert_eq!(
            clash.lines(C),
            ["a", OURS_MARK, "X", SPLIT_MARK, "Y", THEIRS_MARK, "c"]
        );
        assert!(clash.regions[0].conflict);
        let mut chosen = clash;
        chosen.choose(0, Variant::B);
        assert_eq!(text(&chosen, C), "a Y c");
        chosen.choose(0, Variant::Ancestor);
        assert_eq!(text(&chosen, C), "a b c");
        chosen.restore(0, C);
        assert_eq!(chosen.lines(C).len(), 7, "back to both");
        assert_eq!(chosen.regions[0].variant, Some(Variant::Combined));
    }

    #[test]
    fn test_an_edit_in_the_merge_grows_its_region_and_moves_the_later_ones() {
        let mut session = Session::new(Kind::Merge, vec![side("a X c Q"), side("a Y c R")], None, None);
        let second = session.regions[1].lines[C].clone();
        session.merge_edited(2, 1);
        assert_eq!(session.regions[0].variant, Some(Variant::Edited));
        assert_eq!(session.regions[1].lines[C], second.start + 1..second.end + 1);
    }
}
