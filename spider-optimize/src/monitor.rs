//! A tripwire on the success rate of applied edits.
//!
//! The gate decides from a model's estimate. Once edits are being applied,
//! the outcomes that come back are the only evidence that the estimate still
//! holds, and a model that has gone wrong keeps applying edits with the same
//! confidence it had when it was right. A [`Monitor`] holds the last `window`
//! outcomes and compares the success rate of the requests it edited with the
//! success rate of the requests it left alone. When the edited rate is worse
//! by more than the configured drop, with the noise of both rates taken off,
//! it trips, and the client falls back to shadow until someone resets it.
//!
//! # What the comparison is and is not
//!
//! The two groups are not a paired trial. The gate chose which requests to
//! edit, and it chose them because they scored well, so the edited group
//! leans towards the sites the model likes. A drop between the two rates can
//! be the edit doing harm, or the model choosing badly, or the two groups
//! fetching different sites on a bad afternoon. That is why this is a
//! tripwire and not a measurement: it stops the bleeding, and the paired
//! rows say what happened. The bound is a one sided normal approximation on
//! the difference of two independent proportions, which is loose at the
//! window sizes involved and takes no account of the confounding.
//!
//! # Concurrency
//!
//! One `AtomicU8` per outcome, a head counter and a latch. An observer packs
//! its outcome into one byte and stores it, so a verdict taken while other
//! tasks are observing reads whole outcomes and never a mixture of two. Two
//! observers landing on one slot at the same moment keep one of the two
//! outcomes, which is one outcome of `window` lost. Nothing locks and nothing
//! allocates after [`Monitor::new`].

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

/// The fewest outcomes a window may hold.
pub const MIN_WINDOW: usize = 64;

/// The most outcomes a window may hold.
pub const MAX_WINDOW: usize = 8_192;

/// The one sided bound used when none is given, the 95th percentile of a
/// standard normal.
pub const DEFAULT_Z: f32 = 1.645;

/// The slot holds an outcome.
const VALID: u8 = 0b001;
/// The request went out with an edit applied.
const APPLIED: u8 = 0b010;
/// The request got what the caller asked for.
const SUCCESS: u8 = 0b100;

/// What a monitor watches for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorConfig {
    /// How many of the latest outcomes are compared. Held between
    /// [`MIN_WINDOW`] and [`MAX_WINDOW`].
    pub window: usize,
    /// The fewest applied outcomes in the window before a verdict is given.
    /// At least one, and no more than the window.
    pub min_applied: u32,
    /// The fewest kept outcomes in the window before a verdict is given. At
    /// least one, and no more than the window.
    pub min_kept: u32,
    /// The largest drop in success rate, kept minus applied, that is let
    /// through. `0.02` is two points.
    pub max_drop: f32,
    /// How many standard errors the drop's lower bound sits under the drop.
    /// [`DEFAULT_Z`] unless set.
    pub z: f32,
}

impl Default for MonitorConfig {
    fn default() -> MonitorConfig {
        MonitorConfig {
            window: 1_024,
            min_applied: 100,
            min_kept: 100,
            max_drop: 0.02,
            z: DEFAULT_Z,
        }
    }
}

impl MonitorConfig {
    /// The configuration as the monitor will use it: the window clamped, the
    /// minimums held between one and the window, and a bound or a drop that
    /// is not a finite number replaced by the default.
    fn settled(self) -> MonitorConfig {
        let window = self.window.clamp(MIN_WINDOW, MAX_WINDOW);
        // The window is at most 8,192, so this cast is exact.
        let cap = window as u32;
        let defaults = MonitorConfig::default();
        MonitorConfig {
            window,
            min_applied: self.min_applied.clamp(1, cap),
            min_kept: self.min_kept.clamp(1, cap),
            max_drop: if self.max_drop.is_finite() {
                self.max_drop
            } else {
                defaults.max_drop
            },
            z: if self.z.is_finite() && self.z >= 0.0 {
                self.z
            } else {
                defaults.z
            },
        }
    }
}

/// The two rates over the window and how far apart they are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// How many outcomes in the window went out with an edit.
    pub applied: u32,
    /// How many of those succeeded.
    pub applied_successes: u32,
    /// How many outcomes in the window went out unedited.
    pub kept: u32,
    /// How many of those succeeded.
    pub kept_successes: u32,
    /// The kept success rate minus the applied one. Positive when edits are
    /// doing worse.
    pub drop: f32,
    /// The drop less `z` standard errors of the difference of the two rates.
    pub drop_lower_bound: f32,
}

