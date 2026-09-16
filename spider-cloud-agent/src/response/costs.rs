//! What an attempt cost.

pub use crate::credits::{Credits, Usd, CREDITS_PER_USD};

use serde::{Deserialize, Serialize};

/// The cost breakdown the API returns with a fetch.
///
/// Every field is in US dollars, which is not the unit the account balance is
/// kept in. `/data/credits` answers in credits, and the two differ by
/// [`CREDITS_PER_USD`]. Measured against a real account on 2026-09-15: a fetch
/// reported a `total_cost` of 1.0870316666666668e-05 and the balance fell by
/// 0.108703, a ratio of 10,000.
///
/// So the fields are [`Usd`] rather than `f64`, and reading a charge in the
/// unit the rest of the crate uses means calling [`Costs::total`]. The wire
/// omits the block on some endpoints, so an absent breakdown reads as all zeros
/// rather than as an error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Costs {
    /// Model inference run for this request.
    #[serde(default)]
    pub ai_cost: Usd,
    /// Time spent fetching and rendering.
    #[serde(default)]
    pub compute_cost: Usd,
    /// Storage of files produced by the request.
    #[serde(default)]
    pub file_cost: Usd,
    /// Bytes moved for this request.
    #[serde(default)]
    pub bytes_transferred_cost: Usd,
    /// The sum the account is charged.
    #[serde(default)]
    pub total_cost: Usd,
    /// Converting the page into the requested format.
    #[serde(default)]
    pub transform_cost: Usd,
}

impl Costs {
    /// The charged total, converted to [`Credits`].
    pub fn total(&self) -> Credits {
        Credits::from(self.total_cost)
    }

    /// Model inference, in credits.
    pub fn ai(&self) -> Credits {
        Credits::from(self.ai_cost)
    }

    /// Fetching and rendering, in credits.
    pub fn compute(&self) -> Credits {
        Credits::from(self.compute_cost)
    }

    /// File storage, in credits.
    pub fn file(&self) -> Credits {
        Credits::from(self.file_cost)
    }

    /// Bytes moved, in credits.
    pub fn bytes_transferred(&self) -> Credits {
        Credits::from(self.bytes_transferred_cost)
    }

    /// Format conversion, in credits.
    pub fn transform(&self) -> Credits {
        Credits::from(self.transform_cost)
    }

    /// The parts added up, in credits.
    ///
    /// The line items are in the same unit as the total. On the fetch measured
    /// on 2026-09-15 they summed to the reported `total_cost` to the last digit
    /// the wire carried, so this is a cross-check on a bill rather than a
    /// second opinion about the unit.
    pub fn sum_of_parts(&self) -> Credits {
        Credits::from(
            self.ai_cost
                + self.compute_cost
                + self.file_cost
                + self.bytes_transferred_cost
                + self.transform_cost,
        )
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
    fn ten_thousand_credits_is_one_dollar() {
        assert_eq!(Credits(10_000.0).to_usd(), 1.0);
        assert_eq!(Credits::from_usd(0.25).get(), 2_500.0);
    }

    /// The bug this pins: `total_cost` arrives in dollars and was read as
    /// credits, so every cost the crate reported was 10,000 times too small and
    /// no credit budget could ever trip.
    #[test]
    fn a_cost_block_off_the_wire_is_dollars_and_converts_to_credits() {
        // The exact block a live example.com fetch returned on 2026-09-15. The
        // balance fell by 0.108703 credits for it.
        let costs: Costs = serde_json::from_str(
            r#"{"ai_cost":0.0,
                "bytes_transferred_cost":3.015e-08,
                "compute_cost":1.666666666666667e-09,
                "file_cost":8.385e-07,
                "total_cost":1.0870316666666668e-05,
                "transform_cost":1e-05}"#,
        )
        .expect("costs");

        assert_eq!(costs.total_cost, Usd::new(1.0870316666666668e-05));
        assert!((costs.total().get() - 0.10870316666666668).abs() < 1e-12);
        assert!((costs.transform().get() - 0.1).abs() < 1e-12);
        assert!((costs.sum_of_parts().get() - costs.total().get()).abs() < 1e-12);
        assert_eq!(costs.ai(), Credits::ZERO);
    }

    #[test]
    fn costs_read_a_partial_block() {
        let costs: Costs =
            serde_json::from_str(r#"{"compute_cost":2.0,"total_cost":2.0}"#).expect("costs");
        assert_eq!(costs.total(), Credits(20_000.0));
        assert_eq!(costs.ai_cost, Usd::ZERO);
        assert_eq!(costs.sum_of_parts(), Credits(20_000.0));
    }

    #[test]
    fn a_missing_cost_block_reads_as_nothing_spent() {
        let costs: Costs = serde_json::from_str("{}").expect("costs");
        assert_eq!(costs.total(), Credits::ZERO);
        assert_eq!(costs.sum_of_parts(), Credits::ZERO);
    }

    #[test]
    fn a_null_cost_line_reads_as_nothing_rather_than_failing_the_page() {
        // The block is optional on the wire, and so is any line in it. A
        // null where a number was expected used to fail the whole page, which
        // lost a page the account had already paid for.
        let costs: Costs = serde_json::from_str(
            r#"{"ai_cost":null,"compute_cost":2.0,"total_cost":null,"transform_cost":1.0}"#,
        )
        .expect("costs with null lines");
        assert_eq!(costs.ai_cost, Usd::ZERO);
        assert_eq!(costs.total_cost, Usd::ZERO);
        assert_eq!(costs.compute(), Credits(20_000.0));
    }

    #[test]
    fn a_cost_written_as_a_string_still_counts_against_the_budget() {
        // A number in quotes is still a charge. Reading it as zero would let
        // a crawl run past its budget while every page reported free.
        let costs: Costs =
            serde_json::from_str(r#"{"total_cost":"1.5e-05","compute_cost":"0.25"}"#)
                .expect("costs as strings");
        assert!((costs.total().get() - 0.15).abs() < 1e-9);
        assert_eq!(costs.compute(), Credits(2_500.0));

        let err = serde_json::from_str::<Costs>(r#"{"total_cost":"free"}"#)
            .expect_err("a word is not a charge");
        assert!(err.to_string().contains("free"), "{err}");
    }

    #[test]
    fn a_negative_cost_line_cannot_pull_the_spend_down() {
        // Nothing on a bill is negative. A refund line, or a bug on the
        // service, must not offset the pages that were charged for.
        let costs: Costs =
            serde_json::from_str(r#"{"total_cost":-0.5,"compute_cost":1.0}"#).expect("costs");
        assert_eq!(costs.total(), Credits::ZERO);
        assert_eq!(costs.compute(), Credits(10_000.0));
    }

    #[test]
    fn an_absurd_cost_still_trips_a_budget_rather_than_wrapping() {
        // The largest number the parser accepts. Past that serde_json refuses
        // the document as out of range, which is an error and not a panic.
        let costs: Costs =
            serde_json::from_str(r#"{"total_cost":1.7976931348623157e308}"#).expect("costs");
        assert!(costs.total().get() > 1e300);
        assert!(costs.total().get().is_infinite() || costs.total().get() > 0.0);
        assert!(serde_json::from_str::<Costs>(r#"{"total_cost":1e400}"#).is_err());
    }

    #[test]
    fn credits_add_up() {
        let total: Credits = [Credits(1.5), Credits(2.5)].into_iter().sum();
        assert_eq!(total, Credits(4.0));
    }
}
