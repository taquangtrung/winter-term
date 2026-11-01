//! Charts drawn natively: a Vega-Lite spec turned into an SVG document, for
//! the canvas to draw like any other.
//!
//! Winter draws one mark (bars, lines, areas, points) over inline data,
//! placed by `x`, `y`, and a `color` that splits it into stacked or separate
//! series, with axes, a legend, and a title. A spec past that is refused,
//! and the block shows the text it carries instead.

mod scale;
mod spec;

use std::collections::HashMap;
use std::fmt::Write as _;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime};
use serde_json::{Map, Value};

use scale::{Band, Linear};
use spec::{Aggregate, Channel, Chart, Kind, Mark};

// ========================================================================
// Constants
// ========================================================================

/// The colors series are drawn in, in order: Vega's own, so a chart reads
/// the way it does wherever else it is drawn.
const PALETTE: [&str; 10] = [
    "#4c78a8", "#f58518", "#e45756", "#72b7b2", "#54a24b", "#eeca3b", "#b279a2", "#ff9da6",
    "#9d755d", "#bab0ac",
];

/// The size labels and the title are set in.
const FONT_SIZE: f64 = 11.0;
const TITLE_SIZE: f64 = 13.0;

/// About how wide a character of a label is set, for room to be left for
/// it: a little over half the font size, as a sans-serif's average.
const CHAR_WIDTH: f64 = FONT_SIZE * 0.6;

/// How long a tick is, and the gaps around labels and titles.
const TICK: f64 = 5.0;
const GAP: f64 = 4.0;
const MARGIN: f64 = 10.0;

/// How much of each category's band a bar leaves empty.
const BAR_PADDING: f64 = 0.2;

/// How a line and a point are drawn.
const LINE_WIDTH: f64 = 2.0;
const POINT_RADIUS: f64 = 3.0;

/// How opaque a stacked area is, and the grid lines over the plot.
const AREA_OPACITY: f64 = 0.75;
const GRID_OPACITY: f64 = 0.15;

/// The side of a legend's color swatch.
const SWATCH: f64 = 10.0;

/// How tall labels set at 45 degrees may run below the plot.
const MAX_ROTATED_LABELS: f64 = 120.0;

/// Milliseconds in a day, for how finely a time axis is labelled.
const DAY_MS: f64 = 86_400_000.0;

/// About how many ticks a time axis is given.
const TARGET_TIME_TICKS: i32 = 6;

// ========================================================================
// Data Structures
// ========================================================================

/// The colors a chart is drawn in besides its series: those of the
/// terminal it sits in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartStyle {
    /// Axes, labels, and the title.
    pub foreground: (u8, u8, u8),
}

/// A chart ready to draw: its rows gathered into series and laid out.
struct Plot {
    chart: Chart,
    /// The domain's categories, in order, when it is discrete.
    categories: Vec<String>,
    /// Whether bars run across, their categories down the side.
    horizontal: bool,
    layout: Layout,
    /// The measure axis.
    measure: Linear,
    /// The domain axis when it is a line rather than categories.
    span: Option<Linear>,
    series: Vec<Series>,
    /// Whether the series stack on one another rather than overlap.
    stacked: bool,
}

/// One series: its name, and its values in order along the domain.
struct Series {
    name: Option<String>,
    points: Vec<(Place, f64)>,
}

/// Where a value sits along the domain.
#[derive(Clone, Debug, PartialEq)]
enum Place {
    /// Along a line: a number, or a time in milliseconds.
    At(f64),
    /// In a category, by its index.
    Category(usize),
}

/// Where everything goes, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    /// The whole document.
    height: f64,
    /// The plot's top-left.
    left: f64,
    /// Where the legend starts, when there is one.
    legend: Option<f64>,
    /// Whether the domain's labels are set at 45 degrees to fit.
    rotated: bool,
    top: f64,
    width: f64,
}

// ========================================================================
// Plot
// ========================================================================

