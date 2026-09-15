//! Spend, in credits, in dollars, and in the whole credits a cap has to be.
//!
//! Three types, because the API uses three shapes for money and mixing them is
//! how a budget stops working.
//!
//! [`Credits`] is the crate's unit and the one a caller reasons in. [`Usd`] is
//! what the cost block on a fetch reply is denominated in, and it converts to
//! [`Credits`] only through [`CREDITS_PER_USD`], so a raw wire number cannot be
//! read as credits by accident. [`WholeCredits`] is what the
//! `max_credits_allowed` request parameter has to be, because the service
//! deserializes that one as a `u64` and rejects anything with a decimal point.

use serde::{Deserialize, Serialize};
use std::fmt;

/// How many credits make one US dollar.
pub const CREDITS_PER_USD: f64 = 10_000.0;

/// An amount of spend, in credits.
///
/// Ten thousand credits is a dollar, so a hundred is a penny. Fractions are
/// real: a plain HTTP fetch costs well under one credit, which is the whole
/// reason routing to one is worth doing.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Credits(pub f64);

impl Credits {
    /// Nothing spent.
    pub const ZERO: Credits = Credits(0.0);

    /// An amount given in credits.
    pub const fn new(credits: f64) -> Credits {
        Credits(credits)
    }

    /// An amount given in US dollars, converted at [`CREDITS_PER_USD`].
    pub fn from_usd(usd: f64) -> Credits {
        Credits(usd * CREDITS_PER_USD)
    }

    /// The amount in credits.
    pub const fn get(self) -> f64 {
        self.0
    }

    /// The amount in US dollars, converted at [`CREDITS_PER_USD`].
    pub fn to_usd(self) -> f64 {
        self.0 / CREDITS_PER_USD
    }
}

/// An amount in US dollars.
///
/// The cost block on a fetch reply is in dollars, not in credits, and the two
/// differ by [`CREDITS_PER_USD`]. This type exists so that the wire numbers
/// cannot be handed to [`Credits`] without going through the conversion. The
/// inner value is private for the same reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Usd(f64);

impl Usd {
    /// Nothing spent.
    pub const ZERO: Usd = Usd(0.0);

    /// An amount given in US dollars.
    pub const fn new(usd: f64) -> Usd {
        Usd(usd)
    }

    /// The amount in US dollars.
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl From<Usd> for Credits {
    fn from(usd: Usd) -> Credits {
        Credits::from_usd(usd.get())
    }
}

impl From<Credits> for Usd {
    fn from(credits: Credits) -> Usd {
        Usd(credits.to_usd())
    }
}

impl std::ops::Add for Usd {
    type Output = Usd;

    fn add(self, other: Usd) -> Usd {
        Usd(self.0 + other.0)
    }
}

impl std::iter::Sum for Usd {
    fn sum<I: Iterator<Item = Usd>>(iter: I) -> Usd {
        iter.fold(Usd::ZERO, |total, next| total + next)
    }
}

impl fmt::Display for Usd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${}", self.0)
    }
}

/// A credit cap, as the whole number the service demands.
///
/// `max_credits_allowed` is a `u64` on the service. Sending `30.0` for it comes
/// back as a 400 naming the type, so the cap has to round before it goes out.
/// It rounds down, because a cap that rounded up would allow more than the
/// caller asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WholeCredits(u64);

impl WholeCredits {
    /// A cap given in whole credits.
    pub const fn new(credits: u64) -> WholeCredits {
        WholeCredits(credits)
    }

    /// The largest whole cap that does not exceed `credits`.
    ///
    /// A fraction under one credit floors to zero, and zero means no cap to the
    /// service, so it floors to one instead. A caller who asked for half a
    /// credit wanted the tightest cap there is, not none.
    pub fn floor(credits: Credits) -> WholeCredits {
        let whole = credits.get().max(0.0).floor();
        if whole < 1.0 {
            WholeCredits(1)
        } else {
            WholeCredits(whole.min(u64::MAX as f64) as u64)
        }
    }

    /// The cap in whole credits.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The cap as [`Credits`].
    pub fn credits(self) -> Credits {
        Credits(self.0 as f64)
    }
}

impl fmt::Display for WholeCredits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} credits", self.0)
    }
}

impl std::ops::Add for Credits {
    type Output = Credits;

    fn add(self, other: Credits) -> Credits {
        Credits(self.0 + other.0)
    }
}

impl std::ops::AddAssign for Credits {
    fn add_assign(&mut self, other: Credits) {
        self.0 += other.0;
    }
}

impl std::iter::Sum for Credits {
    fn sum<I: Iterator<Item = Credits>>(iter: I) -> Credits {
        iter.fold(Credits::ZERO, |total, next| total + next)
    }
}

impl fmt::Display for Credits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} credits", self.0)
    }
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
    fn a_dollar_is_ten_thousand_credits() {
        assert_eq!(Credits::from_usd(1.0), Credits(10_000.0));
        assert_eq!(Credits(10_000.0).to_usd(), 1.0);
    }

    #[test]
    fn credits_sum() {
        let total: Credits = [Credits(1.5), Credits(2.5)].into_iter().sum();
        assert_eq!(total, Credits(4.0));
    }

    #[test]
    fn credits_serialize_as_a_bare_number() {
        assert_eq!(serde_json::to_string(&Credits(2.5)).unwrap(), "2.5");
    }

    #[test]
    fn dollars_reach_credits_only_through_the_rate() {
        // The ratio measured against a real account on 2026-09-15: a fetch
        // reported 1.0870316666666668e-05 and the balance fell by 0.108703.
        let reported = Usd::new(1.0870316666666668e-05);
        let charged = Credits::from(reported);

        assert!((charged.get() - 0.10870316666666668).abs() < 1e-15);
        assert_eq!(Credits::from(Usd::new(1.0)), Credits(10_000.0));
        assert_eq!(Usd::from(Credits(10_000.0)), Usd::new(1.0));
    }

    #[test]
    fn dollars_serialize_as_the_bare_number_the_wire_sent() {
        assert_eq!(serde_json::to_string(&Usd::new(0.25)).unwrap(), "0.25");
        assert_eq!(
            serde_json::from_str::<Usd>("1e-05").unwrap(),
            Usd::new(0.00001)
        );
    }

    #[test]
    fn a_whole_cap_rounds_down_so_it_never_allows_more_than_was_asked() {
        assert_eq!(WholeCredits::floor(Credits(30.9)).get(), 30);
        assert_eq!(WholeCredits::floor(Credits(30.0)).get(), 30);

        // A cap under one credit floors to one, because zero reads to the
        // service as no cap at all.
        assert_eq!(WholeCredits::floor(Credits(0.5)).get(), 1);
        assert_eq!(WholeCredits::floor(Credits::ZERO).get(), 1);
        assert_eq!(WholeCredits::floor(Credits(-5.0)).get(), 1);
    }

    #[test]
    fn a_whole_cap_serializes_as_an_integer_because_the_service_rejects_a_float() {
        // The live service answers a float here with:
        // "Deserialization error: max_credits_allowed: invalid type: floating
        // point `30.0`, expected u64".
        assert_eq!(
            serde_json::to_string(&WholeCredits::floor(Credits(30.0))).unwrap(),
            "30"
        );
        assert_eq!(WholeCredits::new(7).credits(), Credits(7.0));
    }
}
