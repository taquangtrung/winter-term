//! Corrections for an unknown word: dictionary stems a short edit away, and
//! a hyphenated split when the word is two known words run together.
//!
//! It runs only when a correction is asked for, so a bounded scan suffices.

use super::dictionary::Dictionary;

// ========================================================================
// Constants
// ========================================================================

/// How many single edits (insert, delete, substitute, swap) a suggestion
/// may be from the word.
const MAX_EDIT_DISTANCE: usize = 2;

/// The shortest half a compound split may have, so short typos are not
/// answered with "a-ttic".
const MIN_COMPOUND_HALF_LENGTH: usize = 2;

/// How many suggestions a correction menu usually shows.
pub const DEFAULT_SUGGESTION_LIMIT: usize = 5;

// ========================================================================
// Suggesting
// ========================================================================

/// Up to `limit` corrections for `word`, lowercase. A compound split comes
/// first, then stems sharing its first letter by edit distance; among
/// equals, the one nearest the word's length, then one of the very letters
/// the word has (`the` for `teh`, two of them swapped), then alphabetically.
pub(crate) fn suggest(dictionary: &Dictionary, word: &str, limit: usize) -> Vec<String> {
    let lower: Vec<char> = word.to_lowercase().chars().collect();
    let Some(&first) = lower.first() else {
        return Vec::new();
    };
    let min_length = lower.len().saturating_sub(MAX_EDIT_DISTANCE).max(1);
    let max_length = lower.len() + MAX_EDIT_DISTANCE;

    let mut letters = lower.clone();
    letters.sort_unstable();
    let mut ranked: Vec<(usize, usize, bool, &str)> = dictionary
        .stems()
        .filter(|stem| stem.starts_with(first))
        .filter_map(|stem| {
            let candidate: Vec<char> = stem.chars().collect();
            if !(min_length..=max_length).contains(&candidate.len()) {
                return None;
            }
            let distance = damerau_levenshtein_distance(&lower, &candidate);
            let mut theirs = candidate.clone();
            theirs.sort_unstable();
            (1..=MAX_EDIT_DISTANCE).contains(&distance).then_some((
                distance,
                candidate.len().abs_diff(lower.len()),
                theirs != letters,
                stem,
            ))
        })
        .collect();
    ranked.sort_unstable();

    let compound = split_compound(dictionary, &lower);
    compound
        .into_iter()
        .chain(ranked.into_iter().map(|(_, _, _, stem)| stem.to_string()))
        .take(limit)
        .collect()
}

/// `word` rejoined with a hyphen when it is exactly two known words run
/// together ("writeup" to "write-up"); three-way splits are not tried.
fn split_compound(dictionary: &Dictionary, word: &[char]) -> Option<String> {
    let last_split = word.len().checked_sub(MIN_COMPOUND_HALF_LENGTH)?;
    (MIN_COMPOUND_HALF_LENGTH..=last_split).find_map(|split_at| {
        let first: String = word[..split_at].iter().collect();
        let second: String = word[split_at..].iter().collect();
        (dictionary.check(&first) && dictionary.check(&second)).then(|| format!("{first}-{second}"))
    })
}

/// Edit distance counting an adjacent swap as one edit, alongside insert,
/// delete, and substitute (the restricted Damerau-Levenshtein distance).
fn damerau_levenshtein_distance(a: &[char], b: &[char]) -> usize {
    let cols = b.len() + 1;
    let mut table = vec![vec![0; cols]; a.len() + 1];
    for (i, row) in table.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in table[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut distance = (table[i - 1][j] + 1)
                .min(table[i][j - 1] + 1)
                .min(table[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                distance = distance.min(table[i - 2][j - 2] + cost);
            }
            table[i][j] = distance;
        }
    }
    table[a.len()][b.len()]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary_of(words: &[&str]) -> Dictionary {
        let mut dictionary = Dictionary::empty();
        for word in words {
            dictionary.add_word(word);
        }
        dictionary
    }

    fn chars(word: &str) -> Vec<char> {
        word.chars().collect()
    }

    #[test]
    fn test_distance_counts_swap_as_one_edit() {
        assert_eq!(
            damerau_levenshtein_distance(&chars("teh"), &chars("the")),
            1
        );
        assert_eq!(
            damerau_levenshtein_distance(&chars("aple"), &chars("apples")),
            2
        );
        assert_eq!(
            damerau_levenshtein_distance(&chars("aple"), &chars("avoid")),
            4
        );
    }

    #[test]
    fn test_suggest_finds_single_edit_typo() {
        let dictionary = dictionary_of(&["apple", "apply", "banana"]);
        assert!(
            suggest(&dictionary, "appli", DEFAULT_SUGGESTION_LIMIT).contains(&"apply".to_string())
        );
    }

    #[test]
    fn test_suggest_ranks_closer_candidate_first() {
        let dictionary = dictionary_of(&["apples", "apple"]);
        assert_eq!(
            suggest(&dictionary, "aple", DEFAULT_SUGGESTION_LIMIT)[0],
            "apple"
        );
    }

    #[test]
    fn test_suggest_excludes_candidates_beyond_max_distance() {
        let dictionary = dictionary_of(&["apple", "avoid"]);
        let suggestions = suggest(&dictionary, "aple", DEFAULT_SUGGESTION_LIMIT);
        assert!(suggestions.contains(&"apple".to_string()));
        assert!(!suggestions.contains(&"avoid".to_string()));
    }

    #[test]
    fn test_suggest_puts_a_swap_of_the_word_s_own_letters_first() {
        let dictionary = dictionary_of(&["te", "th", "tea", "ted", "tee", "the"]);
        assert_eq!(suggest(&dictionary, "teh", 1), ["the"]);
    }

    #[test]
    fn test_suggest_puts_compound_split_ahead_of_edits() {
        let dictionary = dictionary_of(&["write", "up", "writeout"]);
        let suggestions = suggest(&dictionary, "writeup", DEFAULT_SUGGESTION_LIMIT);
        assert_eq!(suggestions[0], "write-up");
        assert!(suggestions.contains(&"writeout".to_string()));
    }

    #[test]
    fn test_suggest_honours_limit_including_compound() {
        let dictionary = dictionary_of(&["write", "up", "writeout"]);
        assert_eq!(suggest(&dictionary, "writeup", 1), ["write-up"]);
    }
}
