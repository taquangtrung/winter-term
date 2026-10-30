//! Reading what each system says about the machine: the files Linux keeps
//! under `/proc`, the commands macOS and the BSDs answer through, the table
//! `df` prints, `nvidia-smi`'s CSV, and the JSON a PowerShell query writes on
//! Windows. Pure text in, readings out, so each format is testable anywhere.

use serde_json::Value;

use crate::model::system::{
    CoreLoad, CoreReading, CpuTicks, DiskReading, GpuReading, MemoryReading,
};

// ========================================================================
// Constants
// ========================================================================

/// Bytes in a kilobyte.
const BYTES_PER_KB: u64 = 1024;

/// Kilobytes in a mebibyte, the unit `nvidia-smi` reports memory in.
const KB_PER_MIB: u64 = 1024;

/// Hundredths in one, for load averages kept as whole numbers.
const HUNDREDTHS: f64 = 100.0;

/// Tenths of a percent in a percent.
const TENTHS_PER_PERCENT: f64 = 10.0;

/// Filesystem types that are not storage: kernel interfaces and overlays.
const VIRTUAL_FS_TYPES: [&str; 22] = [
    "autofs",
    "bpf",
    "binfmt_misc",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "overlay",
    "proc",
    "pstore",
    "rpc_pipefs",
    "securityfs",
    "squashfs",
    "sysfs",
    "tmpfs",
];

/// What macOS's `df` calls pseudo filesystems.
const MAC_VIRTUAL_PREFIXES: [&str; 2] = ["devfs", "map "];

/// The one support volume under `/System/Volumes` worth listing: the others
/// share the container `/` and this already show.
const MAC_DATA_VOLUME: &str = "/System/Volumes/Data";
const MAC_SYSTEM_VOLUMES: &str = "/System/Volumes/";

/// Fields of a Linux `df -TkP` row before the mount point, which holds spaces:
/// filesystem, type, size, used, available, capacity.
const LINUX_DF_FIELDS: usize = 6;

/// Units of the swap sizes `sysctl vm.swapusage` prints, in kilobytes.
const SWAP_UNITS: [(char, f64); 3] = [('K', 1.0), ('M', 1024.0), ('G', 1024.0 * 1024.0)];

// ========================================================================
// Linux
// ========================================================================

/// The `cpu` and `cpuN` lines of `/proc/stat` as counters: the machine's
/// total, then each core in order.
pub fn parse_proc_stat_cpu(raw: &str) -> Option<(CpuTicks, Vec<CpuTicks>)> {
    let mut total = None;
    let mut cores = Vec::new();
    for line in raw.lines() {
        let mut fields = line.split_whitespace();
        let Some(label) = fields.next().filter(|label| label.starts_with("cpu")) else {
            continue;
        };
        let counts: Vec<u64> = fields.map_while(|field| field.parse().ok()).collect();
        // user nice system idle iowait irq softirq steal guest guest_nice
        if counts.len() < 5 {
            return None;
        }
        let all: u64 = counts.iter().sum();
        let ticks = CpuTicks {
            busy: all - counts[3] - counts[4],
            total: all,
        };
        if label == "cpu" {
            total = Some(ticks);
        } else {
            cores.push(ticks);
        }
    }
    Some((total?, cores))
}

/// The processor's name and each logical processor's clock in megahertz from
/// `/proc/cpuinfo`.
pub fn parse_cpuinfo(raw: &str) -> (String, Vec<u32>) {
    let mut model = String::new();
    let mut mhz = Vec::new();
    for line in raw.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "model name" if model.is_empty() => model = value.trim().to_string(),
            "cpu MHz" => mhz.push(value.trim().parse::<f64>().map_or(0, |mhz| mhz as u32)),
            _ => {}
        }
    }
    (model, mhz)
}