impl Plot {
    /// The plot `spec` describes, or `None` for one Winter does not draw.
    fn new(spec: &Value) -> Option<Self> {
        let chart = Chart::parse(spec)?;
        let (domain, measure, horizontal) = match (chart.mark, chart.x.kind, chart.y.kind) {
            (Mark::Bar, kind, _) if kind != Kind::Quantitative => (&chart.x, &chart.y, false),
            (Mark::Bar, _, kind) if kind != Kind::Quantitative => (&chart.y, &chart.x, true),
            (Mark::Bar, ..) => return None,
            (_, _, Kind::Quantitative) => (&chart.x, &chart.y, false),
            _ => return None,
        };
        if measure.kind != Kind::Quantitative {
            return None;
        }
        // Bars stand for a category each, a time included.
        let discrete = domain.is_discrete() || chart.mark == Mark::Bar;
        let (categories, series) = gather(&chart, domain, measure, discrete)?;
        let stacked = chart.color.is_some() && matches!(chart.mark, Mark::Area | Mark::Bar);
        let (low, high) = measure_range(&series, stacked)?;
        let (along, across) = match horizontal {
            true => (chart.height, chart.width),
            false => (chart.width, chart.height),
        };
        let measure_axis = Linear::nice(low.min(0.0), high.max(0.0), across);
        let span = match discrete {
            true => None,
            false => {
                let (low, high) = domain_range(&series)?;
                // Time runs from the first row to the last, its ticks on the
                // calendar; numbers widen to round ones, zero included.
                Some(match domain.kind {
                    Kind::Temporal => Linear::exact(low, high, along),
                    _ => Linear::nice(low.min(0.0), high, along),
                })
            }
        };
        let mut plot = Self {
            categories,
            chart,
            horizontal,
            layout: Layout {
                height: 0.0,
                left: 0.0,
                legend: None,
                rotated: false,
                top: 0.0,
                width: 0.0,
            },
            measure: measure_axis,
            series,
            span,
            stacked,
        };
        plot.layout = plot.lay_out();
        Some(plot)
    }

    /// The domain channel: the one the rows are placed along.
    fn domain(&self) -> &Channel {
        match self.horizontal {
            true => &self.chart.y,
            false => &self.chart.x,
        }
    }

    /// The labels along the domain, where they are drawn, and along the
    /// measure.
    fn domain_labels(&self) -> Vec<(f64, String)> {
        match &self.span {
            Some(span) => {
                let (ticks, labels) = match self.domain().kind {
                    Kind::Temporal => {
                        let ticks = time_ticks(span.low, span.high);
                        let labels = time_labels(&ticks, span.high - span.low);
                        (ticks, labels)
                    }
                    _ => {
                        let ticks = span.ticks();
                        let labels = number_labels(&ticks, span.step);
                        (ticks, labels)
                    }
                };
                ticks
                    .iter()
                    .map(|tick| span.place(*tick))
                    .zip(labels)
                    .collect()
            }
            None => {
                let band = self.band();
                self.categories
                    .iter()
                    .enumerate()
                    .map(|(index, label)| (band.middle(index), label.clone()))
                    .collect()
            }
        }
    }

    fn measure_labels(&self) -> Vec<(f64, String)> {
        let ticks = self.measure.ticks();
        let labels = number_labels(&ticks, self.measure.step);
        ticks
            .iter()
            .map(|tick| self.measure.place(*tick))
            .zip(labels)
            .collect()
    }

    /// The categories' bands along the domain.
    fn band(&self) -> Band {
        let length = match self.horizontal {
            true => self.chart.height,
            false => self.chart.width,
        };
        Band {
            count: self.categories.len(),
            length,
            padding: BAR_PADDING,
        }
    }

    /// Where the plot, the axes, the legend, and the title go.
    fn lay_out(&self) -> Layout {
        let widest = |labels: &[(f64, String)]| {
            labels
                .iter()
                .map(|(_, label)| text_width(label))
                .fold(0.0, f64::max)
        };
        let domain_labels = self.domain_labels();
        let measure_labels = self.measure_labels();
        let (left_labels, bottom_labels) = match self.horizontal {
            true => (&domain_labels, &measure_labels),
            false => (&measure_labels, &domain_labels),
        };
        // Labels that would run into each other across the bottom are set
        // at 45 degrees instead.
        let room = match (&self.span, self.horizontal) {
            (None, false) => self.band().step(),
            _ => f64::MAX,
        };
        let rotated = widest(bottom_labels) + GAP > room;
        let bottom = match rotated {
            true => {
                (widest(bottom_labels) * std::f64::consts::FRAC_1_SQRT_2).min(MAX_ROTATED_LABELS)
            }
            false => FONT_SIZE,
        } + TICK
            + GAP * 2.0
            + FONT_SIZE
            + GAP;
        let left = MARGIN + FONT_SIZE + GAP * 2.0 + widest(left_labels) + TICK;
        let top = match &self.chart.title {
            Some(_) => MARGIN + TITLE_SIZE + GAP * 2.0,
            None => MARGIN,
        };
        let names: Vec<&str> = self
            .series
            .iter()
            .filter_map(|series| series.name.as_deref())
            .collect();
        let legend_width = match names.is_empty() {
            true => 0.0,
            false => {
                let title = self.chart.color.as_deref().map_or(0.0, text_width);
                let widest = names
                    .iter()
                    .map(|name| text_width(name))
                    .fold(title, f64::max);
                MARGIN + SWATCH + GAP + widest
            }
        };
        let plot_right = left + self.chart.width;
        // A label centred on the plot's right edge runs half its width past
        // it.
        let overhang = match (&self.span, self.horizontal) {
            (Some(_), false) | (_, true) => bottom_labels
                .last()
                .map_or(0.0, |(_, label)| text_width(label) / 2.0),
            (None, false) => 0.0,
        };
        Layout {
            height: top + self.chart.height + bottom + MARGIN,
            left,
            legend: (legend_width > 0.0).then_some(plot_right + MARGIN),
            rotated,
            top,
            width: plot_right + legend_width.max(overhang) + MARGIN,
        }
    }

