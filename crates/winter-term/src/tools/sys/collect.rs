//! Taking a snapshot of the machine. This is the side-effecting half of the
//! system monitor: it reads `/proc` on Linux, asks `sysctl` and `vm_stat` on
//! macOS and the BSDs, and runs PowerShell on Windows. What it reads is turned
//! into readings by [`super::parse`], which holds all the format knowledge.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::system::{
    CoreLoad, CoreReading, CpuSample, MemoryReading, Overview, SystemSample,
};
use crate::tools::gpu;
use crate::tools::run::run_text;

use super::parse;

// ========================================================================
// Constants
// ========================================================================

/// Where Linux keeps what it knows about the machine.
const PROC_DIR: &str = "/proc";

/// The distribution's description.
const OS_RELEASE: &str = "/etc/os-release";

/// The kernel's own record of the machine's name and version.
const HOSTNAME_FILE: &str = "/proc/sys/kernel/hostname";
const KERNEL_RELEASE_FILE: &str = "/proc/sys/kernel/osrelease";

/// What `nvidia-smi` is asked for, one card to a line.
const GPU_QUERY: [&str; 2] = [
    "--query-gpu=name,memory.total,memory.used,utilization.gpu,temperature.gpu",
    "--format=csv,noheader,nounits",
];

/// The PowerShell that describes the machine as JSON: per-core load from the
/// performance counters (already a percentage), the processor and operating
/// system, page files, and the fixed drives. A leading comma keeps PowerShell
/// from flattening one row into the next.
const WINDOWS_SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue'; \
$cpu=@(Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor | ForEach-Object { ,@($_.Name, $_.PercentProcessorTime) }); \
$p=Get-CimInstance Win32_Processor | Select-Object -First 1; \
$os=Get-CimInstance Win32_OperatingSystem; \
$swap=@(Get-CimInstance Win32_PageFileUsage | ForEach-Object { ,@($_.AllocatedBaseSize, $_.CurrentUsage) }); \
$disks=@([IO.DriveInfo]::GetDrives() | Where-Object { $_.DriveType -eq 'Fixed' -and $_.IsReady } | \
ForEach-Object { ,@($_.Name, $_.DriveFormat, $_.TotalSize, $_.AvailableFreeSpace) }); \
ConvertTo-Json -Compress -Depth 4 @{cpu=$cpu; model=$p.Name; mhz=$p.MaxClockSpeed; totalKb=$os.TotalVisibleMemorySize; \
freeKb=$os.FreePhysicalMemory; caption=$os.Caption; version=$os.Version; \
uptime=((Get-Date) - $os.LastBootUpTime).TotalSeconds; swap=$swap; disks=$disks; host=$env:COMPUTERNAME}";

// ========================================================================
// Functions
// ========================================================================

/// The whole machine, or why it could not be read.
pub fn collect() -> Result<SystemSample, String> {
    let mut sample = if cfg!(windows) {
        collect_windows()?
    } else if cfg!(target_os = "linux") {
        collect_linux()?
    } else {
        collect_mac()?
    };
    sample.gpus = gpu::query_nvidia(&GPU_QUERY)
        .map(|csv| parse::parse_nvidia_gpus(&csv))
        .unwrap_or_default();
    Ok(sample)
}

/// Linux: the files under `/proc`, and `df` for the volumes.
fn collect_linux() -> Result<SystemSample, String> {
    let read = |name: &str| fs::read_to_string(format!("{PROC_DIR}/{name}"));
    let stat = read("stat").map_err(|e| format!("cannot read {PROC_DIR}/stat: {e}"))?;
    let (total, per_core) =
        parse::parse_proc_stat_cpu(&stat).ok_or("cannot read the processor counters")?;
    let (model, mhz) = read("cpuinfo")
        .map(|text| parse::parse_cpuinfo(&text))
        .unwrap_or_default();
    let cores = per_core
        .into_iter()
        .enumerate()
        .map(|(index, ticks)| CoreReading {
            load: CoreLoad::Ticks(ticks),
            mhz: mhz.get(index).copied().unwrap_or(0),
        })
        .collect();
    let memory = read("meminfo")
        .ok()
        .and_then(|text| parse::parse_meminfo(&text))
        .ok_or("cannot read the memory figures")?;
    let overview = Overview {
        arch: std::env::consts::ARCH.to_string(),
        hostname: trimmed(HOSTNAME_FILE),
        os: fs::read_to_string(OS_RELEASE)
            .ok()
            .and_then(|text| parse::parse_os_release(&text))
            .unwrap_or_else(|| std::env::consts::OS.to_string()),
        release: trimmed(KERNEL_RELEASE_FILE),
        uptime_secs: read("uptime")
            .ok()
            .and_then(|text| parse::parse_uptime(&text))
            .unwrap_or(0),
    };
    Ok(SystemSample {
        cpu: CpuSample {
            cores,
            load: read("loadavg")
                .ok()
                .and_then(|text| parse::parse_loadavg(&text)),
            model,
            total: CoreLoad::Ticks(total),
        },
        disks: run_text("df", &["-TkP"])
            .map(|text| parse::parse_df_linux(&text))
            .unwrap_or_default(),
        gpus: Vec::new(),
        memory,
        overview,
    })
}

