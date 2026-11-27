//! How the system monitor lays a snapshot out: the machine in one line by
//! the heading, then a responsive dashboard of cards using sparklines,
//! vertical core bars, compact capacity rows, and traffic rates.

use std::borrow::Cow;

use misty::layout::Align;
use misty::view::{Extent, Space, canvas, column, item, row, section, sparkline};
use misty::{BoxStyle, Element, Rect, ThemeToken};

use winter_model::system::{
    CoreReading, CpuSample, DiskReading, GpuReading, MemoryReading, SystemSample,
};

use crate::table::text;
use crate::units::{format_kb, format_percent};

use super::traffic::TrafficRate;
use super::usage::Usage;

// ========================================================================
// Constants
// ========================================================================

/// A share at or past this reads as trouble, and at or past [`WARNING_PERCENT`]
/// as something to watch.
const CRITICAL_PERCENT: f64 = 90.0;
const WARNING_PERCENT: f64 = 75.0;

/// The ids of the cards, the sections `]` and `[` move between.
pub const CPU: &str = "sys.cpu";
pub const MEMORY: &str = "sys.memory";
pub const GPU: &str = "sys.gpu";
pub const DISKS: &str = "sys.disks";
pub const NETWORK: &str = "sys.network";
pub const DISK_IO: &str = "sys.disk_io";

/// The cards' headings.
const CPU_TITLE: &str = "CPU";
const MEMORY_TITLE: &str = "Memory";
const GPU_TITLE: &str = "GPU";
const DISKS_TITLE: &str = "Disks";
const NETWORK_TITLE: &str = "Network";
const DISK_IO_TITLE: &str = "Disk Activity";

/// What a traffic card says before it has two snapshots to measure between.
const MEASURING: &str = "measuring…";

/// The marks before a network interface's received and sent rates.
const RECEIVED_MARK: char = '\u{2193}';
const SENT_MARK: char = '\u{2191}';

/// Bytes in a kilobyte, for wording a rate in the units sizes use.
const BYTES_PER_KB: u64 = 1024;

/// The least a traffic history line is scaled to, in bytes a second, so a
/// quiet device draws a flat line rather than noise blown up to the top.
const MIN_RATE_CEILING: f32 = 64.0 * 1024.0;

/// How wide a traffic rate's cell is, in text columns.
const RATE_COLS: f32 = 11.0;

/// What the load in the CPU card means, for its tooltip.
const LOAD_ABOUT: &str = "Load: how many processes are running or waiting to, on average over the last 1, 5, and \
     15 minutes";

/// Separates inline facts in headings and subtitles.
const FACT_SEPARATOR: &str = " \u{b7} ";

/// How many cores fit on one row of the vertical core bar chart before
/// wrapping onto a second row.
const MAX_CORES_PER_ROW: usize = 32;

/// Height of the per-core vertical bar chart in text rows (single and wrapped).
const CORE_CHART_ROWS: f32 = 3.0;
const CORE_CHART_WRAPPED_ROWS: f32 = 4.5;

/// Gap between vertical core bars and between wrapped core rows, in pixels.
const CORE_BAR_GAP_PX: f32 = 2.0;

/// Corner radius for vertical core bars, in pixels.
const CORE_BAR_RADIUS_PX: f32 = 2.0;

/// Minimum visible bar height so idle cores still show a baseline, in pixels.
const MIN_BAR_HEIGHT_PX: f32 = 2.0;

/// Height of the thin flat capacity bar in text rows, and its drawn
/// thickness and corner radius in pixels.
const FLAT_BAR_ROWS: f32 = 0.25;
const FLAT_BAR_HEIGHT_PX: f32 = 2.0;
const FLAT_BAR_RADIUS_PX: f32 = 1.0;

/// Fixed column widths for compact single-line readings.
const SHORT_LABEL_COLS: f32 = 6.0;
const SHARE_COLS: f32 = 7.0;

