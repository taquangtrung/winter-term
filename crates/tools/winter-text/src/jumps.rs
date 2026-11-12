//! The lines a tab offers Scout to jump between, and their syntax colors:
//! every line with text on it, by its number, colored as the editor colors
//! it, the grammar walked from the top in the time a frame gives.

use std::path::Path;
use std::time::{Duration, Instant};

use winter_model::editor::{JumpTarget, TargetStyle, TargetStyles};
use winter_model::paint::{Style, StyleRun};
use winter_syntax::LineState;

use super::language::grammar_of;

// ========================================================================
// Functions
// ========================================================================

/// The lines of `lines` to jump between, each labeled with its number:
/// those with text on them, trimmed, or every line of a file of blanks.
pub(super) fn targets(lines: &[String]) -> Vec<JumpTarget> {
    let label =
        |index: usize, text: &str| JumpTarget::new(index, format!("{:>5}  {text}", index + 1));
    let mut targets: Vec<JumpTarget> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| label(index, line.trim()))
        .collect();
    if targets.is_empty() {
        targets = lines
            .iter()
            .enumerate()
            .map(|(index, line)| label(index, line))
            .collect();
    }
    targets
}

/// The syntax colors of the `wanted` lines of the file at `path`, as many
/// as `budget` reaches, each as runs over its text past the indentation a
/// target's label trims. `walked` keeps where each line begins from one
/// call to the next, the lines walked in one frame not walked again in the
/// next; the caller empties it when the text changes.
pub(super) fn styles(
    path: &Path,
    lines: &[String],
    wanted: &[usize],
    budget: Duration,
    walked: &mut Vec<LineState>,
) -> TargetStyles {
    let Some(grammar) = grammar_of(path) else {
        return TargetStyles::default();
    };
    let started = Instant::now();
    if walked.is_empty() {
        walked.push(LineState::start(grammar));
    }
    let mut styles = Vec::new();
    let mut pending = false;
    // The budget is for the lines between the ones shown, which only say
    // where the next begins; each call walks a line at least, so one that
    // spends its time loading the grammars still gets on.
    let mut walked_one = false;
    for &index in wanted {
        while walked.len() <= index && walked.len() <= lines.len() {
            let at = walked.len() - 1;
            if walked_one && !wanted.contains(&at) && started.elapsed() >= budget {
                pending = true;
                break;
            }
            let next = winter_syntax::next_state(&lines[at], &walked[at]);
            walked.push(next);
            walked_one = true;
        }
        let (Some(line), Some(state)) = (lines.get(index), walked.get(index)) else {
            continue;
        };
        let (colors, _) = winter_syntax::highlight(line, state);
        let indent = line.chars().take_while(|c| c.is_whitespace()).count();
        styles.push(TargetStyle {
            runs: runs_of(&colors[indent.min(colors.len())..]),
            target: index,
        });
    }
    TargetStyles { pending, styles }
}

/// `colors`, a style a char, as runs of one style.
fn runs_of(colors: &[Style]) -> Vec<StyleRun> {
    let mut runs: Vec<StyleRun> = Vec::new();
    for style in colors {
        match runs.last_mut() {
            Some(run) if run.style == *style => run.chars += 1,
            _ => runs.push(StyleRun {
                chars: 1,
                style: *style,
            }),
        }
    }
    runs
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blank_lines_are_skipped_and_a_target_is_colored_past_its_indent() {
        let lines: Vec<String> = ["fn main() {", "", "    let x = 1;", "}"]
            .map(String::from)
            .to_vec();
        let shown: Vec<usize> = targets(&lines).iter().map(|target| target.target).collect();
        assert_eq!(shown, [0, 2, 3], "the blank line has nothing to jump to");
        let colored = styles(
            Path::new("a.rs"),
            &lines,
            &[2],
            Duration::from_secs(5),
            &mut Vec::new(),
        );
        assert!(!colored.pending);
        let runs = &colored.styles[0].runs;
        let chars: usize = runs.iter().map(|run| run.chars).sum();
        assert_eq!(
            chars,
            "let x = 1;".len(),
            "the indent trimmed, as the label is"
        );
        assert!(runs.len() > 1, "a keyword colored apart from the rest");
    }

    #[test]
    fn test_a_walk_cut_short_by_its_budget_goes_on_where_it_stopped() {
        let lines = vec!["let x = 1;".to_owned(); 200];
        let mut walked = Vec::new();
        let path = Path::new("a.rs");
        let first = styles(path, &lines, &[199], Duration::ZERO, &mut walked);
        assert!(first.pending && first.styles.is_empty(), "no time at all");
        assert_eq!(walked.len(), 2, "but a line walked all the same");
        let mut rounds = 0;
        while styles(
            path,
            &lines,
            &[199],
            Duration::from_micros(200),
            &mut walked,
        )
        .pending
        {
            rounds += 1;
            assert!(rounds < 10_000, "each round walks further");
        }
        assert_eq!(walked.len(), 200, "every line above the last walked once");
    }
}
