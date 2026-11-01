//! The subset of a Hunspell `.aff` file a word check needs: single-level
//! prefix and suffix rules, with their stem conditions and cross-product flag.
//!
//! Every other directive (compounding, REP, ICONV, TRY) is skipped by design.

// ========================================================================
// Constants
// ========================================================================

/// Hunspell's notation for an empty strip or add field.
const EMPTY_FIELD: &str = "0";

/// Hunspell's condition that any stem satisfies.
const ANY_CONDITION: &str = ".";

/// A condition position that accepts any character.
const CONDITION_ANY_CHAR: char = '.';

/// Open and close a bracketed character class in a condition.
const CONDITION_CLASS_OPEN: char = '[';
const CONDITION_CLASS_CLOSE: char = ']';

/// Negates a character class when it comes first inside the brackets.
const CONDITION_CLASS_NEGATION: char = '^';

/// The header field that lets a rule set combine with the opposite affix kind.
const CROSS_PRODUCT_YES: &str = "Y";

/// Fields in a rule set header: kind, flag, cross product, rule count.
const HEADER_FIELD_COUNT: usize = 4;

/// The keyword that opens a prefix rule line.
const PREFIX_KEYWORD: &str = "PFX";

/// The keyword that opens a suffix rule line.
const SUFFIX_KEYWORD: &str = "SFX";

/// Flags are single ASCII characters, so a set of them fits in one `u128`.
const FLAG_SET_CAPACITY: u32 = u128::BITS;

// ========================================================================
// Data Structures
// ========================================================================

/// Every prefix and suffix rule set in one `.aff` file, keyed by flag.
#[derive(Debug, Default)]
pub(crate) struct AffixRules {
    pub prefixes: Vec<RuleSet>,
    pub suffixes: Vec<RuleSet>,
}

/// One flag's rules, and whether they may combine with the opposite kind.
#[derive(Debug)]
pub(crate) struct RuleSet {
    pub cross_product: bool,
    pub flag: char,
    pub rules: Vec<AffixRule>,
}

/// A rule: a stem meeting `condition` has `strip` removed and `add` attached.
#[derive(Debug)]
pub(crate) struct AffixRule {
    pub add: String,
    /// What the stem must start with (prefix) or end with (suffix), one
    /// element per character; empty for Hunspell's bare `.`.
    pub condition: Vec<ConditionChar>,
    pub strip: String,
}

/// One character position of a rule's stem condition.
#[derive(Debug, PartialEq)]
pub(crate) enum ConditionChar {
    /// `.`: any character.
    Any,
    /// A literal or a bracketed class such as `[aeiou]` or `[^ey]`.
    Class(CharClass),
}

/// A set of characters a position must (or, when negated, must not) hold.
#[derive(Debug, PartialEq)]
pub(crate) struct CharClass {
    pub chars: Vec<char>,
    pub negated: bool,
}

/// Which end of the word an affix attaches to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AffixKind {
    Prefix,
    Suffix,
}

/// The single-character flags a dictionary stem carries.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FlagSet {
    bits: u128,
}

// ========================================================================
// AffixRule
// ========================================================================

impl AffixRule {
    /// The stem `word` would come from under this prefix rule, if `word`
    /// has the rule's shape and the stem meets its condition.
    pub fn strip_prefix(&self, word: &str) -> Option<String> {
        let rest = word.strip_prefix(self.add.as_str())?;
        let stem = format!("{}{rest}", self.strip);
        let fits = !stem.is_empty() && matches_condition(stem.chars(), self.condition.iter());
        fits.then_some(stem)
    }

    /// The stem `word` would come from under this suffix rule, if `word`
    /// has the rule's shape and the stem meets its condition.
    pub fn strip_suffix(&self, word: &str) -> Option<String> {
        let rest = word.strip_suffix(self.add.as_str())?;
        let stem = format!("{rest}{}", self.strip);
        let fits =
            !stem.is_empty() && matches_condition(stem.chars().rev(), self.condition.iter().rev());
        fits.then_some(stem)
    }
}

// ========================================================================
// AffixRules
// ========================================================================

impl AffixRules {
    /// Reads every `PFX`/`SFX` rule set out of `.aff` text; a header whose
    /// rule lines run short keeps the rules it has.
    pub fn parse(aff: &str) -> Self {
        let mut rules = Self::default();
        let mut lines = aff.lines();
        while let Some(line) = lines.next() {
            let header: Vec<&str> = line.split_whitespace().collect();
            if header.len() != HEADER_FIELD_COUNT {
                continue;
            }
            let kind = match header[0] {
                PREFIX_KEYWORD => AffixKind::Prefix,
                SUFFIX_KEYWORD => AffixKind::Suffix,
                _ => continue,
            };
            let Some(flag) = single_char(header[1]) else {
                continue;
            };
            let count: usize = header[3].parse().unwrap_or(0);
            let mut set = RuleSet {
                cross_product: header[2] == CROSS_PRODUCT_YES,
                flag,
                rules: Vec::with_capacity(count),
            };
            for line in lines.by_ref().take(count) {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() < HEADER_FIELD_COUNT
                    || fields[0] != header[0]
                    || fields[1] != header[1]
                {
                    break;
                }
                set.rules.push(AffixRule {
                    add: affix_field(fields[3]),
                    condition: parse_condition(fields.get(4).copied().unwrap_or(ANY_CONDITION)),
                    strip: affix_field(fields[2]),
                });
            }
            match kind {
                AffixKind::Prefix => rules.prefixes.push(set),
                AffixKind::Suffix => rules.suffixes.push(set),
            }
        }
        rules
    }
}