/// The room around a reading's line, in text columns across and rows
/// down, as a page's items have.
const READING_PADDING: (f32, f32) = (1.0, 0.2);

/// Vertical gap between a reading's header and its thin capacity bar.
const BLOCK_GAP_ROWS: f32 = 0.25;

/// Gap between items in a horizontal reading row.
const ROW_GAP_COLS: f32 = 1.0;

/// Tenths of a percent in a percent.
const TENTHS: f64 = 10.0;

/// Tenths of a percent in a full 100%.
const FULL_TENTHS: u32 = 1000;

/// The share a bar fills for a whole.
const WHOLE_PERCENT: f32 = 100.0;

/// The readings a history line needs before it is drawn: a line takes two.
const MIN_HISTORY: usize = 2;

/// What the CPU and memory history lines are scaled to: a whole.
pub const HISTORY_CEILING: f32 = WHOLE_PERCENT;

/// Seconds in a minute, an hour and a day.
const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

// ========================================================================
// Data Structures
// ========================================================================

/// The recent history the cards trace: the processor's total load and the
/// memory in use, as percentages, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub cpu: Vec<f32>,
    /// Every disk's bytes a second, read and written together.
    pub disk_io: Vec<f32>,
    pub memory: Vec<f32>,
    /// Every interface's bytes a second, received and sent together.
    pub network: Vec<f32>,
}

/// How fast each interface and disk moved bytes, `None` until two snapshots
/// are in.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Rates {
    pub disk_io: Option<Vec<TrafficRate>>,
    pub network: Option<Vec<TrafficRate>>,
}

// ========================================================================
// Functions
// ========================================================================

/// The ids of the cards `sample` shows, in reading order: the traffic cards
/// only where the system counts it, a GPU card only where there is a GPU.
pub fn card_ids(sample: &SystemSample) -> Vec<&'static str> {
    let mut ids = vec![CPU, MEMORY];
    if !sample.network.is_empty() {
        ids.push(NETWORK);
    }
    ids.push(DISKS);
    if !sample.disk_io.is_empty() {
        ids.push(DISK_IO);
    }
    if !sample.gpus.is_empty() {
        ids.push(GPU);
    }
    ids
}

/// The machine in one line, for the heading: its name, its system, and how
/// long it has been up.
pub fn machine_line<'a, A: 'static>(sample: &SystemSample) -> Element<'a, A> {
    let overview = &sample.overview;
    let facts = [
        overview.hostname.clone(),
        format!("{} {} ({})", overview.os, overview.release, overview.arch),
        format!("up {}", format_uptime(overview.uptime_secs)),
    ];
    text(facts.join(FACT_SEPARATOR)).muted().into()
}

/// The dashboard for `sample`: its cards stacked in a single column so
/// labels and amounts keep their room, with history lines from `history`.
pub fn dashboard<'a, A: 'static>(
    sample: &'a SystemSample,
    usage: &'a Usage,
    rates: &'a Rates,
    history: &'a History,
) -> Element<'a, A> {
    let mut cards = vec![
        cpu_card(sample, usage, history),
        memory_card(&sample.memory, history),
    ];
    if !sample.network.is_empty() {
        cards.push(traffic_card(
            NETWORK,
            NETWORK_TITLE,
            rates.network.as_deref(),
            &history.network,
            network_rates,
        ));
    }
    cards.push(disk_card(&sample.disks));
    if !sample.disk_io.is_empty() {
        cards.push(traffic_card(
            DISK_IO,
            DISK_IO_TITLE,
            rates.disk_io.as_deref(),
            &history.disk_io,
            disk_rates,
        ));
    }
    if !sample.gpus.is_empty() {
        cards.push(gpu_card(&sample.gpus));
    }
    column(cards)
}

/// A duration as `3d 4h 5m`, leaving off the units that are zero at the front.
pub fn format_uptime(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / DAY, seconds % DAY / HOUR, seconds % HOUR / MINUTE);
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if days > 0 || hours > 0 {
        parts.push(format!("{hours}h"));
    }
    parts.push(format!("{minutes}m"));
    parts.join(" ")
}

