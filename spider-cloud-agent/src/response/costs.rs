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
///
/// The total can be more than the five named parts add up to. When an outside
/// vendor served the page, what it billed is counted in `total_cost` and
/// reported under [`Costs::vendor`], not as one of the parts, so
/// [`Costs::sum_of_parts`] is a subtotal of the service's own charges and
/// [`Costs::total`] is the bill.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// The outside vendor that served the page and what that cost, when one
    /// did. The service writes the block only on a vendor-served page, so a
    /// page it fetched itself reads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<VendorCosts>,
}

/// Who served a page when the service handed it to an outside vendor, and
/// what that cost.
///
/// This is the half of a routing decision a caller cannot otherwise see.
/// Without it a page can be served by a vendor and billed the markup for it
/// while the reply says nothing about which vendor, what it charged, or whose
/// key paid. Amounts are in US dollars like the rest of the block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VendorCosts {
    /// The vendor, by its canonical name.
    #[serde(default)]
    pub provider: Option<String>,
    /// The vendor route that served the page, such as `vendor.unlocker`.
    #[serde(default)]
    pub route: String,
    /// What the vendor itself charged. The pass-through, not what the caller
    /// pays.
    #[serde(default)]
    pub vendor_cost: Usd,
    /// What the caller is charged for the dispatch: the vendor cost plus the
    /// markup, or the markup alone on the caller's own key.
    #[serde(default)]
    pub billed_cost: Usd,
    /// The dispatch ran on the caller's own vendor key, so the caller paid
    /// the markup here and the vendor invoiced them directly.
    #[serde(default)]
    pub byok: bool,
    /// How many vendor routes were tried before one served the page. One
    /// means the first choice worked.
    #[serde(default)]
    pub attempts: u32,
}

impl VendorCosts {
    /// What the caller was charged for the vendor dispatch, in credits.
    pub fn billed(&self) -> Credits {
        Credits::from(self.billed_cost)
    }

    /// What the vendor itself charged, in credits.
    pub fn charged_by_vendor(&self) -> Credits {
        Credits::from(self.vendor_cost)
    }
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

    /// The five named parts added up, in credits.
    ///
    /// The line items are in the same unit as the total. On the fetch measured
    /// on 2026-09-15 they summed to the reported `total_cost` to the last digit
    /// the wire carried, so this is a cross-check on a bill rather than a
    /// second opinion about the unit. A vendor-served page is the one case
    /// where the two part company: the vendor's charge is in the total and in
    /// [`Costs::vendor`], not in any of the five parts, so this subtotal reads
    /// below [`Costs::total`] by that amount.
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
    fn a_vendor_served_page_names_the_vendor_and_the_total_includes_its_charge() {
        // The block the service writes on a vendor-served page: the five parts
        // plus a vendor entry, with the vendor's charge counted in the total
        // and in no part. The formatted strings ride along and are ignored.
        let costs: Costs = serde_json::from_str(
            r#"{"file_cost":0.0,"transform_cost":0.00001,"compute_cost":0.0002,
                "ai_cost":0.0,"bytes_transferred_cost":0.0,"total_cost":0.00321,
                "vendor":{"provider":"vendor","route":"vendor.unlocker",
                          "vendor_cost":0.002,"billed_cost":0.003,"byok":false,"attempts":2},
                "total_cost_formatted":"$0.00321"}"#,
        )
        .expect("costs");
        let vendor = costs.vendor.as_ref().expect("a vendor");
        assert_eq!(vendor.provider.as_deref(), Some("vendor"));
        assert_eq!(vendor.route, "vendor.unlocker");
        assert_eq!(vendor.charged_by_vendor(), Credits(20.0));
        assert_eq!(vendor.billed(), Credits(30.0));
        assert!(!vendor.byok);
        assert_eq!(vendor.attempts, 2);
        assert!((costs.total().get() - 32.1).abs() < 1e-9);
        assert!((costs.sum_of_parts().get() - 2.1).abs() < 1e-9);
        assert!(costs.total() > costs.sum_of_parts());
    }

    #[test]
    fn a_page_the_service_fetched_itself_names_no_vendor() {
        let costs: Costs =
            serde_json::from_str(r#"{"compute_cost":2.0,"total_cost":2.0}"#).expect("costs");
        assert!(costs.vendor.is_none());
        assert!(!serde_json::to_string(&costs)
            .expect("json")
            .contains("vendor"));
    }

    #[test]
    fn credits_add_up() {
        let total: Credits = [Credits(1.5), Credits(2.5)].into_iter().sum();
        assert_eq!(total, Credits(4.0));
    }
}
