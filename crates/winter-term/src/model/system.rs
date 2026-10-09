//! What a system monitor reads from the machine: one snapshot of its CPU,
//! memory, disks and GPUs, before any rate has been worked out from it.

// ========================================================================
// Data Structures
// ========================================================================

/// The whole machine at one moment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemSample {
    /// Processor load.
    pub cpu: CpuSample,
    /// Mounted local volumes.
    pub disks: Vec<DiskReading>,
    /// Graphics cards, empty where there is no tool to ask.
    pub gpus: Vec<GpuReading>,
    /// Physical memory and swap.
    pub memory: MemoryReading,
    /// Which machine this is.
    pub overview: Overview,
}

/// Processor load, overall and per core.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuSample {
    /// Each logical core, empty where the system will not say per core.
    pub cores: Vec<CoreReading>,
    /// Load averages over one, five and fifteen minutes, in hundredths.
    pub load: Option<[u32; 3]>,
    /// The processor's marketing name.
    pub model: String,
    /// The machine as a whole.
    pub total: CoreLoad,
}

/// One core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoreReading {
    /// How busy it is.
    pub load: CoreLoad,
    /// Its clock speed in megahertz, or zero when unknown.
    pub mhz: u32,
}

/// How busy a core is, in whichever form the system reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreLoad {
    /// A share the system already worked out, in tenths of a percent.
    Percent(u32),
    /// Counters since boot, from which a share needs two snapshots.
    Ticks(CpuTicks),
}

/// Scheduler counters since boot, in whatever unit the system counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuTicks {
    /// Ticks spent doing work.
    pub busy: u64,
    /// All ticks, busy or not.
    pub total: u64,
}

/// One mounted volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiskReading {
    /// Kilobytes free to an ordinary user.
    pub available_kb: u64,
    /// The filesystem's type, where the system tells.
    pub fs_type: Option<String>,
    /// Where it is mounted.
    pub mount: String,
    /// Size in kilobytes.
    pub total_kb: u64,
    /// Kilobytes in use.
    pub used_kb: u64,
}

/// One graphics card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuReading {
    /// Video memory in kilobytes.
    pub mem_total_kb: u64,
    /// Video memory in use, in kilobytes.
    pub mem_used_kb: u64,
    /// The card's name.
    pub name: String,
    /// Temperature in degrees Celsius, where it reports one.
    pub temp_c: Option<u32>,
    /// How busy the card is.
    pub util_percent: u32,
}

/// Physical memory and swap, in kilobytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryReading {
    /// Memory that programs could use without swapping: free plus what the
    /// system would give back from caches.
    pub available_kb: u64,
    /// Swap in use.
    pub swap_used_kb: u64,
    /// Swap configured.
    pub swap_total_kb: u64,
    /// Physical memory installed.
    pub total_kb: u64,
}

/// Which machine this is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Overview {
    /// The processor architecture.
    pub arch: String,
    /// The machine's name.
    pub hostname: String,
    /// The operating system's name.
    pub os: String,
    /// Its kernel or version string.
    pub release: String,
    /// Seconds since boot.
    pub uptime_secs: u64,
}
