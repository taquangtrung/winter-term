//! The status view's painting tests.

use super::*;

use crate::tools::git::diff::parse_diff;
use crate::tools::git::parse::Ref;

fn file(section: Section, path: &str, code: char) -> FileStatus {
    FileStatus {
        section,
        path: path.to_string(),
        renamed_from: None,
        code,
    }
}

fn status_with(files: Vec<FileStatus>) -> Status {
    Status {
        ahead: 0,
        behind: 0,
        branch: Some("main".to_string()),
        files,
        upstream: None,
    }
}

fn text(row: &ViewRow) -> String {
    row.spans.iter().map(|span| span.text.clone()).collect()
}

fn open(_block: Block) -> bool {
    false
}

fn shut(_block: Block) -> bool {
    true
}

fn no_diffs(_file: &FileRow) -> Option<FileDiff> {
    None
}

/// A fixed "now" so relative ages in the rendered rows are deterministic.
const NOW: i64 = 1_800_000_000;

/// [`build`] with the arguments the view supplies at runtime pinned: no
/// repository path (so the `Repo:` line is left off and row 0 is the `Head:`
/// line) and a fixed clock.
fn built(
    status: &Status,
    commits: &[Commit],
    collapsed: &dyn Fn(Block) -> bool,
    diffs: &dyn Fn(&FileRow) -> Option<FileDiff>,
) -> Vec<ViewRow> {
    build(content(status, commits), collapsed, diffs)
}

/// The view's content with everything the tool reads separately left
/// empty, and a fixed clock: what a test that is not about the stashes or
/// the upstream draws.
fn content<'a>(status: &'a Status, commits: &'a [Commit]) -> StatusContent<'a> {
    StatusContent {
        status,
        commits,
        more_commits: false,
        unpulled: &[],
        stashes: &[],
        progress: None,
        root: None,
        now: NOW,
    }
}

#[test]
fn test_the_recent_commits_say_when_there_are_more_to_show() {
    let status = Status::default();
    let commits = vec![commit("aaa1111", "one")];
    let drawn = |more_commits: bool| -> Vec<String> {
        build(
            StatusContent {
                more_commits,
                ..content(&status, &commits)
            },
            &open,
            &no_diffs,
        )
        .iter()
        .map(text)
        .collect()
    };
    let with_more = drawn(true);
    let at = with_more
        .iter()
        .position(|row| row == LOG_MORE)
        .expect("offered");
    assert_eq!(with_more[at - 1], "", "set apart from the last commit");
    assert!(!drawn(false).iter().any(|row| row == LOG_MORE));
}

/// A commit with only the fields a test cares about set.
fn commit(hash: &str, subject: &str) -> Commit {
    Commit {
        hash: hash.to_string(),
        subject: subject.to_string(),
        author: "Someone".to_string(),
        time: NOW - 3_600,
        refs: Vec::new(),
    }
}

#[test]
fn test_an_empty_section_gets_no_heading() {
    // A clean tree should not read as four empty headings.
    let rows = built(&status_with(Vec::new()), &[], &open, &no_diffs);
    assert!(!rows.iter().any(|row| matches!(row.item, Item::Heading(_))));
}

#[test]
fn test_a_collapsed_section_keeps_its_heading_and_count() {
    let status = status_with(vec![
        file(Section::Staged, "a.rs", 'M'),
        file(Section::Staged, "b.rs", 'A'),
    ]);
    let rows = built(&status, &[], &shut, &no_diffs);
    let heading = rows
        .iter()
        .find(|row| matches!(row.item, Item::Heading(Section::Staged)))
        .expect("the heading");
    assert!(text(heading).contains("(2)"), "got {:?}", text(heading));
    assert!(!rows.iter().any(|row| matches!(row.item, Item::File(_))));
}

#[test]
fn test_a_rename_shows_where_it_came_from() {
    let mut renamed = file(Section::Staged, "new.rs", 'R');
    renamed.renamed_from = Some("old.rs".to_string());
    let rows = built(&status_with(vec![renamed]), &[], &open, &no_diffs);
    let row = rows
        .iter()
        .find(|row| matches!(row.item, Item::File(_)))
        .expect("the file row");
    assert!(text(row).contains("old.rs → new.rs"), "got {:?}", text(row));
}

#[test]
fn test_a_file_row_remembers_which_section_listed_it() {
    // Staging and unstaging are the same key on different sections, so the
    // row has to carry which one it came from.
    let status = status_with(vec![
        file(Section::Staged, "both.rs", 'M'),
        file(Section::Unstaged, "both.rs", 'M'),
    ]);
    let rows = built(&status, &[], &open, &no_diffs);
    let sections: Vec<Section> = rows
        .iter()
        .filter_map(|row| match &row.item {
            Item::File(file) => Some(file.section),
            _ => None,
        })
        .collect();
    assert_eq!(sections, [Section::Unstaged, Section::Staged]);
}

