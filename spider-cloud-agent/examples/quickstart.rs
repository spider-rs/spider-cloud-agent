//! Fetch one page and print it.
//!
//! The key comes from `SPIDER_API_KEY`, `SPIDER_CLOUD_API_KEY` or
//! `~/.spider/credentials`, in that order. With none of them set this prints
//! where it looked and stops.
//!
//! ```text
//! cargo run --example quickstart
//! ```

use spider_cloud_agent::{Need, Spider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spider = match Spider::new() {
        Ok(spider) => spider,
        Err(reason) => {
            eprintln!("{reason}");
            return Ok(());
        }
    };

    // Need::Markdown asks the service for markdown and switches off the
    // metadata, headers, cookies and links that would otherwise come with it.
    let page = spider
        .scrape("https://example.com")
        .need(Need::Markdown)
        .send()
        .await?;

    println!("{}", page.text().unwrap_or_default());
    println!(
        "\n{} in {} call(s), {} bytes off the wire",
        page.cost,
        page.attempt_count(),
        page.thrift.wire_bytes
    );

    Ok(())
}
