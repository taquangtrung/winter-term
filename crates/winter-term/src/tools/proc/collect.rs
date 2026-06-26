//! Taking a snapshot of every process. This is the side-effecting half of the
//! monitor: it reads `/proc` on Linux, runs `ps` elsewhere on Unix, and runs
//! PowerShell on Windows. What it reads is turned into readings by
//! [`super::parse`], which holds all the format knowledge.

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::model::process::{CpuReading, ProcessSample, RawProcess};

use crate::tools::gpu;
use crate::tools::run::run_text;

use super::parse;

// ========================================================================
// Constants
// ========================================================================

/// Where Linux keeps what it knows about processes.
const PROC_DIR: &str = "/proc";

/// Where Unix keeps user names.
const PASSWD_FILE: &str = "/etc/passwd";

/// The columns `ps` is asked for, with the command line last because it holds
/// spaces: pid, parent, CPU, resident kilobytes, state, owner, arguments.
const PS_COLUMNS: &str = "pid=,ppid=,pcpu=,rss=,state=,user=,args=";

/// The columns for a second `ps`, which gives each process's executable.
const PS_COMM_COLUMNS: &str = "pid=,comm=";

/// The PowerShell that lists the machine's memory and processes as JSON. Each
/// process is an array `[pid, ppid, name, path, command line, cpu time, working
/// set]`; the leading comma keeps PowerShell from flattening one into the next.
const WINDOWS_SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue'; \
$mem=(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory; \
$procs=@(Get-CimInstance Win32_Process | ForEach-Object { ,@($_.ProcessId, \
$_.ParentProcessId, $_.Name, $_.ExecutablePath, $_.CommandLine, \
($_.KernelModeTime + $_.UserModeTime), $_.WorkingSetSize) }); \
ConvertTo-Json -Compress -Depth 3 @{mem=$mem; procs=$procs}";

// ========================================================================
// Functions
// ========================================================================

/// Every process on the machine, or why they could not be listed. Stops early
/// and returns what it has when `cancel` is set, which the caller discards.
pub fn collect(cancel: &AtomicBool) -> Result<ProcessSample, String> {
    let taken = Instant::now();
    let mut sample = if cfg!(windows) {
        collect_windows(taken)?
    } else if cfg!(target_os = "linux") {
        collect_proc(taken, cancel)?
    } else {
        collect_ps(taken)?
    };
    attach_video_memory(&mut sample, query_video_memory());
    Ok(sample)
}

/// Give each process in `sample` the video memory `usage` lists for it.
fn attach_video_memory(sample: &mut ProcessSample, usage: Option<HashMap<u32, u64>>) {
    let Some(usage) = usage else {
        return;
    };
    sample.has_gpu = true;
    for entry in &mut sample.entries {
        entry.gpu_kb = usage.get(&entry.pid).copied();
    }
}

/// Per-process video memory from `nvidia-smi`, or nothing when there is no
/// NVIDIA GPU or the tool does not answer in time.
fn query_video_memory() -> Option<HashMap<u32, u64>> {
    gpu::query_nvidia(&["-q", "-x"]).map(|xml| parse::parse_nvidia_smi_xml(&xml))
}

/// Read each numbered directory of `/proc`. A process can exit between being
/// listed and being read, so a directory that will not read is skipped rather
/// than failing the snapshot.
fn collect_proc(taken: Instant, cancel: &AtomicBool) -> Result<ProcessSample, String> {
    let listing = fs::read_dir(PROC_DIR).map_err(|e| format!("cannot read {PROC_DIR}: {e}"))?;
    let users = fs::read_to_string(PASSWD_FILE)
        .map(|text| parse::parse_passwd(&text))
        .unwrap_or_default();
    let total_mem_kb = fs::read_to_string(format!("{PROC_DIR}/meminfo"))
        .map(|text| parse::parse_meminfo_total(&text))
        .unwrap_or(0);

    let mut entries = Vec::new();
    for entry in listing.flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        entries.extend(read_proc_entry(pid, &users));
    }
    Ok(ProcessSample {
        entries,
        has_gpu: false,
        taken,
        total_mem_kb,
    })
}

/// One `/proc/<pid>` directory as a process, or nothing when it has gone.
fn read_proc_entry(pid: u32, users: &HashMap<u32, String>) -> Option<RawProcess> {
    let dir = format!("{PROC_DIR}/{pid}");
    let stat = parse::parse_proc_stat(&fs::read_to_string(format!("{dir}/stat")).ok()?)?;
    let (rss_kb, uid) = fs::read_to_string(format!("{dir}/status"))
        .map(|text| parse::parse_proc_status(&text))
        .unwrap_or((0, None));
    let cmdline = fs::read(format!("{dir}/cmdline"))
        .map(|bytes| parse::parse_proc_cmdline(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_default();
    let cwd = fs::read_link(format!("{dir}/cwd"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    let exec_path = fs::read_link(format!("{dir}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    Some(RawProcess {
        command: if cmdline.is_empty() {
            stat.name.clone()
        } else {
            cmdline
        },
        cpu: CpuReading::Time(stat.cpu_millis),
        cwd,
        exec_path,
        gpu_kb: None,
        name: stat.name,
        pid,
        ppid: Some(stat.ppid),
        rss_kb,
        state: stat.state,
        user: uid.map(|uid| users.get(&uid).cloned().unwrap_or_else(|| uid.to_string())),
    })
}

/// Ask `ps` for the table, and `sysctl` and `lsof` for what it leaves out.
/// Both of those are best effort: a sandbox may refuse them, and the monitor
/// is still worth having without a working directory column.
fn collect_ps(taken: Instant) -> Result<ProcessSample, String> {
    let table = run_text("ps", &["-axo", PS_COLUMNS])?;
    let names = run_text("ps", &["-axo", PS_COMM_COLUMNS])
        .map(|text| parse::parse_ps_comm(&text))
        .unwrap_or_default();
    let cwds = run_text("lsof", &["-a", "-d", "cwd", "-Fpn"])
        .map(|text| parse::parse_lsof_cwd(&text))
        .unwrap_or_default();
    let total_mem_kb = run_text("sysctl", &["-n", "hw.memsize"])
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map_or(0, |bytes| bytes / 1024);
    Ok(ProcessSample {
        entries: parse::parse_ps_rows(&table, &names, &cwds),
        has_gpu: false,
        taken,
        total_mem_kb,
    })
}

/// Ask PowerShell for the processes as JSON.
fn collect_windows(taken: Instant) -> Result<ProcessSample, String> {
    let text = run_text(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT],
    )?;
    let (total_mem_kb, entries) = parse::parse_windows_json(&text)
        .ok_or_else(|| "PowerShell did not list the processes".to_string())?;
    Ok(ProcessSample {
        entries,
        has_gpu: false,
        taken,
        total_mem_kb,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn test_the_snapshot_lists_this_process_with_its_real_parent_and_memory() {
        // The one check against the live /proc: a field read from the wrong
        // offset gives a plausible-looking but wrong pid, parent, or size.
        let sample = collect(&AtomicBool::new(false)).unwrap();
        let me = sample
            .entries
            .iter()
            .find(|entry| entry.pid == std::process::id())
            .expect("this process is listed");
        assert_eq!(me.ppid, Some(std::os::unix::process::parent_id()));
        assert!(me.rss_kb > 0);
        assert!(me.user.is_some());
        assert!(sample.total_mem_kb > 0);
    }
}