/// Memory and swap from `/proc/meminfo`. Memory counts as available by the
/// kernel's own `MemAvailable`, which includes what caches would give back.
pub fn parse_meminfo(raw: &str) -> Option<MemoryReading> {
    let value = |name: &str| -> Option<u64> {
        raw.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
    };
    let total_kb = value("MemTotal")?;
    let swap_total_kb = value("SwapTotal").unwrap_or(0);
    let swap_free_kb = value("SwapFree").unwrap_or(0);
    Some(MemoryReading {
        available_kb: value("MemAvailable").or_else(|| value("MemFree"))?,
        swap_total_kb,
        swap_used_kb: swap_total_kb.saturating_sub(swap_free_kb),
        total_kb,
    })
}

/// The three load averages in hundredths from `/proc/loadavg`.
pub fn parse_loadavg(raw: &str) -> Option<[u32; 3]> {
    let mut fields = raw.split_whitespace();
    let mut next = || fields.next()?.parse::<f64>().ok().map(hundredths);
    Some([next()?, next()?, next()?])
}

/// Whole seconds since boot from `/proc/uptime`.
pub fn parse_uptime(raw: &str) -> Option<u64> {
    let seconds: f64 = raw.split_whitespace().next()?.parse().ok()?;
    (seconds >= 0.0).then_some(seconds as u64)
}

/// The distribution's name from `/etc/os-release`.
pub fn parse_os_release(raw: &str) -> Option<String> {
    raw.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|name| name.trim().trim_matches('"').to_string())
        .filter(|name| !name.is_empty())
}

/// Volumes from `df -TkP` (Linux): a row is filesystem, type, size, used,
/// available, capacity, then the mount point, which may hold spaces.
pub fn parse_df_linux(raw: &str) -> Vec<DiskReading> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let (fields, mount) = split_fields::<LINUX_DF_FIELDS>(line)?;
            let [_, fs_type, total, used, available, _] = fields;
            let mount = mount.trim();
            if mount.is_empty() || VIRTUAL_FS_TYPES.contains(&fs_type) {
                return None;
            }
            disk(mount, Some(fs_type), total, used, available)
        })
        .collect()
}

// ========================================================================
// macOS and the BSDs
// ========================================================================

/// Volumes from `df -P -k`: filesystem (which may hold spaces, as in
/// `map auto_home`), size, used, available, capacity, mount point (which may
/// too). The capacity column, the first ending in `%` after three numbers,
/// is what the two ends are found from.
pub fn parse_df_mac(raw: &str) -> Vec<DiskReading> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let capacity = (4..tokens.len()).find(|&at| {
                tokens[at].ends_with('%')
                    && tokens[at - 3..at].iter().all(|n| n.parse::<u64>().is_ok())
            })?;
            let filesystem = tokens[..capacity - 3].join(" ");
            let mount = line[offset_after(line, tokens[capacity])..].trim();
            if MAC_VIRTUAL_PREFIXES
                .iter()
                .any(|prefix| filesystem.starts_with(prefix))
                || (mount.starts_with(MAC_SYSTEM_VOLUMES) && mount != MAC_DATA_VOLUME)
            {
                return None;
            }
            disk(
                mount,
                Some(&filesystem),
                tokens[capacity - 3],
                tokens[capacity - 2],
                tokens[capacity - 1],
            )
        })
        .collect()
}

