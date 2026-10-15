//! Working out how busy each process is from two snapshots of how much CPU
//! time it has used.

use std::collections::HashMap;
use std::time::Instant;

use crate::model::process::{CpuReading, ProcessSample};

use super::view::Process;

// ========================================================================
// Constants
// ========================================================================

/// Milliseconds in a second.
const MILLIS_PER_SECOND: f64 = 1000.0;

/// Tenths of a percent in a percent.
const TENTHS_PER_PERCENT: f64 = 10.0;

// ========================================================================
// Data Structures
// ========================================================================

/// What the last snapshot said about each process's CPU time.
#[derive(Debug, Default)]
pub struct CpuTracker {
    previous: HashMap<u32, Baseline>,
}

/// One process's CPU time as of one snapshot.
#[derive(Clone, Copy, Debug)]
struct Baseline {
    at: Instant,
    millis: u64,
}

// ========================================================================
// CpuTracker
// ========================================================================

impl CpuTracker {
    /// An empty tracker, which reports every process idle until it has seen a
    /// second snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// The processes in `sample`, each with its CPU use since the snapshot
    /// before it, which becomes the one the next is measured against.
    pub fn rates(&mut self, sample: &ProcessSample) -> Vec<Process> {
        let mut baselines = HashMap::with_capacity(sample.entries.len());
        let processes = sample
            .entries
            .iter()
            .map(|raw| {
                let percent = match raw.cpu {
                    CpuReading::Rate(tenths) => f64::from(tenths) / TENTHS_PER_PERCENT,
                    CpuReading::Time(millis) => {
                        baselines.insert(
                            raw.pid,
                            Baseline {
                                at: sample.taken,
                                millis,
                            },
                        );
                        self.percent_since_last(raw.pid, millis, sample.taken)
                    }
                };
                Process::from_raw(raw, percent, sample.total_mem_kb)
            })
            .collect();
        // Only processes still running keep a baseline, so a pid the system
        // hands to a new process is measured from scratch.
        self.previous = baselines;
        processes
    }

    /// Share of one core used between the last snapshot and `at`, or nothing
    /// when there is no earlier reading or the counter went backwards (a new
    /// process under a reused pid).
    fn percent_since_last(&self, pid: u32, millis: u64, at: Instant) -> f64 {
        let Some(before) = self.previous.get(&pid) else {
            return 0.0;
        };
        let elapsed = at.saturating_duration_since(before.at).as_secs_f64();
        if elapsed <= 0.0 || millis < before.millis {
            return 0.0;
        }
        100.0 * (millis - before.millis) as f64 / MILLIS_PER_SECOND / elapsed
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::model::process::{ProcState, RawProcess};

    use super::*;

    fn raw(pid: u32, cpu: CpuReading) -> RawProcess {
        RawProcess {
            command: "x".to_string(),
            cpu,
            cwd: None,
            exec_path: None,
            gpu_kb: None,
            name: "x".to_string(),
            pid,
            ppid: None,
            rss_kb: 0,
            state: ProcState::Running,
            user: None,
        }
    }

    fn sample(taken: Instant, entries: Vec<RawProcess>) -> ProcessSample {
        ProcessSample {
            entries,
            has_gpu: false,
            taken,
            total_mem_kb: 0,
        }
    }

    #[test]
    fn test_the_first_snapshot_reports_every_process_idle() {
        let mut tracker = CpuTracker::new();
        let now = Instant::now();
        let first = tracker.rates(&sample(now, vec![raw(1, CpuReading::Time(9000))]));
        assert_eq!(first[0].cpu_percent, 0.0);
    }

    #[test]
    fn test_cpu_time_over_wall_time_is_the_share_of_a_core() {
        let mut tracker = CpuTracker::new();
        let start = Instant::now();
        tracker.rates(&sample(start, vec![raw(1, CpuReading::Time(1000))]));
        let later = start + Duration::from_secs(2);
        let rates = tracker.rates(&sample(later, vec![raw(1, CpuReading::Time(2000))]));
        // One CPU-second over two wall-seconds.
        assert_eq!(rates[0].cpu_percent, 50.0);
    }

    #[test]
    fn test_two_busy_threads_pass_a_hundred_percent() {
        let mut tracker = CpuTracker::new();
        let start = Instant::now();
        tracker.rates(&sample(start, vec![raw(1, CpuReading::Time(0))]));
        let later = start + Duration::from_secs(1);
        let rates = tracker.rates(&sample(later, vec![raw(1, CpuReading::Time(2000))]));
        assert_eq!(rates[0].cpu_percent, 200.0);
    }

    #[test]
    fn test_a_counter_that_goes_backwards_is_a_reused_pid_not_negative_use() {
        let mut tracker = CpuTracker::new();
        let start = Instant::now();
        tracker.rates(&sample(start, vec![raw(1, CpuReading::Time(5000))]));
        let later = start + Duration::from_secs(1);
        let rates = tracker.rates(&sample(later, vec![raw(1, CpuReading::Time(10))]));
        assert_eq!(rates[0].cpu_percent, 0.0);
    }

    #[test]
    fn test_a_pid_that_vanished_is_measured_from_scratch_when_it_returns() {
        let mut tracker = CpuTracker::new();
        let start = Instant::now();
        tracker.rates(&sample(start, vec![raw(1, CpuReading::Time(1000))]));
        tracker.rates(&sample(start + Duration::from_secs(1), vec![]));
        // Were the old baseline kept, this would read as 9 CPU-seconds in one.
        let rates = tracker.rates(&sample(
            start + Duration::from_secs(2),
            vec![raw(1, CpuReading::Time(10_000))],
        ));
        assert_eq!(rates[0].cpu_percent, 0.0);
    }

    #[test]
    fn test_a_rate_the_system_worked_out_passes_through_in_percent() {
        let mut tracker = CpuTracker::new();
        let rates = tracker.rates(&sample(Instant::now(), vec![raw(1, CpuReading::Rate(125))]));
        assert_eq!(rates[0].cpu_percent, 12.5);
    }
}