/// The machine's total with its model and load, its history, then a vertical
/// bar chart of each core's load.
fn cpu_card<'a, A: 'static>(
    sample: &'a SystemSample,
    usage: &'a Usage,
    history: &'a History,
) -> Element<'a, A> {
    let cpu = &sample.cpu;
    let mut card = section(CPU_TITLE);

    if let Some(summary) = cpu_summary(cpu) {
        let mut line =
            Element::from(text(summary).muted()).padding(READING_PADDING.0, READING_PADDING.1);
        if cpu.load.is_some() {
            line = line.tooltip(LOAD_ABOUT);
        }
        card = card.item(line);
    }

    card = card
        .item(headline_reading(
            "sys.cpu.total",
            "Total",
            usage.total,
            None,
        ))
        .item(history_line(&history.cpu, "sys.cpu.history"));

    if !cpu.cores.is_empty() {
        card = card.item(core_bars(&cpu.cores, &usage.cores));
    }

    Element::from(card).id(CPU)
}

/// Model, core count, and load averages for the CPU card's summary line.
fn cpu_summary(cpu: &CpuSample) -> Option<String> {
    let mut parts = Vec::new();
    if !cpu.model.is_empty() {
        parts.push(cpu.model.clone());
    }
    if !cpu.cores.is_empty() {
        parts.push(counted_cores(cpu.cores.len()));
    }
    if let Some([one, five, fifteen]) = cpu.load {
        let load = |hundredths: u32| format!("{:.2}", f64::from(hundredths) / 100.0);
        parts.push(format!(
            "load {} {} {}",
            load(one),
            load(five),
            load(fifteen)
        ));
    }
    (!parts.is_empty()).then(|| parts.join(FACT_SEPARATOR))
}

