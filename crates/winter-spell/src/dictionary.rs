//! A word list for checking: the stems of a bundled Hunspell dictionary
//! with their affix flags, plus words the user added outright.
//!
//! Lookups ignore case; a word is known as a stem or as one stem with a
//! prefix, a suffix, or one of each.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

use super::affix::{AffixRules, FlagSet};
use super::extract::{extract_prose_text, Prose};
use super::suggest::suggest;
use super::tokenizer::{should_check_token, tokenize_with, TokenizeContext, WordToken};

// ========================================================================
// Constants
// ========================================================================

/// American English: the `.aff` rules and `.dic` stems.
const EN_US_AFF: &str = include_str!("../data/en-us.aff");
const EN_US_DIC: &str = include_str!("../data/en-us.dic");

/// British English: the `.aff` rules and `.dic` stems.
const EN_GB_AFF: &str = include_str!("../data/en-gb.aff");
const EN_GB_DIC: &str = include_str!("../data/en-gb.dic");

/// English combining-form prefixes ("pre", "non"), shared by every variant.
const EN_HYPHEN_PREFIXES: &str = include_str!("../data/en-hyphen-prefixes.txt");

/// The licence the bundled dictionary data ships under, for an about page.
pub const DICTIONARY_LICENSE: &str = include_str!("../data/dictionaries.LICENSE.txt");

/// Opens a comment line in the plain word lists.
const COMMENT_MARK: char = '#';

/// Joins the halves of a contraction or possessive in a `.dic` entry.
const APOSTROPHE: char = '\'';

/// Separates the language and region of a dictionary id.
const ID_SEPARATOR: &str = "-";

/// Accepted in place of the id separator, as in locale names.
const ID_SEPARATOR_ALTERNATIVE: char = '_';

/// Separates a `.dic` entry's word from its flags.
const FLAG_SEPARATOR: char = '/';

/// The id every bundled variant is named by, for a settings value.
const EN_US_ID: &str = "en-us";
const EN_GB_ID: &str = "en-gb";

// ========================================================================
// Data Structures
// ========================================================================

/// Which bundled dictionary to load.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum DictionaryId {
    /// British English (`en-gb`).
    EnGb,
    /// American English (`en-us`).
    #[default]
    EnUs,
}

/// A settings value that names no bundled dictionary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownDictionaryId {
    /// The value as given.
    pub id: String,
}

/// A loaded word list. Build it once and share it: loading parses about
/// fifty thousand stems.
#[derive(Debug, Default)]
pub struct Dictionary {
    hyphen_prefixes: HashSet<String>,
    rules: AffixRules,
    /// Every stem, lowercase, with the affix flags it carries.
    stems: HashMap<String, FlagSet>,
}

// ========================================================================
// Dictionary
// ========================================================================

impl Dictionary {
    // --------------------------------------------------------------------
    // Construction
    // --------------------------------------------------------------------

    /// The bundled dictionary `id`, parsed from data built into the binary.
    pub fn bundled(id: DictionaryId) -> Self {
        let (aff, dic, hyphen_prefixes) = id.data();
        Self::from_hunspell(aff, dic, hyphen_prefixes)
    }

    /// A dictionary with no words or rules, for words added one by one.
    pub fn empty() -> Self {
        Self::default()
    }

    fn from_hunspell(aff: &str, dic: &str, hyphen_prefixes: &str) -> Self {
        Self {
            hyphen_prefixes: word_list_lines(hyphen_prefixes)
                .map(str::to_lowercase)
                .collect(),
            rules: AffixRules::parse(aff),
            stems: parse_dictionary(dic),
        }
    }

    // --------------------------------------------------------------------
    // Words
    // --------------------------------------------------------------------

    /// Makes `word` known outright, in any case; words are never removed.
    pub fn add_word(&mut self, word: &str) {
        self.stems.entry(word.to_lowercase()).or_default();
    }

    /// Whether `word` is spelled correctly, ignoring case.
    pub fn check(&self, word: &str) -> bool {
        let lower = word.to_lowercase();
        self.stems.contains_key(&lower) || self.is_derived_word(&lower)
    }

    // --------------------------------------------------------------------
    // Text
    // --------------------------------------------------------------------

    /// Up to `limit` corrections for `word`, lowercase, best first.
    pub fn suggest(&self, word: &str, limit: usize) -> Vec<String> {
        suggest(self, word, limit)
    }