/// What the window says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Too few outcomes of one kind or the other to say anything.
    Warming {
        /// How many applied outcomes the window holds.
        applied: u32,
        /// How many kept outcomes the window holds.
        kept: u32,
    },
    /// The edited requests are not doing measurably worse.
    Healthy(Estimate),
    /// The edited requests are doing worse by more than the allowed drop.
    Degraded(Estimate),
}

/// The last `window` outcomes and whether they have tripped it.
///
/// Cheap to share by reference from any number of tasks. See the module
/// documentation for what the comparison can and cannot tell you.
#[derive(Debug)]
pub struct Monitor {
    ring: Box<[AtomicU8]>,
    head: AtomicUsize,
    tripped: AtomicBool,
    config: MonitorConfig,
}

impl Default for Monitor {
    fn default() -> Monitor {
        Monitor::new(MonitorConfig::default())
    }
}

impl Monitor {
    /// A monitor with this configuration, settled as [`MonitorConfig`]
    /// describes. This is the one allocation.
    pub fn new(config: MonitorConfig) -> Monitor {
        let config = config.settled();
        let mut ring = Vec::with_capacity(config.window);
        ring.resize_with(config.window, AtomicU8::default);
        Monitor {
            ring: ring.into_boxed_slice(),
            head: AtomicUsize::new(0),
            tripped: AtomicBool::new(false),
            config,
        }
    }

    /// The configuration in use, after clamping.
    pub fn config(&self) -> &MonitorConfig {
        &self.config
    }

    /// Record one settled outcome: whether the request went out edited, and
    /// whether it got what the caller asked for.
    ///
    /// The oldest outcome in the window makes room. The window is then judged,
    /// so a monitor trips on the observation that crossed the line without
    /// anyone asking for a [`Monitor::verdict`].
    pub fn observe(&self, applied: bool, success: bool) {
        let mut packed = VALID;
        if applied {
            packed |= APPLIED;
        }
        if success {
            packed |= SUCCESS;
        }
        // The ring is never empty, so the remainder is always in range and
        // the lookup always lands. The counter wraps at the end of usize,
        // which puts one observation out of turn once in a process lifetime.
        let at = self.head.fetch_add(1, Ordering::Relaxed) % self.ring.len();
        if let Some(slot) = self.ring.get(at) {
            slot.store(packed, Ordering::Relaxed);
        }
        if !self.tripped.load(Ordering::Relaxed) {
            let _ = self.verdict();
        }
    }

    /// Judge the window as it stands, and latch [`Monitor::tripped`] when it
    /// is [`Verdict::Degraded`].
    ///
    /// The verdict describes the window now, so it reads healthy again once
    /// healthy outcomes have pushed the bad ones out. The latch does not.
    pub fn verdict(&self) -> Verdict {
        let mut applied = 0u32;
        let mut applied_successes = 0u32;
        let mut kept = 0u32;
        let mut kept_successes = 0u32;
        for slot in &self.ring {
            let packed = slot.load(Ordering::Relaxed);
            if packed & VALID == 0 {
                continue;
            }
            let succeeded = u32::from(packed & SUCCESS != 0);
            if packed & APPLIED != 0 {
                applied += 1;
                applied_successes += succeeded;
            } else {
                kept += 1;
                kept_successes += succeeded;
            }
        }
        if applied < self.config.min_applied || kept < self.config.min_kept {
            return Verdict::Warming { applied, kept };
        }

        // Both counts are at least one here, so neither rate divides by zero.
        let applied_rate = applied_successes as f32 / applied as f32;
        let kept_rate = kept_successes as f32 / kept as f32;
        let drop = kept_rate - applied_rate;
        let variance = kept_rate * (1.0 - kept_rate) / kept as f32
            + applied_rate * (1.0 - applied_rate) / applied as f32;
        let drop_lower_bound = drop - self.config.z * variance.max(0.0).sqrt();
        let estimate = Estimate {
            applied,
            applied_successes,
            kept,
            kept_successes,
            drop,
            drop_lower_bound,
        };
        if drop_lower_bound > self.config.max_drop {
            self.tripped.store(true, Ordering::Relaxed);
            Verdict::Degraded(estimate)
        } else {
            Verdict::Healthy(estimate)
        }
    }