/// Physical memory and its history, and swap where there is any.
fn memory_card<'a, A: 'static>(memory: &MemoryReading, history: &'a History) -> Element<'a, A> {
    let used = memory.total_kb.saturating_sub(memory.available_kb);
    let mut card = section(MEMORY_TITLE)
        .item(headline_reading(
            "sys.memory.ram",
            "RAM",
            tenths_of(used, memory.total_kb),
            Some(amounts(used, memory.total_kb)),
        ))
        .item(history_line(&history.memory, "sys.memory.history"));
    if memory.swap_total_kb > 0 {
        card = card.item(compact_reading(
            "sys.memory.swap",
            "Swap",
            tenths_of(memory.swap_used_kb, memory.swap_total_kb),
            amounts(memory.swap_used_kb, memory.swap_total_kb),
        ));
    }
    Element::from(card).id(MEMORY)
}

/// Each card's name, video memory, load, and temperature on a two-line block.
fn gpu_card<'a, A: 'static>(gpus: &'a [GpuReading]) -> Element<'a, A> {
    let rows = gpus.iter().enumerate().map(|(index, gpu)| {
        let mut facts = vec![
            amounts(gpu.mem_used_kb, gpu.mem_total_kb),
            format!("load {}%", gpu.util_percent),
        ];
        if let Some(temp) = gpu.temp_c {
            facts.push(format!("{temp}\u{b0}C"));
        }
        let tenths = tenths_of(gpu.mem_used_kb, gpu.mem_total_kb);
        block_reading(
            format!("sys.gpu.{index}"),
            &gpu.name,
            None,
            tenths,
            facts.join(FACT_SEPARATOR),
        )
    });
    Element::from(section(GPU_TITLE).items(rows)).id(GPU)
}

/// Each mounted volume on a two-line block so mount paths and sizes keep
/// their full text.
fn disk_card<'a, A: 'static>(disks: &'a [DiskReading]) -> Element<'a, A> {
    let card = match disks.is_empty() {
        true => section(DISKS_TITLE).item(item("no local disk volumes found")),
        false => section(DISKS_TITLE).items(disks.iter().map(|disk| {
            let tenths = tenths_of(disk.used_kb, disk.total_kb);
            block_reading(
                format!("sys.disk.{}", disk.mount),
                &disk.mount,
                disk.fs_type.as_deref(),
                tenths,
                amounts(disk.used_kb, disk.total_kb),
            )
        })),
    };
    Element::from(card).id(DISKS)
}

/// A card of traffic, known as `id`: the history of every device's rate
/// together, then a line per device with its rates as `rates_of` words
/// them, or what it waits on before the second snapshot.
fn traffic_card<'a, A: 'static>(
    id: &'static str,
    title: &'static str,
    rates: Option<&'a [TrafficRate]>,
    history: &'a [f32],
    rates_of: fn(&TrafficRate) -> [(String, String); 2],
) -> Element<'a, A> {
    let Some(rates) = rates else {
        return Element::from(section(title).item(item(MEASURING))).id(id);
    };
    let lines = rates.iter().map(move |rate| {
        let mut cells = vec![Element::from(text(rate.name.clone())).grow(1.0)];
        let mut about = Vec::new();
        for (shown, said) in rates_of(rate) {
            cells.push(Element::from(text(shown).muted()).width(Extent::Cells(RATE_COLS)));
            about.push(said);
        }
        row(cells)
            .gap(Space::Cells(ROW_GAP_COLS))
            .padding(READING_PADDING.0, READING_PADDING.1)
            .id(format!("{id}.{}", rate.name))
            .tooltip(about.join(", "))
    });
    Element::from(
        section(title)
            .item(rate_history_line(history, id))
            .items(lines),
    )
    .id(id)
}

/// An interface's rates as its line shows them, marked, and as its tooltip
/// says them.
fn network_rates(rate: &TrafficRate) -> [(String, String); 2] {
    let (received, sent) = (format_rate(rate.in_per_sec), format_rate(rate.out_per_sec));
    [
        (
            format!("{RECEIVED_MARK} {received}"),
            format!("Receiving {received}"),
        ),
        (format!("{SENT_MARK} {sent}"), format!("sending {sent}")),
    ]
}

/// A disk's rates as its line shows them, and as its tooltip says them.
fn disk_rates(rate: &TrafficRate) -> [(String, String); 2] {
    let (read, written) = (format_rate(rate.in_per_sec), format_rate(rate.out_per_sec));
    [
        (format!("read {read}"), format!("Reading {read}")),
        (format!("write {written}"), format!("writing {written}")),
    ]
}

/// Bytes a second in the units sizes use: `1.2M/s`.
fn format_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_kb(bytes_per_sec / BYTES_PER_KB))
}

/// A line of a traffic card's `history`, scaled to its peak, known as
/// `{id}.history`.
fn rate_history_line<'a, A: 'static>(history: &'a [f32], id: &str) -> Element<'a, A> {
    let peak = history.iter().copied().fold(0.0, f32::max);
    let line = column([sparkline(
        history,
        peak.max(MIN_RATE_CEILING),
        ThemeToken::Data,
    )])
    .padding(READING_PADDING.0, READING_PADDING.1)
    .id(format!("{id}.history"));
    if history.len() < MIN_HISTORY {
        return line;
    }
    let now = history.last().copied().unwrap_or_default();
    line.tooltip(format!(
        "The last {} readings, both ways together: now {}, highest {}",
        history.len(),
        format_rate(now as u64),
        format_rate(peak as u64)
    ))
}

/// A headline reading paired with a sparkline: label, optional facts, and
/// right-aligned percentage without a redundant horizontal bar.
fn headline_reading<'a, A: 'static>(
    id: impl Into<Cow<'a, str>>,
    label: &str,
    tenths: u32,
    facts: Option<String>,
) -> Element<'a, A> {
    let tone = tone(tenths);
    let mut cells = vec![
        Element::from(text(label.to_string())).width(Extent::Cells(SHORT_LABEL_COLS)),
        Element::from(text(facts.unwrap_or_default()).muted()).grow(1.0),
        share_cell(tenths, tone),
    ];
    row(cells.drain(..))
        .gap(Space::Cells(ROW_GAP_COLS))
        .padding(READING_PADDING.0, READING_PADDING.1)
        .id(id)
}

