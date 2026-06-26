//! Reading what each system says about its processes: the files Linux keeps
//! under `/proc`, the table `ps` prints on macOS and the BSDs, and the JSON a
//! PowerShell query writes on Windows. Pure text in, readings out, so each
//! format is testable on any machine.

use std::collections::HashMap;

use serde_json::Value;

use crate::model::process::{CpuReading, ProcState, RawProcess};

// ========================================================================
// Constants
// ========================================================================

/// Bytes in a kilobyte.
const BYTES_PER_KB: u64 = 1024;

/// Kilobytes in the mebibytes `nvidia-smi` reports.
const KB_PER_MIB: f64 = 1024.0;

/// How many fields `ps` prints before the command line, which holds spaces.
const PS_FIELDS: usize = 6;

/// Tenths of a percent in a percent.
const TENTHS_PER_PERCENT: f64 = 10.0;

/// Windows counts process time in units of a hundred nanoseconds; this many
/// of them make a millisecond.
const WINDOWS_UNITS_PER_MILLI: u64 = 10_000;

/// The process id Windows gives the idle time it accounts to no process.
const WINDOWS_IDLE_PID: u32 = 0;

/// Milliseconds one scheduler tick stands for. Linux counts process time in
/// ticks of `1 / USER_HZ` second, and `USER_HZ` is 100 on every platform
/// Linux runs on.
const MILLIS_PER_TICK: u64 = 10;

// ========================================================================
// Data Structures
// ========================================================================

/// The fields of `/proc/<pid>/stat` the monitor uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcStat {
    /// CPU time spent so far in milliseconds, user and kernel together.
    pub cpu_millis: u64,
    /// The executable's name.
    pub name: String,
    /// The parent's id.
    pub ppid: u32,
    /// What the process is doing.
    pub state: ProcState,
}

// ========================================================================
// Linux
// ========================================================================

/// Read a `/proc/<pid>/stat` line. The name sits in parentheses and may hold
/// spaces and parentheses of its own, so the fields after it are found from
/// the last closing one.
pub fn parse_proc_stat(raw: &str) -> Option<ProcStat> {
    let open = raw.find('(')?;
    let close = raw.rfind(')')?;
    if close < open {
        return None;
    }
    let after: Vec<&str> = raw.get(close + 1..)?.split_whitespace().collect();
    // After the name: state, ppid, then eleven fields to user time and kernel
    // time at positions eleven and twelve.
    let state = after.first()?.chars().next()?;
    let ppid = after.get(1)?.parse().ok()?;
    let user: u64 = after.get(11)?.parse().ok()?;
    let kernel: u64 = after.get(12)?.parse().ok()?;
    Some(ProcStat {
        cpu_millis: (user + kernel) * MILLIS_PER_TICK,
        name: raw[open + 1..close].to_string(),
        ppid,
        state: state_from_code(state),
    })
}

/// Resident kilobytes and the owner's uid from a `/proc/<pid>/status` file.
/// A kernel thread has no `VmRSS` line, and reads as no memory.
pub fn parse_proc_status(raw: &str) -> (u64, Option<u32>) {
    let mut rss_kb = 0;
    let mut uid = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss_kb = first_number(rest).unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("Uid:") {
            uid = first_number(rest).and_then(|value| u32::try_from(value).ok());
        }
    }
    (rss_kb, uid)
}

