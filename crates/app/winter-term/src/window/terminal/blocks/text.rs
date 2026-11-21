//! The plain text a structured block is shown as when drawn natively: a CSV
//! table laid out in columns, JSON pretty-printed.

use serde_json::Value;

// ========================================================================
// Functions
// ========================================================================

/// Format CSV rows into a column-aligned monospace table. Simple split on `,`;
/// quoted commas are not handled (acceptable for a preview).
pub(in crate::window) fn csv_to_table(csv: &str) -> String {
    let rows: Vec<Vec<&str>> = csv
        .lines()
        .map(|line| line.split(',').map(str::trim).collect())
        .collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; columns];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            out.push_str(cell);
            for _ in cell.chars().count()..widths[i] {
                out.push(' ');
            }
        }
        out.push('\n');
    }
    out
}

/// Pretty-print a JSON value. The bundle may carry it as a JSON string (from a
/// shell client) or as a structured value; both are normalized to pretty text.
pub(in crate::window) fn json_to_text(value: &Value) -> String {
    let parsed = value
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let target = parsed.as_ref().unwrap_or(value);
    serde_json::to_string_pretty(target).unwrap_or_else(|_| value.to_string())
}