/// A compact single-line reading for short labels (`Swap`) with a thin
/// flat capacity bar underneath.
fn compact_reading<'a, A: 'static>(
    id: impl Into<Cow<'a, str>>,
    label: &str,
    tenths: u32,
    facts: String,
) -> Element<'a, A> {
    let tone = tone(tenths);
    let header = row([
        Element::from(text(label.to_string())).width(Extent::Cells(SHORT_LABEL_COLS)),
        Element::from(text(facts).muted()).grow(1.0),
        share_cell(tenths, tone),
    ])
    .gap(Space::Cells(ROW_GAP_COLS));

    column([header, flat_bar(share(tenths), tone)])
        .gap(Space::Cells(BLOCK_GAP_ROWS))
        .padding(READING_PADDING.0, READING_PADDING.1)
        .id(id)
}

/// A compact capacity reading with label, details, and a thin flat bar
/// underneath, so long names never truncate.
fn block_reading<'a, A: 'static>(
    id: impl Into<Cow<'a, str>>,
    label: &str,
    sublabel: Option<&str>,
    tenths: u32,
    facts: String,
) -> Element<'a, A> {
    let tone = tone(tenths);
    let mut left = vec![text(label.to_string()).into()];
    if let Some(sub) = sublabel {
        left.push(text(sub.to_string()).muted().into());
    }
    let cells = [
        row(left).gap(Space::Cells(ROW_GAP_COLS)).grow(1.0),
        text(facts).muted().into(),
        share_cell(tenths, tone),
    ];
    let header = row(cells).gap(Space::Cells(ROW_GAP_COLS));

    column([header, flat_bar(share(tenths), tone)])
        .gap(Space::Cells(BLOCK_GAP_ROWS))
        .padding(READING_PADDING.0, READING_PADDING.1)
        .id(id)
}

/// A thin flat capacity bar that reads as a usage indicator rather than a
/// scrollbar.
fn flat_bar<'a, A: 'static>(fraction: f32, tone: ThemeToken) -> Element<'a, A> {
    let fraction = fraction.clamp(0.0, 1.0);
    let bar: Element<'a, A> = canvas(move |frame, area, theme| {
        let y = (area.y + (area.height - FLAT_BAR_HEIGHT_PX) / 2.0).round();
        let track = Rect::new(area.x, y, area.width, FLAT_BAR_HEIGHT_PX);
        let style = |color: ThemeToken| BoxStyle {
            border: None,
            corner_radius: FLAT_BAR_RADIUS_PX,
            fill: theme.color(color).into(),
            shadow: None,
        };
        frame.draw_box(track, &style(ThemeToken::Border));
        let filled_width = (area.width * fraction).round();
        if filled_width > 0.0 {
            let filled = Rect::new(area.x, y, filled_width, FLAT_BAR_HEIGHT_PX);
            frame.draw_box(filled, &style(tone));
        }
    })
    .into();
    bar.height(Extent::Cells(FLAT_BAR_ROWS))
}

/// Right-aligned percentage cell colored by load threshold.
fn share_cell<'a, A: 'static>(tenths: u32, tone: ThemeToken) -> Element<'a, A> {
    // Aligned by the column, not by padding spaces, as the UI font's
    // digits and spaces differ in width.
    let share = text(format!("{}%", format_percent(percent(tenths)))).color(tone);
    column([share.into()])
        .align(Align::End)
        .width(Extent::Cells(SHARE_COLS))
}

/// `count` cores, `1 core` for one.
fn counted_cores(count: usize) -> String {
    match count {
        1 => "1 core".to_string(),
        count => format!("{count} cores"),
    }
}

