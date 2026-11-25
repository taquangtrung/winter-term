//! A Vega-Lite spec read into the chart it describes, for the part of the
//! language Winter draws: one mark over inline data, placed by `x`, `y`, and
//! optionally `color`. Anything past that is refused, so the block falls back
//! to the text it carries rather than drawing a different chart.

use serde_json::{Map, Value};

// ========================================================================
// Constants
// ========================================================================

/// The plot's size when the spec gives none, in pixels.
const DEFAULT_WIDTH: f64 = 400.0;
const DEFAULT_HEIGHT: f64 = 240.0;

/// Spec keys that compose or reshape charts, which Winter does not draw.
const UNSUPPORTED_KEYS: [&str; 7] = [
    "concat", "facet", "hconcat", "layer", "repeat", "spec", "vconcat",
];

/// Encoding channels that split a chart into many, which Winter does not
/// draw.
const UNSUPPORTED_CHANNELS: [&str; 3] = ["column", "facet", "row"];

// ========================================================================
// Data Structures
// ========================================================================

/// One chart: a mark over rows of data, placed by its channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    /// The field the color, and so the series, is told by.
    pub color: Option<String>,
    /// The plot's height in pixels, axes and legend not included.
    pub height: f64,
    pub mark: Mark,
    /// The data, one object a row.
    pub rows: Vec<Map<String, Value>>,
    pub title: Option<String>,
    /// The plot's width in pixels, axes and legend not included.
    pub width: f64,
    pub x: Channel,
    pub y: Channel,
}

/// How each row is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mark {
    Area,
    Bar,
    Line,
    Point,
}

/// What a position channel shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    /// How rows sharing a place are combined.
    pub aggregate: Option<Aggregate>,
    /// The field it reads, `None` for a count of rows.
    pub field: Option<String>,
    pub kind: Kind,
    /// What its axis is titled.
    pub title: String,
}

/// What kind of values a channel holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Nominal,
    Ordinal,
    Quantitative,
    Temporal,
}

/// How the values of rows sharing a place are combined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Aggregate {
    Count,
    Max,
    Mean,
    Min,
    Sum,
}

// ========================================================================
// Aggregate
// ========================================================================

impl Aggregate {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "count" => Self::Count,
            "max" => Self::Max,
            "average" | "mean" => Self::Mean,
            "min" => Self::Min,
            "sum" => Self::Sum,
            _ => return None,
        })
    }

    /// `values` combined.
    pub fn combine(self, values: &[f64]) -> f64 {
        match self {
            Self::Count => values.len() as f64,
            Self::Max => values.iter().copied().fold(f64::MIN, f64::max),
            Self::Mean => values.iter().sum::<f64>() / values.len().max(1) as f64,
            Self::Min => values.iter().copied().fold(f64::MAX, f64::min),
            Self::Sum => values.iter().sum(),
        }
    }

    /// The word an axis title starts with for it.
    fn word(self) -> &'static str {
        match self {
            Self::Count => "Count",
            Self::Max => "Max",
            Self::Mean => "Mean",
            Self::Min => "Min",
            Self::Sum => "Sum",
        }
    }
}

// ========================================================================
// Chart
// ========================================================================

impl Chart {
    /// The chart `spec` describes, or `None` for one Winter does not draw.
    pub fn parse(spec: &Value) -> Option<Self> {
        let spec = spec.as_object()?;
        if UNSUPPORTED_KEYS.iter().any(|key| spec.contains_key(*key)) {
            return None;
        }
        if spec
            .get("transform")
            .and_then(Value::as_array)
            .is_some_and(|steps| !steps.is_empty())
        {
            return None;
        }
        let rows = spec
            .get("data")?
            .get("values")?
            .as_array()?
            .iter()
            .filter_map(|row| row.as_object().cloned())
            .collect();
        let mark = match spec.get("mark")? {
            Value::String(name) => Mark::parse(name)?,
            mark => Mark::parse(mark.get("type")?.as_str()?)?,
        };
        let encoding = spec.get("encoding")?.as_object()?;
        if UNSUPPORTED_CHANNELS
            .iter()
            .any(|channel| encoding.contains_key(*channel))
        {
            return None;
        }
        let color = encoding
            .get("color")
            .and_then(|color| color.get("field"))
            .and_then(Value::as_str)
            .map(str::to_string);
        Some(Self {
            color,
            height: size(spec.get("height"), DEFAULT_HEIGHT),
            mark,
            rows,
            title: title(spec.get("title")),
            width: size(spec.get("width"), DEFAULT_WIDTH),
            x: Channel::parse(encoding.get("x")?)?,
            y: Channel::parse(encoding.get("y")?)?,
        })
    }
}

// ========================================================================
// Channel
// ========================================================================

