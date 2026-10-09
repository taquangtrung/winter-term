//! The order the monitor lists processes in: sorted, filtered, and either as a
//! flat list or as the tree of who started whom.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::model::process::{ProcState, RawProcess};

// ========================================================================
// Constants
// ========================================================================

/// The columns the list can be sorted by, in the order `s` steps through them.
pub const SORT_ORDER: [SortKey; 8] = [
    SortKey::Pid,
    SortKey::Name,
    SortKey::Cpu,
    SortKey::Memory,
    SortKey::Gpu,
    SortKey::State,
    SortKey::User,
    SortKey::Command,
];

// ========================================================================
// Data Structures
// ========================================================================

/// A process, with the rates worked out from two snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    /// The full command line.
    pub command: String,
    /// Share of one core in use: a busy multi-threaded process passes 100.
    pub cpu_percent: f64,
    /// The directory it runs in, where the system tells.
    pub cwd: Option<String>,
    /// The executable's full path, where the system tells.
    pub exec_path: Option<String>,
    /// Video memory it holds in kilobytes, where a GPU tool reports it.
    pub gpu_kb: Option<u64>,
    /// Share of physical memory resident.
    pub mem_percent: f64,
    /// The executable's name.
    pub name: String,
    /// The process id.
    pub pid: u32,
    /// The parent's id, where the system tells.
    pub ppid: Option<u32>,
    /// Resident memory in kilobytes.
    pub rss_kb: u64,
    /// What it is doing.
    pub state: ProcState,
    /// The owner's name, where the system tells.
    pub user: Option<String>,
}

/// How the list is laid out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewMode {
    /// One globally sorted list.
    Flat,
    /// Children listed under the process that started them.
    Tree,
}

/// Which way a sort runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortDirection {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

/// The column a sort compares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortKey {
    /// By the whole command line.
    Command,
    /// By CPU use.
    Cpu,
    /// By video memory held.
    Gpu,
    /// By resident memory.
    Memory,
    /// By the name the command runs under.
    Name,
    /// By process id.
    Pid,
    /// By what the process is doing.
    State,
    /// By owner.
    User,
}

/// What decides which processes are listed and in what order.
#[derive(Clone, Copy, Debug)]
pub struct ViewOptions<'a> {
    /// Tree parents whose children are folded away.
    pub collapsed: &'a HashSet<u32>,
    /// Which way the sort runs.
    pub direction: SortDirection,
    /// Text a process must hold to be listed; empty lists everything.
    pub filter: &'a str,
    /// Flat or tree.
    pub mode: ViewMode,
    /// The column to sort by.
    pub sort: SortKey,
}

/// One listed process: where it is in the snapshot and where it sits in the
/// tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewRow {
    /// Whether its children are folded away.
    pub folded: bool,
    /// Whether anything is listed beneath it.
    pub has_children: bool,
    /// Its place in the snapshot the layout was made from.
    pub index: usize,
    /// How many ancestors are listed above it; zero in a flat list.
    pub depth: usize,
}

// ========================================================================
// Process
// ========================================================================

impl Process {
    /// A reading as a process, given the rates worked out for it and the
    /// physical memory its share is measured against.
    pub fn from_raw(raw: &RawProcess, cpu_percent: f64, total_mem_kb: u64) -> Self {
        let mem_percent = match total_mem_kb {
            0 => 0.0,
            total => 100.0 * raw.rss_kb as f64 / total as f64,
        };
        Self {
            command: raw.command.clone(),
            cpu_percent,
            cwd: raw.cwd.clone(),
            exec_path: raw.exec_path.clone(),
            gpu_kb: raw.gpu_kb,
            mem_percent,
            name: raw.name.clone(),
            pid: raw.pid,
            ppid: raw.ppid,
            rss_kb: raw.rss_kb,
            state: raw.state,
            user: raw.user.clone(),
        }
    }

    /// Whether `needle`, already lowercased, is in the name, the command line,
    /// the pid, or the owner.
    fn contains(&self, needle: &str) -> bool {
        self.name.to_lowercase().contains(needle)
            || self.command.to_lowercase().contains(needle)
            || self.pid.to_string().contains(needle)
            || self
                .user
                .as_deref()
                .is_some_and(|user| user.to_lowercase().contains(needle))
    }
}

// ========================================================================
// SortKey
// ========================================================================

