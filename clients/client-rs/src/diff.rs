//! The minimal RFC 6902 patch turning one JSON value into another.

use serde_json::{json, Value};

// ========================================================================
// Diffing
// ========================================================================

/// The minimal RFC 6902 ops that turn `old` into `new` at `path`.
///
/// Objects recurse per key (`remove`/`add`/nested diff); an array that's
/// `old` plus a tail becomes one `add` per appended item; anything else -
/// scalars, a changed type, an array changed anywhere but its tail -
/// becomes a single `replace`. A full minimal array diff (LCS) is out of
/// scope.
pub(crate) fn diff(old: &Value, new: &Value, path: &str) -> Vec<Value> {
    if old == new {
        return Vec::new();
    }
    if let (Value::Object(old_map), Value::Object(new_map)) = (old, new) {
        let mut ops = Vec::new();
        for key in old_map.keys() {
            if !new_map.contains_key(key) {
                ops.push(json!({
                    "op": "remove",
                    "path": format!("{path}/{}", escape_pointer_segment(key)),
                }));
            }
        }
        for (key, new_value) in new_map {
            let key_path = format!("{path}/{}", escape_pointer_segment(key));
            match old_map.get(key) {
                None => ops.push(json!({"op": "add", "path": key_path, "value": new_value})),
                Some(old_value) if old_value != new_value => {
                    ops.extend(diff(old_value, new_value, &key_path));
                }
                Some(_) => {}
            }
        }
        return ops;
    }
    if let (Value::Array(old_items), Value::Array(new_items)) = (old, new) {
        if new_items.len() >= old_items.len() && new_items[..old_items.len()] == old_items[..] {
            return new_items[old_items.len()..]
                .iter()
                .map(|item| json!({"op": "add", "path": format!("{path}/-"), "value": item}))
                .collect();
        }
    }
    vec![json!({"op": "replace", "path": path, "value": new})]
}

/// Encode `segment` as one RFC 6901 JSON Pointer path component.
fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_of_equal_values_is_empty() {
        assert_eq!(
            diff(&json!({"a": 1}), &json!({"a": 1}), ""),
            Vec::<Value>::new()
        );
        assert_eq!(
            diff(&json!([1, 2]), &json!([1, 2]), ""),
            Vec::<Value>::new()
        );
    }

    #[test]
    fn test_diff_object_adds_removes_and_replaces_top_level_keys() {
        let old = json!({"a": 1, "b": 2, "c": 3});
        let new = json!({"a": 1, "b": 20, "d": 4});
        let ops = diff(&old, &new, "");
        assert!(ops.contains(&json!({"op": "remove", "path": "/c"})));
        assert!(ops.contains(&json!({"op": "add", "path": "/d", "value": 4})));
        assert!(ops.contains(&json!({"op": "replace", "path": "/b", "value": 20})));
        assert_eq!(ops.len(), 3);
    }

    #[test]
    fn test_diff_object_recurses_into_nested_objects_for_a_leaf_level_op() {
        // A change three levels down must not blanket-replace the top
        // object - that's exactly the friction `update_from` removes.
        let old = json!({"table": {"rows": {"count": 1}}});
        let new = json!({"table": {"rows": {"count": 2}}});
        assert_eq!(
            diff(&old, &new, ""),
            vec![json!({"op": "replace", "path": "/table/rows/count", "value": 2})]
        );
    }

    #[test]
    fn test_diff_array_pure_append_emits_one_add_per_new_item() {
        let old = json!([1, 2]);
        let new = json!([1, 2, 3, 4]);
        assert_eq!(
            diff(&old, &new, "/values"),
            vec![
                json!({"op": "add", "path": "/values/-", "value": 3}),
                json!({"op": "add", "path": "/values/-", "value": 4}),
            ]
        );
    }

    #[test]
    fn test_diff_array_changed_anywhere_but_the_tail_falls_back_to_replace() {
        let old = json!([1, 2, 3]);
        let new = json!([1, 5, 3]);
        assert_eq!(
            diff(&old, &new, "/values"),
            vec![json!({"op": "replace", "path": "/values", "value": [1, 5, 3]})]
        );

        let shrunk = json!([1]);
        assert_eq!(
            diff(&old, &shrunk, "/values"),
            vec![json!({"op": "replace", "path": "/values", "value": [1]})]
        );
    }

    #[test]
    fn test_diff_escapes_pointer_special_characters_in_keys() {
        let old = json!({});
        let new = json!({"a/b": 1, "c~d": 2});
        let ops = diff(&old, &new, "");
        assert!(ops.contains(&json!({"op": "add", "path": "/a~1b", "value": 1})));
        assert!(ops.contains(&json!({"op": "add", "path": "/c~0d", "value": 2})));
    }

    #[test]
    fn test_diff_type_change_is_a_single_replace() {
        assert_eq!(
            diff(&json!({"a": 1}), &json!([1, 2]), ""),
            vec![json!({"op": "replace", "path": "", "value": [1, 2]})]
        );
    }
}
