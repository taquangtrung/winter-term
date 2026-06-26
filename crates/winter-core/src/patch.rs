//! Minimal RFC 6902 JSON Patch application for live TBP blocks.
//!
//! Live blocks carry their updates as JSON Patch arrays (`open`/`patch` in
//! `docs/terminal-block-protocol-spec.md`). Folding them into a current state
//! needs only the operation semantics of RFC 6902, not its atomicity: a
//! terminal display cannot roll back a half-applied patch, so [`apply`] is
//! best-effort: malformed operations are skipped, and a failing `test`
//! stops the remaining operations, the closest analogue of the RFC's
//! all-or-nothing contract a streaming display can offer.

use serde_json::Value;

// ============================================================================
// Application
// ============================================================================

/// Apply a JSON Patch, an array of RFC 6902 operations, to `document`,
/// in place. Anything that is not an operation array is a no-op; operations
/// that do not typecheck or whose paths do not resolve are skipped.
pub fn apply(document: &mut Value, patch: &Value) {
    let Some(ops) = patch.as_array() else {
        return;
    };
    for op in ops {
        if !apply_op(document, op) {
            // A failed `test` invalidates the operations after it (RFC 6902
            // would reject the whole patch); everything else is skipped
            // individually.
            break;
        }
    }
}

/// Apply one operation. Returns whether evaluation should continue.
fn apply_op(document: &mut Value, op: &Value) -> bool {
    let Some(name) = op.get("op").and_then(Value::as_str) else {
        return true;
    };
    let Some(path) = op.get("path").and_then(Value::as_str) else {
        return true;
    };
    let Some(tokens) = pointer_tokens(path) else {
        return true;
    };
    match name {
        "add" => {
            let Some(value) = op.get("value") else {
                return true;
            };
            add_at(document, &tokens, value.clone());
        }
        "remove" => {
            take_at(document, &tokens);
        }
        "replace" => {
            let Some(value) = op.get("value") else {
                return true;
            };
            replace_at(document, &tokens, value.clone());
        }
        "move" => {
            let Some(from) = op.get("from").and_then(Value::as_str) else {
                return true;
            };
            let Some(from_tokens) = pointer_tokens(from) else {
                return true;
            };
            if let Some(value) = take_at(document, &from_tokens) {
                add_at(document, &tokens, value);
            }
        }
        "copy" => {
            let Some(from) = op.get("from").and_then(Value::as_str) else {
                return true;
            };
            let Some(from_tokens) = pointer_tokens(from) else {
                return true;
            };
            if let Some(value) = navigate(document, &from_tokens).cloned() {
                add_at(document, &tokens, value);
            }
        }
        "test" => {
            let Some(expected) = op.get("value") else {
                return true;
            };
            let passed = navigate(document, &tokens)
                .map(|current| *current == *expected)
                .unwrap_or(false);
            if !passed {
                return false;
            }
        }
        _ => {}
    }
    true
}

// ============================================================================
// Navigation
// ============================================================================

/// Split a JSON Pointer (RFC 6901) into unescaped reference tokens. `None`
/// when the pointer is malformed: it must be empty (the whole document) or
/// start with `/`. Escape decoding: `~1` → `/`, then `~0` → `~`.
fn pointer_tokens(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    if !pointer.starts_with('/') {
        return None;
    }
    Some(
        pointer[1..]
            .split('/')
            .map(|token| token.replace("~1", "/").replace("~0", "~"))
            .collect(),
    )
}

