//! An XML document's outline: each element holding other elements, by its
//! tag and its `id`, `name`, or `key` where it gives one, nested as the
//! elements nest (a project file's groups, a plist's dictionaries). An
//! element holding only text, or nothing, is a value, and is left out;
//! comments, CDATA, and declarations are passed over.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Constants
// ========================================================================

/// The attributes whose value names an element, the first found.
const NAME_ATTRIBUTES: [&str; 3] = ["id", "name", "key"];

/// What opens and closes the markup that is not an element.
const SKIPPED: [(&str, &str); 4] = [
    ("<!--", "-->"),
    ("<![CDATA[", "]]>"),
    ("<?", "?>"),
    ("<!", ">"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// An element the scan is inside: where its tag is, what it is called,
/// and the outline entry it is listed as, once it holds an element.
struct Open {
    label: String,
    listed: bool,
    row: usize,
}

// ========================================================================
// Functions
// ========================================================================

/// The outline of the XML document `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    let mut outline = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    // The end of the markup being passed over, while inside it.
    let mut skipping: Option<&str> = None;
    // The tag being read, while it runs across lines, and its line.
    let mut tag: Option<(usize, String)> = None;
    for (row, line) in lines.iter().enumerate() {
        let mut rest = line.as_str();
        while !rest.is_empty() {
            if let Some(end) = skipping {
                match rest.find(end) {
                    Some(at) => {
                        rest = &rest[at + end.len()..];
                        skipping = None;
                    }
                    None => rest = "",
                }
                continue;
            }
            if let Some((at, text)) = tag.as_mut() {
                match end_of_tag(rest) {
                    Some(close) => {
                        text.push_str(&rest[..close]);
                        let (at, text) = (*at, std::mem::take(text));
                        tag = None;
                        rest = &rest[close + 1..];
                        take_tag(&mut outline, &mut open, at, &text);
                    }
                    None => {
                        text.push_str(rest);
                        text.push(' ');
                        rest = "";
                    }
                }
                continue;
            }
            let Some(at) = rest.find('<') else {
                break;
            };
            rest = &rest[at..];
            if let Some((_, end)) = SKIPPED.iter().find(|(start, _)| rest.starts_with(start)) {
                skipping = Some(end);
                rest = &rest[1..];
                continue;
            }
            tag = Some((row, String::new()));
            rest = &rest[1..];
        }
    }
    outline
}

/// Where the tag `text` starts with ends, at its `>` outside quotes.
fn end_of_tag(text: &str) -> Option<usize> {
    let mut quoted: Option<char> = None;
    for (at, c) in text.char_indices() {
        match (quoted, c) {
            (Some(quote), _) if c == quote => quoted = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quoted = Some(c),
            (None, '>') => return Some(at),
            (None, _) => {}
        }
    }
    None
}

/// Take the tag `text` (what its `<` and `>` hold), on line `row`: an end
/// tag closes its element, any other is an element its holder holds, which
/// lists the holder; a start tag opens one.
fn take_tag(outline: &mut Vec<JumpTarget>, open: &mut Vec<Open>, row: usize, text: &str) {
    if let Some(name) = text.strip_prefix('/') {
        let name = name.trim();
        if let Some(at) = open
            .iter()
            .rposition(|element| element.label.split(' ').next() == Some(name))
        {
            open.truncate(at);
        }
        return;
    }
    if let Some(holder) = open.last_mut().filter(|holder| !holder.listed) {
        holder.listed = true;
        let level = open.len() - 1;
        let holder = &open[level];
        outline.push(entry(holder.row, level, &holder.label));
    }
    if text.trim_end().ends_with('/') {
        return;
    }
    open.push(Open {
        label: label_of(text),
        listed: false,
        row,
    });
}

/// What an element's tag `text` names it: its tag, and the value of its
/// first naming attribute.
fn label_of(text: &str) -> String {
    let name = text.split_whitespace().next().unwrap_or_default();
    let named = NAME_ATTRIBUTES.iter().find_map(|attribute| {
        let at = text.find(&format!(" {attribute}="))? + attribute.len() + 2;
        let quote = text[at..]
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\''))?;
        let value = &text[at + 1..];
        Some(value[..value.find(quote)?].to_string())
    });
    match named {
        Some(value) => format!("{name} {value}"),
        None => name.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{labels, lines};

    #[test]
    fn test_elements_holding_elements_nest_and_values_are_left_out() {
        let doc = lines(concat!(
            "<?xml version=\"1.0\"?>\n",
            "<!-- <ignored><x/></ignored> -->\n",
            "<Project Sdk=\"Microsoft.NET.Sdk\">\n",
            "  <PropertyGroup>\n",
            "    <TargetFramework>net8.0</TargetFramework>\n",
            "  </PropertyGroup>\n",
            "  <ItemGroup\n",
            "      Label=\"a > b\">\n",
            "    <PackageReference Include=\"X\" />\n",
            "  </ItemGroup>\n",
            "  <Target name='Build'><Exec Command=\"x\"/></Target>\n",
            "  <Empty></Empty>\n",
            "  <Text><![CDATA[<not><tags/></not>]]></Text>\n",
            "</Project>\n",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    3  Project",
                "    4    PropertyGroup",
                "    7    ItemGroup",
                "   11    Target Build",
            ]
        );
    }
}