#[test]
fn test_the_header_shows_tracking_and_divergence() {
    let status = Status {
        ahead: 2,
        behind: 1,
        branch: Some("main".to_string()),
        files: Vec::new(),
        upstream: Some("origin/main".to_string()),
    };
    let rows = built(&status, &[], &open, &no_diffs);
    let block: Vec<String> = rows.iter().map(text).collect();
    let head = block.iter().find(|line| line.starts_with("Head:")).unwrap();
    let merge = block
        .iter()
        .find(|line| line.starts_with("Merge:"))
        .unwrap();
    assert_eq!(head, "Head:     main", "the head line names the branch");
    assert_eq!(
        merge, "Merge:    origin/main ↑2 ↓1",
        "the merge line names the upstream and the divergence"
    );

    // A side with nothing waiting is left out rather than counted as zero.
    let ahead_only = Status { behind: 0, ..status };
    let rows = built(&ahead_only, &[], &open, &no_diffs);
    let merge = rows.iter().map(text).find(|line| line.starts_with("Merge:")).unwrap();
    assert_eq!(merge, "Merge:    origin/main ↑2");
}

#[test]
fn test_the_header_labels_align_their_values() {
    // The block is read down its left edge, so every value starts in the
    // same column regardless of how long its label is.
    let status = status_with(Vec::new());
    let rows = built(&status, &[], &open, &no_diffs);
    for row in rows.iter().take(2) {
        let label = &row.spans[0].text;
        assert_eq!(
            label.chars().count(),
            LABEL_WIDTH,
            "label {label:?} is not padded to the value column"
        );
    }
}

#[test]
fn test_a_branch_with_no_upstream_says_so() {
    let rows = built(&status_with(Vec::new()), &[], &open, &no_diffs);
    let merge = rows
        .iter()
        .map(text)
        .find(|line| line.starts_with("Merge:"))
        .expect("the merge line");
    assert_eq!(merge, format!("Merge:    {NO_UPSTREAM}"));
}

#[test]
fn test_a_repository_with_no_commit_yet_says_so() {
    let status = Status {
        ahead: 0,
        behind: 0,
        branch: None,
        files: Vec::new(),
        upstream: None,
    };
    let rows = built(&status, &[], &open, &no_diffs);
    assert_eq!(text(&rows[0]), format!("Head:     {EMPTY_HEAD}"));
}

#[test]
fn test_a_tag_on_head_shows_in_the_header() {
    let mut tagged = commit("abc1234", "ship it");
    tagged.refs = vec![
        Ref {
            kind: RefKind::Head,
            name: "main".to_string(),
        },
        Ref {
            kind: RefKind::Tag,
            name: "v1.4.0".to_string(),
        },
    ];
    let rows = built(&status_with(Vec::new()), &[tagged], &open, &no_diffs);
    let tag = rows
        .iter()
        .map(text)
        .find(|line| line.starts_with("Tag:"))
        .expect("the tag line");
    assert_eq!(tag, "Tag:      v1.4.0");
}

#[test]
fn test_the_head_line_carries_the_head_commits_subject() {
    let rows = built(
        &status_with(Vec::new()),
        &[commit("abc1234", "fix the parser")],
        &open,
        &no_diffs,
    );
    assert_eq!(text(&rows[0]), "Head:     main fix the parser");
}

#[test]
fn test_commits_carry_their_hash_for_acting_on() {
    let commits = vec![commit("abc1234", "do the thing")];
    let rows = built(&status_with(Vec::new()), &commits, &open, &no_diffs);
    assert!(rows
        .iter()
        .any(|row| row.item == Item::Commit("abc1234".to_string())));
}

#[test]
fn test_a_commit_reads_as_an_identity_line_over_an_authorship_line() {
    let commits = vec![commit("abc1234", "do the thing")];
    let rows = built(&status_with(Vec::new()), &commits, &open, &no_diffs);
    let commit_rows: Vec<&ViewRow> = rows
        .iter()
        .filter(|row| row.item == Item::Commit("abc1234".to_string()))
        .collect();
    assert_eq!(commit_rows.len(), 2, "a commit takes two rows");
    assert_eq!(
        text(commit_rows[0]),
        "abc1234 * do the thing",
        "the identity line carries the hash, the graph mark, and the subject"
    );
    assert_eq!(
        text(commit_rows[1]),
        "        Someone   1 hour",
        "the authorship line sits under the graph column"
    );
    assert_eq!(
        commit_rows[1].wrap_indent, 8,
        "both rows wrap to the column their text starts at"
    );
}

#[test]
fn test_refs_are_drawn_beside_a_commit_by_what_kind_they_are() {
    let mut decorated = commit("abc1234", "do the thing");
    decorated.refs = vec![
        Ref {
            kind: RefKind::Head,
            name: "main".to_string(),
        },
        Ref {
            kind: RefKind::Remote,
            name: "origin/main".to_string(),
        },
        Ref {
            kind: RefKind::Tag,
            name: "v1.0".to_string(),
        },
    ];
    let rows = built(&status_with(Vec::new()), &[decorated], &open, &no_diffs);
    let row = rows
        .iter()
        .find(|row| row.item == Item::Commit("abc1234".to_string()))
        .expect("the commit row");
    assert_eq!(text(row), "abc1234 * main origin/main v1.0 do the thing");
    let styles: Vec<Style> = row.spans.iter().map(|span| span.style).collect();
    assert!(styles.contains(&Style::RefHead), "got {styles:?}");
    assert!(styles.contains(&Style::RefRemote), "got {styles:?}");
    assert!(styles.contains(&Style::RefTag), "got {styles:?}");
}

