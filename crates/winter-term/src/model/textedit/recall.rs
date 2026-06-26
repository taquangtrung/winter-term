//! Earlier answers to the same question, for a prompt to bring back with the
//! arrow keys: what was searched for last, the name last given.

use std::collections::HashMap;

// ========================================================================
// Constants
// ========================================================================

/// How many answers each question keeps, oldest dropped first.
const ANSWERS_KEPT: usize = 50;

// ========================================================================
// Data Structures
// ========================================================================

/// The answers given to each question, oldest first, each kept once.
#[derive(Clone, Debug, Default)]
pub struct Answers {
    by_question: HashMap<&'static str, Vec<String>>,
}

/// Where a prompt has got to walking back through its question's answers,
/// and what was typed before the walk began, to come back to at the end.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Recall {
    draft: String,
    /// The answer showing, or `None` for what was being typed.
    index: Option<usize>,
}

// ========================================================================
// Answers
// ========================================================================

impl Answers {
    /// Keep `answer` to `question` as its newest, moving it there if it was
    /// given before. An empty answer is nothing to bring back.
    pub fn record(&mut self, question: &'static str, answer: &str) {
        if answer.is_empty() {
            return;
        }
        let answers = self.by_question.entry(question).or_default();
        answers.retain(|given| given != answer);
        answers.push(answer.to_string());
        if answers.len() > ANSWERS_KEPT {
            answers.remove(0);
        }
    }

    /// The answers given to `question`, oldest first.
    pub fn of(&self, question: &str) -> &[String] {
        self.by_question.get(question).map_or(&[], Vec::as_slice)
    }
}

// ========================================================================
// Recall
// ========================================================================

impl Recall {
    /// The answer before the one showing, keeping `typed` to come back to
    /// when the walk leaves it. `None` where there is nothing older.
    pub fn older(&mut self, answers: &[String], typed: &str) -> Option<String> {
        let index = match self.index {
            None if answers.is_empty() => return None,
            None => {
                self.draft = typed.to_string();
                answers.len() - 1
            }
            Some(0) => return None,
            Some(at) => at - 1,
        };
        self.index = Some(index);
        answers.get(index).cloned()
    }

    /// The answer after the one showing, or what was being typed after the
    /// newest. `None` where the walk has not begun.
    pub fn newer(&mut self, answers: &[String]) -> Option<String> {
        let at = self.index?;
        match at + 1 < answers.len() {
            true => {
                self.index = Some(at + 1);
                answers.get(at + 1).cloned()
            }
            false => {
                self.index = None;
                Some(std::mem::take(&mut self.draft))
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_walk_goes_back_through_answers_and_returns_to_what_was_typed() {
        let mut answers = Answers::default();
        answers.record("search", "alpha");
        answers.record("search", "beta");
        answers.record("search", "alpha");
        answers.record("rename", "other");
        let given = answers.of("search");
        assert_eq!(given, ["beta", "alpha"], "a repeat moves to the newest");

        let mut recall = Recall::default();
        assert_eq!(recall.newer(given), None, "nothing newer before the walk");
        assert_eq!(recall.older(given, "draf").as_deref(), Some("alpha"));
        assert_eq!(recall.older(given, "alpha").as_deref(), Some("beta"));
        assert_eq!(recall.older(given, "beta"), None, "nothing older than the oldest");
        assert_eq!(recall.newer(given).as_deref(), Some("alpha"));
        assert_eq!(recall.newer(given).as_deref(), Some("draf"), "back to the draft");
    }
}