    /// The SVG document drawn in `style`.
    fn draw(&self, style: &ChartStyle) -> String {
        let (r, g, b) = style.foreground;
        let ink = format!("#{r:02x}{g:02x}{b:02x}");
        let layout = self.layout;
        let mut svg = String::new();
        let _ = write!(
            svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}" font-family="sans-serif" font-size="{FONT_SIZE}" fill="{ink}">"#,
            w = layout.width.ceil(),
            h = layout.height.ceil(),
        );
        if let Some(title) = &self.chart.title {
            let _ = write!(
                svg,
                r#"<text x="{x:.1}" y="{y:.1}" font-size="{TITLE_SIZE}" font-weight="bold" text-anchor="middle">{text}</text>"#,
                x = layout.left + self.chart.width / 2.0,
                y = MARGIN + TITLE_SIZE,
                text = escape(title),
            );
        }
        let _ = write!(
            svg,
            r#"<g transform="translate({:.1},{:.1})">"#,
            layout.left, layout.top
        );
        self.draw_grid(&mut svg, &ink);
        self.draw_marks(&mut svg);
        self.draw_axes(&mut svg, &ink);
        svg.push_str("</g>");
        self.draw_legend(&mut svg);
        svg.push_str("</svg>");
        svg
    }

    /// Faint lines across the plot at each of the measure's ticks.
    fn draw_grid(&self, svg: &mut String, ink: &str) {
        let (width, height) = (self.chart.width, self.chart.height);
        for (at, _) in self.measure_labels() {
            let (x1, y1, x2, y2) = match self.horizontal {
                true => (at, 0.0, at, height),
                false => (0.0, height - at, width, height - at),
            };
            let _ = write!(
                svg,
                r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{ink}" stroke-opacity="{GRID_OPACITY}"/>"#
            );
        }
    }

    /// The axes along the left and the bottom, their ticks, labels, and
    /// titles.
    fn draw_axes(&self, svg: &mut String, ink: &str) {
        let (width, height) = (self.chart.width, self.chart.height);
        let domain_labels = self.domain_labels();
        let measure_labels = self.measure_labels();
        let (left, bottom) = match self.horizontal {
            true => (&domain_labels, &measure_labels),
            false => (&measure_labels, &domain_labels),
        };
        let _ = write!(
            svg,
            r#"<line x1="0" y1="{height:.1}" x2="{width:.1}" y2="{height:.1}" stroke="{ink}"/><line x1="0" y1="0" x2="0" y2="{height:.1}" stroke="{ink}"/>"#
        );
        for (at, label) in bottom {
            let _ = write!(
                svg,
                r#"<line x1="{at:.1}" y1="{height:.1}" x2="{at:.1}" y2="{tick:.1}" stroke="{ink}"/>"#,
                tick = height + TICK
            );
            let y = height + TICK + GAP + FONT_SIZE;
            let _ = match self.layout.rotated {
                true => write!(
                    svg,
                    r#"<text x="{at:.1}" y="{y:.1}" text-anchor="end" transform="rotate(-45 {at:.1} {y:.1})">{}</text>"#,
                    escape(label)
                ),
                false => write!(
                    svg,
                    r#"<text x="{at:.1}" y="{y:.1}" text-anchor="middle">{}</text>"#,
                    escape(label)
                ),
            };
        }
        for (at, label) in left {
            // Down the side, values run upwards from the foot; categories
            // run down from the head.
            let y = match self.horizontal {
                true => *at,
                false => height - at,
            };
            let _ = write!(
                svg,
                r#"<line x1="{tick:.1}" y1="{y:.1}" x2="0" y2="{y:.1}" stroke="{ink}"/><text x="{x:.1}" y="{baseline:.1}" text-anchor="end">{}</text>"#,
                escape(label),
                tick = -TICK,
                x = -TICK - GAP,
                baseline = y + FONT_SIZE / 3.0,
            );
        }
        let (left_title, bottom_title) = (&self.chart.y.title, &self.chart.x.title);
        let below = self.layout.height - self.layout.top - MARGIN - GAP;
        let _ = write!(
            svg,
            r#"<text x="{x:.1}" y="{below:.1}" text-anchor="middle" font-weight="bold">{}</text>"#,
            escape(bottom_title),
            x = width / 2.0,
        );
        let side = -(self.layout.left - MARGIN - FONT_SIZE);
        let _ = write!(
            svg,
            r#"<text x="0" y="0" text-anchor="middle" font-weight="bold" transform="translate({side:.1},{y:.1}) rotate(-90)">{}</text>"#,
            escape(left_title),
            y = height / 2.0,
        );
    }

    /// The bars, lines, areas, or points.
    fn draw_marks(&self, svg: &mut String) {
        match self.chart.mark {
            Mark::Bar => self.draw_bars(svg),
            Mark::Area => self.draw_areas(svg),
            Mark::Line => self.draw_lines(svg),
            Mark::Point => self.draw_points(svg),
        }
    }

    fn draw_bars(&self, svg: &mut String) {
        let band = self.band();
        let mut base = vec![0.0; self.categories.len()];
        for (index, series) in self.series.iter().enumerate() {
            let color = PALETTE[index % PALETTE.len()];
            for (place, value) in &series.points {
                let Place::Category(at) = place else {
                    continue;
                };
                let from = if self.stacked { base[*at] } else { 0.0 };
                let to = from + value;
                if self.stacked {
                    base[*at] = to;
                }
                let (low, high) = (
                    self.measure.place(from.min(to)),
                    self.measure.place(from.max(to)),
                );
                let (x, y, w, h) = match self.horizontal {
                    true => (low, band.start(*at), high - low, band.width()),
                    false => (
                        band.start(*at),
                        self.chart.height - high,
                        band.width(),
                        high - low,
                    ),
                };
                let _ = write!(
                    svg,
                    r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{color}"/>"#
                );
            }
        }
    }

    /// Where along the domain `place` is drawn, in pixels.
    fn along(&self, place: &Place) -> f64 {
        match (place, &self.span) {
            (Place::At(value), Some(span)) => span.place(*value),
            (Place::Category(index), _) => self.band().middle(*index),
            (Place::At(_), None) => 0.0,
        }
    }

    /// The pixel point of `value` at `place`, the measure running upwards.
    fn point(&self, place: &Place, value: f64) -> (f64, f64) {
        (
            self.along(place),
            self.chart.height - self.measure.place(value),
        )
    }

    fn draw_lines(&self, svg: &mut String) {
        for (index, series) in self.series.iter().enumerate() {
            let color = PALETTE[index % PALETTE.len()];
            let points: Vec<String> = series
                .points
                .iter()
                .map(|(place, value)| {
                    let (x, y) = self.point(place, *value);
                    format!("{x:.1},{y:.1}")
                })
                .collect();
            let _ = write!(
                svg,
                r#"<polyline points="{}" fill="none" stroke="{color}" stroke-width="{LINE_WIDTH}" stroke-linejoin="round"/>"#,
                points.join(" ")
            );
        }
    }

    fn draw_points(&self, svg: &mut String) {
        for (index, series) in self.series.iter().enumerate() {
            let color = PALETTE[index % PALETTE.len()];
            for (place, value) in &series.points {
                let (x, y) = self.point(place, *value);
                let _ = write!(
                    svg,
                    r#"<circle cx="{x:.1}" cy="{y:.1}" r="{POINT_RADIUS}" fill="none" stroke="{color}" stroke-width="{LINE_WIDTH}"/>"#
                );
            }
        }
    }

    /// Areas filled down to the one below, or to zero for the first.
    fn draw_areas(&self, svg: &mut String) {
        let mut below: HashMap<String, f64> = HashMap::new();
        for (index, series) in self.series.iter().enumerate() {
            let color = PALETTE[index % PALETTE.len()];
            let mut top = Vec::new();
            let mut bottom = Vec::new();
            for (place, value) in &series.points {
                let key = format!("{place:?}");
                let base = if self.stacked {
                    below.get(&key).copied().unwrap_or(0.0)
                } else {
                    0.0
                };
                top.push(self.point(place, base + value));
                bottom.push(self.point(place, base));
                if self.stacked {
                    below.insert(key, base + value);
                }
            }
            bottom.reverse();
            let outline: Vec<String> = top
                .iter()
                .chain(&bottom)
                .map(|(x, y)| format!("{x:.1},{y:.1}"))
                .collect();
            let _ = write!(
                svg,
                r#"<polygon points="{}" fill="{color}" fill-opacity="{AREA_OPACITY}"/>"#,
                outline.join(" ")
            );
        }
    }

    /// The series' names beside their colors, titled by the color field.
    fn draw_legend(&self, svg: &mut String) {
        let Some(x) = self.layout.legend else {
            return;
        };
        let mut y = self.layout.top;
        if let Some(title) = &self.chart.color {
            let _ = write!(
                svg,
                r#"<text x="{x:.1}" y="{baseline:.1}" font-weight="bold">{}</text>"#,
                escape(title),
                baseline = y + FONT_SIZE,
            );
            y += FONT_SIZE + GAP * 2.0;
        }
        for (index, series) in self.series.iter().enumerate() {
            let Some(name) = &series.name else {
                continue;
            };
            let color = PALETTE[index % PALETTE.len()];
            let _ = write!(
                svg,
                r#"<rect x="{x:.1}" y="{y:.1}" width="{SWATCH}" height="{SWATCH}" fill="{color}"/><text x="{tx:.1}" y="{baseline:.1}">{}</text>"#,
                escape(name),
                tx = x + SWATCH + GAP,
                baseline = y + SWATCH - 1.0,
            );
            y += FONT_SIZE + GAP;
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The SVG of the chart `spec` describes, drawn in `style`, or `None` for a
/// spec Winter does not draw.
pub fn render(spec: &Value, style: &ChartStyle) -> Option<String> {
    Some(Plot::new(spec)?.draw(style))
}

/// How big the chart `spec` describes is drawn, in pixels, or `None` for a
/// spec Winter does not draw.
pub fn size(spec: &Value) -> Option<(f32, f32)> {
    let layout = Plot::new(spec)?.layout;
    Some((layout.width.ceil() as f32, layout.height.ceil() as f32))
}

/// The rows of `chart` gathered into series along the domain, combined where
/// they share a place: the categories in order when the domain is
/// `discrete`, and the series in the order they first appear.
fn gather(
    chart: &Chart,
    domain: &Channel,
    measure: &Channel,
    discrete: bool,
) -> Option<(Vec<String>, Vec<Series>)> {
    let mut categories: Vec<String> = Vec::new();
    let mut names: Vec<Option<String>> = Vec::new();
    let mut grouped: HashMap<(usize, String), Vec<f64>> = HashMap::new();
    let mut order: Vec<(usize, String)> = Vec::new();
    let mut lone: Vec<Vec<(Place, f64)>> = Vec::new();
    // Bars and areas, and anything asked to, combine the rows sharing a
    // place; lines and points with no aggregate draw every row.
    let combine = measure.aggregate.is_some() || matches!(chart.mark, Mark::Area | Mark::Bar);
    for row in &chart.rows {
        let name = chart.color.as_ref().map(|field| label(row.get(field)));
        let series = match names.iter().position(|known| *known == name) {
            Some(at) => at,
            None => {
                names.push(name);
                lone.push(Vec::new());
                names.len() - 1
            }
        };
        let value = match measure.aggregate {
            Some(Aggregate::Count) => 1.0,
            _ => number(field_of(row, measure)?)?,
        };
        let key = match discrete {
            true => label(field_of(row, domain)),
            false => {
                let at = match domain.kind {
                    Kind::Temporal => timestamp(field_of(row, domain)?)?,
                    _ => number(field_of(row, domain)?)?,
                };
                format!("{at}")
            }
        };
        if discrete && !categories.contains(&key) {
            categories.push(key.clone());
        }
        match combine {
            true => {
                let slot = (series, key);
                if !grouped.contains_key(&slot) {
                    order.push(slot.clone());
                }
                grouped.entry(slot).or_default().push(value);
            }
            false => {
                let place = match discrete {
                    true => Place::Category(categories.iter().position(|c| *c == key)?),
                    false => Place::At(key.parse().ok()?),
                };
                lone[series].push((place, value));
            }
        }
    }
    sort_categories(&mut categories, domain.kind);
    let aggregate = match measure.aggregate {
        Some(Aggregate::Count) => Aggregate::Sum,
        Some(aggregate) => aggregate,
        None => Aggregate::Sum,
    };
    for (series, key) in order {
        let values = &grouped[&(series, key.clone())];
        let place = match discrete {
            true => Place::Category(categories.iter().position(|c| *c == key)?),
            false => Place::At(key.parse().ok()?),
        };
        lone[series].push((place, aggregate.combine(values)));
    }
    let series = names
        .into_iter()
        .zip(lone)
        .map(|(name, mut points)| {
            points.sort_by(|a, b| place_order(&a.0).total_cmp(&place_order(&b.0)));
            Series { name, points }
        })
        .collect();
    Some((categories, series))
}

/// Where `place` falls in order along the domain.
fn place_order(place: &Place) -> f64 {
    match place {
        Place::At(value) => *value,
        Place::Category(index) => *index as f64,
    }
}

/// Categories in the order an axis lists them: as numbers when they all are
/// numbers, otherwise as text, nominal fields included, as Vega sorts them.
fn sort_categories(categories: &mut [String], kind: Kind) {
    if kind == Kind::Temporal {
        categories.sort_by(|a, b| {
            let at = |text: &str| timestamp(&Value::String(text.to_string())).unwrap_or(0.0);
            at(a).total_cmp(&at(b))
        });
        return;
    }
    if categories.iter().all(|c| c.parse::<f64>().is_ok()) {
        categories.sort_by(|a, b| {
            let number = |text: &str| text.parse::<f64>().unwrap_or(0.0);
            number(a).total_cmp(&number(b))
        });
        return;
    }
    categories.sort();
}

/// The lowest and highest the measure reaches, stacked series summed.
fn measure_range(series: &[Series], stacked: bool) -> Option<(f64, f64)> {
    let values: Vec<f64> = match stacked {
        true => {
            let mut totals: HashMap<String, f64> = HashMap::new();
            for series in series {
                for (place, value) in &series.points {
                    *totals.entry(format!("{place:?}")).or_default() += value;
                }
            }
            totals.into_values().collect()
        }
        false => series
            .iter()
            .flat_map(|series| series.points.iter().map(|(_, value)| *value))
            .collect(),
    };
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (low.is_finite() && high.is_finite()).then_some((low, high))
}

/// The lowest and highest place along a continuous domain.
fn domain_range(series: &[Series]) -> Option<(f64, f64)> {
    let places: Vec<f64> = series
        .iter()
        .flat_map(|series| series.points.iter().map(|(place, _)| place_order(place)))
        .collect();
    let low = places.iter().copied().fold(f64::INFINITY, f64::min);
    let high = places.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (low.is_finite() && high.is_finite()).then_some((low, high))
}

/// The value of `row` in `channel`'s field.
fn field_of<'r>(row: &'r Map<String, Value>, channel: &Channel) -> Option<&'r Value> {
    row.get(channel.field.as_deref()?)
}

