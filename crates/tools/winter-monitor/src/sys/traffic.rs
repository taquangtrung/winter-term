//! Working out how fast each network interface and disk moves bytes from two
//! snapshots of its counters.

use std::collections::HashMap;
use std::time::Instant;

use winter_model::system::TrafficReading;

// ========================================================================
// Data Structures
// ========================================================================

/// The counters the last snapshot saw, by device, and when it was taken.
#[derive(Debug, Default)]
pub struct TrafficTracker {
    previous: HashMap<String, (u64, u64)>,
    taken: Option<Instant>,
}

/// How fast one device moved bytes between two snapshots, each way.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrafficRate {
    /// Bytes a second in: received, or read.
    pub in_per_sec: u64,
    /// The device's name.
    pub name: String,
    /// Bytes a second out: sent, or written.
    pub out_per_sec: u64,
}

// ========================================================================
// TrafficTracker
// ========================================================================

impl TrafficTracker {
    /// How fast each of `readings`, taken at `now`, moved since the snapshot
    /// before, which they replace. `None` for the first snapshot, which has
    /// nothing to be measured against.
    pub fn rates(&mut self, readings: &[TrafficReading], now: Instant) -> Option<Vec<TrafficRate>> {
        let elapsed = self
            .taken
            .map(|taken| now.saturating_duration_since(taken).as_secs_f64());
        let rates = elapsed.filter(|secs| *secs > 0.0).map(|secs| {
            readings
                .iter()
                .map(|reading| {
                    let (before_in, before_out) = self
                        .previous
                        .get(&reading.name)
                        .copied()
                        .unwrap_or((reading.in_bytes, reading.out_bytes));
                    // A counter that went backwards was reset: no rate for it.
                    let per_sec = |now: u64, before: u64| {
                        (now.saturating_sub(before) as f64 / secs).round() as u64
                    };
                    TrafficRate {
                        in_per_sec: per_sec(reading.in_bytes, before_in),
                        name: reading.name.clone(),
                        out_per_sec: per_sec(reading.out_bytes, before_out),
                    }
                })
                .collect()
        });
        self.previous = readings
            .iter()
            .map(|reading| (reading.name.clone(), (reading.in_bytes, reading.out_bytes)))
            .collect();
        self.taken = Some(now);
        rates
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn reading(name: &str, in_bytes: u64, out_bytes: u64) -> TrafficReading {
        TrafficReading {
            in_bytes,
            name: name.to_string(),
            out_bytes,
        }
    }

    #[test]
    fn test_a_rate_is_the_bytes_between_two_snapshots_over_the_time_between() {
        let mut tracker = TrafficTracker::default();
        let start = Instant::now();
        assert_eq!(tracker.rates(&[reading("eth0", 1000, 500)], start), None);
        let rates = tracker
            .rates(
                &[reading("eth0", 5000, 2500)],
                start + Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(rates[0].in_per_sec, 2000);
        assert_eq!(rates[0].out_per_sec, 1000);
    }

    #[test]
    fn test_a_device_new_or_reset_since_the_last_snapshot_reads_idle() {
        // A rate from zero would count every byte since boot as moved in the
        // last two seconds; a counter reset would underflow.
        let mut tracker = TrafficTracker::default();
        let start = Instant::now();
        tracker.rates(&[reading("eth0", 9000, 9000)], start);
        let later = start + Duration::from_secs(1);
        let rates = tracker
            .rates(
                &[reading("eth0", 10, 10), reading("wlan0", 7000, 7000)],
                later,
            )
            .unwrap();
        assert_eq!((rates[0].in_per_sec, rates[0].out_per_sec), (0, 0));
        assert_eq!((rates[1].in_per_sec, rates[1].out_per_sec), (0, 0));
    }
}
