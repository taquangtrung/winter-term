//! How the system monitor lays a snapshot out: sections of styled rows, with
//! a meter bar wherever the reference monitor draws a gauge.

use crate::model::page::{PageRow, PageSpan, PageStyle};
use crate::model::system::{CoreReading, DiskReading, GpuReading, MemoryReading, SystemSample};
use crate::tools::proc::format::{format_kb, format_percent};

use super::usage::Usage;

// ========================================================================
// Constants
// ========================================================================

/// A share at or past this reads as trouble, and at or past [`WARNING_PERCENT`]
/// as something to watch.
const CRITICAL_PERCENT: f64 = 90.0;
const WARNING_PERCENT: f64 = 75.0;

/// Cells the meter bar of the wide gauges takes, bounded so a huge pane does
/// not stretch one into a line and a tiny one still shows something.
const MIN_BAR: usize = 10;
const MAX_BAR: usize = 40;

/// The meter bar of a core or a disk row.
const SMALL_BAR: usize = 10;

/// Cells one core takes in the grid: index, bar, share, clock, and the gap.
const CORE_CELL: usize = 29;

/// The characters a meter bar is drawn with.
const BAR_FULL: char = '\u{2588}';
const BAR_EMPTY: char = '\u{2591}';

/// Tenths of a percent in a percent.
const TENTHS: f64 = 10.0;

/// Margin before a section's rows.
const INDENT: &str = "  ";

/// Seconds in a minute, an hour and a day.
const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

// ========================================================================
// Data Structures
// ========================================================================

/// One titled block of the page.
#[derive(Clone, Debug)]
pub struct Section {
    /// The rows under the title.
    pub rows: Vec<PageRow>,
    /// The heading, painted as the section's first row.
    pub title: String,
}

// ========================================================================
// Functions
// ========================================================================

