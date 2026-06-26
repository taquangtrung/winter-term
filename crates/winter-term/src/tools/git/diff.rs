//! Reading a unified diff into hunks, and writing one hunk back out as a patch
//! git can apply on its own.

// ========================================================================
// Constants
// ========================================================================

/// Marks the start of a hunk.
const HUNK_MARK: &str = "@@";

/// Lines before the first hunk: the file header a patch has to carry.
const HEADER_PREFIXES: [&str; 6] = [
    "diff --git ",
    "index ",
    "old mode ",
    "new mode ",
    "--- ",
    "+++ ",
];

// ========================================================================
// Data Structures
// ========================================================================

/// One file's diff: the header a patch needs, and the hunks under it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileDiff {
    /// The `diff --git` preamble, verbatim, so a reconstructed patch applies.
    pub header: Vec<String>,
    /// Every hunk, in file order.
    pub hunks: Vec<Hunk>,
}

/// One hunk: its `@@` line and the lines under it, all verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@` line, with whatever trailing context git wrote.
    pub header: String,
    /// The hunk's body lines, each keeping its leading space, `+`, or `-`.
    pub lines: Vec<String>,
}

// ========================================================================
// FileDiff
// ========================================================================

impl FileDiff {
    /// Whether the diff has anything to show.
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }

    /// One hunk written out as a standalone patch: the file header, then that
    /// hunk alone. This is what lets a single hunk be staged while the rest of
    /// the file is left alone.
    pub fn patch_for(&self, index: usize) -> Option<String> {
        let hunk = self.hunks.get(index)?;
        let mut patch = String::new();
        for line in &self.header {
            patch.push_str(line);
            patch.push('\n');
        }
        patch.push_str(&hunk.header);
        patch.push('\n');
        for line in &hunk.lines {
            patch.push_str(line);
            patch.push('\n');
        }
        Some(patch)
    }
}

// ========================================================================
// Hunk
// ========================================================================

impl Hunk {
    /// How many lines the hunk adds and removes, for a one-line summary.
    pub fn counts(&self) -> (usize, usize) {
        let added = self.lines.iter().filter(|l| l.starts_with('+')).count();
        let removed = self.lines.iter().filter(|l| l.starts_with('-')).count();
        (added, removed)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Parse one file's unified diff. Anything before the first hunk is kept as the
/// header, since a patch without it has nothing to apply against.
pub fn parse_diff(text: &str) -> FileDiff {
    let mut diff = FileDiff::default();
    for line in text.lines() {
        if line.starts_with(HUNK_MARK) {
            diff.hunks.push(Hunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        match diff.hunks.last_mut() {
            // Inside a hunk: every line belongs to it, including the `\ No
            // newline at end of file` marker, which a patch must keep.
            Some(hunk) => hunk.lines.push(line.to_string()),
            None if HEADER_PREFIXES.iter().any(|p| line.starts_with(p)) => {
                diff.header.push(line.to_string());
            }
            None => {}
        }
    }
    diff
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 1234567..89abcde 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,4 @@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
 }
@@ -20,3 +20,4 @@ fn other() {
     let x = 1;
+    let y = 2;
     drop(x);
";

    #[test]
    fn test_every_hunk_is_found_with_its_body() {
        let diff = parse_diff(DIFF);
        assert_eq!(diff.hunks.len(), 2);
        assert!(diff.hunks[0].header.starts_with("@@ -1,4 +1,4 @@"));
        assert_eq!(diff.hunks[0].lines.len(), 4);
    }

    #[test]
    fn test_the_file_header_is_kept_verbatim() {
        // A patch missing the `---`/`+++` pair has nothing to apply against,
        // and git rejects it with "unrecognized input".
        let diff = parse_diff(DIFF);
        assert_eq!(diff.header.len(), 4);
        assert_eq!(diff.header[0], "diff --git a/src/main.rs b/src/main.rs");
        assert!(diff.header.iter().any(|l| l.starts_with("+++ ")));
    }

    #[test]
    fn test_a_single_hunk_patch_carries_the_header_and_only_that_hunk() {
        // This is the whole mechanism behind staging one hunk: the second
        // hunk's lines must not be in the first hunk's patch.
        let diff = parse_diff(DIFF);
        let patch = diff.patch_for(0).expect("the first hunk");
        assert!(patch.starts_with("diff --git"));
        assert!(patch.contains("@@ -1,4 +1,4 @@"));
        assert!(!patch.contains("let y = 2"), "got {patch}");
        assert!(patch.ends_with('\n'), "git needs the trailing newline");
    }

    #[test]
    fn test_a_hunk_header_keeps_its_trailing_context() {
        // git writes the enclosing function after the `@@`, and dropping it
        // changes the line the patch reads as its anchor.
        let diff = parse_diff(DIFF);
        assert!(diff.hunks[1].header.ends_with("fn other() {"));
    }

    #[test]
    fn test_counts_come_from_the_line_prefixes() {
        let diff = parse_diff(DIFF);
        assert_eq!(diff.hunks[0].counts(), (1, 1));
        assert_eq!(diff.hunks[1].counts(), (1, 0));
    }

    #[test]
    fn test_a_missing_newline_marker_stays_in_the_hunk() {
        // Dropped, the patch describes a file ending differently than it does,
        // and git refuses to apply it.
        let text = "\
diff --git a/f b/f
--- a/f
+++ b/f
@@ -1 +1 @@
-one
+two
\\ No newline at end of file
";
        let diff = parse_diff(text);
        assert!(diff.hunks[0]
            .lines
            .iter()
            .any(|line| line.starts_with("\\ No newline")));
    }

    #[test]
    fn test_output_with_no_hunks_is_empty_not_a_failure() {
        // A file whose only change is a mode bit has a header and no hunks.
        let diff = parse_diff("diff --git a/f b/f\nold mode 100644\nnew mode 100755\n");
        assert!(diff.is_empty());
        assert_eq!(diff.patch_for(0), None);
    }
}