/// The command line from `/proc/<pid>/cmdline`, whose arguments are separated
/// by NUL bytes. Empty for a kernel thread, which has none.
pub fn parse_proc_cmdline(raw: &str) -> String {
    raw.split('\0')
        .filter(|argument| !argument.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Physical memory in kilobytes from `/proc/meminfo`.
pub fn parse_meminfo_total(raw: &str) -> u64 {
    raw.lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(first_number)
        .unwrap_or(0)
}

/// User names by uid from an `/etc/passwd` file.
pub fn parse_passwd(raw: &str) -> HashMap<u32, String> {
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next().filter(|name| !name.is_empty())?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

/// What a `/proc` state letter means.
fn state_from_code(code: char) -> ProcState {
    match code {
        'R' => ProcState::Running,
        'S' | 'D' | 'I' => ProcState::Sleeping,
        'T' | 't' => ProcState::Stopped,
        'Z' => ProcState::Zombie,
        _ => ProcState::Unknown,
    }
}

// ========================================================================
// ps (macOS and the BSDs)
// ========================================================================

/// Rows of `ps -axo pid=,ppid=,pcpu=,rss=,state=,user=,args=` as processes.
/// `names` is each process's executable from a second `ps` (see
/// [`parse_ps_comm`]), since the first word of `args` stops at the first space
/// in a path like `/Applications/Visual Studio Code.app/...`.
pub fn parse_ps_rows(
    output: &str,
    names: &HashMap<u32, String>,
    cwds: &HashMap<u32, String>,
) -> Vec<RawProcess> {
    output
        .lines()
        .filter_map(|line| {
            let (fields, args) = split_fields::<PS_FIELDS>(line)?;
            let [pid, ppid, pcpu, rss, state, user] = fields;
            let pid: u32 = pid.parse().ok()?;
            let args = args.trim();
            let executable = names
                .get(&pid)
                .map(String::as_str)
                .or_else(|| args.split(' ').next())
                .unwrap_or_default();
            Some(RawProcess {
                command: if args.is_empty() {
                    executable.to_string()
                } else {
                    args.to_string()
                },
                cpu: CpuReading::Rate(tenths_of_percent(pcpu)),
                cwd: cwds.get(&pid).cloned(),
                exec_path: executable.starts_with('/').then(|| executable.to_string()),
                gpu_kb: None,
                name: executable_name(executable),
                pid,
                ppid: ppid.parse().ok(),
                rss_kb: rss.parse().ok()?,
                state: state_from_ps(state),
                user: Some(user.to_string()),
            })
        })
        .collect()
}

/// Rows of `ps -axo pid=,comm=` as each process's executable. On macOS `comm`
/// is the full path, spaces and all.
pub fn parse_ps_comm(output: &str) -> HashMap<u32, String> {
    output
        .lines()
        .filter_map(|line| {
            let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
            let rest = rest.trim();
            (!rest.is_empty()).then_some((pid.parse().ok()?, rest.to_string()))
        })
        .collect()
}

/// The working directories from `lsof -a -d cwd -Fpn`, which writes a `p<pid>`
/// line and then an `n<path>` line for it.
pub fn parse_lsof_cwd(output: &str) -> HashMap<u32, String> {
    let mut cwds = HashMap::new();
    let mut current = None;
    for line in output.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            current = pid.parse().ok();
        } else if let (Some(path), Some(pid)) = (line.strip_prefix('n'), current) {
            cwds.insert(pid, path.to_string());
        }
    }
    cwds
}

/// What a `ps` state column means: its first letter says it.
fn state_from_ps(code: &str) -> ProcState {
    match code.chars().next() {
        Some('R') => ProcState::Running,
        Some('S' | 'I' | 'D' | 'U') => ProcState::Sleeping,
        Some('T') => ProcState::Stopped,
        Some('Z') => ProcState::Zombie,
        _ => ProcState::Unknown,
    }
}

/// A process's name from its executable: the file name, without the
/// parentheses `ps` puts round a name it only knows from the kernel.
fn executable_name(executable: &str) -> String {
    let unwrapped = executable
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(executable);
    unwrapped
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(unwrapped)
        .to_string()
}

/// A `ps` percentage as tenths of a percent. A column that does not read as a
/// number is no use.
fn tenths_of_percent(text: &str) -> u32 {
    text.parse::<f64>()
        .ok()
        .filter(|percent| percent.is_finite() && *percent > 0.0)
        .map_or(0, |percent| (percent * TENTHS_PER_PERCENT).round() as u32)
}

/// The first `N` whitespace-separated fields of `line` and what remains of it,
/// untouched, so a command line keeps its spaces.
fn split_fields<const N: usize>(line: &str) -> Option<([&str; N], &str)> {
    let mut fields = [""; N];
    let mut rest = line;
    for field in &mut fields {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        *field = &rest[..end];
        rest = &rest[end..];
    }
    Some((fields, rest))
}

// ========================================================================
// Windows
// ========================================================================

/// The JSON the Windows query writes: the machine's physical memory and, per
/// process, `[pid, ppid, name, path, command line, cpu time, working set]`.
/// Returns physical memory in kilobytes and the processes, or `None` when the
/// text is not that JSON.
pub fn parse_windows_json(text: &str) -> Option<(u64, Vec<RawProcess>)> {
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}').trim()).ok()?;
    let total_mem_kb = value["mem"].as_u64().unwrap_or(0) / BYTES_PER_KB;
    let processes = value["procs"]
        .as_array()?
        .iter()
        .filter_map(|row| windows_process(row.as_array()?))
        .collect();
    Some((total_mem_kb, processes))
}