    /// The words of `text`, with this dictionary's hyphen prefixes dropped
    /// and its known hyphenated compounds kept whole.
    pub fn tokenize(&self, text: &str) -> Vec<WordToken> {
        let context = TokenizeContext {
            hyphen_prefixes: &self.hyphen_prefixes,
            is_known_word: |word: &str| self.check(word),
        };
        tokenize_with(text, &context)
    }

    /// The words of `text` worth checking that this dictionary rejects.
    pub fn misspellings(&self, text: &str) -> Vec<WordToken> {
        self.tokenize(text)
            .into_iter()
            .filter(|token| should_check_token(&token.word) && !self.check(&token.word))
            .collect()
    }

    /// The words of `text`, read as `prose`, that this dictionary rejects:
    /// only its prose is checked, and offsets are into `text` itself.
    pub fn misspellings_in(&self, text: &str, prose: Prose) -> Vec<WordToken> {
        let extracted = extract_prose_text(text, prose);
        self.misspellings(extracted.text())
            .into_iter()
            .map(|token| {
                let original = extracted.original_range(token.start..token.end);
                WordToken {
                    end: original.end,
                    start: original.start,
                    word: token.word,
                }
            })
            .collect()
    }

    /// Every stem, lowercase, in no particular order.
    pub(crate) fn stems(&self) -> impl Iterator<Item = &str> {
        self.stems.keys().map(String::as_str)
    }

    // --------------------------------------------------------------------
    // Derivation
    // --------------------------------------------------------------------

    /// Whether `word` (lowercase) is a stem with one prefix, one suffix,
    /// or a cross-product pair of both; chained affixes are not tried.
    fn is_derived_word(&self, word: &str) -> bool {
        let carries = |stem: Option<String>, flag: char| {
            stem.and_then(|stem| self.stems.get(&stem).copied())
                .is_some_and(|flags| flags.contains(flag))
        };
        let by_suffix = self.rules.suffixes.iter().any(|set| {
            set.rules
                .iter()
                .any(|rule| carries(rule.strip_suffix(word), set.flag))
        });
        let by_prefix = || {
            self.rules.prefixes.iter().any(|set| {
                set.rules
                    .iter()
                    .any(|rule| carries(rule.strip_prefix(word), set.flag))
            })
        };
        by_suffix || by_prefix() || self.is_cross_product_word(word)
    }

    fn is_cross_product_word(&self, word: &str) -> bool {
        let mut prefix_sets = self.rules.prefixes.iter().filter(|set| set.cross_product);
        let suffix_sets = || self.rules.suffixes.iter().filter(|set| set.cross_product);
        prefix_sets.any(|prefix_set| {
            prefix_set
                .rules
                .iter()
                .filter_map(|rule| rule.strip_prefix(word))
                .any(|inner| {
                    suffix_sets().any(|suffix_set| {
                        suffix_set
                            .rules
                            .iter()
                            .filter_map(|rule| rule.strip_suffix(&inner))
                            .filter_map(|stem| self.stems.get(&stem))
                            .any(|flags| {
                                flags.contains(prefix_set.flag) && flags.contains(suffix_set.flag)
                            })
                    })
                })
        })
    }
}

// ========================================================================
// DictionaryId
// ========================================================================

impl DictionaryId {
    /// Every bundled dictionary, for a picker.
    pub const ALL: [Self; 2] = [Self::EnUs, Self::EnGb];

    /// The id a settings file names this dictionary by.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnGb => EN_GB_ID,
            Self::EnUs => EN_US_ID,
        }
    }

    /// The `.aff` and `.dic` texts, and the hyphen prefixes of its language.
    fn data(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::EnGb => (EN_GB_AFF, EN_GB_DIC, EN_HYPHEN_PREFIXES),
            Self::EnUs => (EN_US_AFF, EN_US_DIC, EN_HYPHEN_PREFIXES),
        }
    }
}

impl fmt::Display for DictionaryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DictionaryId {
    type Err = UnknownDictionaryId;

    /// Reads "en-us" or "en-gb", in any case and with `_` for `-`.
    fn from_str(id: &str) -> Result<Self, Self::Err> {
        let normalized = id
            .trim()
            .to_ascii_lowercase()
            .replace(ID_SEPARATOR_ALTERNATIVE, ID_SEPARATOR);
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == normalized)
            .ok_or_else(|| UnknownDictionaryId { id: id.to_string() })
    }
}