impl Channel {
    fn parse(channel: &Value) -> Option<Self> {
        let aggregate = match channel.get("aggregate") {
            Some(name) => Some(Aggregate::parse(name.as_str()?)?),
            None => None,
        };
        let field = channel.get("field").and_then(Value::as_str);
        if field.is_none() && aggregate != Some(Aggregate::Count) {
            return None;
        }
        let kind = match channel.get("type").and_then(Value::as_str) {
            Some(name) => Kind::parse(name)?,
            None if aggregate.is_some() => Kind::Quantitative,
            None => return None,
        };
        let named = channel
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string);
        let title = named.unwrap_or_else(|| match (aggregate, field) {
            (Some(Aggregate::Count), _) => "Count of Records".to_string(),
            (Some(aggregate), Some(field)) => format!("{} of {field}", aggregate.word()),
            (_, field) => field.unwrap_or_default().to_string(),
        });
        Some(Self {
            aggregate,
            field: field.map(str::to_string),
            kind,
            title,
        })
    }

    /// Whether it places values by category rather than along a line.
    pub fn is_discrete(&self) -> bool {
        matches!(self.kind, Kind::Nominal | Kind::Ordinal)
    }
}

// ========================================================================
// Kind
// ========================================================================

impl Kind {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "N" | "nominal" => Self::Nominal,
            "O" | "ordinal" => Self::Ordinal,
            "Q" | "quantitative" => Self::Quantitative,
            "T" | "temporal" => Self::Temporal,
            _ => return None,
        })
    }
}

// ========================================================================
// Mark
// ========================================================================

impl Mark {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "area" => Self::Area,
            "bar" => Self::Bar,
            "line" => Self::Line,
            "circle" | "point" | "square" => Self::Point,
            _ => return None,
        })
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// A size the spec gives in pixels, or `default` for none, for `container`,
/// and for one too small to draw in.
fn size(value: Option<&Value>, default: f64) -> f64 {
    value
        .and_then(Value::as_f64)
        .filter(|size| *size >= 20.0)
        .unwrap_or(default)
}

/// The chart's title, written either as a string or as `{"text": ...}`.
fn title(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        title => title.get("text")?.as_str().map(str::to_string),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_a_simple_bar_chart_is_read_whole() {
        let chart = Chart::parse(&json!({
            "data": {"values": [{"a": "A", "b": 28}, {"a": "B", "b": 55}]},
            "mark": {"type": "bar", "tooltip": true},
            "encoding": {
                "x": {"field": "a", "type": "nominal"},
                "y": {"field": "b", "type": "quantitative", "aggregate": "sum"}
            },
            "title": {"text": "Totals"},
            "width": 300
        }))
        .expect("a chart Winter draws");
        assert_eq!(chart.mark, Mark::Bar);
        assert_eq!(chart.rows.len(), 2);
        assert_eq!(chart.title.as_deref(), Some("Totals"));
        assert_eq!((chart.width, chart.height), (300.0, DEFAULT_HEIGHT));
        assert_eq!(chart.y.title, "Sum of b");
        assert!(chart.x.is_discrete());
    }

    #[test]
    fn test_what_winter_cannot_draw_is_refused_rather_than_drawn_wrong() {
        let layered = json!({"layer": [], "data": {"values": []}});
        assert_eq!(Chart::parse(&layered), None);
        let fetched = json!({
            "data": {"url": "https://example.com/data.csv"},
            "mark": "line",
            "encoding": {"x": {"field": "a", "type": "quantitative"},
                         "y": {"field": "b", "type": "quantitative"}}
        });
        assert_eq!(Chart::parse(&fetched), None, "data off the network");
        let faceted = json!({
            "data": {"values": []},
            "mark": "point",
            "encoding": {"x": {"field": "a", "type": "quantitative"},
                         "y": {"field": "b", "type": "quantitative"},
                         "row": {"field": "c", "type": "nominal"}}
        });
        assert_eq!(Chart::parse(&faceted), None);
    }

    #[test]
    fn test_a_count_needs_no_field_and_names_its_own_axis() {
        let chart = Chart::parse(&json!({
            "data": {"values": [{"a": "x"}]},
            "mark": "bar",
            "encoding": {"x": {"field": "a", "type": "nominal"},
                         "y": {"aggregate": "count"}}
        }))
        .expect("a count chart");
        assert_eq!(chart.y.title, "Count of Records");
        assert_eq!(chart.y.kind, Kind::Quantitative);
    }

    #[test]
    fn test_aggregates_combine_the_values_sharing_a_place() {
        let values = [2.0, 4.0, 9.0];
        assert_eq!(Aggregate::Sum.combine(&values), 15.0);
        assert_eq!(Aggregate::Mean.combine(&values), 5.0);
        assert_eq!(Aggregate::Max.combine(&values), 9.0);
        assert_eq!(Aggregate::Count.combine(&values), 3.0);
    }
}
