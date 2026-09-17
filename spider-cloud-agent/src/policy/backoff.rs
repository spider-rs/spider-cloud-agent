//! How long to wait before sending a request again.
//!
//! The service tells you the wait for a rate limit and for load shedding. When it
//! does not, the curve here fills the gap: each retry doubles the one before, up to a
//! ceiling.

use std::time::Duration;

/// How much of a computed delay is drawn at random.
///
/// Randomizing matters when many clients fail at the same moment, because a fixed
/// curve makes them all come back at the same moment too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Jitter {
    /// Wait exactly what the curve says.
    None,
    /// Wait between half and all of what the curve says, drawn from this seed and the
    /// retry count. The draw is a pure function of those two numbers, so a run of the
    /// simulator produces the same sleeps every time.
    Seeded(u64),
}

/// The retry curve, and whether the service gets to override it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// The first wait. Each further retry doubles it.
    pub base: Duration,
    /// The longest computed wait. A server-directed wait is never shortened.
    pub cap: Duration,
    /// How much of the wait is drawn at random.
    pub jitter: Jitter,
    /// Use the wait the service asked for, when it asked for one.
    pub honor_retry_after: bool,
}

impl Default for Backoff {
    /// Half a second doubling to a ceiling of thirty, no jitter, and the service's own
    /// figure wins.
    fn default() -> Backoff {
        Backoff {
            base: Duration::from_millis(500),
            cap: Duration::from_secs(30),
            jitter: Jitter::None,
            honor_retry_after: true,
        }
    }
}

impl Backoff {
    /// The default curve with the draw seeded, for a process running many clients at
    /// once against one account.
    pub fn seeded(seed: u64) -> Backoff {
        Backoff {
            jitter: Jitter::Seeded(seed),
            ..Backoff::default()
        }
    }

    /// How long to wait before retry number `retries`, counting the first retry as
    /// zero.
    ///
    /// `retry_after` is the span the service asked for, taken from `Retry-After` and
    /// falling back to `RateLimit-Reset`. It wins over the curve when
    /// [`Backoff::honor_retry_after`] is set, and is never shortened. Seeded jitter
    /// adds at most the base delay plus one millisecond. The budget refuses a wait
    /// that cannot fit its wall.
    pub fn delay(&self, retries: u32, retry_after: Option<Duration>) -> Duration {
        if self.honor_retry_after {
            if let Some(asked) = retry_after {
                return asked.saturating_add(match self.jitter {
                    Jitter::None => Duration::ZERO,
                    Jitter::Seeded(seed) => Duration::from_millis(1)
                        .saturating_add(self.base.min(self.cap).mul_f64(unit_draw(seed, retries))),
                });
            }
        }

        let doubled = self.base.saturating_mul(1u32 << retries.min(16));
        let capped = doubled.min(self.cap);

        match self.jitter {
            Jitter::None => capped,
            Jitter::Seeded(seed) => capped.mul_f64(0.5 + 0.5 * unit_draw(seed, retries)),
        }
    }
}

/// A number in `[0, 1)` from a seed and a counter, using SplitMix64.
///
/// Nothing here needs cryptographic quality. It needs to be the same number on every
/// machine on every run, which a random number generator pulled from the environment
/// would not be.
fn unit_draw(seed: u64, counter: u32) -> f64 {
    let mut z = seed
        .wrapping_add(u64::from(counter).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // 53 bits is every value an f64 can hold exactly between zero and one.
    ((z >> 11) as f64) / ((1u64 << 53) as f64)
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;

    #[test]
    fn the_curve_doubles_until_it_hits_the_ceiling() {
        let backoff = Backoff {
            base: Duration::from_millis(500),
            cap: Duration::from_secs(4),
            jitter: Jitter::None,
            honor_retry_after: true,
        };
        assert_eq!(backoff.delay(0, None), Duration::from_millis(500));
        assert_eq!(backoff.delay(1, None), Duration::from_secs(1));
        assert_eq!(backoff.delay(2, None), Duration::from_secs(2));
        assert_eq!(backoff.delay(3, None), Duration::from_secs(4));
        assert_eq!(backoff.delay(9, None), Duration::from_secs(4));
    }

    #[test]
    fn the_service_figure_is_never_shortened() {
        let backoff = Backoff {
            cap: Duration::from_secs(10),
            ..Backoff::default()
        };
        assert_eq!(
            backoff.delay(0, Some(Duration::from_secs(7))),
            Duration::from_secs(7)
        );
        assert_eq!(
            backoff.delay(0, Some(Duration::from_secs(600))),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn the_service_figure_is_ignored_when_it_was_turned_off() {
        let backoff = Backoff {
            honor_retry_after: false,
            ..Backoff::default()
        };
        assert_eq!(
            backoff.delay(0, Some(Duration::from_secs(7))),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn a_seeded_draw_repeats_and_stays_within_half_the_curve() {
        let backoff = Backoff::seeded(42);
        let first = backoff.delay(3, None);
        assert_eq!(first, Backoff::seeded(42).delay(3, None));

        let plain = Duration::from_millis(500) * 8;
        assert!(first >= plain / 2, "{first:?} below half of {plain:?}");
        assert!(first <= plain, "{first:?} above {plain:?}");
    }

    #[test]
    fn two_seeds_disagree() {
        assert_ne!(
            Backoff::seeded(1).delay(2, None),
            Backoff::seeded(2).delay(2, None)
        );
    }

    #[test]
    fn f2_server_waits_get_positive_bounded_seeded_jitter() {
        let asked = Duration::from_secs(600);
        let first = Backoff::seeded(1).delay(0, Some(asked));
        assert_eq!(first, Backoff::seeded(1).delay(0, Some(asked)));
        assert_ne!(first, Backoff::seeded(2).delay(0, Some(asked)));
        assert!(first > asked);
        assert!(first <= asked + Duration::from_millis(501));
    }

    #[test]
    fn the_draw_covers_its_range() {
        let mut low = false;
        let mut high = false;
        for seed in 0..500u64 {
            let d = unit_draw(seed, 0);
            assert!((0.0..1.0).contains(&d), "seed {seed} drew {d}");
            low |= d < 0.25;
            high |= d > 0.75;
        }
        assert!(low && high, "the draw is stuck in the middle of its range");
    }
}