/// One process from a row of the Windows query. The idle pseudo-process, and
/// a row without a pid, are not processes.
fn windows_process(row: &[Value]) -> Option<RawProcess> {
    let pid = u32::try_from(row.first()?.as_u64()?).ok()?;
    if pid == WINDOWS_IDLE_PID {
        return None;
    }
    let text = |at: usize| {
        row.get(at)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    };
    let name = text(2).map_or_else(|| pid.to_string(), str::to_string);
    let time = row.get(5).and_then(Value::as_u64).unwrap_or(0);
    let working_set = row.get(6).and_then(Value::as_u64).unwrap_or(0);
    Some(RawProcess {
        command: text(4).map_or_else(|| name.clone(), str::to_string),
        cpu: CpuReading::Time(time / WINDOWS_UNITS_PER_MILLI),
        cwd: None,
        exec_path: text(3).map(str::to_string),
        gpu_kb: None,
        name,
        pid,
        ppid: row
            .get(1)
            .and_then(Value::as_u64)
            .and_then(|ppid| u32::try_from(ppid).ok()),
        rss_kb: working_set / BYTES_PER_KB,
        state: ProcState::Running,
        user: None,
    })
}

// ========================================================================
// GPU (NVIDIA)
// ========================================================================

/// Video memory in kilobytes by pid from `nvidia-smi -q -x`, summed over the
/// GPUs a process uses. The XML query is the one that lists graphics
/// processes (the display server, every accelerated window) as well as
/// compute ones. A process whose memory is `N/A`, as under a Windows display
/// driver, is left out rather than counted as zero.
pub fn parse_nvidia_smi_xml(text: &str) -> HashMap<u32, u64> {
    let mut usage: HashMap<u32, u64> = HashMap::new();
    for block in text.split("<process_info>").skip(1) {
        let block = block.split("</process_info>").next().unwrap_or_default();
        let Some(pid) = tag_text(block, "pid").and_then(|pid| pid.parse::<u32>().ok()) else {
            continue;
        };
        let Some(mib) = tag_text(block, "used_memory")
            .and_then(|memory| memory.split_whitespace().next())
            .and_then(|number| number.parse::<f64>().ok())
            .filter(|mib| mib.is_finite() && *mib >= 0.0)
        else {
            continue;
        };
        *usage.entry(pid).or_default() += (mib * KB_PER_MIB) as u64;
    }
    usage
}

/// The text between `<tag>` and `</tag>` in `block`.
fn tag_text<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let rest = block.split_once(&format!("<{tag}>"))?.1;
    Some(rest.split_once(&format!("</{tag}>"))?.0.trim())
}

// ========================================================================
// Shared
// ========================================================================