// ========================================================================
// ConditionChar
// ========================================================================

impl ConditionChar {
    fn accepts(&self, c: char) -> bool {
        match self {
            Self::Any => true,
            Self::Class(class) => class.chars.contains(&c) != class.negated,
        }
    }
}

// ========================================================================
// FlagSet
// ========================================================================

impl FlagSet {
    /// The flags written after a `.dic` entry's slash; non-ASCII flags,
    /// which neither bundled file uses, are dropped.
    pub fn parse(flags: &str) -> Self {
        let mut set = Self::default();
        for flag in flags.chars() {
            if let Some(bit) = flag_bit(flag) {
                set.bits |= bit;
            }
        }
        set
    }

    pub fn contains(self, flag: char) -> bool {
        flag_bit(flag).is_some_and(|bit| self.bits & bit != 0)
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }
}

// ========================================================================
// Helpers
// ========================================================================

fn affix_field(field: &str) -> String {
    if field == EMPTY_FIELD {
        String::new()
    } else {
        field.to_string()
    }
}

fn flag_bit(flag: char) -> Option<u128> {
    let code = u32::from(flag);
    (code < FLAG_SET_CAPACITY).then(|| 1u128 << code)
}

/// Whether the characters of `stem`, in the order given, meet `condition`
/// one for one; a stem shorter than the condition fails it.
fn matches_condition<'a>(
    mut stem: impl Iterator<Item = char>,
    mut condition: impl Iterator<Item = &'a ConditionChar>,
) -> bool {
    condition.all(|element| stem.next().is_some_and(|c| element.accepts(c)))
}

/// Splits a condition such as `[^aeiou]y` into one element per position.
fn parse_condition(condition: &str) -> Vec<ConditionChar> {
    if condition == ANY_CONDITION {
        return Vec::new();
    }
    let mut elements = Vec::new();
    let mut chars = condition.chars();
    while let Some(c) = chars.next() {
        let element = match c {
            CONDITION_ANY_CHAR => ConditionChar::Any,
            CONDITION_CLASS_OPEN => {
                let mut class = CharClass {
                    chars: Vec::new(),
                    negated: false,
                };
                for inner in chars.by_ref() {
                    match inner {
                        CONDITION_CLASS_CLOSE => break,
                        CONDITION_CLASS_NEGATION if class.chars.is_empty() && !class.negated => {
                            class.negated = true
                        }
                        other => class.chars.push(other),
                    }
                }
                ConditionChar::Class(class)
            }
            literal => ConditionChar::Class(CharClass {
                chars: vec![literal],
                negated: false,
            }),
        };
        elements.push(element);
    }
    elements
}

fn single_char(field: &str) -> Option<char> {
    let mut chars = field.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn suffix_rule(strip: &str, add: &str, condition: &str) -> AffixRule {
        AffixRule {
            add: affix_field(add),
            condition: parse_condition(condition),
            strip: affix_field(strip),
        }
    }

    #[test]
    fn test_parse_condition_reads_negated_class_then_literal() {
        let condition = parse_condition("[^aeiou]y");
        assert_eq!(
            condition,
            vec![
                ConditionChar::Class(CharClass {
                    chars: vec!['a', 'e', 'i', 'o', 'u'],
                    negated: true,
                }),
                ConditionChar::Class(CharClass {
                    chars: vec!['y'],
                    negated: false,
                }),
            ]
        );
    }

    #[test]
    fn test_strip_suffix_checks_condition_against_stem_end() {
        let rule = suffix_rule("y", "ied", "[^aeiou]y");
        assert_eq!(rule.strip_suffix("tidied").as_deref(), Some("tidy"));
        // "staied" strips to "stay", whose vowel before the y fails the
        // condition, so the rule must not claim it.
        assert_eq!(rule.strip_suffix("staied"), None);
    }

    #[test]
    fn test_parse_reads_rule_sets_and_skips_other_directives() {
        let aff = "SET UTF-8\nTRY abc\nREP 1\nREP a ei\n\
                   SFX D Y 2\nSFX D 0 d e\nSFX D 0 ed [^ey]\n\
                   PFX U N 1\nPFX U 0 un .\n";
        let rules = AffixRules::parse(aff);
        assert_eq!(rules.suffixes.len(), 1);
        assert_eq!(rules.suffixes[0].flag, 'D');
        assert!(rules.suffixes[0].cross_product);
        assert_eq!(rules.suffixes[0].rules.len(), 2);
        assert_eq!(rules.prefixes.len(), 1);
        assert!(!rules.prefixes[0].cross_product);
        assert!(rules.prefixes[0].rules[0].condition.is_empty());
    }

    #[test]
    fn test_flag_set_distinguishes_case() {
        let flags = FlagSet::parse("Gm");
        assert!(flags.contains('G'));
        assert!(flags.contains('m'));
        assert!(!flags.contains('g'));
        assert!(!flags.contains('M'));
    }
}
