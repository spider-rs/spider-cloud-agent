//! What the two tiers actually saved, and a whole crawl in one string.

use crate::response::Pages;
use crate::thrift::tokens::{approx_tokens, TokenBudget};
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

impl Pages {
    /// The whole set as one string, sized to fit a context window.
    ///
    /// Boilerplate is learned across the set before anything is cut, so the
    /// navigation and footer a crawl repeats on every page are paid for no
    /// times rather than once a page. What is left is shared out in proportion
    /// to what each page costs, so a long page gets more of the budget than a
    /// short one and a short one keeps all of itself.
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
        let costs: Vec<usize> = trimmed.iter().map(|text| approx_tokens(text)).collect();
        let shares = budget.split(&costs);

        let mut out = String::new();
        for ((page, first_pass), ceiling) in self.ok().zip(&trimmed).zip(shares) {
            let body = trimmer.trim(first_pass, budget.total.map(|_| ceiling)).text;
            out.push_str(&heading(page.url.as_str()));
            if body.trim().is_empty() {
                out.push_str("(no content)\n");
            } else {
                out.push_str(body.trim_end());
                out.push('\n');
            }
            out.push('\n');
        }

        for failed in self.failed() {
            out.push_str(&heading(failed.url.as_str()));
            out.push_str(&format!("(not served: {})\n\n", failed.status));
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