/// Memory and swap on macOS from `sysctl -n hw.memsize` (bytes), `vm_stat`,
/// and `sysctl vm.swapusage`. `vm_stat` counts pages of the size its header
/// names. Free pages alone are nearly nothing on a Mac that has been up a
/// while, since idle memory is kept as cache, so what counts as available is
/// free plus inactive pages, the ones the kernel reclaims first.
pub fn parse_mac_memory(memsize: &str, vm_stat: &str, swap: &str) -> Option<MemoryReading> {
    let total_kb = memsize.trim().parse::<u64>().ok()? / BYTES_PER_KB;
    if total_kb == 0 {
        return None;
    }
    let page_bytes = vm_stat
        .split("page size of ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        .unwrap_or(4096);
    let pages = |label: &str| -> Option<u64> {
        vm_stat
            .lines()
            .find_map(|line| line.strip_prefix(label)?.strip_prefix(':'))
            .and_then(|rest| rest.trim().trim_end_matches('.').parse().ok())
    };
    let available_pages = pages("Pages free")? + pages("Pages inactive").unwrap_or(0);
    let (swap_total_kb, swap_used_kb) = parse_mac_swap(swap);
    Some(MemoryReading {
        available_kb: (available_pages * page_bytes / BYTES_PER_KB).min(total_kb),
        swap_total_kb,
        swap_used_kb,
        total_kb,
    })
}

/// The load averages in hundredths from `sysctl -n vm.loadavg`: `{ 1.2 1.3 1.4 }`.
pub fn parse_mac_loadavg(raw: &str) -> Option<[u32; 3]> {
    let mut numbers = raw
        .split(|c: char| c.is_whitespace() || c == '{' || c == '}')
        .filter(|field| !field.is_empty());
    let mut next = || numbers.next()?.parse::<f64>().ok().map(hundredths);
    Some([next()?, next()?, next()?])
}

/// The boot time in seconds since the epoch from `sysctl -n kern.boottime`:
/// `{ sec = 1700000000, usec = 123456 } Tue Nov 14 ...`.
pub fn parse_mac_boot_time(raw: &str) -> Option<u64> {
    raw.split("sec =")
        .nth(1)?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Share of all cores in use, in tenths of a percent, from the `%cpu` of every
/// process (`ps -A -o %cpu=`) over `cores` logical cores. `ps` averages over a
/// short decaying window, so this follows the load rather than matching a
/// monitor that samples counters, which is all macOS offers without a library.
pub fn parse_ps_cpu_total(raw: &str, cores: u32) -> u32 {
    let sum: f64 = raw
        .lines()
        .filter_map(|line| line.trim().parse::<f64>().ok())
        .filter(|percent| percent.is_finite() && *percent > 0.0)
        .sum();
    let share = sum / f64::from(cores.max(1));
    (share.min(100.0) * TENTHS_PER_PERCENT).round() as u32
}

/// Total and used swap in kilobytes from `vm.swapusage`:
/// `total = 2048.00M  used = 1024.50M  free = 1023.50M`.
fn parse_mac_swap(raw: &str) -> (u64, u64) {
    let amount = |label: &str| -> u64 {
        let Some(rest) = raw.split(label).nth(1) else {
            return 0;
        };
        let text = rest
            .trim_start_matches([' ', '='])
            .split_whitespace()
            .next();
        let Some(text) = text else {
            return 0;
        };
        let unit = text.chars().last().unwrap_or('M');
        let number: f64 = text
            .trim_end_matches(char::is_alphabetic)
            .parse()
            .unwrap_or(0.0);
        let factor = SWAP_UNITS
            .iter()
            .find(|(candidate, _)| *candidate == unit)
            .map_or(1.0, |(_, factor)| *factor);
        (number * factor) as u64
    };
    (amount("total"), amount("used"))
}

// ========================================================================
// GPU
// ========================================================================

/// Cards from `nvidia-smi --query-gpu=name,memory.total,memory.used,
/// utilization.gpu,temperature.gpu --format=csv,noheader,nounits`. A field the
/// card does not report reads `[N/A]`, which is no number.
pub fn parse_nvidia_gpus(raw: &str) -> Vec<GpuReading> {
    raw.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let [name, total, used, util, temp] = fields[..] else {
                return None;
            };
            let total_mib: u64 = total.parse().ok().filter(|mib| *mib > 0)?;
            Some(GpuReading {
                mem_total_kb: total_mib * KB_PER_MIB,
                mem_used_kb: used.parse::<u64>().unwrap_or(0) * KB_PER_MIB,
                name: name.to_string(),
                temp_c: temp.parse().ok(),
                util_percent: util.parse().unwrap_or(0),
            })
        })
        .collect()
}

// ========================================================================
// Windows
// ========================================================================