// ========================================================================
// UnknownDictionaryId
// ========================================================================

impl fmt::Display for UnknownDictionaryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown dictionary \"{}\"", self.id)
    }
}

impl Error for UnknownDictionaryId {}

// ========================================================================
// Helpers
// ========================================================================

/// Every `.dic` stem, lowercase, with its flags merged across duplicate
/// entries. Entries other than letters with internal apostrophes (the
/// ordinal compounds such as `0th`, and the leading count) are skipped.
fn parse_dictionary(dic: &str) -> HashMap<String, FlagSet> {
    let mut stems: HashMap<String, FlagSet> = HashMap::with_capacity(dic.lines().count());
    for line in dic.lines() {
        let entry = line.trim();
        let (word, flags) = entry.split_once(FLAG_SEPARATOR).unwrap_or((entry, ""));
        if !is_dictionary_word(word) {
            continue;
        }
        let flags = FlagSet::parse(flags);
        stems
            .entry(word.to_ascii_lowercase())
            .and_modify(|existing| *existing = existing.union(flags))
            .or_insert(flags);
    }
    stems
}

/// ASCII letters, with apostrophes allowed only between two letters.
fn is_dictionary_word(word: &str) -> bool {
    !word.is_empty()
        && word
            .split(APOSTROPHE)
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphabetic()))
}

