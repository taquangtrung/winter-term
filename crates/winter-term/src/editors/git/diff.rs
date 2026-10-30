//! Reading a unified diff into hunks, and writing one hunk back out as a patch
//! git can apply on its own.

use std::ops::RangeInclusive;

// ========================================================================
// Constants
// ========================================================================

/// Marks the start of a hunk.
const HUNK_MARK: &str = "@@";

/// Opens a file's header: every line from here to the first hunk is part of
/// what a patch has to carry, `new file mode` and renames included.
const FILE_MARK: &str = "diff ";

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

impl FileDiff {
    /// Only the changed lines `chosen` (indices into hunk `index`'s body) as a
    /// standalone patch, for staging or unstaging part of a hunk. The other
    /// changes are written so the patch still applies: one that is not on the
    /// side being applied from is dropped, and one that is becomes context.
    /// Staging applies the patch forwards, so an unchosen `+` goes and an
    /// unchosen `-` stays as context; unstaging (`reverse`) applies it the
    /// other way round, so the two swap. `None` where nothing chosen changes.
    pub fn patch_for_lines(
        &self,
        index: usize,
        chosen: RangeInclusive<usize>,
        reverse: bool,
    ) -> Option<String> {
        let hunk = self.hunks.get(index)?;
        let (gone, kept) = match reverse {
            true => ('-', '+'),
            false => ('+', '-'),
        };
        let mut body: Vec<String> = Vec::new();
        let mut changes = 0;
        let mut last_dropped = false;
        for (at, line) in hunk.lines.iter().enumerate() {
            // The no-newline marker belongs to the line before it, and goes
            // where that line goes.
            if line.starts_with('\\') {
                if !last_dropped {
                    body.push(line.clone());
                }
                continue;
            }
            let picked = chosen.contains(&at);
            let first = line.chars().next();
            last_dropped = false;
            match first {
                Some(c) if (c == '+' || c == '-') && picked => {
                    changes += 1;
                    body.push(line.clone());
                }
                Some(c) if c == gone => last_dropped = true,
                Some(c) if c == kept => body.push(format!(" {}", &line[1..])),
                _ => body.push(line.clone()),
            }
        }
        if changes == 0 {
            return None;
        }
        let side = |mark: char| {
            body.iter()
                .filter(|line| line.starts_with(' ') || line.starts_with(mark))
                .count()
        };
        let (old_start, new_start) = hunk.starts()?;
        let mut patch = String::new();
        for line in &self.header {
            patch.push_str(line);
            patch.push('\n');
        }
        patch.push_str(&format!(
            "{HUNK_MARK} -{old_start},{} +{new_start},{} {HUNK_MARK}\n",
            side('-'),
            side('+')
        ));
        for line in &body {
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
    /// Where the hunk starts on the old side and on the new, from its header.
    fn starts(&self) -> Option<(usize, usize)> {
        let counts = self.header.strip_prefix(HUNK_MARK)?;
        let start = |mark: char| -> Option<usize> {
            let side = counts
                .split_whitespace()
                .find_map(|part| part.strip_prefix(mark))?;
            side.split(',').next()?.parse().ok()
        };
        Some((start('-')?, start('+')?))
    }

    /// How many lines the hunk adds and removes, for a one-line summary.
    pub fn counts(&self) -> (usize, usize) {
        let added = self.lines.iter().filter(|l| l.starts_with('+')).count();
        let removed = self.lines.iter().filter(|l| l.starts_with('-')).count();
        (added, removed)
    }

    /// The line the hunk starts at on the new side, counting from one, which
    /// is where it is in the file as it stands now. `None` for a header git
    /// wrote in some shape this does not read, rather than a guess at line
    /// one: landing somewhere arbitrary is worse than landing at the top.
    pub fn new_start(&self) -> Option<usize> {
        let counts = self.header.strip_prefix(HUNK_MARK)?;
        let new_side = counts
            .split_whitespace()
            .find_map(|part| part.strip_prefix('+'))?;
        let start = new_side.split(',').next()?;
        start.parse().ok()
    }

    /// The line in the file as it stands now that row `row` of the hunk is
    /// at, counting rows from its header: the header lands where the hunk
    /// starts, and a line on the line it is, its context and added lines
    /// counted down from there. A removed line is no longer in the file, so
    /// it lands where it was, on the line that now follows it.
    pub fn new_line_at(&self, row: usize) -> Option<usize> {
        let before = self
            .lines
            .iter()
            .take(row.saturating_sub(1))
            .filter(|line| line.starts_with(' ') || line.starts_with('+'))
            .count();
        // A hunk deleting a whole file starts at line zero, which no file has.
        Some((self.new_start()? + before).max(1))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Parse one file's unified diff. Everything from the `diff` line to the
/// first hunk is kept as the header, verbatim, since a patch without it has
/// nothing to apply against. Lines split on `\n` alone, so a CRLF file's
/// carriage returns stay in the patch that has to match it.
pub fn parse_diff(text: &str) -> FileDiff {
    let mut diff = FileDiff::default();
    for line in text.split_terminator('\n') {
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
            None if !diff.header.is_empty() || line.starts_with(FILE_MARK) => {
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
    fn test_part_of_a_hunk_stages_alone_and_git_takes_the_patch() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("winter-git-lines-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs");
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").expect("write");
        git(&["add", "f.txt"]);
        git(&["commit", "-qm", "base"]);
        std::fs::write(dir.join("f.txt"), "ONE\ntwo\nTHREE\n").expect("write");
        let diff = parse_diff(&git(&["diff", "-U5", "f.txt"]));
        let body = &diff.hunks[0].lines;
        let first_change = body.iter().position(|l| l == "-one").expect("a removal");

        // Stage the first change, the `-one`/`+ONE` pair, and nothing else.
        let patch = diff
            .patch_for_lines(0, first_change..=first_change + 1, false)
            .expect("a change was chosen");
        let mut apply = Command::new("git")
            .args(["apply", "--cached", "-"])
            .current_dir(&dir)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("git apply runs");
        use std::io::Write;
        apply
            .stdin
            .take()
            .expect("stdin")
            .write_all(patch.as_bytes())
            .expect("patch sent");
        assert!(
            apply.wait().expect("waited").success(),
            "git took the patch:\n{patch}"
        );
        assert_eq!(
            git(&["show", ":f.txt"]),
            "ONE\ntwo\nthree\n",
            "only the first change is staged"
        );

        // And the staged half comes back out the other way.
        let staged = parse_diff(&git(&["diff", "--cached", "-U5", "f.txt"]));
        let at = staged.hunks[0]
            .lines
            .iter()
            .position(|l| l == "+ONE")
            .expect("an addition");
        assert!(staged.patch_for_lines(0, at..=at, true).is_some());
        let context = body
            .iter()
            .position(|l| l == " two")
            .expect("a context line");
        assert_eq!(
            diff.patch_for_lines(0, context..=context, false),
            None,
            "context changes nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scratch repository, and a way to run git in it and feed it a patch.
    struct Repo(std::path::PathBuf);

    impl Repo {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("winter-git-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            let repo = Self(dir);
            repo.git(&["init", "-q"]);
            repo.git(&["config", "core.autocrlf", "false"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&self.0)
                .output()
                .expect("git runs");
            String::from_utf8_lossy(&out.stdout).to_string()
        }

        fn apply(&self, args: &[&str], patch: &str) -> bool {
            use std::io::Write;
            let mut apply = std::process::Command::new("git")
                .arg("apply")
                .args(args)
                .arg("-")
                .current_dir(&self.0)
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("git apply runs");
            apply.stdin.take().expect("stdin").write_all(patch.as_bytes()).expect("patch sent");
            apply.wait().expect("waited").success()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_a_new_file_s_hunk_unstages_without_touching_any_other_path() {
        // Without `new file mode` in its header, reversing the hunk out of the
        // index left a `dev/null` entry there and dropped the real file.
        let repo = Repo::new("newfile");
        std::fs::write(repo.0.join("base.txt"), "base\n").expect("write");
        repo.git(&["add", "base.txt"]);
        repo.git(&["commit", "-qm", "base"]);
        std::fs::write(repo.0.join("new.txt"), "one\ntwo\n").expect("write");
        repo.git(&["add", "new.txt"]);
        let staged = parse_diff(&repo.git(&["diff", "--cached", "new.txt"]));
        let patch = staged.patch_for(0).expect("a hunk");
        assert!(repo.apply(&["--cached", "--reverse"], &patch), "git took:\n{patch}");
        assert_eq!(repo.git(&["status", "--porcelain"]), "?? new.txt\n");
    }

    #[test]
    fn test_a_hunk_of_a_crlf_file_stages() {
        // Splitting the diff with `lines` dropped every carriage return, so
        // the patch matched no line of the file.
        let repo = Repo::new("crlf");
        std::fs::write(repo.0.join("f.txt"), "one\r\ntwo\r\n").expect("write");
        repo.git(&["add", "f.txt"]);
        repo.git(&["commit", "-qm", "base"]);
        std::fs::write(repo.0.join("f.txt"), "ONE\r\ntwo\r\n").expect("write");
        let diff = parse_diff(&repo.git(&["diff", "f.txt"]));
        let patch = diff.patch_for(0).expect("a hunk");
        assert!(repo.apply(&["--cached"], &patch), "git took:\n{patch:?}");
        assert_eq!(repo.git(&["show", ":f.txt"]), "ONE\r\ntwo\r\n");
    }

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
    fn test_each_row_of_a_hunk_lands_on_its_line_in_the_file_as_it_stands() {
        let diff = parse_diff(DIFF);
        let first = &diff.hunks[0];
        // Header, ` fn main() {`, `-old`, `+new`, ` }`: the removed line is
        // gone from the file, so it and the line replacing it share a line.
        let lines: Vec<Option<usize>> = (0..5).map(|row| first.new_line_at(row)).collect();
        assert_eq!(lines, [Some(1), Some(1), Some(2), Some(2), Some(3)]);
        // ` drop(x)` comes after a context line and an added one.
        assert_eq!(diff.hunks[1].new_line_at(3), Some(22));
    }

    #[test]
    fn test_a_hunk_says_which_line_it_starts_at_on_the_new_side() {
        // Opening a file from a hunk lands on this line, so reading the old
        // side's number, or the count after the comma, opens the wrong place.
        let diff = parse_diff(DIFF);
        assert_eq!(diff.hunks[0].new_start(), Some(1));
        assert_eq!(diff.hunks[1].new_start(), Some(20));

        // A single-line hunk writes no count at all.
        let single = parse_diff("@@ -7 +9 @@\n-old\n+new\n");
        assert_eq!(single.hunks[0].new_start(), Some(9));

        // A header in no shape this reads offers no line rather than line one.
        let odd = Hunk {
            header: "@@ nonsense @@".to_string(),
            lines: Vec::new(),
        };
        assert_eq!(odd.new_start(), None);
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