/// Everything the Windows query writes, before the parts that need no
/// PowerShell (the architecture) are added.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsSnapshot {
    /// Each logical core's share in tenths of a percent, and the clock.
    pub cores: Vec<CoreReading>,
    /// Volumes.
    pub disks: Vec<DiskReading>,
    /// The machine's name.
    pub hostname: String,
    /// Memory and swap.
    pub memory: MemoryReading,
    /// The processor's name.
    pub model: String,
    /// The edition and version, as Windows words them.
    pub os: String,
    /// The version number.
    pub release: String,
    /// The machine as a whole, in tenths of a percent.
    pub total_percent: u32,
    /// Seconds since boot.
    pub uptime_secs: u64,
}

/// The JSON the Windows query writes. Perf counters name the whole machine
/// `_Total` and each core by its number.
pub fn parse_windows_json(text: &str) -> Option<WindowsSnapshot> {
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}').trim()).ok()?;
    let mhz = u32::try_from(value["mhz"].as_u64().unwrap_or(0)).unwrap_or(0);
    let mut cores = Vec::new();
    let mut total_percent = 0;
    for row in value["cpu"].as_array()? {
        let (Some(name), Some(percent)) = (row[0].as_str(), row[1].as_f64()) else {
            continue;
        };
        let tenths = (percent.clamp(0.0, 100.0) * TENTHS_PER_PERCENT).round() as u32;
        if name == "_Total" {
            total_percent = tenths;
        } else {
            cores.push((name.parse::<u32>().unwrap_or(u32::MAX), tenths));
        }
    }
    cores.sort_unstable_by_key(|(index, _)| *index);
    let kb = |key: &str| value[key].as_u64().unwrap_or(0);
    let total_kb = kb("totalKb");
    let (mut swap_total_kb, mut swap_used_kb) = (0, 0);
    for row in value["swap"].as_array().into_iter().flatten() {
        // Both columns are in megabytes.
        swap_total_kb += row[0].as_u64().unwrap_or(0) * KB_PER_MIB;
        swap_used_kb += row[1].as_u64().unwrap_or(0) * KB_PER_MIB;
    }
    let disks = value["disks"]
        .as_array()?
        .iter()
        .filter_map(|row| {
            let total = row[2].as_u64()? / BYTES_PER_KB;
            let available = row[3].as_u64()? / BYTES_PER_KB;
            let mount = row[0].as_str()?.trim_end_matches(['\\', '/']);
            (total > 0).then(|| DiskReading {
                available_kb: available,
                fs_type: row[1].as_str().map(str::to_string),
                mount: mount.to_string(),
                total_kb: total,
                used_kb: total.saturating_sub(available),
            })
        })
        .collect();
    Some(WindowsSnapshot {
        cores: cores
            .into_iter()
            .map(|(_, tenths)| CoreReading {
                load: CoreLoad::Percent(tenths),
                mhz,
            })
            .collect(),
        disks,
        hostname: value["host"].as_str().unwrap_or_default().to_string(),
        memory: MemoryReading {
            available_kb: kb("freeKb"),
            swap_total_kb,
            swap_used_kb,
            total_kb,
        },
        model: value["model"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
        os: value["caption"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
        release: value["version"].as_str().unwrap_or_default().to_string(),
        total_percent,
        uptime_secs: value["uptime"]
            .as_f64()
            .map_or(0, |secs| secs.max(0.0) as u64),
    })
}

// ========================================================================
// Shared
// ========================================================================

/// A volume from the numbers of a `df` row, or nothing for a zero-sized one
/// (a pseudo filesystem that slipped through).
fn disk(
    mount: &str,
    fs_type: Option<&str>,
    total: &str,
    used: &str,
    available: &str,
) -> Option<DiskReading> {
    let total_kb: u64 = total.parse().ok().filter(|kb| *kb > 0)?;
    Some(DiskReading {
        available_kb: available.parse().ok()?,
        fs_type: fs_type.map(str::to_string),
        mount: mount.to_string(),
        total_kb,
        used_kb: used.parse().ok()?,
    })
}

/// A load average as a whole number of hundredths.
fn hundredths(load: f64) -> u32 {
    (load.max(0.0) * HUNDREDTHS).round() as u32
}

/// The first `N` whitespace-separated fields of `line` and what remains of it.
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

/// Where in `line` the text after `token` begins, `token` being a slice of it.
fn offset_after(line: &str, token: &str) -> usize {
    token.as_ptr() as usize - line.as_ptr() as usize + token.len()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_ticks_exclude_idle_and_iowait_from_the_busy_count() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\ncpu1 40 0 30 400 30 0 0 0 0 0\nintr 1 2 3\n";
        let (total, cores) = parse_proc_stat_cpu(stat).unwrap();
        // Waiting on a disk is not work: 1000 ticks, 150 of them busy.
        assert_eq!(
            total,
            CpuTicks {
                busy: 150,
                total: 1000
            }
        );
        assert_eq!(cores.len(), 2);
        assert_eq!(
            cores[1],
            CpuTicks {
                busy: 70,
                total: 500
            }
        );
    }

    #[test]
    fn test_a_truncated_stat_is_not_a_reading() {
        assert_eq!(parse_proc_stat_cpu("cpu 1 2 3\n"), None);
        assert_eq!(parse_proc_stat_cpu("intr 1\n"), None);
    }

    #[test]
    fn test_cpuinfo_gives_the_first_model_and_one_clock_per_processor() {
        let info = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i7\ncpu MHz\t\t: 3200.512\n\nprocessor\t: 1\nmodel name\t: Intel(R) Core(TM) i7\ncpu MHz\t\t: 800.0\n";
        let (model, mhz) = parse_cpuinfo(info);
        assert_eq!(model, "Intel(R) Core(TM) i7");
        assert_eq!(mhz, [3200, 800]);
    }

    #[test]
    fn test_available_memory_prefers_memavailable_and_falls_back_to_memfree() {
        let with = "MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 600 kB\nSwapTotal: 200 kB\nSwapFree: 50 kB\n";
        let memory = parse_meminfo(with).unwrap();
        assert_eq!(memory.available_kb, 600);
        assert_eq!((memory.swap_total_kb, memory.swap_used_kb), (200, 150));
        let old = "MemTotal: 1000 kB\nMemFree: 100 kB\n";
        assert_eq!(parse_meminfo(old).unwrap().available_kb, 100);
        assert_eq!(parse_meminfo("nothing"), None);
    }

    #[test]
    fn test_load_uptime_and_distribution_name() {
        assert_eq!(
            parse_loadavg("0.52 0.60 1.006 2/900 1234\n"),
            Some([52, 60, 101])
        );
        assert_eq!(parse_uptime("12345.67 99999.0\n"), Some(12345));
        let release = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n";
        assert_eq!(
            parse_os_release(release).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
    }

    #[test]
    fn test_linux_df_keeps_spaces_in_a_mount_and_drops_virtual_and_empty_volumes() {
        let df = "Filesystem Type 1024-blocks Used Available Capacity Mounted on\n\
            /dev/sda1 ext4 1000000 400000 600000 41% /\n\
            tmpfs tmpfs 8000 0 8000 0% /run\n\
            /dev/sdb1 vfat 2000 1000 1000 50% /media/my disk\n\
            none ext4 0 0 0 0% /empty\n";
        let disks = parse_df_linux(df);
        let mounts: Vec<&str> = disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, ["/", "/media/my disk"]);
        assert_eq!(disks[0].fs_type.as_deref(), Some("ext4"));
        assert_eq!((disks[0].used_kb, disks[0].available_kb), (400000, 600000));
    }

    #[test]
    fn test_mac_df_reads_spaced_names_and_skips_support_volumes() {
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
            /dev/disk3s1s1 488245288 20000000 300000000 7% /\n\
            devfs 200 200 0 100% /dev\n\
            map auto_home 0 0 0 100% /System/Volumes/Data/home\n\
            /dev/disk3s5 488245288 150000000 300000000 34% /System/Volumes/Data\n\
            /dev/disk3s6 488245288 20480 300000000 1% /System/Volumes/VM\n\
            /dev/disk4s1 1000 500 500 50% /Volumes/My Disk\n";
        let mounts: Vec<String> = parse_df_mac(df).into_iter().map(|d| d.mount).collect();
        assert_eq!(mounts, ["/", "/System/Volumes/Data", "/Volumes/My Disk"]);
    }

    #[test]
    fn test_mac_memory_counts_free_and_inactive_pages_as_available() {
        let vm_stat = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:    1000.\nPages inactive:    3000.\nPages speculative:  50.\n";
        let swap = "total = 2048.00M  used = 1024.50M  free = 1023.50M  (encrypted)";
        let memory = parse_mac_memory("17179869184", vm_stat, swap).unwrap();
        assert_eq!(memory.total_kb, 16_777_216);
        // 4000 pages of 16 KiB.
        assert_eq!(memory.available_kb, 64_000);
        assert_eq!(memory.swap_total_kb, 2048 * 1024);
        assert_eq!(memory.swap_used_kb, 1_049_088);
        assert_eq!(parse_mac_memory("0", vm_stat, swap), None);
    }

    #[test]
    fn test_mac_load_and_boot_time_come_out_of_their_braces() {
        assert_eq!(
            parse_mac_loadavg("{ 1.64 1.78 1.82 }\n"),
            Some([164, 178, 182])
        );
        let boot = "{ sec = 1700000000, usec = 123456 } Tue Nov 14 22:13:20 2023\n";
        assert_eq!(parse_mac_boot_time(boot), Some(1_700_000_000));
    }

    #[test]
    fn test_the_process_cpu_sum_is_a_share_of_all_cores_and_capped() {
        // 200% across four cores is half the machine.
        assert_eq!(parse_ps_cpu_total("100.0\n50.0\n 50.0\n0.0\n", 4), 500);
        // A multi-threaded burst over the core count must not read past full.
        assert_eq!(parse_ps_cpu_total("900.0\n", 2), 1000);
        assert_eq!(parse_ps_cpu_total("", 0), 0);
    }

    #[test]
    fn test_gpu_rows_survive_a_field_the_card_does_not_report() {
        let csv = "NVIDIA GeForce RTX 3080, 10240, 2048, 17, 54\nQuadro, 4096, 100, [N/A], [N/A]\nbad row\n";
        let gpus = parse_nvidia_gpus(csv);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].mem_total_kb, 10240 * 1024);
        assert_eq!((gpus[0].util_percent, gpus[0].temp_c), (17, Some(54)));
        // Unreported temperature is unknown, not zero degrees.
        assert_eq!((gpus[1].util_percent, gpus[1].temp_c), (0, None));
    }

    #[test]
    fn test_windows_json_orders_cores_numerically_and_splits_out_the_total() {
        let text = "\u{feff}{\"cpu\":[[\"10\",5.0],[\"2\",50.5],[\"_Total\",20.0],[\"0\",99.0]],\"mhz\":3600,\"model\":\" Ryzen \",\"totalKb\":16777216,\"freeKb\":8388608,\"caption\":\"Microsoft Windows 11 Pro\",\"version\":\"10.0.22631\",\"uptime\":3661.9,\"swap\":[[2048,512]],\"disks\":[[\"C:\\\\\",\"NTFS\",1073741824,536870912]],\"host\":\"BOX\"}";
        let snapshot = parse_windows_json(text).unwrap();
        // "10" sorts after "2" by number, not before it as text.
        let loads: Vec<CoreLoad> = snapshot.cores.iter().map(|c| c.load).collect();
        assert_eq!(
            loads,
            [
                CoreLoad::Percent(990),
                CoreLoad::Percent(505),
                CoreLoad::Percent(50)
            ]
        );
        assert_eq!(snapshot.total_percent, 200);
        assert_eq!(snapshot.model, "Ryzen");
        assert_eq!(snapshot.memory.swap_used_kb, 512 * 1024);
        assert_eq!(snapshot.uptime_secs, 3661);
        assert_eq!(snapshot.disks[0].mount, "C:");
        assert_eq!(snapshot.disks[0].used_kb, 524_288);
    }

    #[test]
    fn test_text_that_is_not_the_windows_query_is_rejected() {
        assert!(parse_windows_json("Get-CimInstance : Access denied").is_none());
    }
}