/// A value as a number, written as one or as text.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|value: &f64| value.is_finite())
}

/// A value as a time in milliseconds: a number of them, or a date written
/// the ways data usually writes one.
fn timestamp(value: &Value) -> Option<f64> {
    let text = match value {
        Value::Number(number) => return number.as_f64(),
        Value::String(text) => text.trim(),
        _ => return None,
    };
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Some(at.timestamp_millis() as f64);
    }
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(at) = NaiveDateTime::parse_from_str(text, format) {
            return Some(at.and_utc().timestamp_millis() as f64);
        }
    }
    let date = NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(&format!("{text}-01"), "%Y-%m-%d"))
        .or_else(|_| NaiveDate::parse_from_str(&format!("{text}-01-01"), "%Y-%m-%d"))
        .ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() as f64)
}

/// A value as a category's label.
fn label(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => "null".to_string(),
        Some(value) => value.to_string(),
    }
}

/// Labels for `ticks` a `step` apart: as few decimals as tell them apart,
/// and thousands and millions shortened.
fn number_labels(ticks: &[f64], step: f64) -> Vec<String> {
    let largest = ticks.iter().map(|tick| tick.abs()).fold(0.0, f64::max);
    let (divisor, suffix) = match largest {
        l if l >= 1e9 => (1e9, "B"),
        l if l >= 1e6 => (1e6, "M"),
        l if l >= 1e4 => (1e3, "k"),
        _ => (1.0, ""),
    };
    let scaled = step / divisor;
    let decimals = match scaled >= 1.0 {
        true => 0,
        false => (-scaled.log10().floor()) as usize,
    };
    ticks
        .iter()
        .map(|tick| format!("{:.*}{suffix}", decimals, tick / divisor))
        .collect()
}