/// The page for `sample`, a section per kind of thing it describes, laid out
/// for a pane `cols` wide. GPU and swap appear only where there is some.
pub fn sections(sample: &SystemSample, usage: &Usage, cols: usize) -> Vec<Section> {
    let bar = (cols / 3).clamp(MIN_BAR, MAX_BAR);
    let mut sections = vec![
        overview_section(sample),
        cpu_section(sample, usage, cols, bar),
        memory_section(&sample.memory, bar),
    ];
    if !sample.gpus.is_empty() {
        sections.push(gpu_section(&sample.gpus, bar));
    }
    sections.push(disk_section(&sample.disks));
    sections
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

/// Host, system, uptime and load.
fn overview_section(sample: &SystemSample) -> Section {
    let overview = &sample.overview;
    let mut first = vec![PageSpan::plain(INDENT)];
    first.extend(pair("Host", &overview.hostname));
    first.extend(pair(
        "OS",
        &format!("{} {} ({})", overview.os, overview.release, overview.arch),
    ));
    let mut second = vec![PageSpan::plain(INDENT)];
    second.extend(pair("Uptime", &format_uptime(overview.uptime_secs)));
    if let Some([one, five, fifteen]) = sample.cpu.load {
        let load = |hundredths: u32| format!("{:.2}", f64::from(hundredths) / 100.0);
        second.extend(pair(
            "Load",
            &format!("{} {} {}", load(one), load(five), load(fifteen)),
        ));
    }
    Section {
        rows: vec![first, second],
        title: "System Overview".to_string(),
    }
}

/// The machine's total, then a grid of cores as many to a row as fit.
fn cpu_section(sample: &SystemSample, usage: &Usage, cols: usize, bar: usize) -> Section {
    let cpu = &sample.cpu;
    let model = if cpu.model.is_empty() {
        "CPU".to_string()
    } else {
        cpu.model.clone()
    };
    let title = match cpu.cores.len() {
        0 => format!("CPU Usage  {model}"),
        count => format!("CPU Usage  {model} ({count} cores)"),
    };
    let mut rows = vec![gauge("Total", usage.total, bar, String::new())];
    let per_row = (cols.saturating_sub(INDENT.len()) / CORE_CELL).max(1);
    for chunk in cpu
        .cores
        .iter()
        .enumerate()
        .collect::<Vec<_>>()
        .chunks(per_row)
    {
        let mut row = vec![PageSpan::plain(INDENT)];
        for (index, core) in chunk {
            let percent = usage.cores.get(*index).copied().unwrap_or(0);
            row.extend(core_cell(*index, core, percent));
        }
        rows.push(row);
    }
    Section { rows, title }
}

/// Physical memory, and swap where there is any.
fn memory_section(memory: &MemoryReading, bar: usize) -> Section {
    let used = memory.total_kb.saturating_sub(memory.available_kb);
    let mut rows = vec![gauge(
        "RAM",
        tenths_of(used, memory.total_kb),
        bar,
        format!(
            "{} of {} used, {} free",
            format_kb(used),
            format_kb(memory.total_kb),
            format_kb(memory.available_kb)
        ),
    )];
    if memory.swap_total_kb > 0 {
        rows.push(gauge(
            "Swap",
            tenths_of(memory.swap_used_kb, memory.swap_total_kb),
            bar,
            format!(
                "{} of {} used, {} free",
                format_kb(memory.swap_used_kb),
                format_kb(memory.swap_total_kb),
                format_kb(memory.swap_total_kb.saturating_sub(memory.swap_used_kb))
            ),
        ));
    }
    Section {
        rows,
        title: "Memory & Swap".to_string(),
    }
}

/// One card's video memory, load and temperature.
fn gpu_section(gpus: &[GpuReading], bar: usize) -> Section {
    let mut rows = Vec::new();
    for gpu in gpus {
        let mut facts = format!(
            "{} of {} used, load {}%",
            format_kb(gpu.mem_used_kb),
            format_kb(gpu.mem_total_kb),
            gpu.util_percent
        );
        if let Some(temp) = gpu.temp_c {
            facts.push_str(&format!(", {temp}\u{b0}C"));
        }
        rows.push(vec![
            PageSpan::plain(INDENT),
            PageSpan::new(PageStyle::Accent, gpu.name.clone()),
        ]);
        rows.push(gauge(
            "VRAM",
            tenths_of(gpu.mem_used_kb, gpu.mem_total_kb),
            bar,
            facts,
        ));
    }
    Section {
        rows,
        title: "GPU".to_string(),
    }
}

/// A table of volumes with a bar for each one's use.
fn disk_section(disks: &[DiskReading]) -> Section {
    let title = "Storage / Disks".to_string();
    if disks.is_empty() {
        return Section {
            rows: vec![vec![
                PageSpan::plain(INDENT),
                PageSpan::new(PageStyle::Dim, "no local disk volumes found"),
            ]],
            title,
        };
    }
    let mount_width = disks
        .iter()
        .map(|d| d.mount.chars().count())
        .max()
        .unwrap_or(0);
    let type_width = disks
        .iter()
        .map(|d| d.fs_type.as_deref().unwrap_or("-").chars().count())
        .max()
        .unwrap_or(0);
    let mut rows = vec![vec![
        PageSpan::plain(INDENT),
        PageSpan::new(
            PageStyle::Header,
            format!(
                "{:<mount_width$} {:<type_width$} {:<width$}  USED / TOTAL",
                "MOUNT",
                "TYPE",
                "USAGE",
                width = SMALL_BAR + 7
            ),
        ),
    ]];
    for disk in disks {
        let tenths = tenths_of(disk.used_kb, disk.total_kb);
        let mut row = vec![
            PageSpan::plain(INDENT),
            PageSpan::plain(format!(
                "{:<mount_width$} {:<type_width$} ",
                disk.mount,
                disk.fs_type.as_deref().unwrap_or("-")
            )),
        ];
        row.push(meter(tenths, SMALL_BAR));
        row.push(PageSpan::new(
            level(tenths),
            format!(" {:>5}%", format_percent(percent(tenths))),
        ));
        row.push(PageSpan::plain(format!(
            "  {} / {}, {} free",
            format_kb(disk.used_kb),
            format_kb(disk.total_kb),
            format_kb(disk.available_kb)
        )));
        rows.push(row);
    }
    Section { rows, title }
}

/// `label  [bar]  share  detail`.
fn gauge(label: &str, tenths: u32, bar: usize, detail: String) -> PageRow {
    let mut row = vec![
        PageSpan::plain(INDENT),
        PageSpan::new(PageStyle::Dim, format!("{label:<6} ")),
        meter(tenths, bar),
        PageSpan::new(
            level(tenths),
            format!(" {:>5}%", format_percent(percent(tenths))),
        ),
    ];
    if !detail.is_empty() {
        row.push(PageSpan::plain(format!("  {detail}")));
    }
    row
}

/// One core of the grid: its number, a bar, its share, and its clock.
fn core_cell(index: usize, core: &CoreReading, tenths: u32) -> Vec<PageSpan> {
    let clock = match core.mhz {
        0 => String::new(),
        mhz => format!("{:.1}G", f64::from(mhz) / 1000.0),
    };
    vec![
        PageSpan::new(PageStyle::Dim, format!("{index:>3} ")),
        meter(tenths, SMALL_BAR),
        PageSpan::new(
            level(tenths),
            format!(" {:>5}%", format_percent(percent(tenths))),
        ),
        PageSpan::new(PageStyle::Dim, format!(" {clock:>5}  ")),
    ]
}

/// A bar `width` cells wide filled to `tenths` of a percent, colored by how
/// full it is.
fn meter(tenths: u32, width: usize) -> PageSpan {
    let filled = (percent(tenths) / 100.0 * width as f64).round() as usize;
    let filled = filled.min(width);
    let text: String = std::iter::repeat_n(BAR_FULL, filled)
        .chain(std::iter::repeat_n(BAR_EMPTY, width - filled))
        .collect();
    PageSpan::new(level(tenths), text)
}

/// The color a share reads in: green, then yellow, then red.
fn level(tenths: u32) -> PageStyle {
    match percent(tenths) {
        p if p >= CRITICAL_PERCENT => PageStyle::ChangeDeleted,
        p if p >= WARNING_PERCENT => PageStyle::ChangeModified,
        _ => PageStyle::ChangeAdded,
    }
}

/// A share in tenths of a percent as a percentage.
fn percent(tenths: u32) -> f64 {
    f64::from(tenths) / TENTHS
}

/// `part` of `whole` in tenths of a percent, none of it for an empty whole.
fn tenths_of(part: u64, whole: u64) -> u32 {
    if whole == 0 {
        return 0;
    }
    (u128::from(part.min(whole)) * 1000 / u128::from(whole)) as u32
}

/// `label value  `: a dim label and its value.
fn pair(label: &str, value: &str) -> [PageSpan; 2] {
    [
        PageSpan::new(PageStyle::Dim, format!("{label} ")),
        PageSpan::plain(format!("{value}   ")),
    ]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::model::page::row_text;
    use crate::model::system::{CoreLoad, CpuSample, CpuTicks, Overview};

    use super::*;

    fn sample(cores: usize, gpus: Vec<GpuReading>, swap_total_kb: u64) -> SystemSample {
        SystemSample {
            cpu: CpuSample {
                cores: (0..cores)
                    .map(|_| CoreReading {
                        load: CoreLoad::Ticks(CpuTicks { busy: 0, total: 1 }),
                        mhz: 3200,
                    })
                    .collect(),
                load: Some([52, 60, 101]),
                model: "Test CPU".to_string(),
                total: CoreLoad::Percent(0),
            },
            disks: vec![DiskReading {
                available_kb: 600,
                fs_type: Some("ext4".to_string()),
                mount: "/".to_string(),
                total_kb: 1000,
                used_kb: 400,
            }],
            gpus,
            memory: MemoryReading {
                available_kb: 4 * 1024 * 1024,
                swap_total_kb,
                swap_used_kb: 0,
                total_kb: 16 * 1024 * 1024,
            },
            overview: Overview {
                arch: "x86_64".to_string(),
                hostname: "box".to_string(),
                os: "Linux".to_string(),
                release: "6.8".to_string(),
                uptime_secs: 3 * DAY + 4 * HOUR + 5 * MINUTE,
            },
        }
    }

    fn text(section: &Section) -> Vec<String> {
        section.rows.iter().map(row_text).collect()
    }

    #[test]
    fn test_uptime_leaves_off_units_that_are_zero_at_the_front() {
        assert_eq!(format_uptime(3 * DAY + 4 * HOUR + 5 * MINUTE), "3d 4h 5m");
        assert_eq!(format_uptime(2 * HOUR + 7 * MINUTE), "2h 7m");
        assert_eq!(format_uptime(59), "0m");
        // A day and no hours still says the hours, or "1d 5m" reads as 1d 5h.
        assert_eq!(format_uptime(DAY + 5 * MINUTE), "1d 0h 5m");
    }

    #[test]
    fn test_gpu_and_swap_sections_appear_only_when_there_is_something_to_show() {
        let none = sections(&sample(2, vec![], 0), &Usage::default(), 120);
        let titles: Vec<&str> = none.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "System Overview",
                "CPU Usage  Test CPU (2 cores)",
                "Memory & Swap",
                "Storage / Disks"
            ]
        );
        assert_eq!(none[2].rows.len(), 1, "no swap row without swap");
        let gpu = GpuReading {
            mem_total_kb: 1024 * 1024,
            mem_used_kb: 512 * 1024,
            name: "RTX".to_string(),
            temp_c: Some(54),
            util_percent: 17,
        };
        let some = sections(&sample(2, vec![gpu], 1024), &Usage::default(), 120);
        assert_eq!(some.len(), 5);
        assert_eq!(some[2].rows.len(), 2, "swap gets its own row");
        let gpu_rows = text(&some[3]);
        assert!(
            gpu_rows[1].contains("512.0M of 1.0G used, load 17%, 54\u{b0}C"),
            "got {gpu_rows:?}"
        );
    }

    #[test]
    fn test_the_core_grid_packs_as_many_cores_to_a_row_as_the_pane_holds() {
        let usage = Usage::default();
        // Eight cores; a pane of 2 + 3 * 29 cells holds three to a row.
        let wide = sections(&sample(8, vec![], 0), &usage, 2 + 3 * CORE_CELL);
        assert_eq!(wide[1].rows.len(), 1 + 3, "total plus ceil(8 / 3) rows");
        // A pane narrower than one cell still shows one core per row.
        let narrow = sections(&sample(8, vec![], 0), &usage, 5);
        assert_eq!(narrow[1].rows.len(), 1 + 8);
    }

    #[test]
    fn test_a_meter_fills_in_proportion_and_never_past_its_width() {
        assert_eq!(meter(0, 10).text.matches(BAR_FULL).count(), 0);
        assert_eq!(meter(500, 10).text.matches(BAR_FULL).count(), 5);
        assert_eq!(meter(1000, 10).text.matches(BAR_FULL).count(), 10);
        assert_eq!(meter(5000, 10).text.chars().count(), 10);
    }

    #[test]
    fn test_a_gauge_turns_yellow_at_seventy_five_and_red_at_ninety() {
        assert_eq!(level(749), PageStyle::ChangeAdded);
        assert_eq!(level(750), PageStyle::ChangeModified);
        assert_eq!(level(899), PageStyle::ChangeModified);
        assert_eq!(level(900), PageStyle::ChangeDeleted);
    }

    #[test]
    fn test_an_empty_whole_is_no_share_rather_than_a_division_by_zero() {
        assert_eq!(tenths_of(5, 0), 0);
        assert_eq!(tenths_of(7, 3), 1000);
    }

    #[test]
    fn test_a_disk_row_names_its_mount_usage_and_free_space() {
        let section = sections(&sample(1, vec![], 0), &Usage::default(), 120)
            .pop()
            .unwrap();
        let rows = text(&section);
        assert!(rows[1].contains("/ ext4"), "got {rows:?}");
        assert!(rows[1].contains("40.0%"), "got {rows:?}");
        assert!(rows[1].contains("400K / 1000K, 600K free"), "got {rows:?}");
    }
}
