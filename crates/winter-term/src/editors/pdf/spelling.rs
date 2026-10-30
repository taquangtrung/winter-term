//! The words of a page that are spelled wrong, by the line and the
//! characters each covers, for the viewer to underline them and step from
//! one to the next.
//!
//! Typesetting breaks a long word over two lines with a hyphen, and the
//! halves (`differ-` and `ent`) are no words: a word broken so is checked
//! whole, and its halves are let be when it is spelled right.

use winter_spell::{Dictionary, WordToken};

use super::sentences::Run;
use super::text::PageText;

// ========================================================================
// Functions
// ========================================================================

/// The words of `text` that `dictionary` does not know, down the page.
pub fn misspelled(text: &PageText, dictionary: &Dictionary) -> Vec<Run> {
    let lines: Vec<String> = text
        .lines
        .iter()
        .map(|line| line.text.iter().collect())
        .collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let tokens = dictionary.tokenize(line);
        for token in dictionary.misspellings(line) {
            if rejoined(&lines, index, &tokens, &token, dictionary) {
                continue;
            }
            found.push(Run {
                from: line[..token.start].chars().count(),
                line: index,
                to: line[..token.end].chars().count().saturating_sub(1),
            });
        }
    }
    found
}

/// Whether `token`, a word of line `index` that `tokens` are all the words
/// of, is half of a word broken over a line's end, the whole of which is
/// spelled right.
fn rejoined(
    lines: &[String],
    index: usize,
    tokens: &[WordToken],
    token: &WordToken,
    dictionary: &Dictionary,
) -> bool {
    let is = |other: Option<&WordToken>| other.is_some_and(|other| other.start == token.start);
    // The first half: the line's last word, a hyphen straight after it.
    if is(tokens.last()) && lines[index][token.end..].trim_end() == "-" {
        let next = lines.get(index + 1).map(|next| dictionary.tokenize(next));
        if let Some(rest) = next.as_ref().and_then(|tokens| tokens.first()) {
            return dictionary.check(&format!("{}{}", token.word, rest.word));
        }
    }
    // The second half: the line's first word, after a line ending in one.
    if is(tokens.first()) && index > 0 {
        let before = &lines[index - 1];
        let start = dictionary.tokenize(before).pop();
        if let Some(start) = start.filter(|start| before[start.end..].trim_end() == "-") {
            return dictionary.check(&format!("{}{}", start.word, token.word));
        }
    }
    false
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_spell::DictionaryId;

    use super::super::text::tests::words;
    use super::*;

    fn page(lines: &[&str]) -> PageText {
        let glyphs = lines
            .iter()
            .enumerate()
            .flat_map(|(row, line)| words(line, 40.0, 60.0 + row as f32 * 14.0, 10.0))
            .collect();
        PageText::new(glyphs)
    }

    /// The words of `lines` found misspelled.
    fn found(lines: &[&str]) -> Vec<String> {
        let text = page(lines);
        let dictionary = Dictionary::bundled(DictionaryId::EnUs);
        misspelled(&text, &dictionary)
            .iter()
            .map(|run| text.lines[run.line].text[run.from..=run.to].iter().collect())
            .collect()
    }

    #[test]
    fn test_a_misspelled_word_is_found_by_the_characters_it_covers() {
        assert_eq!(found(&["We recieve the", "letter and teh reply."]), ["recieve", "teh"]);
    }

    #[test]
    fn test_a_word_broken_over_a_line_is_checked_whole() {
        assert!(found(&["the results are differ-", "ent from ours"]).is_empty());
        // Broken or not, a word spelled wrong is still wrong.
        assert_eq!(found(&["the results are diffr-", "ent from ours"]), ["diffr", "ent"]);
    }
}
