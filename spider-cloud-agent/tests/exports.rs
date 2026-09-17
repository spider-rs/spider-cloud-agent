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

/// The crate's public modules and root re-exports, read off `lib.rs`, with
/// whether each item sits behind the `optimize` feature.
fn documented_exports() -> Vec<(String, bool)> {
    let source = include_str!("../src/lib.rs");
    let mut out = Vec::new();
    let mut gated = false;
    let mut pending_use = String::new();
    for line in source.lines().map(str::trim) {
        if line.starts_with("#[cfg(feature = \"optimize\")]") {
            gated = true;
            continue;
        }
        if line.starts_with("#[") {
            continue;
        }
        if let Some(name) = line
            .strip_prefix("pub mod ")
            .and_then(|rest| rest.strip_suffix(';'))
        {
            out.push((format!("mod {name}"), gated));
        } else if line.starts_with("pub use ") || !pending_use.is_empty() {
            pending_use.push_str(line);
            pending_use.push(' ');
            if line.ends_with(';') {
                let paths = pending_use.trim_start_matches("pub use ");
                // `a::{b, c}` names b and c, and `a::b` names b.
                let paths = paths.split_once('{').map_or(paths, |(_, inner)| inner);
                let names = paths
                    .split(['}', ',', ';'])
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(|part| part.rsplit("::").next().unwrap_or(part).to_string());
                out.extend(names.map(|name| (name, gated)));
                pending_use.clear();
            }
        }
        if !line.is_empty() {
            gated = false;
        }
    }
    out
}

/// What 0.6.0 exports from the crate root, in `lib.rs` order.
const EXPORTS: &[&str] = &[
    "mod auth",
    "mod client",
    "mod credits",
    "mod error",
    "mod memory",
    "mod ops",
    "mod params",
    "mod policy",
    "mod record",
    "mod response",
    "mod routing",
    "mod status",
    "mod thrift",
    "RunBudget",
    "RunSpend",
    "Spider",
    "SpiderBuilder",
    "Transport",
    "Credits",
    "Usd",
    "WholeCredits",
    "CREDITS_PER_USD",
    "AuthCause",
    "BudgetKind",
    "Error",
    "Recovery",
    "SiteMemoryStore",
    "Country",
    "ProxyPool",
    "RequestMode",
    "RequestParams",
    "ReturnFormat",
    "SearchParams",
    "WaitFor",
    "Budget",
    "Ladder",
    "Policy",
    "Rung",
    "Step",
    "JsonlRecorder",
    "Recorder",
    "Attempt",
    "Body",
    "FailedPage",
    "Outcome",
    "Page",
    "PageResult",
    "Pages",
    "Explorer",
    "featurize",
    "Action",
    "AttemptOutcome",
    "CallerPins",
    "DeclaredNeed",
    "HeuristicRouter",
    "RouteDecision",
    "RouteInput",
    "RouteSource",
    "Router",
    "RouterVersion",
    "SiteMemory",
    "StatusClass",
    "Wait",
    "ApiClass",
    "ApiStatus",
    "PageClass",
    "PageStatus",
    "Need",
    "ThriftReport",
    "TokenBudget",
];

#[cfg(not(feature = "optimize"))]
#[test]
fn without_the_optimizer_the_export_list_is_unchanged() {
    let exports = documented_exports();
    let ungated: Vec<&str> = exports
        .iter()
        .filter(|(_, gated)| !gated)
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(ungated, EXPORTS);
    let gated: Vec<&str> = exports
        .iter()
        .filter(|(_, gated)| *gated)
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        gated,
        ["mod optimize"],
        "only the module is behind the feature"
    );

    // And the client prints nothing about an optimizer it cannot hold.
    let spider = spider_cloud_agent::Spider::with_key("not-a-key").expect("a client");
    assert!(!format!("{spider:?}").contains("optimize"));
}

#[cfg(feature = "optimize")]
#[test]
fn with_the_optimizer_the_new_names_can_be_written_down() {
    use spider_cloud_agent::optimize::{
        summarize, ApplyMode, Choice, Clock, ComparisonRecorder, DecisionLog, EditSet, Gate,
        JsonlComparisonRecorder, NoModel, Optimizer, Reason, ResourceSource, ResourceSummary,
        Schema, Scorer,
    };

    let exports = documented_exports();
    let names: Vec<&str> = exports.iter().map(|(name, _)| name.as_str()).collect();
    let mut expected: Vec<&str> = EXPORTS.to_vec();
    let at = expected
        .iter()
        .position(|name| *name == "mod ops")
        .expect("ops")
        + 1;
    expected.insert(at, "mod optimize");
    assert_eq!(names, expected);

    struct Day;
    impl Clock for Day {
        fn day(&self) -> u32 {
            20_000
        }
    }
    struct Nothing;
    impl ResourceSource for Nothing {
        fn resources(&self, _: &url::Url) -> Option<ResourceSummary> {
            None
        }
    }
    let scorer: &dyn Scorer = &NoModel;
    assert!(scorer.version().is_none());
    let optimizer: Optimizer = Optimizer::new(NoModel, Gate::default(), ApplyMode::Apply)
        .with_clock(Day)
        .with_resources(Nothing);
    assert_eq!(optimizer.mode(), ApplyMode::Apply);
    assert_eq!(Optimizer::shadow(NoModel).mode(), ApplyMode::Shadow);
    let log = DecisionLog {
        choice: Choice::Keep(Reason::NoModel),
        candidates: 1,
        applied: false,
    };
    assert!(!log.applied && EditSet::keep().is_keep());
    let _: Schema = Schema::v1();
    let page = url::Url::parse("https://example.com").expect("a url");
    assert_eq!(summarize(&page, &[]), ResourceSummary::default());

    let recorder = JsonlComparisonRecorder::new(std::io::sink()).with_salt(3);
    let recorder: &dyn ComparisonRecorder = &recorder;
    assert_eq!(recorder.salt(), 3);

    let spider = spider_cloud_agent::Spider::builder()
        .key("not-a-key")
        .optimizer(optimizer)
        .build()
        .expect("a client");
    assert!(spider.optimizer().is_some() && spider.comparison_recorder().is_none());
    let printed = format!("{spider:?}");
    assert!(printed.contains("optimizer: \"set\""), "{printed}");
    assert!(printed.contains("comparison: \"none\""), "{printed}");
}