/// `used` of `total` kilobytes, as `12.3G of 31.2G`.
fn amounts(used: u64, total: u64) -> String {
    format!("{} of {}", format_kb(used), format_kb(total))
}

/// A line of `history`, the recent shares of a whole, known as `id`:
/// there from the first reading so the card keeps its shape, drawn once
/// there are two readings to draw it between.
fn history_line<'a, A: 'static>(history: &'a [f32], id: &'static str) -> Element<'a, A> {
    let line = column([sparkline(history, HISTORY_CEILING, ThemeToken::Data)])
        .padding(READING_PADDING.0, READING_PADDING.1)
        .id(id);
    if history.len() < MIN_HISTORY {
        return line;
    }
    let (low, high) = history
        .iter()
        .fold((f32::MAX, f32::MIN), |(low, high), &at| {
            (low.min(at), high.max(at))
        });
    let now = history.last().copied().unwrap_or_default();
    line.tooltip(format!(
        "The last {} readings: now {}%, lowest {}%, highest {}%",
        history.len(),
        format_percent(f64::from(now)),
        format_percent(f64::from(low)),
        format_percent(f64::from(high))
    ))
}

/// A vertical bar chart of every core's load, wrapping onto two rows when a
/// machine has more than [`MAX_CORES_PER_ROW`] logical cores.
fn core_bars<'a, A: 'static>(cores: &[CoreReading], usage: &[u32]) -> Element<'a, A> {
    let shares: Vec<u32> = (0..cores.len())
        .map(|index| usage.get(index).copied().unwrap_or(0).min(FULL_TENTHS))
        .collect();
    let rows = core_chart_rows(cores.len());
    let tooltip = cores_summary(cores, &shares);
    let chart: Element<'a, A> = canvas(move |frame, area, theme| {
        draw_core_bars(frame, area, theme, &shares);
    })
    .into();

    column([chart.height(Extent::Cells(rows))])
        .padding(READING_PADDING.0, READING_PADDING.1)
        .id("sys.cpu.cores")
        .tooltip(tooltip)
}

/// Draw vertical bars for each core in `shares`, split into two rows when
/// there are more than [`MAX_CORES_PER_ROW`] cores.
fn draw_core_bars(frame: &mut misty::Frame, area: Rect, theme: &misty::Theme, shares: &[u32]) {
    if shares.is_empty() || area.width <= 0.0 || area.height <= 0.0 {
        return;
    }
    let row_count = if shares.len() > MAX_CORES_PER_ROW {
        2
    } else {
        1
    };
    let per_row = shares.len().div_ceil(row_count);
    let row_gap = if row_count > 1 { CORE_BAR_GAP_PX } else { 0.0 };
    let row_height = ((area.height - row_gap) / row_count as f32).max(MIN_BAR_HEIGHT_PX);

    let style = |color: ThemeToken| BoxStyle {
        border: None,
        corner_radius: CORE_BAR_RADIUS_PX,
        fill: theme.color(color).into(),
        shadow: None,
    };

    for (row_index, chunk) in shares.chunks(per_row).enumerate() {
        let row_y = area.y + row_index as f32 * (row_height + row_gap);
        let gaps = CORE_BAR_GAP_PX * chunk.len().saturating_sub(1) as f32;
        let bar_width = ((area.width - gaps) / per_row as f32).max(1.0);
        for (col_index, &tenths) in chunk.iter().enumerate() {
            let x = area.x + col_index as f32 * (bar_width + CORE_BAR_GAP_PX);
            let slot = Rect::new(x, row_y, bar_width, row_height);
            frame.draw_box(slot, &style(ThemeToken::CursorLine));

            let fill_height = (row_height * share(tenths))
                .round()
                .clamp(MIN_BAR_HEIGHT_PX, row_height);
            let filled = Rect::new(x, row_y + row_height - fill_height, bar_width, fill_height);
            frame.draw_box(filled, &style(tone(tenths)));
        }
    }
}

