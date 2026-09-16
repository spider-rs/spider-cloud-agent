//! Walk the escalation ladder under a budget, and read the trail afterwards.
//!
//! The first attempt uses what the router picked. When the site refuses it,
//! the policy engine raises one documented request parameter at a time and
//! sends again, until the page arrives or the budget stops it. Either way the
//! trail says what was tried and what it cost.
//!
//! ```text
//! cargo run --example escalation -- https://example.com
//! ```

use std::time::Duration;

use spider_cloud_agent::{Budget, Error, Need, Spider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spider = match Spider::new() {
        Ok(spider) => spider,
        Err(reason) => {
            eprintln!("{reason}");
            return Ok(());
        }
    };

    let target = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_string());

    // Three calls or ninety seconds, whichever runs out first. Budget also
    // caps credits, and a credit cap is copied onto the request so the service
    // stops where this client does.
    let budget = Budget::default()
        .with_attempts(3)
        .with_wall(Duration::from_secs(90));

    match spider
        .scrape(&target)
        .need(Need::Markdown)
        .budget(budget)
        .send()
        .await
    {
        Ok(page) => {
            println!("{} after {} call(s)", page.status, page.attempt_count());
            for (n, attempt) in page.attempts.iter().enumerate() {
                println!("  {n}: {} in {:?}", attempt.api, attempt.elapsed);
            }
        }
        Err(Error::Exhausted {
            attempts,
            last,
            reason,
            ..
        }) => {
            println!("no page after {} call(s): {reason}", attempts.len());
            for (n, attempt) in attempts.iter().enumerate() {
                match attempt.page {
                    Some(status) => println!("  {n}: {status}, {} credits", attempt.cost),
                    // Nothing reached the site, so there is only the call plane.
                    None => println!("  {n}: {}", attempt.api),
                }
            }
            if let Some(failed) = last {
                println!(
                    "the site said {}, next change: {:?}",
                    failed.status, failed.hint
                );
            }
        }
        Err(Error::BudgetExceeded { kind, attempts }) => {
            println!(
                "stopped by the {kind} cap after {} attempts",
                attempts.len()
            )
        }
        Err(Error::InsufficientCredits) => println!("the account is empty, do not retry"),
        Err(other) => return Err(other.into()),
    }

    Ok(())
}
