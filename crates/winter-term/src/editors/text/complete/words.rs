//! Words: every word already in the buffer, nearest the cursor first, then
//! those in the other open buffers, as Emacs' dabbrev finds them.

use super::{is_word_char, Candidate, Context, Kind, Reply};

/// The shortest word worth offering.
const MIN_WORD: usize = 2;

/// The most words read out of the buffers, past which a file is a data dump
/// rather than prose or code.
const MAX_WORDS: usize = 5000;

/// The words of the buffers, the one at the cursor left out.
pub(crate) fn complete(context: &Context) -> Option<Reply> {
    let start = context.word_start();
    if start == context.col {
        return None;
    }
    let mut order: Vec<(usize, String, bool)> = Vec::new();
    for (row, line) in context.lines.iter().enumerate() {
        for (col, word) in words_of(line) {
            if row == context.row && col == start {
                continue;
            }
            order.push((row.abs_diff(context.row), word, true));
        }
    }
    order.sort_by_key(|(distance, _, _)| *distance);
    for lines in context.others {
        for line in lines.iter() {
            order.extend(words_of(line).map(|(_, word)| (usize::MAX, word, false)));
        }
    }
    let candidates = order
        .into_iter()
        .take(MAX_WORDS)
        .map(|(_, word, here)| Candidate {
            detail: None,
            insert: word.clone(),
            kind: Kind::Word,
            label: word,
            source: if here { "buffer" } else { "buffers" },
        })
        .collect();
    Some(Reply { candidates, start })
}

/// Each word of `line` and the column it starts at.
fn words_of(line: &str) -> impl Iterator<Item = (usize, String)> + '_ {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if !is_word_char(chars[at]) {
            at += 1;
            continue;
        }
        let begin = at;
        while at < chars.len() && is_word_char(chars[at]) {
            at += 1;
        }
        if at - begin >= MIN_WORD && !chars[begin].is_ascii_digit() {
            found.push((begin, chars[begin..at].iter().collect()));
        }
    }
    found.into_iter()
}

#[cfg(test)]
mod tests {
    use super::super::tests::context;
    use super::*;

    #[test]
    fn test_words_nearest_the_cursor_come_first_and_the_one_typed_is_left_out() {
        let lines = vec!["alpha".to_string(), "beta al".to_string(), "almond 42".to_string()];
        let context = super::super::Context { row: 1, ..context("beta al", &lines, "text") };
        let reply = complete(&context).expect("a word is being typed");
        assert_eq!(reply.start, 5);
        let labels: Vec<&str> = reply.candidates.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["beta", "alpha", "almond"]);
    }
}