/// The value at `tokens` from the document root, mutably.
fn navigate<'a>(document: &'a mut Value, tokens: &[String]) -> Option<&'a mut Value> {
    let mut current = document;
    for token in tokens {
        match current {
            Value::Object(map) => {
                current = map.get_mut(token)?;
            }
            Value::Array(array) => {
                let index = token.parse::<usize>().ok()?;
                current = array.get_mut(index)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Insert `value` at `tokens` (an object key, an array slot, or the whole
/// document for an empty pointer). Array indices may equal the length (a
/// legal insert position); `-` appends.
fn add_at(document: &mut Value, tokens: &[String], value: Value) {
    let Some((last, parents)) = tokens.split_last() else {
        // An empty pointer addresses the whole document: `add` replaces it.
        *document = value;
        return;
    };
    match navigate(document, parents) {
        Some(Value::Object(map)) => {
            map.insert(last.clone(), value);
        }
        Some(Value::Array(array)) => {
            if last == "-" {
                array.push(value);
            } else if let Ok(index) = last.parse::<usize>() {
                if index <= array.len() {
                    array.insert(index, value);
                }
            }
        }
        _ => {}
    }
}

/// Remove and return the value at `tokens`, if it exists.
fn take_at(document: &mut Value, tokens: &[String]) -> Option<Value> {
    let (last, parents) = tokens.split_last()?;
    match navigate(document, parents)? {
        Value::Object(map) => map.remove(last),
        Value::Array(array) => {
            let index = last.parse::<usize>().ok()?;
            if index < array.len() {
                Some(array.remove(index))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Overwrite the value at `tokens`; unlike `add`, the target must exist.
fn replace_at(document: &mut Value, tokens: &[String], value: Value) {
    if let Some(slot) = navigate(document, tokens) {
        *slot = value;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_add_into_object_and_array() {
        let mut doc = json!({"a": 1, "list": [1, 2]});
        apply(
            &mut doc,
            &json!([
                {"op": "add", "path": "/b", "value": 2},
                {"op": "add", "path": "/list/-", "value": 3},
                {"op": "add", "path": "/list/0", "value": 0},
            ]),
        );
        assert_eq!(doc, json!({"a": 1, "b": 2, "list": [0, 1, 2, 3]}));
    }

    #[test]
    fn test_add_with_root_pointer_replaces_the_document() {
        let mut doc = json!({"old": true});
        apply(
            &mut doc,
            &json!([{"op": "add", "path": "", "value": [1, 2]}]),
        );
        assert_eq!(doc, json!([1, 2]));
    }

    #[test]
    fn test_remove_from_object_and_array() {
        let mut doc = json!({"a": 1, "list": [1, 2, 3]});
        apply(
            &mut doc,
            &json!([
                {"op": "remove", "path": "/a"},
                {"op": "remove", "path": "/list/1"},
            ]),
        );
        assert_eq!(doc, json!({"list": [1, 3]}));
    }

    #[test]
    fn test_replace_requires_the_target_to_exist() {
        let mut doc = json!({"a": 1});
        apply(
            &mut doc,
            &json!([{"op": "replace", "path": "/b", "value": 2}]),
        );
        assert_eq!(doc, json!({"a": 1}), "replace of a missing key is skipped");

        apply(
            &mut doc,
            &json!([{"op": "replace", "path": "/a", "value": 9}]),
        );
        assert_eq!(doc, json!({"a": 9}));
    }

    #[test]
    fn test_move_relocates_a_value() {
        let mut doc = json!({"a": [10, 20], "b": null});
        apply(
            &mut doc,
            &json!([{"op": "move", "from": "/a/1", "path": "/b"}]),
        );
        assert_eq!(doc, json!({"a": [10], "b": 20}));
    }

    #[test]
    fn test_copy_leaves_the_source_in_place() {
        let mut doc = json!({"a": [10, 20]});
        apply(
            &mut doc,
            &json!([{"op": "copy", "from": "/a/0", "path": "/b"}]),
        );
        assert_eq!(doc, json!({"a": [10, 20], "b": 10}));
    }

    #[test]
    fn test_test_passes_on_equal_values() {
        let mut doc = json!({"v": 1});
        apply(
            &mut doc,
            &json!([
                {"op": "test", "path": "/v", "value": 1},
                {"op": "replace", "path": "/v", "value": 2},
            ]),
        );
        assert_eq!(doc, json!({"v": 2}));
    }

    #[test]
    fn test_failing_test_stops_the_remaining_operations() {
        // RFC 6902 rejects the whole patch when a test fails; the
        // best-effort analogue stops applying the rest, so a stale display
        // never applies updates past a failed precondition.
        let mut doc = json!({"v": 1});
        apply(
            &mut doc,
            &json!([
                {"op": "replace", "path": "/v", "value": 0},
                {"op": "test", "path": "/v", "value": 1},
                {"op": "replace", "path": "/v", "value": 2},
            ]),
        );
        assert_eq!(doc, json!({"v": 0}));
    }

    #[test]
    fn test_pointer_escapes_decode() {
        let mut doc = json!({"a/b": 1, "m~n": 2});
        apply(
            &mut doc,
            &json!([{"op": "replace", "path": "/a~1b", "value": 3}]),
        );
        apply(
            &mut doc,
            &json!([{"op": "replace", "path": "/m~0n", "value": 4}]),
        );
        assert_eq!(doc, json!({"a/b": 3, "m~n": 4}));
    }

    #[test]
    fn test_malformed_operations_are_skipped_individually() {
        let mut doc = json!({"a": 1});
        apply(
            &mut doc,
            &json!([
                {"op": "add", "path": "/missing/deeply", "value": 1},
                {"op": "add", "path": "/list/99", "value": 1},
                {"op": "add"},
                {"op": "replace", "path": "no-leading-slash", "value": 1},
                {"op": "teleport", "path": "/a"},
                {"op": "add", "path": "/b", "value": 2},
            ]),
        );
        assert_eq!(doc, json!({"a": 1, "b": 2}));
    }

    #[test]
    fn test_non_array_patch_is_a_no_op() {
        let mut doc = json!({"a": 1});
        apply(&mut doc, &json!({"op": "remove", "path": "/a"}));
        apply(&mut doc, &json!("not a patch"));
        apply(&mut doc, &json!(null));
        assert_eq!(doc, json!({"a": 1}));
    }
}