#[test]
fn test_the_commits_ahead_of_the_upstream_are_marked_unpushed() {
    // A reverse-chronological log puts the unpushed commits first, so the
    // count of commits ahead is also how many rows carry the mark.
    let status = Status {
        ahead: 1,
        behind: 0,
        branch: Some("main".to_string()),
        files: Vec::new(),
        upstream: Some("origin/main".to_string()),
    };
    let commits = vec![commit("newer12", "not pushed"), commit("older34", "pushed")];
    let rows = built(&status, &commits, &open, &no_diffs);
    let line = |hash: &str| -> String {
        rows.iter()
            .filter(|row| row.item == Item::Commit(hash.to_string()))
            .map(text)
            .next()
            .expect("the commit row")
    };
    assert!(
        line("newer12").contains(MARK_UNPUSHED),
        "got {:?}",
        line("newer12")
    );
    assert!(
        !line("older34").contains(MARK_UNPUSHED),
        "got {:?}",
        line("older34")
    );
}

#[test]
fn test_the_log_view_heads_its_commits_and_offers_more() {
    let commits = vec![commit("abc1234", "do the thing")];
    let rows = log_rows(&commits, None, NOW, true);
    let lines: Vec<String> = rows.iter().map(text).collect();
    assert_eq!(lines[0], LOG_TITLE, "the list is headed");
    assert_eq!(lines[1], "abc1234 * do the thing");
    assert_eq!(lines[2], "        Someone   1 hour");
    assert_eq!(
        lines.last().map(String::as_str),
        Some(LOG_MORE),
        "a log that can grow says so"
    );
}

#[test]
fn test_hunk_lines_reserve_their_marker_column_when_wrapped() {
    let diff = parse_diff("--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n");
    let file = FileRow {
        path: "f".to_string(),
        section: Section::Staged,
    };
    let rows = hunk_rows(&file, &diff);
    // The hunk's header band wraps from the pane's edge like any row;
    // its body lines reserve the marker column their text sits after.
    assert_eq!(rows[0].wrap_indent, 0);
    assert_eq!(rows[1].wrap_indent, MARKER_WIDTH);
    assert_eq!(rows[2].wrap_indent, MARKER_WIDTH);
}

#[test]
fn test_a_diff_outputs_lines_reserve_their_marker_column() {
    let lines: Vec<String> = "diff --git a/f b/f\nindex 123..456 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n fn main() {\n-    old();\n+    new();\n"
        .lines()
        .map(str::to_string)
        .collect();
    let (_, indents) = diff_rows(&lines);
    // Nothing before the hunk wraps past a marker; the hunk's context and
    // changed lines all carry the one-column marker their wraps keep.
    assert_eq!(&indents[..5], &[0, 0, 0, 0, 0]);
    assert_eq!(&indents[5..], &[1, 1, 1]);
}

