//! Trust tiers governing how much capability a block's content is granted.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ============================================================================
// Data Structures
// ============================================================================

/// How much capability a block's rendered content is granted. The emitting tool
/// *requests* a tier; the terminal *clamps* it by policy via [`TrustTier::clamp_to`].
///
/// Variants are declared in ascending capability order, and [`Ord`] follows that
/// order, so `Isolated < Restricted < Trusted`. Never compare tiers by any other
/// means: the clamp depends on this ordering being the capability ordering.
///
/// ```
/// use winter_proto::TrustTier;
///
/// assert!(TrustTier::Isolated < TrustTier::Restricted);
/// assert!(TrustTier::Restricted < TrustTier::Trusted);
/// ```
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustTier {
    /// Content from the network or AI output: sandboxed iframe, unique origin,
    /// no scripts unless explicitly opted in.
    Isolated,
    /// Unknown local CLIs (the default): CSP applied, no network, no top-level
    /// navigation.
    #[default]
    Restricted,
    /// First-party tools or a user-configured allowlist: full DOM and scripts.
    Trusted,
}

// ============================================================================
// TrustTier
// ============================================================================

impl TrustTier {
    /// Lower a tier *requested* on the wire to the highest tier the terminal's
    /// policy permits, returning the weaker of the two.
    ///
    /// Every tier arriving from a PTY is attacker-controlled: any byte stream
    /// reaching the terminal (a `cat` of a downloaded file, output piped from
    /// `curl`, a program on the far side of `ssh`) can spell `trust=trusted`.
    /// Nothing on the wire authenticates the emitter, so a requested tier is
    /// only ever a ceiling to be clamped, never a grant.
    ///
    /// ```
    /// use winter_proto::TrustTier;
    ///
    /// // A hostile stream asking for full scripting gets the policy ceiling.
    /// assert_eq!(
    ///     TrustTier::Trusted.clamp_to(TrustTier::Restricted),
    ///     TrustTier::Restricted
    /// );
    /// // Clamping never *raises* a tier the emitter asked to be weaker.
    /// assert_eq!(
    ///     TrustTier::Isolated.clamp_to(TrustTier::Trusted),
    ///     TrustTier::Isolated
    /// );
    /// ```
    pub fn clamp_to(self, ceiling: TrustTier) -> TrustTier {
        self.min(ceiling)
    }

    /// The canonical wire spelling used in TBP escape parameters.
    pub fn as_str(self) -> &'static str {
        match self {
            TrustTier::Isolated => "isolated",
            TrustTier::Restricted => "restricted",
            TrustTier::Trusted => "trusted",
        }
    }
}

impl FromStr for TrustTier {
    type Err = UnknownTier;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "isolated" => Ok(TrustTier::Isolated),
            "restricted" => Ok(TrustTier::Restricted),
            "trusted" => Ok(TrustTier::Trusted),
            _ => Err(UnknownTier),
        }
    }
}

/// Returned when a parameter value is not a recognized trust tier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownTier;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wire_spelling_round_trips() {
        for tier in [
            TrustTier::Isolated,
            TrustTier::Restricted,
            TrustTier::Trusted,
        ] {
            assert_eq!(TrustTier::from_str(tier.as_str()), Ok(tier));
        }
    }

    #[test]
    fn test_unknown_spelling_is_rejected() {
        assert_eq!(TrustTier::from_str("admin"), Err(UnknownTier));
    }

    #[test]
    fn test_capability_order_is_ascending() {
        assert!(TrustTier::Isolated < TrustTier::Restricted);
        assert!(TrustTier::Restricted < TrustTier::Trusted);
    }

    #[test]
    fn test_clamp_lowers_a_request_above_the_ceiling() {
        assert_eq!(
            TrustTier::Trusted.clamp_to(TrustTier::Restricted),
            TrustTier::Restricted
        );
        assert_eq!(
            TrustTier::Restricted.clamp_to(TrustTier::Isolated),
            TrustTier::Isolated
        );
    }

    #[test]
    fn test_clamp_never_raises_a_request_below_the_ceiling() {
        assert_eq!(
            TrustTier::Isolated.clamp_to(TrustTier::Trusted),
            TrustTier::Isolated
        );
        assert_eq!(
            TrustTier::Restricted.clamp_to(TrustTier::Trusted),
            TrustTier::Restricted
        );
    }

    #[test]
    fn test_clamp_to_the_default_ceiling_denies_scripting() {
        // The shipped default policy: nothing on the wire can reach Trusted.
        for requested in [
            TrustTier::Isolated,
            TrustTier::Restricted,
            TrustTier::Trusted,
        ] {
            assert_ne!(
                requested.clamp_to(TrustTier::default()),
                TrustTier::Trusted,
                "{requested:?} escaped the default ceiling"
            );
        }
    }
}