/// How many text rows tall the core bar chart is for `core_count` cores.
fn core_chart_rows(core_count: usize) -> f32 {
    if core_count > MAX_CORES_PER_ROW {
        CORE_CHART_WRAPPED_ROWS
    } else {
        CORE_CHART_ROWS
    }
}

/// Tooltip summarizing per-core load and the busiest core's frequency.
fn cores_summary(cores: &[CoreReading], shares: &[u32]) -> String {
    if shares.len() == 1 {
        let busy = format_percent(percent(shares[0]));
        return match cores[0].mhz {
            0 => format!("Core 0: {busy}% busy"),
            mhz => format!(
                "Core 0: {busy}% busy, at {:.2} GHz",
                f64::from(mhz) / 1000.0
            ),
        };
    }
    let sum: u64 = shares.iter().map(|&s| u64::from(s)).sum();
    let avg = (sum / shares.len() as u64) as u32;
    let (peak_idx, &peak_tenths) = shares
        .iter()
        .enumerate()
        .max_by_key(|&(_, tenths)| tenths)
        .unwrap_or((0, &0));
    let peak_mhz = cores.get(peak_idx).map_or(0, |c| c.mhz);
    let peak_clock = if peak_mhz > 0 {
        format!(" ({:.2} GHz)", f64::from(peak_mhz) / 1000.0)
    } else {
        String::new()
    };
    format!(
        "{}: avg {}%, peak Core {} at {}%{}",
        counted_cores(cores.len()),
        format_percent(percent(avg)),
        peak_idx,
        format_percent(percent(peak_tenths)),
        peak_clock
    )
}

/// The color a share reads in: the data color, then the warning color, then
/// danger.
fn tone(tenths: u32) -> ThemeToken {
    match percent(tenths) {
        p if p >= CRITICAL_PERCENT => ThemeToken::Danger,
        p if p >= WARNING_PERCENT => ThemeToken::Warning,
        _ => ThemeToken::Data,
    }
}

/// A share in tenths of a percent as a percentage.
fn percent(tenths: u32) -> f64 {
    f64::from(tenths) / TENTHS
}

/// A share in tenths of a percent as a meter's fraction.
fn share(tenths: u32) -> f32 {
    percent(tenths) as f32 / WHOLE_PERCENT
}

/// `part` of `whole` in tenths of a percent, none of it for an empty whole.
pub fn tenths_of(part: u64, whole: u64) -> u32 {
    if whole == 0 {
        return 0;
    }
    (u128::from(part.min(whole)) * 1000 / u128::from(whole)) as u32
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uptime_leaves_off_units_that_are_zero_at_the_front() {
        assert_eq!(format_uptime(3 * DAY + 4 * HOUR + 5 * MINUTE), "3d 4h 5m");
        assert_eq!(format_uptime(2 * HOUR + 7 * MINUTE), "2h 7m");
        assert_eq!(format_uptime(59), "0m");
        assert_eq!(format_uptime(DAY + 5 * MINUTE), "1d 0h 5m");
    }

    #[test]
    fn test_a_share_turns_to_warning_at_seventy_five_and_danger_at_ninety() {
        assert_eq!(tone(749), ThemeToken::Data);
        assert_eq!(tone(750), ThemeToken::Warning);
        assert_eq!(tone(899), ThemeToken::Warning);
        assert_eq!(tone(900), ThemeToken::Danger);
    }

    #[test]
    fn test_an_empty_whole_is_no_share_rather_than_a_division_by_zero() {
        assert_eq!(tenths_of(5, 0), 0);
        assert_eq!(tenths_of(7, 3), 1000);
    }

    #[test]
    fn test_core_chart_wraps_only_past_thirty_two_cores() {
        assert_eq!(core_chart_rows(16), CORE_CHART_ROWS);
        assert_eq!(core_chart_rows(MAX_CORES_PER_ROW), CORE_CHART_ROWS);
        assert_eq!(
            core_chart_rows(MAX_CORES_PER_ROW + 1),
            CORE_CHART_WRAPPED_ROWS
        );
    }
}
