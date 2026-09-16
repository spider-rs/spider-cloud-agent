//! Ask the router what it would send, without sending anything.
//!
//! The router runs locally and reads no clock, no file and no socket, so this
//! example needs no key and costs nothing. `explain` returns the rule that
//! decided, which is what makes a first attempt arguable after the fact.
//!
//! ```text
//! cargo run --example routing
//! ```

use spider_cloud_agent::{
    DeclaredNeed, HeuristicRouter, RouteInput, Router, SiteMemory, StatusClass,
};
use url::Url;

fn main() -> Result<(), url::ParseError> {
    let router = HeuristicRouter::new();

    // What a caller's own records would say about a site that keeps refusing.
    let refused = SiteMemory {
        observations: 6,
        success_rate: 0.1,
        streak: -3,
        last_status: StatusClass::Blocked,
    };

    let cases = [
        ("https://example.com/feed.xml", DeclaredNeed::Markdown, None),
        ("https://example.com/pricing", DeclaredNeed::Markdown, None),
        ("https://example.com/pricing", DeclaredNeed::Links, None),
        (
            "https://example.com/pricing",
            DeclaredNeed::Markdown,
            Some(&refused),
        ),
    ];

    for (address, need, memory) in cases {
        let url = Url::parse(address)?;
        let mut input = RouteInput::new(&url, need);
        if let Some(memory) = memory {
            input = input.with_memory(memory);
        }

        let (decision, rule) = router.explain(&input);
        println!("{address}  want {need:?}");
        println!(
            "  {:?} from {:?}, wait {}ms, ladder starts at {}",
            decision.mode(),
            decision.proxy(),
            decision.wait().millis(),
            decision.start_rung
        );
        println!(
            "  {rule:?}, {:.0}% sure, source {:?}\n",
            decision.confidence * 100.0,
            decision.source
        );
    }

    // A client takes any implementation of the same trait, and every name this
    // file uses comes from spider-cloud-agent, so a router of your own needs no
    // second crate in the manifest.
    println!("answered by {:?}", router.version());

    Ok(())
}
