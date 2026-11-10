//! A picker's rows: what each one shows, and which of its fields a query is
//! matched against and highlighted in.

use std::ops::BitOr;

use winter_model::paint::IconKind;

// ========================================================================
// Data Structures
// ========================================================================

/// One choice in a picker, as it is drawn: a label, a description after it
/// on its line, a detail on a second line under it, and a hint at the line's
/// right. A source fills only the fields its rows need.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Drawn dimmer after the label, on its line: a file's directory, a
    /// buffer's kind. Empty for none.
    pub description: String,
    /// A second line under the label, for text too long or too uneven to
    /// share the label's line. Empty for a row of one line.
    pub detail: String,
    /// Drawn dim at the line's right, for a key chord or a pane's digit only.
    /// Empty for none.
    pub hint: String,
    /// What the row is, drawn before its label: a file's type, a folder.
    pub icon: Option<IconKind>,
    /// What names the row to its source, which holds the data behind it.
    pub key: RowKey,
    /// What the row is, drawn first.
    pub label: String,
}

/// What names a row to its source: the source keeps the typed data behind a
/// row (a path and line, a tab), so rows stay plain.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RowKey(pub u64);

/// One field of a row a query can be matched against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field {
    /// The text after the label on its line.
    Description,
    /// The second line.
    Detail,
    /// The row's name, drawn first.
    Label,
}

/// The fields of a source's rows that its query is matched against and
/// highlighted in: one flag for both, so a picker never lights a field it did
/// not match. Combine with `|`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MatchOn(u8);

/// The letters of each field that matched the query, as char indices.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Highlights {
    /// In the description.
    pub description: Vec<usize>,
    /// In the detail.
    pub detail: Vec<usize>,
    /// In the label.
    pub label: Vec<usize>,
}

// ========================================================================
// Row
// ========================================================================

impl Row {
    /// A row of one line, showing only `label`.
    pub fn new(key: RowKey, label: impl Into<String>) -> Self {
        Self {
            description: String::new(),
            detail: String::new(),
            hint: String::new(),
            icon: None,
            key,
            label: label.into(),
        }
    }

    /// The same row, with `text` after its label.
    pub fn with_description(self, text: impl Into<String>) -> Self {
        Self {
            description: text.into(),
            ..self
        }
    }

    /// The same row, with `text` on a second line.
    pub fn with_detail(self, text: impl Into<String>) -> Self {
        Self {
            detail: text.into(),
            ..self
        }
    }

    /// The same row, with `hint` at its right.
    pub fn with_hint(self, hint: impl Into<String>) -> Self {
        Self {
            hint: hint.into(),
            ..self
        }
    }

    /// The same row, with `icon` before its label.
    pub fn with_icon(self, icon: IconKind) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }

    /// How many lines the row takes: two with a detail, else one.
    pub fn lines(&self) -> usize {
        match self.detail.is_empty() {
            true => 1,
            false => 2,
        }
    }

    /// The text of `field`.
    pub fn field(&self, field: Field) -> &str {
        match field {
            Field::Description => &self.description,
            Field::Detail => &self.detail,
            Field::Label => &self.label,
        }
    }
}

// ========================================================================
// MatchOn
// ========================================================================

impl MatchOn {
    /// The description.
    pub const DESCRIPTION: Self = Self(1 << 1);
    /// The detail.
    pub const DETAIL: Self = Self(1 << 2);
    /// The label.
    pub const LABEL: Self = Self(1);

    /// The fields named, in the order a row draws them.
    pub fn fields(self) -> Vec<Field> {
        [
            (Self::LABEL, Field::Label),
            (Self::DESCRIPTION, Field::Description),
            (Self::DETAIL, Field::Detail),
        ]
        .into_iter()
        .filter(|(flag, _)| self.0 & flag.0 != 0)
        .map(|(_, field)| field)
        .collect()
    }
}

impl BitOr for MatchOn {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

// ========================================================================
// Highlights
// ========================================================================

impl Highlights {
    /// The matched letters of `field`, to add to.
    pub fn field_mut(&mut self, field: Field) -> &mut Vec<usize> {
        match field {
            Field::Description => &mut self.description,
            Field::Detail => &mut self.detail,
            Field::Label => &mut self.label,
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
    fn test_match_on_names_its_fields_in_drawing_order() {
        let on = MatchOn::DETAIL | MatchOn::LABEL;
        assert_eq!(on.fields(), [Field::Label, Field::Detail]);
        assert_eq!(MatchOn::DESCRIPTION.fields(), [Field::Description]);
    }
}
