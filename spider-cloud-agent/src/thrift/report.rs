//! What the two tiers actually saved, and a whole crawl in one string.

use crate::response::Pages;
use crate::thrift::tokens::{max_tokens, TokenBudget};
use crate::thrift::trim::{TrimReason, TrimSettings, Trimmer};

/// What a request cost in bytes and in tokens, before and after.
///
/// `wire_bytes` is what arrived, so it already carries the first tier's saving:
/// a request that asked for markdown never received the markup. The second
/// tier is the gap between `wire_bytes` and `returned_bytes`.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ThriftReport {
    /// Bytes the service sent, body only.
    pub wire_bytes: usize,
    /// Bytes handed to the caller once the trimming was done.
    pub returned_bytes: usize,
    /// Roughly what the response would have cost a model as it arrived.
    pub approx_tokens_in: usize,
    /// Roughly what it costs after trimming.
    pub approx_tokens_out: usize,
    /// Every cut that removed something.
    pub trimmed: Vec<TrimReason>,
}

impl ThriftReport {
    /// A report for a response that was never trimmed.
    pub fn untrimmed(wire_bytes: usize, tokens: usize) -> ThriftReport {
        ThriftReport {
            wire_bytes,
            returned_bytes: wire_bytes,
            approx_tokens_in: tokens,
            approx_tokens_out: tokens,
            trimmed: Vec::new(),
        }
    }

    /// How many bytes the trimming removed.
    pub fn saved_bytes(&self) -> usize {
        self.wire_bytes.saturating_sub(self.returned_bytes)
    }

    /// How many tokens the trimming removed.
    pub fn saved_tokens(&self) -> usize {
        self.approx_tokens_in.saturating_sub(self.approx_tokens_out)
    }

    /// The share of the arriving bytes that reached the caller, from zero to
    /// one. A response with nothing in it reads as one, having lost nothing.
    pub fn kept_share(&self) -> f64 {
        if self.wire_bytes == 0 {
            return 1.0;
        }
        self.returned_bytes as f64 / self.wire_bytes as f64
    }

    /// The share of the arriving tokens that reached the caller.
    pub fn kept_token_share(&self) -> f64 {
        if self.approx_tokens_in == 0 {
            return 1.0;
        }
        self.approx_tokens_out as f64 / self.approx_tokens_in as f64
    }

    /// Whether anything was cut after the response arrived.
    pub fn trimmed_anything(&self) -> bool {
        !self.trimmed.is_empty()
    }
}

/// How a page is introduced in a digest.
fn heading(url: &str) -> String {
    format!("## {url}\n")
}

/// What stands under the heading of a page that came back with nothing in it.
const NO_CONTENT: &str = "(no content)";

/// What stands under the heading of a page the site refused.
fn not_served(status: &crate::status::PageStatus) -> String {
    format!("(not served: {status})")
}