/// The non-blank, non-comment lines of a plain word list, trimmed.
fn word_list_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(COMMENT_MARK))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;
    use std::time::Instant;

    use super::super::extract::CommentStyle;
    use super::*;

    /// One suffix (with an e-stripping and an e-keeping rule) and one
    /// prefix, both cross-product, over stems carrying both, one, or none.
    const SYNTHETIC_AFF: &str = "SFX G Y 2\nSFX G   e     ing        e\n\
                                 SFX G   0     ing        [^e]\n\
                                 PFX U Y 1\nPFX U   0     un         .\n";
    const SYNTHETIC_DIC: &str = "3\nclean/GU\nlike/G\njump\n";

    fn synthetic() -> Dictionary {
        Dictionary::from_hunspell(SYNTHETIC_AFF, SYNTHETIC_DIC, "")
    }

    fn en_us() -> &'static Dictionary {
        static DICTIONARY: OnceLock<Dictionary> = OnceLock::new();
        DICTIONARY.get_or_init(|| Dictionary::bundled(DictionaryId::EnUs))
    }

    fn en_gb() -> &'static Dictionary {
        static DICTIONARY: OnceLock<Dictionary> = OnceLock::new();
        DICTIONARY.get_or_init(|| Dictionary::bundled(DictionaryId::EnGb))
    }

    fn assert_known(dictionary: &Dictionary, words: &[&str]) {
        for word in words {
            assert!(dictionary.check(word), "expected \"{word}\" to be known");
        }
    }

    fn assert_unknown(dictionary: &Dictionary, words: &[&str]) {
        for word in words {
            assert!(!dictionary.check(word), "expected \"{word}\" to be unknown");
        }
    }

    #[test]
    fn test_dictionary_id_parses_settings_values() {
        assert_eq!("en-us".parse(), Ok(DictionaryId::EnUs));
        assert_eq!(" EN_GB ".parse(), Ok(DictionaryId::EnGb));
        assert!("fr-fr".parse::<DictionaryId>().is_err());
    }

    #[test]
    fn test_added_words_are_known_in_any_case() {
        let mut dictionary = Dictionary::empty();
        dictionary.add_word("Grammarly");
        assert_known(&dictionary, &["grammarly", "GRAMMARLY"]);
        assert_unknown(&dictionary, &["unknownxyz"]);
    }

    #[test]
    fn test_affix_derivation_on_synthetic_rules() {
        let dictionary = synthetic();
        assert_known(
            &dictionary,
            &["clean", "cleaning", "liking", "unclean", "uncleaning"],
        );
        // "jump" exists without the G flag; "frob" does not exist at all.
        assert_unknown(&dictionary, &["jumping", "unfrobbing", "liken"]);
    }

    #[test]
    fn test_bundled_load_time() {
        let started = Instant::now();
        let dictionary = Dictionary::bundled(DictionaryId::EnUs);
        let elapsed = started.elapsed();
        println!(
            "en-us: {} stems loaded in {elapsed:?}",
            dictionary.stems.len()
        );
        assert!(dictionary.stems.len() > 40_000);
    }

    #[test]
    fn test_bundled_hyphen_prefixes() {
        for dictionary in [en_us(), en_gb()] {
            assert!(dictionary.hyphen_prefixes.contains("pre"));
            assert!(!dictionary.hyphen_prefixes.contains("zzzznotaprefix"));
        }
    }

    #[test]
    fn test_bundled_contractions_are_known() {
        assert_known(
            en_us(),
            &["they're", "isn't", "don't", "o'clock", "They're"],
        );
        assert_unknown(en_us(), &["zzzzn't"]);
        assert_known(en_gb(), &["they're", "isn't"]);
    }

    #[test]
    fn test_bundled_variants_stay_regional() {
        assert_known(
            en_us(),
            &["hello", "color", "auditing", "linting", "linter", "linters"],
        );
        assert_known(en_us(), &["pseudocode", "profiler", "callee", "caller"]);
        assert_unknown(en_us(), &["colour", "impl", "zzzznotarealword"]);
        assert_known(en_gb(), &["colour", "substring"]);
        assert_unknown(en_gb(), &["color"]);

        assert_known(
            en_us(),
            &[
                "quantized",
                "quantizing",
                "quantizer",
                "quantizations",
                "regularization",
            ],
        );
        assert_unknown(en_us(), &["quantised"]);
        assert_known(
            en_gb(),
            &[
                "quantised",
                "quantising",
                "quantiser",
                "quantisations",
                "regularisation",
            ],
        );
        assert_unknown(en_gb(), &["quantized"]);
    }

    #[test]
    fn test_bundled_derived_forms() {
        assert_known(
            en_us(),
            &[
                "prover",
                "unhandled",
                "reimplementing",
                "reimplemented",
                "reimplements",
                "entailments",
                "invariants",
                "recurse",
                "recurses",
                "sharding",
                "submodule",
                "submodules",
                "subprocess",
                "substring",
                "substrings",
                "transpile",
                "transpiler",
                "transpiling",
                "allocator",
                "tokenize",
                "tokenizer",
                "tokenized",
                "tokenization",
                "tokenizations",
                "breakpoint",
                "breakpoints",
                "formatter",
                "namespace",
                "namespaces",
                "roadmap",
                "roadmaps",
                "unencodable",
                "encodable",
                "untidied",
                "renderer",
                "summarizer",
                "unprimed",
                "instantiation",
                "reborrowed",
                "reborrowing",
                "subtyping",
                "subtype",
                "typability",
                "uniquify",
                "uniquification",
                "polymorphism",
                "precondition",
                "preconditions",
                "preconditioned",
                "preconditioning",
                "postcondition",
                "postconditions",
                "generalised",
                "definitional",
                "agentic",
                "exfiltrate",
                "exfiltration",
                "exfiltrated",
                "exfiltrating",
                "fintech",
                "scalable",
                "scalability",
                "transformative",
                "transformatively",
                "untrusted",
                "untrusting",
                "verifier",
                "verifiers",
            ],
        );
    }

    #[test]
    fn test_misspellings_skip_identifiers_and_known_compounds() {
        let found: Vec<String> = en_us()
            .misspellings("Teh parser calls getUserId and reads a pre-configured fiel.")
            .into_iter()
            .map(|token| token.word)
            .collect();
        assert_eq!(found, ["Teh", "fiel"]);
    }

    #[test]
    fn test_misspellings_in_checks_only_prose_at_original_offsets() {
        let code = "let qzxv = \"wrdz\"; // the fiel is réad\n";
        let found: Vec<(String, &str)> = en_us()
            .misspellings_in(code, Prose::Comments(CommentStyle::Slash))
            .into_iter()
            .map(|token| (token.word, &code[token.start..token.end]))
            .collect();
        assert_eq!(
            found,
            [("fiel".to_string(), "fiel"), ("réad".to_string(), "réad")]
        );
        let latex = "A \\textbf{teh} {P}rogram cites \\cite{qzxv} and $wrdz$.";
        let found: Vec<&str> = en_us()
            .misspellings_in(latex, Prose::Latex)
            .into_iter()
            .map(|token| &latex[token.start..token.end])
            .collect();
        assert_eq!(found, ["teh"]);
    }

    #[test]
    fn test_bundled_suggestions_include_the_fix() {
        assert!(en_us()
            .suggest("recieve", 5)
            .contains(&"receive".to_string()));
    }
}