fn shown(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

/// The commit view with everything open. The tool itself first draws one
/// shut, and opens it from there.
fn commit_view(lines: &[String]) -> Vec<ViewRow> {
    commit_view_rows(
        &CommitContent::parse(lines),
        opened(&HashSet::new(), &HashSet::new()),
    )
}

/// The folds of a commit whose heading is open, with `files` and `hunks`
/// shut under it.
fn opened<'a>(files: &'a HashSet<usize>, hunks: &'a HashSet<(usize, usize)>) -> CommitFolds<'a> {
    CommitFolds {
        shut: false,
        files,
        hunks,
    }
}

#[test]
fn test_a_commits_summary_reads_with_its_message_loudest() {
    let lines = shown(&[
        "commit abc1234567890 (HEAD -> main)",
        "Author: Someone <a@b.c>",
        "Date:   2026-09-15 12:00:00 +0000",
        "",
        "    do the thing",
        "",
    ]);
    let rows = commit_view(&lines);
    assert_eq!(
        rows[0].spans,
        vec![Span::new(
            Style::Header,
            "commit abc1234567890 (HEAD -> main)"
        )],
        "the identity line heads the view"
    );
    assert_eq!(
        rows[1].spans,
        vec![Span::new(Style::Dim, "Author: Someone <a@b.c>")],
        "the headers recede"
    );
    assert_eq!(
        rows[4].spans,
        vec![Span::plain("do the thing")],
        "the message loses the four columns git indented it by"
    );
    assert!(rows.iter().all(|row| row.wrap_indent == 0));
    assert!(
        rows.iter().all(|row| row.item == Item::None),
        "nothing in the summary is actionable"
    );
}

#[test]
fn test_a_commits_files_hang_off_one_counted_heading() {
    // The commit reads as the status view does: a heading saying how many
    // files it touched, each band saying how many hunks it holds. Both
    // count in the units the keys act in, where git's own line totals
    // count something no key here opens.
    let lines = shown(&[
        "commit abc",
        "",
        "    message",
        "diff --git a/a.rs b/a.rs",
        "--- a/a.rs",
        "+++ b/a.rs",
        "@@ -1 +1 @@",
        "-old",
        "+new",
        "@@ -9 +9 @@",
        "-second",
        "+third",
        "diff --git a/b.rs b/b.rs",
        "--- a/b.rs",
        "+++ b/b.rs",
        "@@ -1 +1 @@",
        "-old",
        "+new",
    ]);
    let rows = commit_view(&lines);
    let line = |needle: &str| -> String {
        rows.iter()
            .map(text)
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no row for {needle}"))
    };
    assert_eq!(line(CHANGES_TITLE), "▼ Changes (2)", "two files under it");
    assert!(line("a.rs").ends_with(" (2)"), "got {:?}", line("a.rs"));
    assert!(line("b.rs").ends_with(" (1)"), "got {:?}", line("b.rs"));
    assert!(
        !rows.iter().any(|row| text(row).contains("changed,")),
        "and nothing said in lines changed"
    );

    // A commit with no file at all heads nothing.
    let bare = commit_view(&shown(&["commit abc", "", "    message"]));
    assert!(bare.iter().all(|row| !text(row).contains(CHANGES_TITLE)));
}

#[test]
fn test_the_stashes_and_the_upstreams_commits_get_sections_of_their_own() {
    let status = Status {
        ahead: 0,
        behind: 2,
        branch: Some("main".to_string()),
        files: Vec::new(),
        upstream: Some("origin/main".to_string()),
    };
    let stashes = vec![
        Stash {
            name: "stash@{0}".to_string(),
            subject: "WIP on main: half a thing".to_string(),
        },
        Stash {
            name: "stash@{1}".to_string(),
            subject: "spike".to_string(),
        },
    ];
    let unpulled = vec![commit("aaa1111", "theirs")];
    let rows = build(
        StatusContent {
            stashes: &stashes,
            unpulled: &unpulled,
            ..content(&status, &[])
        },
        &open,
        &no_diffs,
    );
    let lines: Vec<String> = rows.iter().map(text).collect();

    assert!(
        lines.iter().any(|line| line == "▼ Stashes (2)"),
        "got {lines:?}"
    );
    assert!(lines
        .iter()
        .any(|line| line.contains("stash@{0} WIP on main")));
    assert!(
        lines
            .iter()
            .any(|line| line == "▼ Unpulled from origin/main (1)"),
        "the section names where the commits came from, got {lines:?}"
    );
    assert!(lines.iter().any(|line| line.contains("theirs")));
}

#[test]
fn test_every_heading_folds_the_block_it_heads() {
    let status = Status {
        ahead: 0,
        behind: 1,
        branch: Some("main".to_string()),
        files: vec![file(Section::Staged, "s.rs", 'M')],
        upstream: Some("origin/main".to_string()),
    };
    let stashes = vec![Stash {
        name: "stash@{0}".to_string(),
        subject: "spike".to_string(),
    }];
    let unpulled = vec![commit("aaa1111", "theirs")];
    let rows = build(
        StatusContent {
            stashes: &stashes,
            unpulled: &unpulled,
            ..content(&status, &[commit("bbb2222", "mine")])
        },
        &shut,
        &no_diffs,
    );
    let lines: Vec<String> = rows.iter().map(text).collect();

    // Every block's own heading survives its fold, and nothing under any
    // of them does.
    for heading in ["Staged changes", "Stashes", "Unpulled from", RECENT_TITLE] {
        assert!(
            lines.iter().any(|line| line.contains(heading)),
            "{heading} keeps its heading, got {lines:?}"
        );
    }
    // By hash for the commits: their subjects also read on the `Head:`
    // line, which is not part of any block.
    for hidden in ["s.rs", "stash@{0}", "aaa1111", "bbb2222"] {
        assert!(
            !lines.iter().any(|line| line.contains(hidden)),
            "{hidden} is folded away, got {lines:?}"
        );
    }
    for item in [
        Item::Heading(Section::Staged),
        Item::StashHeading,
        Item::UnpulledHeading,
        Item::RecentHeading,
    ] {
        assert!(fold_block(&item).is_some(), "{item:?} folds a block");
    }
}

#[test]
fn test_an_unfinished_operation_says_what_it_is_and_how_to_finish_it() {
    // Nothing in a porcelain status says a rebase is open: without this
    // line the only sign of one is a section of conflicts and no reason.
    let progress = Progress {
        operation: Operation::Rebase,
        detail: "feature onto abc1234".to_string(),
        step: Some((3, 7)),
    };
    let status = status_with(Vec::new());
    let rows = build(
        StatusContent {
            progress: Some(&progress),
            ..content(&status, &[])
        },
        &open,
        &no_diffs,
    );
    let lines: Vec<String> = rows.iter().map(text).collect();

    assert!(
        lines.iter().any(|line| line.starts_with("State:")
            && line.contains("Rebasing feature onto abc1234")
            && line.contains("(3/7)")),
        "got {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("r c continue")),
        "and the keys that finish it, got {lines:?}"
    );
    let title = rows
        .iter()
        .flat_map(|row| &row.spans)
        .find(|span| span.text.contains("Rebasing"))
        .expect("the operation's name");
    assert_eq!(
        title.style,
        Style::Operation,
        "a header line, not a file's band"
    );
}

#[test]
fn test_a_section_heading_wears_the_hue_of_what_it_holds() {
    // The headings are what the eye lands on first, so each says which
    // section it is by color as well as by name, and the count beside it
    // stays receded.
    let files = vec![
        file(Section::Staged, "s.rs", 'M'),
        file(Section::Unstaged, "u.rs", 'M'),
        file(Section::Untracked, "n.rs", '?'),
        file(Section::Unmerged, "c.rs", 'U'),
    ];
    let rows = built(&status_with(files), &[], &open, &no_diffs);
    let heading = |section: Section| -> &ViewRow {
        rows.iter()
            .find(|row| row.item == Item::Heading(section))
            .unwrap_or_else(|| panic!("no heading for {section:?}"))
    };
    assert_eq!(
        heading(Section::Staged).spans[0].style,
        Style::HeadingStaged
    );
    assert_eq!(
        heading(Section::Unstaged).spans[0].style,
        Style::HeadingUnstaged
    );
    assert_eq!(
        heading(Section::Untracked).spans[0].style,
        Style::HeadingUntracked
    );
    assert_eq!(
        heading(Section::Unmerged).spans[0].style,
        Style::HeadingConflict
    );
    assert_eq!(
        heading(Section::Staged).spans[1].style,
        Style::Dim,
        "the count stays receded"
    );

    // A heading over rows that are not a kind of change takes the hue
    // every other heading of a git view wears.
    let rows = built(
        &status_with(Vec::new()),
        &[commit("abc1234", "do the thing")],
        &open,
        &no_diffs,
    );
    let recent = rows
        .iter()
        .find(|row| row.item == Item::RecentHeading)
        .expect("the recent heading");
    assert_eq!(recent.spans[0].style, Style::HeadingPlain);
}

#[test]
fn test_only_the_arrow_of_a_drifted_commit_is_colored() {
    let status = Status {
        ahead: 1,
        behind: 0,
        branch: Some("main".to_string()),
        files: Vec::new(),
        upstream: Some("origin/main".to_string()),
    };
    let commits = vec![commit("newer12", "not pushed"), commit("older34", "pushed")];
    let unpulled = vec![commit("aaa1111", "theirs")];
    let rows = build(
        StatusContent {
            unpulled: &unpulled,
            ..content(&status, &commits)
        },
        &open,
        &no_diffs,
    );
    let spans = |hash: &str| -> Vec<(Style, String)> {
        rows.iter()
            .find(|row| row.item == Item::Commit(hash.to_string()))
            .expect("the commit row")
            .spans
            .iter()
            .map(|span| (span.style, span.text.clone()))
            .collect()
    };
    for (hash, arrow) in [("newer12", MARK_UNPUSHED), ("aaa1111", MARK_UNPULLED)] {
        let colored: Vec<String> = spans(hash)
            .into_iter()
            .filter(|(style, _)| *style == Style::Unpushed)
            .map(|(_, text)| text)
            .collect();
        assert_eq!(colored, vec![arrow.to_string()], "{hash}: the arrow alone");
    }
    assert!(
        spans("older34")
            .iter()
            .all(|(style, _)| *style != Style::Unpushed),
        "a commit both sides have reads plainly"
    );
}

#[test]
fn test_a_change_is_told_by_the_hue_its_name_wears() {
    // The name and the mark before it carry the change's own color, so a
    // commit's file list reads as a legend rather than as words to be
    // parsed one at a time. The path keeps the band's own color: it says
    // which file, not what happened to it.
    let lines = shown(&[
        "commit abc",
        "",
        "    message",
        "diff --git a/kept.rs b/kept.rs",
        "--- a/kept.rs",
        "+++ b/kept.rs",
        "@@ -1 +1 @@",
        "-old",
        "+new",
        "diff --git a/gone.rs b/gone.rs",
        "deleted file mode 100644",
        "--- a/gone.rs",
        "+++ /dev/null",
        "@@ -1 +0,0 @@",
        "-old",
        "diff --git a/new.rs b/new.rs",
        "new file mode 100644",
        "--- /dev/null",
        "+++ b/new.rs",
        "@@ -0,0 +1 @@",
        "+fresh",
    ]);
    let rows = commit_view(&lines);
    let hue = |needle: &str| -> Style {
        rows.iter()
            .find(|row| text(row).contains(needle))
            .unwrap_or_else(|| panic!("no band for {needle}"))
            .spans[0]
            .style
    };
    assert_eq!(hue("kept.rs"), Style::ChangeModified);
    assert_eq!(hue("gone.rs"), Style::ChangeDeleted);
    assert_eq!(hue("new.rs"), Style::ChangeAdded);

    // The working tree's own codes reach the same colors, including the
    // two the commit view never produces.
    assert_eq!(change_style('?'), Style::ChangeAdded);
    assert_eq!(change_style('U'), Style::ChangeConflict);
    assert_eq!(change_style('R'), Style::ChangeRenamed);
}

#[test]
fn test_a_folds_mark_points_the_way_the_row_stands() {
    // The mark is the only thing on a band that says whether what hangs
    // off it is showing, since a shut band and an open one otherwise read
    // the same, and it has to follow the fold rather than the row's kind.
    let lines = shown(&[
        "commit abc",
        "",
        "    message",
        "diff --git a/f.rs b/f.rs",
        "--- a/f.rs",
        "+++ b/f.rs",
        "@@ -1 +1 @@",
        "-old",
        "+new",
    ]);
    let content = CommitContent::parse(&lines);
    let band = |rows: &[ViewRow], needle: &str| -> String {
        text(
            rows.iter()
                .find(|row| text(row).contains(needle))
                .unwrap_or_else(|| panic!("no row for {needle}")),
        )
    };

    let open = commit_view_rows(&content, opened(&HashSet::new(), &HashSet::new()));
    assert!(band(&open, "f.rs").starts_with(GLYPH_EXPANDED));
    assert!(band(&open, "@@").starts_with(GLYPH_EXPANDED));

    let shut_hunk = HashSet::from([(0, 0)]);
    let hunk_shut = commit_view_rows(&content, opened(&HashSet::new(), &shut_hunk));
    assert!(
        band(&hunk_shut, "f.rs").starts_with(GLYPH_EXPANDED),
        "the file is still open"
    );
    assert!(
        band(&hunk_shut, "@@").starts_with(GLYPH_COLLAPSED),
        "its hunk is not"
    );

    let shut_file = HashSet::from([0]);
    let file_shut = commit_view_rows(&content, opened(&shut_file, &HashSet::new()));
    assert!(band(&file_shut, "f.rs").starts_with(GLYPH_COLLAPSED));
}

#[test]
fn test_a_status_file_row_marks_whether_its_diff_is_showing() {
    let status = status_with(vec![file(Section::Staged, "f.rs", 'M')]);
    let shut = built(&status, &[], &open, &no_diffs);
    let row = shut
        .iter()
        .find(|row| matches!(row.item, Item::File(_)))
        .expect("the file row");
    assert!(
        text(row).starts_with(GLYPH_COLLAPSED),
        "a file whose diff has never been read reads as shut, got {:?}",
        text(row)
    );

    let diff = parse_diff("--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n");
    let with_diff = |_file: &FileRow| Some(diff.clone());
    let open_rows = built(&status, &[], &open, &with_diff);
    let row = open_rows
        .iter()
        .find(|row| matches!(row.item, Item::File(_)))
        .expect("the file row");
    assert!(
        text(row).starts_with(GLYPH_EXPANDED),
        "one showing its diff reads as open, got {:?}",
        text(row)
    );
}

#[test]
fn test_a_bands_path_starts_at_one_column_whatever_happened_to_its_file() {
    // The change is written out, and the words are not all one length, so
    // the column a path starts at has to come from the padding rather than
    // from the word before it: a list of paths that steps in and out with
    // what happened to each one cannot be read down.
    let lines = shown(&[
        "commit abc",
        "",
        "    message",
        "diff --git a/gone.rs b/gone.rs",
        "deleted file mode 100644",
        "--- a/gone.rs",
        "+++ /dev/null",
        "@@ -1 +0,0 @@",
        "-old",
        "diff --git a/kept.rs b/kept.rs",
        "--- a/kept.rs",
        "+++ b/kept.rs",
        "@@ -1 +1 @@",
        "-old",
        "+new",
    ]);
    let rows = commit_view(&lines);
    let band = |needle: &str| -> String {
        let opens_with = format!("{GLYPH_EXPANDED}{needle}");
        text(
            rows.iter()
                .find(|row| text(row).starts_with(&opens_with))
                .unwrap_or_else(|| panic!("no band saying {needle}")),
        )
    };
    let deleted = band("deleted");
    let modified = band("modified");
    assert_eq!(
        deleted.find("gone.rs"),
        modified.find("kept.rs"),
        "both paths start at the same column, got {deleted:?} and {modified:?}"
    );
}

#[test]
fn test_each_file_gets_a_band_with_its_change_and_hunk_count() {
    let lines = shown(&[
        "commit abc",
        "",
        "    message",
        "diff --git a/kept.rs b/kept.rs",
        "--- a/kept.rs",
        "+++ b/kept.rs",
        "@@ -1 +1 @@",
        "-old",
        "+new",
        "diff --git a/gone.txt b/added.txt",
        "new file mode 100644",
        "--- /dev/null",
        "+++ b/added.txt",
        "@@ -0,0 +1 @@",
        "+fresh",
    ]);
    let rows = commit_view(&lines);
    let kept = rows
        .iter()
        .find(|row| text(row).contains("kept.rs"))
        .expect("the kept file's band");
    assert_eq!(
        kept.spans[0],
        Span::new(Style::ChangeModified, "▼ modified    "),
        "the fold mark and the change's name lead the band, in the hue \
         that change wears"
    );
    assert_eq!(
        kept.spans[1],
        Span::new(Style::Section, "   kept.rs"),
        "the path follows on the band itself"
    );
    assert_eq!(
        kept.spans[2],
        Span::new(Style::Dim, " (1)"),
        "the band counts the hunks it holds"
    );
    assert_eq!(
        kept.icon,
        Some(Icon {
            col: fold_width() + STATUS_WIDTH,
            kind: IconKind::File {
                name: "kept.rs".to_string()
            },
            row: 0,
        }),
        "a file in a commit carries its icon, the way the working tree's does"
    );
    assert_eq!(
        kept.item,
        Item::CommitFile(0),
        "the band knows which file it is, so a fold key can act on it"
    );
    let added = rows
        .iter()
        .find(|row| text(row).contains("added.txt"))
        .expect("the added file's band");
    assert_eq!(
        added.spans[0],
        Span::new(Style::ChangeAdded, "▼ new file    "),
        "a new file says so in the words git writes it as"
    );
    assert_eq!(
        added.icon.as_ref().map(|icon| &icon.kind),
        Some(&IconKind::File {
            name: "added.txt".to_string()
        }),
        "the icon is named for the file, so the icon set can match it"
    );
    // The hunk bodies carry the marker column their wraps reserve.
    let body = rows
        .iter()
        .find(|row| text(row).contains("fresh"))
        .expect("the added line");
    assert!(body.spans.iter().any(|span| span.style == Style::AddedEdit));
}

#[test]
fn test_an_icon_leaves_its_columns_clear_in_the_band_text() {
    // The icon is drawn over the row, so the text must not put glyphs where
    // the artwork lands or the two overlap.
    let rows = commit_view(&shown(&[
        "diff --git a/src/deep/path/file.rs b/src/deep/path/file.rs",
        "--- a/src/deep/path/file.rs",
        "+++ b/src/deep/path/file.rs",
        "@@ -1 +1 @@",
        "-a",
        "+b",
    ]));
    let band = rows
        .iter()
        .find(|row| row.item == Item::CommitFile(0))
        .expect("the band");
    let icon = band.icon.as_ref().expect("its icon");
    let text = text(band);
    let reserved: String = text.chars().skip(icon.col).take(Icon::WIDTH).collect();
    assert_eq!(
        reserved, "  ",
        "the icon's columns are blank in the text, got {text:?}"
    );
    assert_eq!(
        icon.kind,
        IconKind::File {
            name: "file.rs".to_string()
        },
        "the icon is named for the leaf, not the whole path"
    );
}

#[test]
fn test_folding_a_file_hides_its_hunks_but_keeps_its_band() {
    let lines = shown(&[
        "diff --git a/f.rs b/f.rs",
        "--- a/f.rs",
        "+++ b/f.rs",
        "@@ -1 +1 @@",
        "-old",
        "+new",
    ]);
    let content = CommitContent::parse(&lines);
    let open = commit_view_rows(&content, opened(&HashSet::new(), &HashSet::new()));
    let shut_file = HashSet::from([0]);
    let shut = commit_view_rows(&content, opened(&shut_file, &HashSet::new()));

    assert!(open.iter().any(|row| text(row).contains("+new")));
    assert!(
        !shut.iter().any(|row| text(row).contains("+new")),
        "a folded file shows none of its diff"
    );
    let band = shut
        .iter()
        .find(|row| row.item == Item::CommitFile(0))
        .expect("the band survives the fold");
    assert_eq!(
        band.spans[1].style,
        Style::SectionFolded,
        "a shut file's path recedes, the way the working tree's does"
    );
    assert!(
        text(band).contains("f.rs"),
        "the band still names the file so it can be unfolded"
    );
    assert_eq!(
        open.iter()
            .find(|row| row.item == Item::CommitFile(0))
            .map(|row| row.spans[1].style),
        Some(Style::Section),
        "an open file's path is the bright one"
    );
}

#[test]
fn test_folding_a_hunk_keeps_its_header_and_drops_its_body() {
    let lines = shown(&[
        "diff --git a/f.rs b/f.rs",
        "--- a/f.rs",
        "+++ b/f.rs",
        "@@ -1 +1 @@",
        "-old",
        "+new",
        "@@ -9 +9 @@",
        "-second",
        "+third",
    ]);
    let content = CommitContent::parse(&lines);
    // Shut the first hunk only, so the second proves folding is per-hunk.
    let shut_hunk = HashSet::from([(0, 0)]);
    let rows = commit_view_rows(&content, opened(&HashSet::new(), &shut_hunk));
    let lines: Vec<String> = rows.iter().map(text).collect();

    assert!(
        lines.iter().any(|line| line.contains("@@ -1 +1 @@")),
        "a folded hunk keeps its header, which carries the counts"
    );
    assert!(
        !lines.iter().any(|line| line.contains("-old")),
        "but shows none of its body, got {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("+third")),
        "the hunk left open still shows its body"
    );
}

#[test]
fn test_every_view_paints_its_diff_flush_left() {
    let lines = shown(&[
        "commit abc",
        "",
        "    message",
        "diff --git a/f.rs b/f.rs",
        "--- a/f.rs",
        "+++ b/f.rs",
        "@@ -1,2 +1,2 @@",
        " context",
        "-old",
        "+new",
    ]);
    let rows = commit_view(&lines);

    // The hunk header opens at column 0, not under an indent.
    let (header_at, header) = rows
        .iter()
        .enumerate()
        .find(|(_, row)| text(row).contains("@@"))
        .expect("the hunk header");
    assert_eq!(
        text(header),
        "▼ @@ -1,2 +1,2 @@  +1 -1",
        "a commit's hunk header starts at the left margin, behind the mark \
         saying it folds"
    );

    // Body lines start with their bare marker, no leading blanks.
    for (offset, expected) in [(1, " context"), (2, "-old"), (3, "+new")] {
        let row = &rows[header_at + offset];
        assert_eq!(
            text(row),
            expected,
            "a commit's diff body line carries only its marker, no indent"
        );
        assert_eq!(
            row.wrap_indent, MARKER_WIDTH,
            "its wrapped continuation still clears just the marker"
        );
    }

    // The working-tree view paints the same hunk the same way: what a line
    // belongs to is read off the band above it, not off an indent.
    let diff = parse_diff("--- a/f.rs\n+++ b/f.rs\n@@ -1,2 +1,2 @@\n context\n-old\n+new\n");
    let file = FileRow {
        path: "f.rs".to_string(),
        section: Section::Staged,
    };
    let tree = hunk_rows(&file, &diff);
    assert_eq!(
        text(&tree[0]),
        "@@ -1,2 +1,2 @@  +1 -1",
        "the status view's hunk header starts at the left margin too, and \
         carries no mark: folding there acts on the file, not the hunk"
    );
    assert_eq!(
        text(&tree[1]),
        " context",
        "its body carries only its marker"
    );
    assert_eq!(tree[1].wrap_indent, MARKER_WIDTH);
}

#[test]
fn test_a_binary_file_shows_gits_note_under_its_band() {
    let lines = shown(&[
        "commit abc",
        "diff --git a/logo.png b/logo.png",
        "index 111..222 100644",
        "Binary files a/logo.png and b/logo.png differ",
    ]);
    let rows = commit_view(&lines);
    let band = rows
        .iter()
        .find(|row| text(row).contains("logo.png"))
        .expect("the band");
    assert_eq!(band.spans[1].style, Style::Section);
    assert_eq!(
        rows.last().map(|row| row.spans.clone()),
        Some(vec![Span::new(
            Style::Dim,
            "Binary files a/logo.png and b/logo.png differ"
        )]),
        "the note stands under the band rather than nothing at all"
    );
}

#[test]
fn test_a_replaced_line_paints_only_its_edited_words_loudly() {
    // The whole line wears its side's tint, and only the edited words
    // wear it harder.
    let diff = parse_diff("--- a/f\n+++ b/f\n@@ -1 +1 @@\n-    old();\n+    new();\n");
    let file = FileRow {
        path: "f".to_string(),
        section: Section::Staged,
    };
    let rows = hunk_rows(&file, &diff);
    let removed = rows
        .iter()
        .find(|row| text(row).contains("old();"))
        .expect("the removed line");
    assert_eq!(
        removed.spans,
        vec![
            Span::new(Style::Removed, "-    "),
            Span::new(Style::RemovedEdit, "old"),
            Span::new(Style::Removed, "();"),
        ]
    );
    let added = rows
        .iter()
        .find(|row| text(row).contains("new();"))
        .expect("the added line");
    assert_eq!(
        added.spans,
        vec![
            Span::new(Style::Added, "+    "),
            Span::new(Style::AddedEdit, "new"),
            Span::new(Style::Added, "();"),
        ]
    );
    assert_eq!(text(added), "+    new();");
}

#[test]
fn test_a_line_without_a_counterpart_paints_wholly_as_the_edit() {
    // A pure insertion has no removed text to word-diff against, so every
    // word of it is the edit and wears the harder tint; only its marker
    // stays structural.
    let diff = parse_diff("--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n keep\n+    added();\n");
    let file = FileRow {
        path: "f".to_string(),
        section: Section::Staged,
    };
    let rows = hunk_rows(&file, &diff);
    let added = rows
        .iter()
        .find(|row| text(row).contains("added();"))
        .expect("the added line");
    assert_eq!(
        added.spans,
        vec![
            Span::new(Style::Added, "+"),
            Span::new(Style::AddedEdit, "    added();"),
        ]
    );
}

#[test]
fn test_a_diff_output_styles_headers_and_decorates_changes() {
    let lines: Vec<String> = "diff --git a/f b/f\nindex 123..456 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n fn main() {\n-    old();\n+    new();\n"
        .lines()
        .map(str::to_string)
        .collect();
    let (rows, _) = diff_rows(&lines);
    assert_eq!(
        rows[0],
        vec![Span::new(Style::Section, "diff --git a/f b/f")]
    );
    assert_eq!(
        rows[1],
        vec![Span::new(Style::Dim, "index 123..456 100644")]
    );
    assert_eq!(rows[2], vec![Span::new(Style::Dim, "--- a/f")]);
    assert_eq!(rows[3], vec![Span::new(Style::Dim, "+++ b/f")]);
    assert_eq!(rows[4], vec![Span::new(Style::Hunk, "@@ -1,3 +1,3 @@")]);
    // Context is ordinary text, where the header detail above it recedes,
    // and the changed lines carry only their edit in the harder tint.
    assert_eq!(rows[5], vec![Span::new(Style::Normal, " fn main() {")]);
    assert_eq!(
        rows[6],
        vec![
            Span::new(Style::Removed, "-    "),
            Span::new(Style::RemovedEdit, "old"),
            Span::new(Style::Removed, "();"),
        ]
    );
    assert_eq!(
        rows[7],
        vec![
            Span::new(Style::Added, "+    "),
            Span::new(Style::AddedEdit, "new"),
            Span::new(Style::Added, "();"),
        ]
    );
}

#[test]
fn test_a_shows_commit_detail_stays_plain_until_its_first_hunk() {
    // `git show` answers with commit metadata above the diff, which stays
    // plain: nothing is a diff's body until a hunk opens.
    let lines: Vec<String> = "commit abc123\nAuthor: A <a@b>\n\n    fix the thing\n\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new\n"
        .lines()
        .map(str::to_string)
        .collect();
    let (rows, _) = diff_rows(&lines);
    assert_eq!(rows[0], vec![Span::plain("commit abc123")]);
    assert_eq!(rows[3], vec![Span::plain("    fix the thing")]);
    assert_eq!(
        rows[9],
        vec![
            Span::new(Style::Removed, "-"),
            Span::new(Style::RemovedEdit, "old"),
        ]
    );
}