impl Pages {
    /// The whole set as one string, sized to fit a context window.
    ///
    /// Boilerplate is learned across the set before anything is cut, so the
    /// navigation and footer a crawl repeats on every page are paid for no
    /// times rather than once a page. What is left is shared out in proportion
    /// to what each page costs, so a long page gets more of the budget than a
    /// short one and a short one keeps all of itself.
    ///
    /// The total covers the whole string, headings and all, as
    /// [`max_tokens`] measures it. Every heading is
    /// always written, so a total too small to hold them comes back as the
    /// headings alone.
    ///
    /// Pages the site refused contribute a line saying so, because which pages
    /// failed is usually part of the answer.
    ///
    /// ```
    /// # use spider_cloud_agent::response::Pages;
    /// # use spider_cloud_agent::TokenBudget;
    /// # fn show(pages: Pages) {
    /// let context = pages.digest(TokenBudget::total(4000));
    /// # let _ = context;
    /// # }
    /// ```
    pub fn digest(&self, budget: TokenBudget) -> String {
        let bodies: Vec<String> = self
            .ok()
            .map(|page| page.text().unwrap_or_default().to_string())
            .collect();

        let mut trimmer = Trimmer::new(TrimSettings::default());
        trimmer.learn(&bodies);

        let trimmed: Vec<String> = bodies
            .iter()
            .map(|body| trimmer.trim(body, None).text)
            .collect();

        // The frame is everything that is not a page body: the headings, the
        // lines under the pages that were refused or came back empty, and the
        // blank line between blocks. It is costed apart from the bodies and
        // taken off the total first, because the total is a promise about the
        // whole string. That is exact rather than approximate: every join in
        // the digest has a newline on one side of it and a non-space on the
        // other, and the ceiling adds across such a join to the digit.
        let mut frame = 0usize;
        for (page, first_pass) in self.ok().zip(&trimmed) {
            frame += max_tokens(&heading(page.url.as_str()));
            if first_pass.trim().is_empty() {
                frame += max_tokens(NO_CONTENT);
            }
        }
        for failed in self.failed() {
            frame += max_tokens(&heading(failed.url.as_str()));
            frame += max_tokens(&not_served(&failed.status));
        }
        // The last block has no blank line after it.
        if !self.is_empty() {
            frame = frame.saturating_sub(1);
        }

        // Costed in the ceiling, because the ceiling is what each share is
        // enforced in. Costed in the estimate, a page that fit the total was
        // still cut, to the estimate's reading of itself, which the ceiling
        // reads three to four times higher.
        let costs: Vec<usize> = trimmed.iter().map(|text| max_tokens(text)).collect();
        let for_bodies = TokenBudget {
            per_page: budget.per_page,
            total: budget.total.map(|total| total.saturating_sub(frame)),
        };
        let shares = for_bodies.split(&costs);

        let mut out = String::new();
        for ((page, first_pass), ceiling) in self.ok().zip(&trimmed).zip(shares) {
            out.push_str(&heading(page.url.as_str()));
            if first_pass.trim().is_empty() {
                out.push_str(NO_CONTENT);
                out.push('\n');
            } else {
                // A share is a ceiling whenever anything is capped. With no
                // total the shares carry the page cap, which used to be worked
                // out and then thrown away. A page whose share holds nothing
                // keeps its heading and nothing under it.
                let ceiling = (!budget.is_unlimited()).then_some(ceiling);
                let body = trimmer.trim(first_pass, ceiling).text;
                let body = body.trim_end();
                if !body.is_empty() {
                    out.push_str(body);
                    out.push('\n');
                }
            }
            out.push('\n');
        }

        for failed in self.failed() {
            out.push_str(&heading(failed.url.as_str()));
            out.push_str(&not_served(&failed.status));
            out.push_str("\n\n");
        }

        out.trim_end().to_string()
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
    use crate::response::page::PageParts;
    use crate::response::{Body, PageResult};
    use crate::thrift::tokens::approx_tokens;

    fn page(url: &str, code: u16, text: &str) -> PageResult {
        let mut parts = PageParts::for_test(url, code);
        parts.body = Body::Text(text.to_string());
        PageResult::from_parts(parts)
    }

    fn shop(url: &str, body: &str) -> PageResult {
        page(
            url,
            200,
            &format!(
                "Example Shop\nHome\nProducts\nAbout\n\n{body}\n\nTerms of sale\nPrivacy notice\n"
            ),
        )
    }

    #[test]
    fn a_report_reads_both_savings() {
        let report = ThriftReport {
            wire_bytes: 1000,
            returned_bytes: 250,
            approx_tokens_in: 400,
            approx_tokens_out: 100,
            trimmed: vec![TrimReason::Whitespace { bytes: 750 }],
        };
        assert_eq!(report.saved_bytes(), 750);
        assert_eq!(report.saved_tokens(), 300);
        assert_eq!(report.kept_share(), 0.25);
        assert_eq!(report.kept_token_share(), 0.25);
        assert!(report.trimmed_anything());
    }

    #[test]
    fn an_untrimmed_report_lost_nothing() {
        let report = ThriftReport::untrimmed(120, 30);
        assert_eq!(report.saved_bytes(), 0);
        assert_eq!(report.kept_share(), 1.0);
        assert!(!report.trimmed_anything());
        assert_eq!(ThriftReport::default().kept_share(), 1.0);
        assert_eq!(ThriftReport::default().kept_token_share(), 1.0);
    }

    #[test]
    fn a_digest_names_every_page_and_drops_what_they_share() {
        let pages = Pages(vec![
            shop("https://example.com/a", "The blue widget is steel."),
            shop("https://example.com/b", "The red widget is brass."),
            shop("https://example.com/c", "The green widget is wood."),
            shop("https://example.com/d", "The black widget is stone."),
        ]);
        let digest = pages.digest(TokenBudget::unlimited());

        for url in ["/a", "/b", "/c", "/d"] {
            assert!(digest.contains(url), "{url} is missing from the digest");
        }
        assert!(digest.contains("blue widget"));
        assert!(!digest.contains("Privacy notice"), "{digest}");
        assert!(!digest.contains("Products"), "{digest}");
    }

    #[test]
    fn a_digest_fits_the_budget_it_was_given() {
        let long = (0..40)
            .map(|n| format!("Sentence number {n} about the widget and what it is made of."))
            .collect::<Vec<_>>()
            .join(" ");
        let pages = Pages(vec![
            page("https://example.com/a", 200, &long),
            page("https://example.com/b", 200, &long),
        ]);
        let digest = pages.digest(TokenBudget::total(200));
        assert!(
            approx_tokens(&digest) <= 260,
            "{} tokens",
            approx_tokens(&digest)
        );
        assert!(digest.contains("truncated"));
    }

    #[test]
    fn a_digest_with_only_a_page_cap_still_caps_each_page() {
        // No total, only a ceiling on each page. The shares were worked out
        // correctly and then thrown away, so the cap did nothing.
        let long = (0..40)
            .map(|n| format!("Sentence number {n} about the widget and what it is made of."))
            .collect::<Vec<_>>()
            .join(" ");
        let pages = Pages(vec![
            page("https://example.com/a", 200, &long),
            page("https://example.com/b", 200, &long),
        ]);
        let digest = pages.digest(TokenBudget::per_page(60));

        for body in digest.split("## https://example.com/").skip(1) {
            let body = body.trim_start_matches(|c: char| c != '\n');
            assert!(
                max_tokens(body) <= 60,
                "a page body cost {} against a cap of 60",
                max_tokens(body)
            );
        }
        assert!(digest.contains("truncated"), "{digest}");
    }

    #[test]
    fn a_digest_whose_pages_fit_the_total_cuts_none_of_them() {
        // The doc promise: a set that already fits inside the total is left
        // alone. The shares were measured in the estimate and enforced in the
        // ceiling, which reads three to four times higher, so every page was
        // cut to a third of itself for the sake of it.
        let short = "The blue widget is steel. It ships the same day.";
        let pages = Pages(vec![
            page("https://example.com/a", 200, short),
            page("https://example.com/b", 200, short),
        ]);
        let digest = pages.digest(TokenBudget::total(10_000));

        assert!(!digest.contains("truncated"), "{digest}");
        assert_eq!(digest.matches("ships the same day").count(), 2, "{digest}");
    }

    #[test]
    fn a_digest_of_many_pages_stays_inside_its_total_as_the_ceiling_measures_it() {
        let long = (0..60)
            .map(|n| format!("第{n}页。蜘蛛云是一个网页抓取服务。Sentence {n} about the widget."))
            .collect::<Vec<_>>()
            .join(" ");
        let pages = Pages(
            (0..5)
                .map(|n| page(&format!("https://example.com/{n}"), 200, &long))
                .collect(),
        );
        let digest = pages.digest(TokenBudget::total(500));

        assert!(
            max_tokens(&digest) <= 500,
            "the digest cost {} against a total of 500",
            max_tokens(&digest)
        );
    }

    #[test]
    fn a_digest_fits_its_total_headings_and_all_on_generated_sets() {
        // A small deterministic generator, so a failing set can be named.
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let words = [
            "widget",
            "steel.",
            "The",
            "ships",
            "同日发货。",
            "🚀",
            "£19.99",
            "\n\n",
            "  ",
            "- https://example.com/only",
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg",
        ];

        for case in 0..300 {
            let count = 1 + (next() % 6) as usize;
            let mut results = Vec::with_capacity(count);
            for n in 0..count {
                let url = format!("https://example.com/p/{n}?q={}", next() % 1000);
                let code = match next() % 5 {
                    0 => 403,
                    1 => 503,
                    _ => 200,
                };
                let length = (next() % 60) as usize;
                let body: String = (0..length)
                    .map(|_| words[(next() % words.len() as u64) as usize])
                    .collect::<Vec<_>>()
                    .join(" ");
                results.push(page(&url, code, &body));
            }
            let pages = Pages(results);

            // With no room for any body the digest is the frame alone, so its
            // cost is what any total has to cover before a body gets a token.
            let frame = max_tokens(&pages.digest(TokenBudget::total(0)));
            for extra in [0, 1, 2, 5, 17, 60, 400] {
                let total = frame + extra;
                let digest = pages.digest(TokenBudget::total(total));
                assert!(
                    max_tokens(&digest) <= total,
                    "case {case}: a total of {total} came back costing {}:\n{digest}",
                    max_tokens(&digest)
                );
                for result in &pages.0 {
                    assert!(
                        digest.contains(result.url().as_str()),
                        "case {case}: {} is missing from the digest",
                        result.url()
                    );
                }
            }
        }
    }

    #[test]
    fn a_refused_page_is_named_rather_than_dropped() {
        let pages = Pages(vec![
            page("https://example.com/a", 200, "Served."),
            page("https://example.com/b", 403, ""),
        ]);
        let digest = pages.digest(TokenBudget::unlimited());
        assert!(digest.contains("https://example.com/b"));
        assert!(digest.contains("not served"));
    }

    #[test]
    fn a_served_page_with_nothing_in_it_says_so() {
        let pages = Pages(vec![page("https://example.com/a", 200, "")]);
        let digest = pages.digest(TokenBudget::unlimited());
        assert!(digest.contains("no content"));
    }
}
