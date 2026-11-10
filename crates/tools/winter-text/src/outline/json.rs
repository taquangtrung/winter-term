//! A JSON document's outline: each key whose value is an object or an
//! array, and each object or array an array holds, by its place and its
//! `name` (or `id`, or `title`) where it gives one, nested as the document
//! nests them. A key holding a plain value is a setting, and is left out.
//! Comments, as JSON with comments allows them, are passed over.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Constants
// ========================================================================

/// The keys whose text names the object an array holds, the first found.
const NAME_KEYS: [&str; 3] = ["name", "id", "title"];

// ========================================================================
// Data Structures
// ========================================================================

/// An object or an array the scan is inside: whether it is an array, how
/// many commas it has passed (the place of the element it is on), and the
/// outline entry it is listed as, where it is.
struct Frame {
    array: bool,
    commas: usize,
    listed: Option<usize>,
    /// Whether an element of an array, so its name is added to its place.
    named_by_place: bool,
}

/// A string that has closed, and the line it opened on.
struct Closed {
    row: usize,
    text: String,
}

// ========================================================================
// Functions
// ========================================================================

/// The outline of the JSON document `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    let mut outline: Vec<JumpTarget> = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    // The string being read, and the line it opened on.
    let mut string: Option<(usize, String)> = None;
    let mut last: Option<Closed> = None;
    let mut key: Option<Closed> = None;
    let mut comment = false;
    for (row, line) in lines.iter().enumerate() {
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    comment = false;
                }
                continue;
            }
            if let Some((_, text)) = string.as_mut() {
                match c {
                    '\\' => {
                        if let Some(escaped) = chars.next() {
                            text.push(escaped);
                        }
                    }
                    '"' => {
                        let (opened, text) = string.take().expect("a string is open");
                        let closed = Closed { row: opened, text };
                        name_the_object(&mut outline, &frames, key.as_ref(), &closed);
                        last = Some(closed);
                    }
                    _ => text.push(c),
                }
                continue;
            }
            match c {
                '/' if chars.peek() == Some(&'/') => break,
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    comment = true;
                }
                '"' => string = Some((row, String::new())),
                ':' => key = last.take(),
                ',' => {
                    key = None;
                    if let Some(frame) = frames.last_mut().filter(|frame| frame.array) {
                        frame.commas += 1;
                    }
                }
                '{' | '[' => {
                    let level = frames.iter().filter(|frame| frame.listed.is_some()).count();
                    let (label, at, named_by_place) = match frames.last() {
                        None => (None, row, false),
                        Some(frame) if frame.array => {
                            (Some(format!("[{}]", frame.commas)), row, c == '{')
                        }
                        Some(_) => match key.take() {
                            Some(key) => (Some(key.text), key.row, false),
                            None => (None, row, false),
                        },
                    };
                    let listed = label.map(|label| {
                        outline.push(entry(at, level, &label));
                        outline.len() - 1
                    });
                    frames.push(Frame {
                        array: c == '[',
                        commas: 0,
                        listed,
                        named_by_place,
                    });
                    last = None;
                }
                '}' | ']' => {
                    frames.pop();
                    key = None;
                    last = None;
                }
                _ => {}
            }
        }
    }
    outline
}

/// Where `value` is the text of a naming key (`key`) of an object an
/// array holds, add it to the object's entry, after its place, once.
fn name_the_object(
    outline: &mut [JumpTarget],
    frames: &[Frame],
    key: Option<&Closed>,
    value: &Closed,
) {
    let Some(key) = key else {
        return;
    };
    if !NAME_KEYS.contains(&key.text.as_str()) {
        return;
    }
    let Some(frame) = frames.last().filter(|frame| frame.named_by_place) else {
        return;
    };
    let Some(listed) = frame.listed.and_then(|at| outline.get_mut(at)) else {
        return;
    };
    if listed.label.ends_with(']') {
        listed.label = format!("{} {}", listed.label, value.text);
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
    fn test_keys_holding_objects_or_arrays_nest_and_plain_values_are_left_out() {
        let doc = lines(concat!(
            "{\n",
            "  \"version\": 2,\n",
            "  // a comment with { a brace\n",
            "  \"lsp\": {\n",
            "    \"auto\": \"{not a block}\",\n",
            "    \"servers\": [\n",
            "      {\"title\": \"t\", \"name\": \"ra\", \"args\": [\"--x\"]},\n",
            "      \"plain\",\n",
            "      {\"command\": \"x\"}\n",
            "    ]\n",
            "  },\n",
            "  /* \"hidden\": { */ \"after\": {}\n",
            "}",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    4  lsp",
                "    6    servers",
                "    7      [0] t",
                "    7        args",
                "    9      [2]",
                "   12  after",
            ]
        );
    }

    #[test]
    fn test_a_key_on_one_line_and_its_object_on_the_next_is_listed_at_the_key() {
        let doc = lines("{\n  \"a\":\n  {\n    \"b\": [1, 2]\n  }\n}");
        let found = outline(&doc);
        assert_eq!(labels(&found), ["    2  a", "    4    b"]);
        assert_eq!(found[0].target, 1);
    }
}
