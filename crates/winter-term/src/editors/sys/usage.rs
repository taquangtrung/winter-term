//! Working out how busy each core is from two snapshots of its scheduler
//! counters.

use crate::model::system::{CoreLoad, CpuSample, CpuTicks};

// ========================================================================
// Constants
// ========================================================================

/// Tenths of a percent in a whole.
const FULL_TENTHS: u64 = 1000;

// ========================================================================
// Data Structures
// ========================================================================

/// The counters the last snapshot saw, machine first and then each core.
#[derive(Debug, Default)]
pub struct UsageTracker {
    previous: Vec<Option<CpuTicks>>,
}

/// How busy the machine and each core have been, in tenths of a percent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    /// Each core, empty where the system does not say per core.
    pub cores: Vec<u32>,
    /// The machine as a whole.
    pub total: u32,
}

// ========================================================================
// UsageTracker
// ========================================================================

impl UsageTracker {
    /// A tracker that has seen nothing, and so reports each core's use since
    /// boot until it has seen a second snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// How busy `cpu` has been since the snapshot before it, which becomes the
    /// one the next is measured against.
    pub fn usage(&mut self, cpu: &CpuSample) -> Usage {
        let loads: Vec<CoreLoad> = std::iter::once(cpu.total)
            .chain(cpu.cores.iter().map(|core| core.load))
            .collect();
        let mut percents = Vec::with_capacity(loads.len());
        let mut next = Vec::with_capacity(loads.len());
        for (index, load) in loads.into_iter().enumerate() {
            let before = self.previous.get(index).copied().flatten();
            match load {
                CoreLoad::Percent(tenths) => {
                    percents.push(tenths);
                    next.push(None);
                }
                CoreLoad::Ticks(now) => {
                    percents.push(tenths_between(before, now));
                    next.push(Some(now));
                }
            }
        }
        self.previous = next;
        let mut percents = percents.into_iter();
        Usage {
            total: percents.next().unwrap_or(0),
            cores: percents.collect(),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Share of the ticks between `before` and `now` that were busy, or since
/// boot when there is no earlier reading or the counters went backwards.
fn tenths_between(before: Option<CpuTicks>, now: CpuTicks) -> u32 {
    let (busy, total) = match before {
        Some(before) if now.total > before.total && now.busy >= before.busy => {
            (now.busy - before.busy, now.total - before.total)
        }
        // No ticks passed, so there is nothing to say about the interval.
        Some(before) if now.total == before.total => return 0,
        _ => (now.busy, now.total),
    };
    if total == 0 {
        return 0;
    }
    (busy.min(total) * FULL_TENTHS / total) as u32
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::model::system::CoreReading;

    use super::*;

    fn cpu(total: CoreLoad, cores: Vec<CoreLoad>) -> CpuSample {
        CpuSample {
            cores: cores
                .into_iter()
                .map(|load| CoreReading { load, mhz: 0 })
                .collect(),
            load: None,
            model: String::new(),
            total,
        }
    }

    fn ticks(busy: u64, total: u64) -> CoreLoad {
        CoreLoad::Ticks(CpuTicks { busy, total })
    }

    #[test]
    fn test_the_first_reading_is_the_share_since_boot() {
        let mut tracker = UsageTracker::new();
        let usage = tracker.usage(&cpu(ticks(250, 1000), vec![ticks(100, 400)]));
        assert_eq!(usage.total, 250);
        assert_eq!(usage.cores, [250]);
    }

    #[test]
    fn test_the_second_reading_is_the_share_of_the_ticks_between_them() {
        let mut tracker = UsageTracker::new();
        tracker.usage(&cpu(ticks(250, 1000), vec![ticks(100, 400)]));
        // 100 more ticks went by and 50 were busy; since boot would say 35%.
        let usage = tracker.usage(&cpu(ticks(300, 1100), vec![ticks(150, 500)]));
        assert_eq!(usage.total, 500);
        assert_eq!(usage.cores, [500]);
    }

    #[test]
    fn test_counters_that_go_backwards_start_over_rather_than_underflow() {
        let mut tracker = UsageTracker::new();
        tracker.usage(&cpu(ticks(900, 1000), vec![]));
        let usage = tracker.usage(&cpu(ticks(10, 100), vec![]));
        assert_eq!(usage.total, 100);
    }

    #[test]
    fn test_a_share_the_system_already_worked_out_passes_through() {
        let mut tracker = UsageTracker::new();
        let usage = tracker.usage(&cpu(
            CoreLoad::Percent(423),
            vec![CoreLoad::Percent(5), CoreLoad::Percent(990)],
        ));
        assert_eq!(usage.total, 423);
        assert_eq!(usage.cores, [5, 990]);
    }

    #[test]
    fn test_no_ticks_between_snapshots_reads_idle_not_since_boot() {
        let mut tracker = UsageTracker::new();
        tracker.usage(&cpu(ticks(500, 1000), vec![]));
        assert_eq!(tracker.usage(&cpu(ticks(500, 1000), vec![])).total, 0);
    }
}