impl SortKey {
    /// The direction a column is first sorted in: the heavy end of a number
    /// first, the start of the alphabet first.
    pub fn default_direction(self) -> SortDirection {
        match self {
            SortKey::Cpu | SortKey::Gpu | SortKey::Memory => SortDirection::Descending,
            SortKey::Command | SortKey::Name | SortKey::Pid | SortKey::State | SortKey::User => {
                SortDirection::Ascending
            }
        }
    }

    /// The column named in the status line.
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Command => "command",
            SortKey::Cpu => "cpu",
            SortKey::Gpu => "vram",
            SortKey::Memory => "mem",
            SortKey::Name => "name",
            SortKey::Pid => "pid",
            SortKey::State => "state",
            SortKey::User => "user",
        }
    }

    /// The column after this one in [`SORT_ORDER`], wrapping.
    pub fn next(self) -> SortKey {
        let at = SORT_ORDER.iter().position(|key| *key == self).unwrap_or(0);
        SORT_ORDER[(at + 1) % SORT_ORDER.len()]
    }

    /// How two processes compare on this column, ties broken by pid so the
    /// list does not shuffle between refreshes.
    fn compare(self, a: &Process, b: &Process) -> Ordering {
        let primary = match self {
            SortKey::Command => a.command.to_lowercase().cmp(&b.command.to_lowercase()),
            SortKey::Cpu => a.cpu_percent.total_cmp(&b.cpu_percent),
            // A process with no reading sorts below one holding none.
            SortKey::Gpu => a.gpu_kb.cmp(&b.gpu_kb),
            SortKey::Memory => a.rss_kb.cmp(&b.rss_kb),
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Pid => a.pid.cmp(&b.pid),
            SortKey::State => state_rank(a.state).cmp(&state_rank(b.state)),
            SortKey::User => a
                .user
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .cmp(&b.user.as_deref().unwrap_or_default().to_lowercase()),
        };
        primary.then(a.pid.cmp(&b.pid))
    }
}

// ========================================================================
// SortDirection
// ========================================================================

impl SortDirection {
    /// The other way round.
    pub fn reversed(self) -> SortDirection {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }

    /// The arrow shown beside the sorted column.
    pub fn arrow(self) -> char {
        match self {
            SortDirection::Ascending => '\u{25b2}',
            SortDirection::Descending => '\u{25bc}',
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The processes to list and where each sits, per `options`.
pub fn layout(processes: &[Process], options: &ViewOptions) -> Vec<ViewRow> {
    let needle = options.filter.trim().to_lowercase();
    match options.mode {
        ViewMode::Flat => flat_rows(processes, options, &needle),
        ViewMode::Tree => tree_rows(processes, options, &needle),
    }
}

/// Every process below `pid`, however deep, children before their parents so
/// a signal reaches the leaves first.
pub fn descendants(processes: &[Process], pid: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for process in processes {
        if let Some(parent) = process.ppid.filter(|parent| *parent != process.pid) {
            children.entry(parent).or_default().push(process.pid);
        }
    }
    let mut found = vec![pid];
    let mut seen: HashSet<u32> = HashSet::from([pid]);
    let mut next = 0;
    while next < found.len() {
        for child in children.get(&found[next]).into_iter().flatten() {
            if seen.insert(*child) {
                found.push(*child);
            }
        }
        next += 1;
    }
    found.remove(0);
    found.reverse();
    found
}

/// Matching processes in one sorted list.
fn flat_rows(processes: &[Process], options: &ViewOptions, needle: &str) -> Vec<ViewRow> {
    let mut indices: Vec<usize> = (0..processes.len())
        .filter(|&index| needle.is_empty() || processes[index].contains(needle))
        .collect();
    sort_indices(&mut indices, processes, options);
    indices
        .into_iter()
        .map(|index| ViewRow {
            depth: 0,
            folded: false,
            has_children: false,
            index,
        })
        .collect()
}

/// The processes as a forest: a match is listed under every ancestor it has,
/// so a filter still shows where a process came from.
fn tree_rows(processes: &[Process], options: &ViewOptions, needle: &str) -> Vec<ViewRow> {
    let by_pid: HashMap<u32, usize> = processes
        .iter()
        .enumerate()
        .map(|(index, process)| (process.pid, index))
        .collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (index, process) in processes.iter().enumerate() {
        let parent = process
            .ppid
            .filter(|parent| *parent != process.pid)
            .and_then(|parent| by_pid.get(&parent));
        match parent {
            Some(parent) => children.entry(*parent).or_default().push(index),
            None => roots.push(index),
        }
    }

    let kept = kept_indices(processes, &by_pid, needle);
    for siblings in children.values_mut() {
        siblings.retain(|index| kept.as_ref().is_none_or(|kept| kept.contains(index)));
        sort_indices(siblings, processes, options);
    }
    roots.retain(|index| kept.as_ref().is_none_or(|kept| kept.contains(index)));
    sort_indices(&mut roots, processes, options);

    let mut rows = Vec::new();
    let mut visited = HashSet::new();
    let mut pending: Vec<(usize, usize)> = roots.into_iter().rev().map(|root| (root, 0)).collect();
    while let Some((index, depth)) = pending.pop() {
        // A parent loop in stale data would otherwise list its members forever.
        if !visited.insert(index) {
            continue;
        }
        let below = children.get(&index).map(Vec::as_slice).unwrap_or_default();
        let folded = options.collapsed.contains(&processes[index].pid) && !below.is_empty();
        rows.push(ViewRow {
            depth,
            folded,
            has_children: !below.is_empty(),
            index,
        });
        if !folded {
            pending.extend(below.iter().rev().map(|child| (*child, depth + 1)));
        }
    }
    rows
}

/// The processes a filter keeps: those it matches and all their ancestors.
/// `None` when there is no filter, which keeps everything.
fn kept_indices(
    processes: &[Process],
    by_pid: &HashMap<u32, usize>,
    needle: &str,
) -> Option<HashSet<usize>> {
    if needle.is_empty() {
        return None;
    }
    let mut kept = HashSet::new();
    for (index, process) in processes.iter().enumerate() {
        if !process.contains(needle) {
            continue;
        }
        let mut current = Some(index);
        while let Some(at) = current.filter(|at| kept.insert(*at)) {
            let process = &processes[at];
            current = process
                .ppid
                .filter(|parent| *parent != process.pid)
                .and_then(|parent| by_pid.get(&parent).copied());
        }
    }
    Some(kept)
}

/// Sort snapshot positions by the column and direction in `options`.
fn sort_indices(indices: &mut [usize], processes: &[Process], options: &ViewOptions) {
    indices.sort_by(|a, b| {
        let ordering = options.sort.compare(&processes[*a], &processes[*b]);
        match options.direction {
            SortDirection::Ascending => ordering,
            SortDirection::Descending => ordering.reverse(),
        }
    });
}

/// The order states sort in: busy ones first when ascending.
fn state_rank(state: ProcState) -> u8 {
    match state {
        ProcState::Running => 0,
        ProcState::Sleeping => 1,
        ProcState::Stopped => 2,
        ProcState::Zombie => 3,
        ProcState::Unknown => 4,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::process::CpuReading;

    fn process(pid: u32, ppid: Option<u32>, name: &str, cpu: f64) -> Process {
        Process {
            command: format!("/usr/bin/{name} --flag"),
            cpu_percent: cpu,
            cwd: None,
            exec_path: None,
            gpu_kb: None,
            mem_percent: 0.0,
            name: name.to_string(),
            pid,
            ppid,
            rss_kb: 0,
            state: ProcState::Sleeping,
            user: Some("root".to_string()),
        }
    }

    fn options<'a>(
        collapsed: &'a HashSet<u32>,
        filter: &'a str,
        mode: ViewMode,
    ) -> ViewOptions<'a> {
        ViewOptions {
            collapsed,
            direction: SortDirection::Descending,
            filter,
            mode,
            sort: SortKey::Cpu,
        }
    }

    fn pids(processes: &[Process], rows: &[ViewRow]) -> Vec<u32> {
        rows.iter().map(|row| processes[row.index].pid).collect()
    }

    fn family() -> Vec<Process> {
        vec![
            process(1, None, "init", 0.0),
            process(10, Some(1), "shell", 1.0),
            process(11, Some(1), "daemon", 9.0),
            process(100, Some(10), "editor", 5.0),
        ]
    }

    #[test]
    fn test_a_tree_lists_children_under_their_parent_sorted_among_siblings() {
        let processes = family();
        let none = HashSet::new();
        let rows = layout(&processes, &options(&none, "", ViewMode::Tree));
        // daemon outranks shell on cpu, and shell's child follows shell.
        assert_eq!(pids(&processes, &rows), [1, 11, 10, 100]);
        let depths: Vec<usize> = rows.iter().map(|row| row.depth).collect();
        assert_eq!(depths, [0, 1, 1, 2]);
    }

    #[test]
    fn test_a_folded_parent_hides_its_whole_subtree() {
        let processes = family();
        let folded = HashSet::from([10]);
        let rows = layout(&processes, &options(&folded, "", ViewMode::Tree));
        assert_eq!(pids(&processes, &rows), [1, 11, 10]);
        assert!(rows[2].folded && rows[2].has_children);
    }

    #[test]
    fn test_a_filter_keeps_the_ancestors_of_a_match() {
        let processes = family();
        let none = HashSet::new();
        let rows = layout(&processes, &options(&none, "editor", ViewMode::Tree));
        // Without init and shell above it the editor would float free of the
        // tree it belongs to.
        assert_eq!(pids(&processes, &rows), [1, 10, 100]);
        assert!(!rows[1].folded && rows[1].has_children);
    }

    #[test]
    fn test_a_flat_filter_lists_only_matches() {
        let processes = family();
        let none = HashSet::new();
        let rows = layout(&processes, &options(&none, "EDITOR", ViewMode::Flat));
        assert_eq!(pids(&processes, &rows), [100]);
    }

    #[test]
    fn test_a_filter_matches_a_pid_and_an_owner() {
        let processes = family();
        let none = HashSet::new();
        let by_pid = layout(&processes, &options(&none, "100", ViewMode::Flat));
        assert_eq!(pids(&processes, &by_pid), [100]);
        let by_user = layout(&processes, &options(&none, "root", ViewMode::Flat));
        assert_eq!(by_user.len(), 4);
    }

    #[test]
    fn test_a_process_whose_parent_is_unlisted_becomes_a_root() {
        let processes = vec![process(5, Some(999), "orphan", 0.0)];
        let none = HashSet::new();
        let rows = layout(&processes, &options(&none, "", ViewMode::Tree));
        assert_eq!(pids(&processes, &rows), [5]);
    }

    #[test]
    fn test_a_parent_loop_in_stale_data_terminates() {
        // Two processes naming each other as parent are both reachable from
        // nowhere, and must neither hang nor be listed twice.
        let processes = vec![
            process(1, Some(2), "a", 0.0),
            process(2, Some(1), "b", 0.0),
            process(3, None, "c", 0.0),
        ];
        let none = HashSet::new();
        let rows = layout(&processes, &options(&none, "", ViewMode::Tree));
        assert_eq!(pids(&processes, &rows), [3]);
        let found = descendants(&processes, 1);
        assert_eq!(found, [2]);
    }

    #[test]
    fn test_descendants_come_children_first() {
        let processes = family();
        // Killing a parent first would orphan its children to init before
        // they were signalled.
        assert_eq!(descendants(&processes, 1), [100, 11, 10]);
        assert_eq!(descendants(&processes, 10), [100]);
        assert!(descendants(&processes, 100).is_empty());
    }

    #[test]
    fn test_video_memory_sorts_biggest_first_with_unknown_last() {
        let mut processes = vec![
            process(1, None, "none", 0.0),
            process(2, None, "small", 0.0),
            process(3, None, "big", 0.0),
        ];
        processes[1].gpu_kb = Some(10);
        processes[2].gpu_kb = Some(5000);
        let none = HashSet::new();
        let mut view = options(&none, "", ViewMode::Flat);
        view.sort = SortKey::Gpu;
        view.direction = SortKey::Gpu.default_direction();
        assert_eq!(pids(&processes, &layout(&processes, &view)), [3, 2, 1]);
    }

    #[test]
    fn test_equal_values_sort_by_pid_so_the_list_holds_still() {
        let processes = vec![
            process(30, None, "c", 2.0),
            process(20, None, "b", 2.0),
            process(10, None, "a", 2.0),
        ];
        let none = HashSet::new();
        let mut view = options(&none, "", ViewMode::Flat);
        view.direction = SortDirection::Ascending;
        assert_eq!(pids(&processes, &layout(&processes, &view)), [10, 20, 30]);
    }

    #[test]
    fn test_the_sort_key_cycles_through_every_column_and_wraps() {
        let mut key = SORT_ORDER[0];
        for expected in SORT_ORDER.iter().skip(1) {
            key = key.next();
            assert_eq!(key, *expected);
        }
        assert_eq!(key.next(), SORT_ORDER[0]);
    }

    #[test]
    fn test_memory_share_is_zero_when_total_memory_is_unknown() {
        let raw = RawProcess {
            command: "x".to_string(),
            cpu: CpuReading::Time(0),
            cwd: None,
            exec_path: None,
            gpu_kb: None,
            name: "x".to_string(),
            pid: 1,
            ppid: None,
            rss_kb: 500,
            state: ProcState::Running,
            user: None,
        };
        assert_eq!(Process::from_raw(&raw, 0.0, 0).mem_percent, 0.0);
        assert_eq!(Process::from_raw(&raw, 0.0, 1000).mem_percent, 50.0);
    }
}