/// Where ticks go on a time axis from `low` to `high` milliseconds: on the
/// first of years, of months, on midnights, or on hours, whichever puts
/// about five across it.
fn time_ticks(low: f64, high: f64) -> Vec<f64> {
    let span = high - low;
    let (Some(start), Some(end)) = (
        DateTime::from_timestamp_millis(low as i64),
        DateTime::from_timestamp_millis(high as i64),
    ) else {
        return Vec::new();
    };
    let millis = |at: NaiveDateTime| at.and_utc().timestamp_millis() as f64;
    let within = |at: f64| at >= low && at <= high;
    if span > 730.0 * DAY_MS {
        let years = (end.year() - start.year()).max(1);
        let step = calendar_step(years, &[1, 2, 5, 10, 20, 50, 100]);
        return (start.year()..=end.year())
            .filter(|year| year % step == 0)
            .filter_map(|year| NaiveDate::from_ymd_opt(year, 1, 1)?.and_hms_opt(0, 0, 0))
            .map(millis)
            .filter(|at| within(*at))
            .collect();
    }
    if span > 60.0 * DAY_MS {
        let months = (end.year() - start.year()) * 12 + end.month() as i32 - start.month() as i32;
        let step = calendar_step(months.max(1), &[1, 2, 3, 6, 12]);
        let mut ticks = Vec::new();
        let (mut year, mut month) = (start.year(), start.month());
        while (year, month) <= (end.year(), end.month()) {
            if (month as i32 - 1) % step == 0 {
                if let Some(at) =
                    NaiveDate::from_ymd_opt(year, month, 1).and_then(|day| day.and_hms_opt(0, 0, 0))
                {
                    ticks.push(millis(at));
                }
            }
            (year, month) = match month {
                12 => (year + 1, 1),
                _ => (year, month + 1),
            };
        }
        return ticks.into_iter().filter(|at| within(*at)).collect();
    }
    let (unit, steps): (f64, &[i32]) = match span > 2.0 * DAY_MS {
        true => (DAY_MS, &[1, 2, 7, 14]),
        false => (DAY_MS / 24.0, &[1, 2, 3, 6, 12]),
    };
    let step = calendar_step((span / unit).ceil() as i32, steps) as f64 * unit;
    // From the first whole day, or hour, the axis shows.
    let first = (low / unit).ceil() * unit;
    (0..)
        .map(|at| first + step * at as f64)
        .take_while(|at| *at <= high)
        .collect()
}