    /// Whether a verdict has been [`Verdict::Degraded`] since the last
    /// [`Monitor::reset`].
    pub fn tripped(&self) -> bool {
        self.tripped.load(Ordering::Relaxed)
    }

    /// Forget every outcome and open the latch.
    ///
    /// An observer running at the same time may land an outcome in a slot
    /// this has already cleared, which is one outcome kept from before the
    /// reset, or in one it has not reached yet, which is one outcome lost.
    pub fn reset(&self) {
        for slot in &self.ring {
            slot.store(0, Ordering::Relaxed);
        }
        self.head.store(0, Ordering::Relaxed);
        self.tripped.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;

    /// A xorshift64, so a stream is the same on every run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        /// A coin that comes up true with this chance.
        fn chance(&mut self, p: f32) -> bool {
            // The top 24 bits, as a fraction in [0, 1).
            let unit = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
            unit < p
        }
    }

    fn config(window: usize, minimum: u32) -> MonitorConfig {
        MonitorConfig {
            window,
            min_applied: minimum,
            min_kept: minimum,
            max_drop: 0.02,
            z: DEFAULT_Z,
        }
    }

    #[test]
    fn a_healthy_stream_never_trips() {
        let monitor = Monitor::new(config(2_048, 100));
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut worst = f32::NEG_INFINITY;
        for _ in 0..10_000 {
            let applied = rng.chance(0.5);
            monitor.observe(applied, rng.chance(0.9));
            if let Verdict::Healthy(estimate) = monitor.verdict() {
                worst = worst.max(estimate.drop_lower_bound);
            }
            assert!(!monitor.tripped());
        }
        assert!(matches!(monitor.verdict(), Verdict::Healthy(_)));
        println!("healthy stream: worst lower bound on the drop {worst:.4}");
        assert!(worst <= 0.02);
    }

    #[test]
    fn degradation_trips_within_the_window_and_the_losses_are_counted() {
        const WINDOW: usize = 1_024;
        const MIN_APPLIED: u32 = 64;
        const KEPT_RATE: f32 = 0.9;
        const DEGRADED_RATE: f32 = 0.6;
        const K: usize = 3_000;

        let monitor = Monitor::new(MonitorConfig {
            min_kept: 64,
            ..config(WINDOW, MIN_APPLIED)
        });
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        let mut tripped_at = None;
        let mut applied_after_k = 0u32;
        let mut applied_failures_after_k = 0u32;
        for step in 0..K + WINDOW {
            let applied = rng.chance(0.5);
            let rate = if applied && step >= K {
                DEGRADED_RATE
            } else {
                KEPT_RATE
            };
            let success = rng.chance(rate);
            if applied && step >= K {
                applied_after_k += 1;
                applied_failures_after_k += u32::from(!success);
            }
            monitor.observe(applied, success);
            assert!(
                step >= K || !monitor.tripped(),
                "tripped at {step} before {K}"
            );
            if monitor.tripped() {
                tripped_at = Some(step);
                break;
            }
        }
        let tripped_at = tripped_at.expect("the drop from 0.9 to 0.6 trips the monitor");
        let delay = tripped_at + 1 - K;
        assert!(delay <= WINDOW, "tripped {delay} outcomes after the drop");

        // What the degraded edits cost before the trip: the applied failures a
        // kept request at the kept rate would not have suffered. The applied
        // outcomes after the drop are at most the ones that fit in one
        // window, plus the minimum the verdict waits for, so the excess
        // failures are bounded by that many at the rate difference.
        let excess = applied_failures_after_k as f32 - (1.0 - KEPT_RATE) * applied_after_k as f32;
        let expected = (KEPT_RATE - DEGRADED_RATE) * applied_after_k as f32;
        let bound = (KEPT_RATE - DEGRADED_RATE) * (MIN_APPLIED as f32 + WINDOW as f32);
        println!(
            "degradation: detected {delay} outcomes after the drop, {applied_after_k} of them applied; \
             {applied_failures_after_k} applied failures, {excess:.1} more than the kept rate would give \
             (expected {expected:.1}, bound {bound:.1})"
        );
        assert!(applied_after_k <= MIN_APPLIED + WINDOW as u32);
        assert!(excess <= bound, "{excess} losses over the bound {bound}");
        assert!(excess > 0.0, "the losses were counted");
        let Verdict::Degraded(estimate) = monitor.verdict() else {
            panic!("the window still reads degraded");
        };
        assert!(estimate.drop_lower_bound > 0.02);
        assert!(estimate.drop > estimate.drop_lower_bound);
    }