/// The first whitespace-separated number in `text`.
fn first_number(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_stat_line_with_parentheses_and_spaces_in_the_name_still_parses() {
        // Finding the name from the first space or the first `)` would take
        // "evil" for the state and misread every field after it.
        let line = "42 (we ird) name) S 7 42 42 0 -1 4194560 100 0 0 0 30 20 0 0 20 0 1 0";
        let stat = parse_proc_stat(line).unwrap();
        assert_eq!(stat.name, "we ird) name");
        assert_eq!(stat.ppid, 7);
        assert_eq!(stat.state, ProcState::Sleeping);
        // 30 user + 20 kernel ticks at ten milliseconds each.
        assert_eq!(stat.cpu_millis, 500);
    }

    #[test]
    fn test_a_truncated_stat_line_is_not_a_process() {
        assert_eq!(parse_proc_stat("42 (bash) S 7 42"), None);
        assert_eq!(parse_proc_stat("garbage"), None);
    }

    #[test]
    fn test_status_gives_resident_memory_and_the_real_uid() {
        let status = "Name:\tbash\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t   2048 kB\n";
        assert_eq!(parse_proc_status(status), (2048, Some(1000)));
    }

    #[test]
    fn test_a_kernel_thread_has_no_resident_memory() {
        let status = "Name:\tkthreadd\nUid:\t0\t0\t0\t0\n";
        assert_eq!(parse_proc_status(status), (0, Some(0)));
    }

    #[test]
    fn test_a_command_line_joins_its_nul_separated_arguments() {
        assert_eq!(parse_proc_cmdline("vim\0-u\0NONE\0"), "vim -u NONE");
        assert_eq!(parse_proc_cmdline(""), "");
    }

    #[test]
    fn test_passwd_maps_uids_to_names_and_skips_malformed_lines() {
        let passwd =
            "root:x:0:0:root:/root:/bin/bash\nbroken\nuser:x:1000:1000::/home/user:/bin/zsh\n";
        let names = parse_passwd(passwd);
        assert_eq!(names.get(&0).map(String::as_str), Some("root"));
        assert_eq!(names.get(&1000).map(String::as_str), Some("user"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn test_meminfo_total_is_read_by_name_not_position() {
        let meminfo = "MemFree:  100 kB\nMemTotal:       16384 kB\n";
        assert_eq!(parse_meminfo_total(meminfo), 16384);
        assert_eq!(parse_meminfo_total("nothing"), 0);
    }

    #[test]
    fn test_ps_rows_keep_the_spaces_in_a_command_line() {
        let names =
            parse_ps_comm("  501 /Applications/Visual Studio Code.app/Contents/MacOS/Electron\n");
        let output = "  501     1  12.5  2048 S    alice /Applications/Visual Studio Code.app/Contents/MacOS/Electron --flag\n";
        let rows = parse_ps_rows(output, &names, &HashMap::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Electron");
        assert_eq!(
            rows[0].command,
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron --flag"
        );
        assert_eq!(rows[0].cpu, CpuReading::Rate(125));
        assert_eq!(rows[0].rss_kb, 2048);
        assert_eq!(rows[0].ppid, Some(1));
        assert_eq!(rows[0].user.as_deref(), Some("alice"));
    }

    #[test]
    fn test_ps_rows_skip_lines_that_are_not_processes() {
        let output = "  PID PPID\nnot a row at all\n  7  1  0.0  10 Z  bob (defunct)\n";
        let rows = parse_ps_rows(output, &HashMap::new(), &HashMap::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 7);
        assert_eq!(rows[0].state, ProcState::Zombie);
    }

    #[test]
    fn test_a_name_only_the_kernel_knows_loses_its_parentheses() {
        assert_eq!(executable_name("(kernel_task)"), "kernel_task");
        assert_eq!(executable_name("/usr/sbin/cron"), "cron");
    }

    #[test]
    fn test_lsof_pairs_each_directory_with_the_pid_before_it() {
        let cwds = parse_lsof_cwd("p10\nn/home/a\np11\nn/tmp\n");
        assert_eq!(cwds.get(&10).map(String::as_str), Some("/home/a"));
        assert_eq!(cwds.get(&11).map(String::as_str), Some("/tmp"));
    }

    #[test]
    fn test_windows_json_gives_processes_and_total_memory() {
        let text = "\u{feff}{\"mem\":17179869184,\"procs\":[[0,0,\"System Idle Process\",null,null,0,8192],[4,0,\"System\",null,null,5000000,40960],[88,4,\"chrome.exe\",\"C:\\\\chrome.exe\",\"chrome.exe --type=gpu\",25000000,10485760]]}";
        let (total, processes) = parse_windows_json(text).unwrap();
        assert_eq!(total, 16_777_216);
        // The idle pseudo-process is dropped.
        assert_eq!(processes.len(), 2);
        let chrome = &processes[1];
        assert_eq!(chrome.pid, 88);
        assert_eq!(chrome.ppid, Some(4));
        assert_eq!(chrome.command, "chrome.exe --type=gpu");
        // 25,000,000 units of 100ns is 2.5 seconds.
        assert_eq!(chrome.cpu, CpuReading::Time(2500));
        assert_eq!(chrome.rss_kb, 10240);
        // A process with no command line falls back to its name.
        assert_eq!(processes[0].command, "System");
    }

    #[test]
    fn test_nvidia_smi_sums_a_process_over_gpus_and_skips_unreadable_memory() {
        let xml = "<gpu><processes>\
            <process_info><pid>10</pid><type>G</type><process_name>/usr/lib/Xorg</process_name><used_memory>349 MiB</used_memory></process_info>\
            <process_info><pid>11</pid><used_memory>N/A</used_memory></process_info>\
            </processes></gpu><gpu><processes>\
            <process_info><pid>10</pid><used_memory>1 MiB</used_memory></process_info>\
            </processes></gpu>";
        let usage = parse_nvidia_smi_xml(xml);
        // Two GPUs hold 349 and 1 MiB of it: 350 MiB, not whichever came last.
        assert_eq!(usage.get(&10), Some(&(350 * 1024)));
        // `N/A` is unknown, which must not read as a process using none.
        assert_eq!(usage.get(&11), None);
    }

    #[test]
    fn test_text_that_is_not_nvidia_smi_output_lists_nothing() {
        assert!(parse_nvidia_smi_xml("NVIDIA-SMI has failed").is_empty());
    }

    #[test]
    fn test_text_that_is_not_the_windows_query_is_rejected() {
        assert!(parse_windows_json("Get-CimInstance : Access denied").is_none());
        assert!(parse_windows_json("{\"mem\":1}").is_none());
    }
}
