//! An SSH configuration's outline, the client's or the server's: each
//! `Host` and `Match` block, under which the settings that follow apply.
//! A setting is left out.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Constants
// ========================================================================

/// The keywords that open a block, as SSH reads them, in any case.
const BLOCKS: [&str; 2] = ["host", "match"];

// ========================================================================
// Functions
// ========================================================================

/// The outline of the SSH configuration `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(row, line)| {
            let text = line.trim();
            let keyword = text
                .split(|c: char| c.is_whitespace() || c == '=')
                .next()
                .unwrap_or_default();
            let opens = BLOCKS
                .iter()
                .any(|block| block.eq_ignore_ascii_case(keyword));
            (opens && keyword.len() < text.len()).then(|| entry(row, 0, text))
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{labels, lines};

    #[test]
    fn test_host_and_match_blocks_are_listed_in_any_case_and_settings_are_not() {
        let doc = lines(concat!(
            "# Host commented\n",
            "Host github.com\n",
            "    User git\n",
            "    HostName github.com\n",
            "match host *.internal\n",
            "Host=box\n",
            "Host\n",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    2  Host github.com",
                "    5  match host *.internal",
                "    6  Host=box",
            ]
        );
    }
}
