//! Ask for three values and leave the page body on the server.
//!
//! `Need::fields` sets `return_format=empty` and a css extraction map, so the
//! response carries the named values and none of the page. This fetches the
//! same address twice, once as the service would answer on its own and once
//! with the need stated, and prints what each one weighed.
//!
//! The saving is in what crosses the wire and what a model then reads. Both
//! calls still fetch the page, so both are billed about the same.
//!
//! ```text
//! cargo run --example fields_only
//! ```

use spider_cloud_agent::response::Body;
use spider_cloud_agent::{Need, Spider};

const TARGET: &str = "https://spider.cloud";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spider = match Spider::new() {
        Ok(spider) => spider,
        Err(reason) => {
            eprintln!("{reason}");
            return Ok(());
        }
    };

    // Need::Raw states no need, so nothing is switched off and the service
    // answers the way it would for any other client.
    let whole = spider.scrape(TARGET).need(Need::Raw).send().await?;

    // A selector starting with a slash is read as XPath, anything else as CSS.
    let fields = spider
        .scrape(TARGET)
        .need(Need::fields([
            ("title", "h1"),
            ("sections", "h2"),
            ("subsections", "h3"),
        ]))
        .send()
        .await?;

    match &fields.body {
        Body::Fields(values) => {
            for (name, value) in values {
                println!("{name}: {value}");
            }
        }
        // A site that served nothing still answers, and the body says so.
        other => println!("no fields came back: {other:?}"),
    }

    println!(
        "\nwhole page:  {} bytes, about {} tokens",
        whole.thrift.wire_bytes, whole.thrift.approx_tokens_out
    );
    println!(
        "fields only: {} bytes, about {} tokens",
        fields.thrift.wire_bytes, fields.thrift.approx_tokens_out
    );

    Ok(())
}