/// macOS and the BSDs: `sysctl`, `vm_stat`, `ps` and `df`. There is no
/// per-core counter without a library, so the cores are left out and the
/// machine's load comes from what the processes are using.
fn collect_mac() -> Result<SystemSample, String> {
    let ask = |name: &str| run_text("sysctl", &["-n", name]).unwrap_or_default();
    let core_count = ask("hw.logicalcpu").trim().parse().unwrap_or(1);
    let load = run_text("ps", &["-A", "-o", "%cpu="])
        .map(|text| parse::parse_ps_cpu_total(&text, core_count))
        .map_err(|e| format!("cannot read the processor load: {e}"))?;
    let memory = parse::parse_mac_memory(
        &ask("hw.memsize"),
        &run_text("vm_stat", &[]).unwrap_or_default(),
        &run_text("sysctl", &["vm.swapusage"]).unwrap_or_default(),
    )
    .unwrap_or(MemoryReading {
        available_kb: 0,
        swap_total_kb: 0,
        swap_used_kb: 0,
        total_kb: 0,
    });
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let overview = Overview {
        arch: std::env::consts::ARCH.to_string(),
        hostname: run_text("hostname", &[])
            .map(|name| name.trim().to_string())
            .unwrap_or_default(),
        os: std::env::consts::OS.to_string(),
        release: run_text("uname", &["-r"])
            .map(|release| release.trim().to_string())
            .unwrap_or_default(),
        uptime_secs: parse::parse_mac_boot_time(&ask("kern.boottime"))
            .map_or(0, |boot| now.saturating_sub(boot)),
    };
    Ok(SystemSample {
        cpu: CpuSample {
            cores: Vec::new(),
            load: parse::parse_mac_loadavg(&ask("vm.loadavg")),
            model: ask("machdep.cpu.brand_string").trim().to_string(),
            total: CoreLoad::Percent(load),
        },
        disks: run_text("df", &["-P", "-k"])
            .map(|text| parse::parse_df_mac(&text))
            .unwrap_or_default(),
        gpus: Vec::new(),
        memory,
        overview,
    })
}

/// Windows: one PowerShell query for everything.
fn collect_windows() -> Result<SystemSample, String> {
    let text = run_text(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT],
    )?;
    let snapshot = parse::parse_windows_json(&text)
        .ok_or_else(|| "PowerShell did not describe the machine".to_string())?;
    Ok(SystemSample {
        cpu: CpuSample {
            cores: snapshot.cores,
            load: None,
            model: snapshot.model,
            total: CoreLoad::Percent(snapshot.total_percent),
        },
        disks: snapshot.disks,
        gpus: Vec::new(),
        memory: snapshot.memory,
        overview: Overview {
            arch: std::env::consts::ARCH.to_string(),
            hostname: snapshot.hostname,
            os: snapshot.os,
            release: snapshot.release,
            uptime_secs: snapshot.uptime_secs,
        },
    })
}

/// A small file's text without its trailing newline, or nothing when it will
/// not read.
fn trimmed(path: &str) -> String {
    fs::read_to_string(path)
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn test_the_snapshot_reads_this_machine_with_one_counter_per_core() {
        // The one check against the live /proc: a column read from the wrong
        // offset gives a plausible-looking but wrong total.
        let sample = collect().unwrap();
        assert!(!sample.cpu.cores.is_empty());
        assert!(!sample.cpu.model.is_empty());
        assert!(sample.memory.total_kb > 0);
        assert!(sample.memory.available_kb <= sample.memory.total_kb);
        assert!(sample.overview.uptime_secs > 0);
        assert!(!sample.overview.hostname.is_empty());
        assert!(!sample.disks.is_empty());
    }
}
