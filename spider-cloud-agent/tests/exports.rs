//! What an adopter can name, using nothing but this crate.
//!
//! None of this compiled before the exports were fixed. The transport and the
//! route table were public inside a private module, and half the router's
//! vocabulary was reachable only by depending on spider-route directly. Method
//! calls kept working through inference, which is why the gap went unnoticed.
//! It shows only when a name has to be written down, in a signature, a field or
//! an impl.

// Integration tests are their own crate root, so the allow blocks inside the
// library's test modules do not reach here. A test that cannot set itself up
// should stop loudly rather than quietly measure nothing.
#![allow(clippy::expect_used)]

use spider_cloud_agent::client::route;
use spider_cloud_agent::{
    Action, CallerPins, DeclaredNeed, RequestMode, RouteDecision, RouteInput, RouteSource, Router,
    RouterVersion, SiteMemory, StatusClass, Transport, Wait,
};

/// The escape hatch is only an escape hatch if it can be passed around.
fn base_of(transport: &Transport) -> String {
    transport.base_url().to_string()
}

/// And held.
struct Holder<'a> {
    transport: &'a Transport,
}

#[test]
fn the_raw_transport_can_be_named() {
    let spider = spider_cloud_agent::Spider::with_key("not-a-key").expect("a client");
    let holder = Holder {
        transport: spider.raw(),
    };
    assert_eq!(base_of(holder.transport), base_of(spider.raw()));
}

#[test]
fn the_route_table_can_be_named() {
    assert_eq!(route::SCRAPE.path, "/scrape");
    assert!(spider_cloud_agent::client::ROUTES.contains(&route::SCRAPE));
}

/// A router an adopter could write, with no second crate in the manifest.
struct AlwaysBrowser;

impl Router for AlwaysBrowser {
    fn route(&self, input: &RouteInput<'_>) -> RouteDecision {
        let pins: &CallerPins<'_> = &input.pins;
        let need: DeclaredNeed = input.need;
        let confidence = match input.memory {
            Some(memory) if memory.last_status == StatusClass::Blocked => 0.9,
            Some(_) => 0.6,
            None => 0.5,
        };
        let action = Action::new(RequestMode::Browser)
            .with_wait(if need == DeclaredNeed::Links {
                Wait::Now
            } else {
                Wait::Settled { millis: 500 }
            })
            .with_proxy(pins.proxy.unwrap_or_default());
        RouteDecision::new(action, RouteSource::Heuristic, confidence)
    }

    fn version(&self) -> RouterVersion {
        RouterVersion::Heuristic
    }
}

#[test]
fn a_router_can_be_written_from_this_crate_alone() {
    let url = url::Url::parse("https://example.com").expect("a url");
    let refused = SiteMemory {
        observations: 4,
        success_rate: 0.1,
        streak: -2,
        last_status: StatusClass::Blocked,
    };
    let input = RouteInput::new(&url, DeclaredNeed::Markdown).with_memory(&refused);
    let decision = AlwaysBrowser.route(&input);
    assert_eq!(decision.source, RouteSource::Heuristic);
    assert_eq!(decision.mode(), RequestMode::Browser);
    assert_eq!(decision.wait().millis(), 500);
}
