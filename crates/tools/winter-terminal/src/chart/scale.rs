//! Where a value lands along an axis, and the round numbers its ticks fall
//! on.

// ========================================================================
// Constants
// ========================================================================

/// About how many ticks an axis is given: enough to read values off, few
/// enough that their labels do not crowd.
const TARGET_TICKS: f64 = 5.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A continuous axis: values from `low` to `high` laid along `length`
/// pixels, the low end at zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Linear {
    pub high: f64,
    pub length: f64,
    pub low: f64,
    /// The gap between ticks, a round number.
    pub step: f64,
}

/// A discrete axis: `count` categories, each given a band of the axis, with
/// `padding` of each band left empty between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub count: usize,
    pub length: f64,
    pub padding: f64,
}

// ========================================================================
// Band
// ========================================================================

impl Band {
    /// How wide each category's band is.
    pub fn step(&self) -> f64 {
        self.length / self.count.max(1) as f64
    }

    /// How wide what is drawn in a band is, the padding left out.
    pub fn width(&self) -> f64 {
        self.step() * (1.0 - self.padding)
    }

    /// Where category `index`'s drawing starts.
    pub fn start(&self, index: usize) -> f64 {
        self.step() * index as f64 + self.step() * self.padding / 2.0
    }

    /// The middle of category `index`'s band.
    pub fn middle(&self, index: usize) -> f64 {
        self.step() * (index as f64 + 0.5)
    }
}

// ========================================================================
// Linear
// ========================================================================

impl Linear {
    /// An axis `length` pixels long over the values from `low` to `high`,
    /// both widened to the round numbers ticks fall on.
    pub fn nice(low: f64, high: f64, length: f64) -> Self {
        let (low, high) = match high > low {
            true => (low, high),
            // A single value is given room around it rather than an axis of
            // no extent, which would put every point on the same pixel.
            false if low == 0.0 => (0.0, 1.0),
            false => (low - low.abs() / 2.0, high + high.abs() / 2.0),
        };
        let step = nice_step((high - low) / TARGET_TICKS);
        Self {
            high: (high / step).ceil() * step,
            length,
            low: (low / step).floor() * step,
            step,
        }
    }

    /// An axis `length` pixels long over exactly the values from `low` to
    /// `high`, for a domain whose ticks are placed by other means.
    pub fn exact(low: f64, high: f64, length: f64) -> Self {
        let high = if high > low { high } else { low + 1.0 };
        Self {
            high,
            length,
            low,
            step: high - low,
        }
    }

    /// How far along the axis `value` lands.
    pub fn place(&self, value: f64) -> f64 {
        (value - self.low) / (self.high - self.low) * self.length
    }

    /// The values the ticks are at, low to high.
    pub fn ticks(&self) -> Vec<f64> {
        let count = ((self.high - self.low) / self.step).round() as usize;
        (0..=count)
            .map(|at| self.low + self.step * at as f64)
            .collect()
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The round number nearest above `rough`: 1, 2, or 5 times a power of ten.
fn nice_step(rough: f64) -> f64 {
    if rough <= 0.0 || !rough.is_finite() {
        return 1.0;
    }
    let power = 10f64.powf(rough.log10().floor());
    let fraction = rough / power;
    let nice = match fraction {
        f if f <= 1.0 => 1.0,
        f if f <= 2.0 => 2.0,
        f if f <= 5.0 => 5.0,
        _ => 10.0,
    };
    nice * power
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_axis_widens_to_round_numbers() {
        let axis = Linear::nice(0.0, 87.0, 100.0);
        assert_eq!((axis.low, axis.high, axis.step), (0.0, 100.0, 20.0));
        assert_eq!(axis.ticks(), vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]);
        assert_eq!(axis.place(50.0), 50.0);
    }

    #[test]
    fn test_small_and_negative_ranges_get_their_own_round_steps() {
        let axis = Linear::nice(-0.3, 0.42, 100.0);
        assert_eq!(axis.step, 0.2);
        assert!(axis.low <= -0.3 && axis.high >= 0.42);
    }

    #[test]
    fn test_a_single_value_is_given_room() {
        let axis = Linear::nice(5.0, 5.0, 100.0);
        assert!(axis.low < 5.0 && axis.high > 5.0);
        let axis = Linear::nice(0.0, 0.0, 100.0);
        assert!(axis.high > 0.0);
    }

    #[test]
    fn test_bands_split_the_axis_and_leave_their_padding_between() {
        let band = Band {
            count: 4,
            length: 400.0,
            padding: 0.2,
        };
        assert_eq!(band.step(), 100.0);
        assert_eq!(band.width(), 80.0);
        assert_eq!(band.start(1), 110.0);
        assert_eq!(band.middle(3), 350.0);
    }
}