    #[test]
    fn a_tripped_monitor_stays_tripped_until_reset() {
        let monitor = Monitor::new(config(64, 8));
        for _ in 0..32 {
            monitor.observe(true, false);
            monitor.observe(false, true);
        }
        assert!(monitor.tripped());
        assert!(matches!(monitor.verdict(), Verdict::Degraded(_)));

        // A window of healthy outcomes reads healthy, and the latch holds.
        for _ in 0..64 {
            monitor.observe(true, true);
            monitor.observe(false, true);
        }
        let Verdict::Healthy(estimate) = monitor.verdict() else {
            panic!("the window is healthy");
        };
        assert_eq!(estimate.applied_successes, estimate.applied);
        assert!(monitor.tripped());

        monitor.reset();
        assert!(!monitor.tripped());
        assert_eq!(
            monitor.verdict(),
            Verdict::Warming {
                applied: 0,
                kept: 0
            }
        );
        monitor.observe(true, true);
        assert_eq!(
            monitor.verdict(),
            Verdict::Warming {
                applied: 1,
                kept: 0
            }
        );
    }

    #[test]
    fn warming_below_the_minimums_never_degrades() {
        let monitor = Monitor::new(config(64, 20));
        // Every edit fails and every kept request succeeds, and there are
        // still too few of one kind.
        for _ in 0..19 {
            monitor.observe(true, false);
        }
        for _ in 0..40 {
            monitor.observe(false, true);
        }
        assert_eq!(
            monitor.verdict(),
            Verdict::Warming {
                applied: 19,
                kept: 40
            }
        );
        assert!(!monitor.tripped());
        monitor.observe(true, false);
        assert!(matches!(monitor.verdict(), Verdict::Degraded(_)));
        assert!(monitor.tripped());

        // Not enough kept outcomes is warming too, whatever the edits do.
        let monitor = Monitor::new(config(64, 20));
        for _ in 0..40 {
            monitor.observe(true, false);
        }
        for _ in 0..19 {
            monitor.observe(false, true);
        }
        assert!(matches!(monitor.verdict(), Verdict::Warming { .. }));
        assert!(!monitor.tripped());
    }

    #[test]
    fn the_configuration_is_clamped_and_never_panics() {
        let wild = Monitor::new(MonitorConfig {
            window: 0,
            min_applied: 0,
            min_kept: u32::MAX,
            max_drop: f32::NAN,
            z: f32::NEG_INFINITY,
        });
        assert_eq!(
            *wild.config(),
            MonitorConfig {
                window: MIN_WINDOW,
                min_applied: 1,
                min_kept: MIN_WINDOW as u32,
                max_drop: 0.02,
                z: DEFAULT_Z,
            }
        );
        let huge = Monitor::new(MonitorConfig {
            window: usize::MAX,
            ..MonitorConfig::default()
        });
        assert_eq!(huge.config().window, MAX_WINDOW);
        for _ in 0..(MAX_WINDOW * 2 + 1) {
            huge.observe(true, true);
        }
        assert!(matches!(huge.verdict(), Verdict::Warming { .. }));
    }

    #[test]
    fn observations_from_many_threads_leave_a_well_formed_ring() {
        const THREADS: usize = 8;
        const EACH: usize = 500;
        let monitor = Monitor::new(config(256, 16));
        std::thread::scope(|scope| {
            for thread in 0..THREADS {
                let monitor = &monitor;
                scope.spawn(move || {
                    let mut rng = Rng(0xdead_beef + thread as u64);
                    for _ in 0..EACH {
                        monitor.observe(rng.chance(0.5), rng.chance(0.9));
                        let _ = monitor.verdict();
                    }
                });
            }
        });

        assert_eq!(monitor.head.load(Ordering::Relaxed), THREADS * EACH);
        let mut valid = 0;
        for slot in &monitor.ring {
            let packed = slot.load(Ordering::Relaxed);
            assert_eq!(packed & !(VALID | APPLIED | SUCCESS), 0, "{packed:#b}");
            assert_ne!(packed, 0, "every slot was written more than once");
            valid += 1;
        }
        assert_eq!(valid, 256);
        let (applied, kept) = match monitor.verdict() {
            Verdict::Warming { applied, kept } => (applied, kept),
            Verdict::Healthy(e) | Verdict::Degraded(e) => (e.applied, e.kept),
        };
        assert_eq!(applied + kept, 256);
    }
}
