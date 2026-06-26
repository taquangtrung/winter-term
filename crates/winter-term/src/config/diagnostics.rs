//! Reporting problems found while reading the configuration.

// ========================================================================
// Diagnostics
// ========================================================================

/// One problem found in a config file: its 1-based `line` and a human `message`.
pub(crate) struct ConfigProblem {
    pub(crate) line: usize,
    pub(crate) message: String,
}
/// Pull the individual problems out of a KDL parse error so each can be
/// reported on its own line. The KDL parser collects every issue as a "related"
/// diagnostic with span info; this surfaces them with 1-based line numbers
/// located against `text`. Falls back to the top-level message when no related
/// diagnostics are available.
pub(crate) fn config_problems(err: &kdl::de::Error, text: &str) -> Vec<ConfigProblem> {
    use miette::Diagnostic;

    let related: Vec<&dyn Diagnostic> = err
        .related()
        .map(|problems| problems.collect())
        .unwrap_or_default();

    // No related diagnostics means the file failed to tokenize at all; report the
    // whole error as a single problem rather than dropping it silently.
    if related.is_empty() {
        return vec![ConfigProblem {
            line: 1,
            message: err.to_string(),
        }];
    }

    related
        .into_iter()
        .map(|problem| {
            let offset = problem
                .labels()
                .and_then(|mut labels| labels.next())
                .map(|label| label.offset())
                .unwrap_or(0);
            ConfigProblem {
                line: line_number(text, offset),
                message: problem.to_string(),
            }
        })
        .collect()
}
/// The 1-based line number containing byte `offset` within `text`.
pub(crate) fn line_number(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    #[test]
    fn test_config_problems_report_unknown_key() {
        let text = "theme \"dark\"\nfont-weigth \"300\"\n";
        let err = kdl::de::from_str::<KdlConfig>(text)
            .err()
            .expect("unknown key should fail to parse");
        let problems = config_problems(&err, text);
        assert!(
            !problems.is_empty(),
            "unknown key produces at least one problem"
        );
    }
}