/// The smallest of `steps` that puts no more than about five ticks across
/// `count` units.
fn calendar_step(count: i32, steps: &[i32]) -> i32 {
    steps
        .iter()
        .copied()
        .find(|step| count / step <= TARGET_TIME_TICKS)
        .unwrap_or_else(|| steps.last().copied().unwrap_or(1))
}

/// Labels for times `ticks` across `span` milliseconds: years, months, days,
/// or hours, whichever tells them apart.
fn time_labels(ticks: &[f64], span: f64) -> Vec<String> {
    let format = match span {
        s if s > 730.0 * DAY_MS => "%Y",
        s if s > 60.0 * DAY_MS => "%Y-%m",
        s if s > 2.0 * DAY_MS => "%m-%d",
        _ => "%H:%M",
    };
    ticks
        .iter()
        .map(|tick| {
            DateTime::from_timestamp_millis(*tick as i64)
                .map(|at| at.format(format).to_string())
                .unwrap_or_default()
        })
        .collect()
}

/// About how wide `text` is set.
fn text_width(text: &str) -> f64 {
    text.chars().count() as f64 * CHAR_WIDTH
}

/// `text` safe inside an SVG element.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const STYLE: ChartStyle = ChartStyle {
        foreground: (200, 200, 200),
    };

    fn bars() -> Value {
        json!({
            "data": {"values": [
                {"k": "b", "v": 2, "s": "x"}, {"k": "a", "v": 3, "s": "x"},
                {"k": "a", "v": 4, "s": "y"}, {"k": "b", "v": 1, "s": "x"}
            ]},
            "mark": "bar",
            "encoding": {
                "x": {"field": "k", "type": "nominal"},
                "y": {"field": "v", "type": "quantitative"},
                "color": {"field": "s"}
            },
            "title": "Stacked <totals>"
        })
    }

    #[test]
    fn test_bars_stack_by_color_and_sum_what_shares_a_place() {
        let plot = Plot::new(&bars()).expect("a chart");
        assert_eq!(plot.categories, ["a", "b"], "sorted as Vega sorts them");
        assert!(plot.stacked);
        let x = &plot.series[0];
        assert_eq!(x.name.as_deref(), Some("x"));
        assert_eq!(
            x.points,
            vec![(Place::Category(0), 3.0), (Place::Category(1), 3.0)],
            "b's two rows are summed"
        );
        assert!(plot.measure.high >= 7.0, "the stack of a reaches 7");
    }

    #[test]
    fn test_the_document_draws_every_bar_a_legend_and_an_escaped_title() {
        let svg = render(&bars(), &STYLE).expect("an SVG");
        assert_eq!(
            svg.matches("<rect").count(),
            3 + 2,
            "three bars, two swatches"
        );
        assert!(svg.contains("Stacked &lt;totals&gt;"));
        assert!(svg.contains("#c8c8c8"), "in the terminal's own ink");
        let (width, height) = size(&bars()).expect("a size");
        assert!(svg.starts_with(&format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}""#
        )));
    }

    #[test]
    fn test_a_line_over_time_is_placed_along_the_dates() {
        let spec = json!({
            "data": {"values": [
                {"d": "2024-03-01", "v": 5}, {"d": "2024-01-01", "v": 1},
                {"d": "2024-02-01", "v": 3}
            ]},
            "mark": "line",
            "encoding": {"x": {"field": "d", "type": "temporal"},
                         "y": {"field": "v", "type": "quantitative"}}
        });
        let plot = Plot::new(&spec).expect("a chart");
        let values: Vec<f64> = plot.series[0].points.iter().map(|(_, v)| *v).collect();
        assert_eq!(values, [1.0, 3.0, 5.0], "in date order, not row order");
        let labels: Vec<String> = plot.domain_labels().into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            labels,
            ["01-01", "01-15", "01-29", "02-12", "02-26"],
            "by the day across two months, from the first, not as raw milliseconds"
        );
    }

    #[test]
    fn test_time_ticks_fall_on_the_calendar() {
        let at = |text: &str| timestamp(&Value::String(text.to_string())).expect("a date");
        let ticks = time_ticks(at("2024-01-15"), at("2024-12-20"));
        let labels = time_labels(&ticks, at("2024-12-20") - at("2024-01-15"));
        assert_eq!(
            labels,
            ["2024-03", "2024-05", "2024-07", "2024-09", "2024-11"]
        );
        let ticks = time_ticks(at("2001-06-01"), at("2024-06-01"));
        assert_eq!(
            time_labels(&ticks, at("2024-06-01") - at("2001-06-01")),
            ["2005", "2010", "2015", "2020"]
        );
    }

    #[test]
    fn test_horizontal_bars_put_their_categories_down_the_side() {
        let spec = json!({
            "data": {"values": [{"k": "a", "v": 3}, {"k": "b", "v": 5}]},
            "mark": "bar",
            "encoding": {"x": {"field": "v", "type": "quantitative"},
                         "y": {"field": "k", "type": "nominal"}}
        });
        let plot = Plot::new(&spec).expect("a chart");
        assert!(plot.horizontal);
        assert_eq!(plot.categories, ["a", "b"]);
    }

    #[test]
    fn test_labels_drop_decimals_they_do_not_need_and_shorten_big_numbers() {
        assert_eq!(number_labels(&[0.0, 0.5, 1.0], 0.5), ["0.0", "0.5", "1.0"]);
        assert_eq!(
            number_labels(&[0.0, 20000.0, 40000.0], 20000.0),
            ["0k", "20k", "40k"]
        );
        assert_eq!(number_labels(&[0.0, 2.0, 4.0], 2.0), ["0", "2", "4"]);
    }

    #[test]
    fn test_many_long_categories_are_set_at_an_angle() {
        let rows: Vec<Value> = (0..30)
            .map(|at| json!({"k": format!("a long category {at}"), "v": at}))
            .collect();
        let spec = json!({
            "data": {"values": rows},
            "mark": "bar",
            "encoding": {"x": {"field": "k", "type": "nominal"},
                         "y": {"field": "v", "type": "quantitative"}}
        });
        assert!(Plot::new(&spec).expect("a chart").layout.rotated);
    }

    #[test]
    fn test_a_mark_with_no_numbers_to_show_is_refused() {
        let spec = json!({
            "data": {"values": [{"k": "a", "v": "x"}]},
            "mark": "line",
            "encoding": {"x": {"field": "k", "type": "nominal"},
                         "y": {"field": "v", "type": "nominal"}}
        });
        assert!(render(&spec, &STYLE).is_none());
    }
}
