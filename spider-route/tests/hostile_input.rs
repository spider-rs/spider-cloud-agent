//! Every shape of input a caller could hand this crate, run through the whole
//! public surface.
//!
//! The router runs on every request inside someone else's process, so the
//! rules it is held to are simple: any url that parses must route without a
//! panic, in time that grows no faster than the url does, and to the same
//! answer every time. The cases below are the ones that tend to break
//! parsers and bucket arithmetic, plus a seeded random loop over a few
//! hundred thousand urls for the shapes nobody thought to list.

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

use spider_route::features::extension_of;
use spider_route::{
    featurize, host_shape, registrable_domain, Action, CallerPins, Country, DeclaredNeed, ExtClass,
    FeatureVector, HeuristicRouter, HostShape, ProxyPool, RequestMode, RouteDecision, RouteInput,
    RouteSource, Router, Rule, SiteMemory, StatusClass, Wait, FEATURES_USED, FEATURE_DIM,
};
use std::time::{Duration, Instant};
use url::Url;

/// Everything the public surface says about one request, so two runs can be
/// compared for determinism.
#[derive(Debug, PartialEq)]
struct Answer {
    vector: FeatureVector,
    decision: RouteDecision,
    rule: Rule,
    shape: HostShape,
    ext: ExtClass,
    registrable: Option<String>,
}

const NEEDS: [DeclaredNeed; 8] = [
    DeclaredNeed::Text,
    DeclaredNeed::Markdown,
    DeclaredNeed::Html,
    DeclaredNeed::Links,
    DeclaredNeed::Metadata,
    DeclaredNeed::Fields,
    DeclaredNeed::Screenshot,
    DeclaredNeed::Raw,
];

const STATUSES: [StatusClass; 9] = [
    StatusClass::Unknown,
    StatusClass::Ok,
    StatusClass::Empty,
    StatusClass::BadRequest,
    StatusClass::NeedsLogin,
    StatusClass::Blocked,
    StatusClass::NotFound,
    StatusClass::RateLimited,
    StatusClass::ServerError,
];

/// Memories at the edges of every field, including the arithmetic a caller
/// could get wrong.
fn hostile_memories() -> Vec<SiteMemory> {
    let mut out = vec![SiteMemory::cold()];

    for rate in [
        0.0,
        1.0,
        0.5,
        -1.0,
        2.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MIN_POSITIVE,
        -0.0,
    ] {
        for (observations, streak) in [
            (0, 0),
            (1, -1),
            (u32::MAX, i16::MIN),
            (u32::MAX, i16::MAX),
            (3, -3),
            (50, 12),
        ] {
            for status in STATUSES {
                out.push(SiteMemory {
                    observations,
                    success_rate: rate,
                    streak,
                    last_status: status,
                });
            }
        }
    }

    out
}

fn answer(input: &RouteInput<'_>) -> Answer {
    let (decision, rule) = HeuristicRouter::new().explain(input);
    let vector = featurize(input);

    // Nothing in the vector may be outside what a weight table can multiply.
    for (slot, value) in vector.as_slice().iter().enumerate() {
        assert!(
            value.is_finite(),
            "slot {slot} is {value} for {}",
            input.url
        );
        assert!(
            (-1.0..=1.0).contains(value),
            "slot {slot} is {value} for {}",
            input.url
        );
    }

    for slot in FEATURES_USED..FEATURE_DIM {
        assert_eq!(vector.get(slot), Some(0.0), "slot {slot} for {}", input.url);
    }

    assert!(
        decision.confidence.is_finite() && (0.0..=1.0).contains(&decision.confidence),
        "confidence {} for {}",
        decision.confidence,
        input.url
    );
    assert_eq!(
        HeuristicRouter::new().route(input),
        decision,
        "route and explain disagree for {}",
        input.url
    );

    Answer {
        vector,
        decision,
        rule,
        shape: host_shape(input.url),
        ext: extension_of(input.url),
        registrable: registrable_domain(input.url).map(str::to_owned),
    }
}

/// The memories the rules branch on, one per rule plus the edges.
fn branching_memories() -> Vec<SiteMemory> {
    let mut out = vec![
        SiteMemory::cold(),
        SiteMemory {
            observations: u32::MAX,
            success_rate: f32::NAN,
            streak: i16::MIN,
            last_status: StatusClass::Blocked,
        },
        SiteMemory {
            observations: u32::MAX,
            success_rate: f32::INFINITY,
            streak: i16::MAX,
            last_status: StatusClass::Ok,
        },
        SiteMemory {
            observations: 40,
            success_rate: 0.95,
            streak: 12,
            last_status: StatusClass::Ok,
        },
        SiteMemory {
            observations: 1,
            success_rate: 0.0,
            streak: -1,
            last_status: StatusClass::Empty,
        },
        SiteMemory {
            observations: 9,
            success_rate: 0.0,
            streak: -4,
            last_status: StatusClass::ServerError,
        },
        SiteMemory {
            observations: 20,
            success_rate: 0.2,
            streak: 1,
            last_status: StatusClass::Ok,
        },
    ];

    for status in STATUSES {
        out.push(SiteMemory {
            observations: 6,
            success_rate: 0.0,
            streak: -2,
            last_status: status,
        });
    }

    out
}

/// Run one url through every need, hour, pin and memory that matters.
fn exercise(url: &Url, memories: &[SiteMemory]) -> Vec<Answer> {
    let country = Country::new("de").unwrap();
    let pins = [
        CallerPins::none(),
        CallerPins {
            mode: Some(RequestMode::Browser),
            proxy: Some(ProxyPool::Residential),
            country: Some(&country),
        },
    ];
    let mut out = Vec::new();

    for need in NEEDS {
        for hour in [None, Some(0), Some(23), Some(255)] {
            for pin in pins {
                for memory in memories {
                    let mut input = RouteInput::new(url, need)
                        .with_pins(pin)
                        .with_memory(memory);

                    if let Some(hour) = hour {
                        input = input.at_hour(hour);
                    }

                    out.push(answer(&input));
                }
            }
        }
    }

    out
}

/// The hand picked list. Anything here that `Url` refuses is fine: the point
/// is that whatever it accepts, this crate accepts too.
fn hostile_urls() -> Vec<String> {
    let mut urls: Vec<String> = [
        "",
        " ",
        "example.com",
        "/path",
        "//example.com/path",
        "http://",
        "http:///",
        "http:",
        "http://example.com",
        "http://example.com.",
        "http://example.com../",
        "http://.example.com/",
        "http://a..b/",
        "http://./",
        "http://../",
        "http://-/",
        "http://_/",
        "http://%20/",
        "http://%ff/",
        "http://[::1]/",
        "http://[::]/",
        "http://[::1%25eth0]/",
        "http://[fe80::1%eth0]/",
        "http://[2001:db8:0:0:0:0:0:1]/",
        "http://[::ffff:192.0.2.1]/",
        "http://[v1.fe80::a]/",
        "http://[::1]:0/",
        "http://[::1]:65535/",
        "http://0.0.0.0/",
        "http://255.255.255.255:65535/",
        "http://192.0.2.1:0/",
        "http://192.0.2.1:65536/",
        "http://0x7f.1/",
        "http://2130706433/",
        "http://127.1/",
        "http://192.168.1/",
        "http://127.0.0.1.example.com/",
        "http://example.com:0/",
        "http://example.com:65535/",
        "http://example.com:65536/",
        "http://example.com:/",
        "http://example.com:abc/",
        "http://xn--bcher-kva.example/",
        "https://xn--e1afmkfd.xn--p1ai/",
        "http://xn--/",
        "http://xn--a/",
        "http://b\u{fc}cher.example/",
        "http://\u{4f60}\u{597d}.\u{4e2d}\u{56fd}/",
        "http://\u{4f8b}\u{3048}.\u{30c6}\u{30b9}\u{30c8}/",
        "http://\u{1f600}.com/",
        "http://user:pass@example.com/",
        "http://user:@example.com/",
        "http://:pass@example.com/",
        "http://@example.com/",
        "http://user@[::1]/",
        "http://localhost/",
        "http://localhost:8080/",
        "http://uk/",
        "http://co.uk/",
        "http://www.co.uk/",
        "http://www/",
        "http://www.www.www.www/",
        "http://a.b.c.d.e.f.g.h.i.j.k.example.co.uk/",
        "http://example.com/%FF%FE/%C0%AF/",
        "http://example.com/%",
        "http://example.com/%%%%",
        "http://example.com/%2F%2F%2F",
        "http://example.com/../../..",
        "http://example.com/a/.",
        "http://example.com/a/..",
        "http://example.com/.",
        "http://example.com/..",
        "http://example.com/...",
        "http://example.com/....",
        "http://example.com/.html",
        "http://example.com/..html",
        "http://example.com/a.",
        "http://example.com/a..",
        "http://example.com/.git",
        "http://example.com/a.HTML",
        "http://example.com/a.tar.gz",
        "http://example.com/1.2.3",
        "http://example.com/a.12345",
        "http://example.com/a.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "http://example.com/a.\u{e9}",
        "http://example.com/f47ac10b58cc4372a5670e02b2c3d479",
        "http://example.com/f47ac10b-58cc-4372-a567-0e02b2c3d479",
        "http://example.com/---",
        "http://example.com/___",
        "http://example.com/-_-",
        "http://example.com/a b",
        "http://example.com/\t\n",
        "http://example.com/\u{0}",
        "http://example.com/\u{7f}",
        "http://example.com/\u{65e5}\u{672c}\u{8a9e}",
        "http://example.com/\u{fc}n\u{ef}c\u{f6}d\u{e9}?\u{43a}\u{43b}\u{44e}\u{447}=\u{437}",
        "http://example.com/?",
        "http://example.com/?&&&&&",
        "http://example.com/?=",
        "http://example.com/?==",
        "http://example.com/?=a",
        "http://example.com/?a",
        "http://example.com/?a=",
        "http://example.com/?a=b=c",
        "http://example.com/?%FF=%FF",
        "http://example.com/?%=%",
        "http://example.com/?utm_",
        "http://example.com/?utm_\u{e9}=1",
        "http://example.com/?UTM_SOURCE=x&GCLID=y",
        "http://example.com/?\u{e9}=1",
        "http://example.com/?1=1",
        "http://example.com/?0",
        "http://example.com/?q=\u{65e5}\u{672c}\u{8a9e}",
        "http://example.com/?q=a&q=b&q=c&q=d&q=e&q=f&q=g",
        "http://example.com/#",
        "http://example.com/#frag",
        "http://example.com/##",
        "http://example.com/?#",
        "http://example.com/#?a=b",
        "data:text/html,<h1>hi</h1>",
        "data:,",
        "data:",
        "javascript:alert(1)",
        "javascript:",
        "mailto:user@example.com",
        "mailto:",
        "tel:+15555555555",
        "about:blank",
        "blob:https://example.com/uuid",
        "file:///etc/passwd",
        "file:///",
        "file://host/share",
        "ftp://example.com/x",
        "ws://example.com/x",
        "ssh://example.com/x",
        "urn:isbn:0451450523",
        "HTTP://EXAMPLE.COM/A/B",
        "http://ExAmPlE.CoM:8080/",
        "http://example.com:80/",
        "https://example.com:443/",
        "https://example.com:80/",
        "http://example.com/\u{fffd}",
        "http://example.com/\u{feff}",
        "http://example.com/\u{200b}",
        "http://example.com/\u{202e}",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();

    // A host with ten thousand labels.
    urls.push(format!("http://{}com/", "a.".repeat(10_000)));
    // A host that is one label a hundred thousand bytes long.
    urls.push(format!("http://{}.com/", "a".repeat(100_000)));
    // A path a megabyte long, in one segment and in ten thousand.
    urls.push(format!("http://example.com/{}", "a".repeat(1 << 20)));
    urls.push(format!("http://example.com/{}", "a/".repeat(10_000)));
    urls.push(format!("http://example.com/{}", "./".repeat(10_000)));
    urls.push(format!("http://example.com/{}", "../".repeat(10_000)));
    urls.push(format!("http://example.com/{}", ".".repeat(10_000)));
    urls.push(format!("http://example.com/{}", "%".repeat(10_000)));
    urls.push(format!("http://example.com/{}", "%FF".repeat(10_000)));
    urls.push(format!("http://example.com/a.{}", "b".repeat(1 << 20)));
    urls.push(format!("http://example.com/{}", "\u{65e5}".repeat(100_000)));
    // A query with a hundred thousand parameters, and one key a megabyte long.
    urls.push(format!("http://example.com/?{}", "a=1&".repeat(100_000)));
    urls.push(format!("http://example.com/?{}", "&".repeat(100_000)));
    urls.push(format!("http://example.com/?{}", "=".repeat(100_000)));
    urls.push(format!("http://example.com/?{}=1", "k".repeat(1 << 20)));
    urls.push(format!(
        "http://example.com/?{}",
        "\u{43a}=\u{437}&".repeat(20_000)
    ));
    // A fragment a megabyte long.
    urls.push(format!("http://example.com/#{}", "f".repeat(1 << 20)));
    // Credentials a megabyte long.
    urls.push(format!(
        "http://{}:{}@example.com/",
        "u".repeat(1 << 19),
        "p".repeat(1 << 19)
    ));

    urls
}

#[test]
fn every_hostile_url_that_parses_routes_and_routes_the_same_way_twice() {
    let memories = branching_memories();
    let cold = [SiteMemory::cold()];
    let mut parsed = 0usize;

    for raw in hostile_urls() {
        let Ok(url) = Url::parse(&raw) else {
            continue;
        };

        parsed += 1;

        // The megabyte urls are about the parser and the featurizer, not the
        // rules, so they take one pass each rather than the whole matrix.
        if raw.len() > 4_096 {
            let input = RouteInput::new(&url, DeclaredNeed::Markdown).with_memory(&cold[0]);
            assert_eq!(
                answer(&input),
                answer(&input),
                "two runs disagree for {raw:.80}"
            );
            continue;
        }

        let first = exercise(&url, &memories);
        let second = exercise(&url, &memories);
        assert_eq!(first, second, "two runs disagree for {raw:.80}");
    }

    // A list that `Url` refused wholesale would pass every assertion above
    // without routing anything.
    assert!(parsed > 100, "only {parsed} of the hostile urls parsed");
}

#[test]
fn every_edge_of_a_memory_routes_and_routes_the_same_way_twice() {
    let url = Url::parse("https://www.example.co.uk/a/b/page-1.html?page=2&id=9").unwrap();
    let memories = hostile_memories();

    let first = exercise(&url, &memories);
    let second = exercise(&url, &memories);
    assert_eq!(first, second);
    assert_eq!(first.len(), NEEDS.len() * 4 * 2 * memories.len());
}

#[test]
fn the_other_public_entry_points_take_hostile_strings() {
    for raw in [
        "",
        " ",
        "us",
        "US",
        " de ",
        "\u{fc}",
        "\u{fc}\u{fc}",
        "u\u{0301}",
        "usa",
        "u",
        "12",
        "u1",
        "\u{1f600}",
        "smart",
        "SMART_MODE",
        " chrome ",
        "\u{130}",
    ] {
        let _ = Country::new(raw);
        let _ = raw.parse::<Country>();
        let _ = RequestMode::from_wire(raw);
    }

    assert_eq!(Country::new("\u{fc}"), None);
    assert_eq!(Country::new("\u{fc}\u{fc}"), None);
    assert_eq!(
        Country::new(" DE ").map(|c| c.as_str().to_owned()),
        Some("de".to_owned())
    );
    // A dotless capital I lowercases to a two byte sequence and must not be
    // read as a letter.
    assert_eq!(RequestMode::from_wire("\u{130}"), None);
}

#[test]
fn a_success_rate_that_is_not_a_number_is_not_a_high_success_rate() {
    // A caller that divides zero successes by zero attempts hands over NaN.
    // The doc on the bucket says anything outside zero to one is clamped
    // because the caller's arithmetic is not trusted, and NaN is the
    // arithmetic least worth trusting. It has to land in the same band as no
    // history at all, and the rules have to read it the way they read a cold
    // site, or a bug in the caller's bookkeeping turns into a claim that the
    // site answers nine times in ten.
    let url = Url::parse("https://example.com/pricing").unwrap();
    let cold = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown));

    for rate in [f32::NAN, -f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
        let memory = SiteMemory {
            observations: 0,
            success_rate: rate,
            streak: 0,
            last_status: StatusClass::Unknown,
        };
        let input = RouteInput::new(&url, DeclaredNeed::Markdown).with_memory(&memory);
        let vector = featurize(&input);

        if rate.is_infinite() && rate > 0.0 {
            // Positive infinity clamps to one, which is the top band, and that
            // is the documented behaviour for an out of range number.
            assert_ne!(vector, cold, "{rate}");
        } else {
            assert_eq!(vector, cold, "a success rate of {rate} moved the vector");
        }

        let (decision, rule) = HeuristicRouter::new().explain(&input);
        assert_eq!(rule, Rule::ColdStart, "{rate}");
        assert_eq!(decision.mode(), RequestMode::Smart, "{rate}");
    }

    // The steady site rule reads the rate too, and NaN must not reach it.
    let steady = SiteMemory {
        observations: 40,
        success_rate: f32::NAN,
        streak: 12,
        last_status: StatusClass::Ok,
    };
    let input = RouteInput::new(&url, DeclaredNeed::Markdown).with_memory(&steady);
    assert_ne!(HeuristicRouter::new().explain(&input).1, Rule::SteadySite);
}

#[test]
fn a_confidence_that_is_not_a_number_does_not_leave_the_range() {
    // `RouteDecision::new` promises a confidence from zero to one. `clamp`
    // passes NaN straight through, and a NaN here reaches the client's
    // recorded rows and its confidence floor, neither of which can hold one.
    for bad in [f32::NAN, -f32::NAN] {
        let decision = RouteDecision::new(Action::new(RequestMode::Http), RouteSource::Model, bad);
        assert_eq!(decision.confidence, 0.0, "{bad}");
    }

    assert_eq!(
        RouteDecision::new(Action::default(), RouteSource::Model, f32::INFINITY).confidence,
        1.0
    );
    assert_eq!(
        RouteDecision::new(Action::default(), RouteSource::Model, f32::NEG_INFINITY).confidence,
        0.0
    );
}

/// The cost of the routing half, with the parse left out because the parse is
/// the url crate's and the caller pays it once anyway.
fn route_cost(url: &Url) -> Duration {
    let input = RouteInput::new(url, DeclaredNeed::Markdown);
    let started = Instant::now();
    let vector = featurize(&input);
    let decision = HeuristicRouter::new().route(&input);
    let registrable = registrable_domain(url);
    let elapsed = started.elapsed();
    std::hint::black_box((vector, decision, registrable));
    elapsed
}

#[test]
fn routing_cost_grows_no_faster_than_the_url_does() {
    // Each of these is a megabyte or so of one shape. Anything quadratic in
    // the length of the path, the query, the host or a single segment would
    // take minutes here rather than milliseconds. The bound is loose enough
    // for a debug build on a loaded machine and still three orders of
    // magnitude below what a quadratic pass would cost.
    let bound = Duration::from_secs(2);
    let big: Vec<(&str, String)> = vec![
        (
            "one segment",
            format!("http://example.com/{}", "a".repeat(1 << 20)),
        ),
        (
            "many segments",
            format!("http://example.com/{}", "ab/".repeat(300_000)),
        ),
        (
            "many dots",
            format!("http://example.com/{}", ".".repeat(1 << 20)),
        ),
        (
            "one extension",
            format!("http://example.com/a.{}", "b".repeat(1 << 20)),
        ),
        (
            "escaped",
            format!("http://example.com/{}", "%41".repeat(300_000)),
        ),
        (
            "many params",
            format!("http://example.com/?{}", "a=1&".repeat(250_000)),
        ),
        (
            "one key",
            format!("http://example.com/?{}=1", "k".repeat(1 << 20)),
        ),
        (
            "empty params",
            format!("http://example.com/?{}", "&".repeat(1 << 20)),
        ),
        ("many labels", format!("http://{}com/", "a.".repeat(60_000))),
        ("one label", format!("http://{}.com/", "a".repeat(60_000))),
        (
            "non ascii path",
            format!("http://example.com/{}", "\u{65e5}".repeat(300_000)),
        ),
        (
            "fragment",
            format!("http://example.com/#{}", "f".repeat(1 << 20)),
        ),
    ];

    for (name, raw) in big {
        let Ok(url) = Url::parse(&raw) else {
            // The url crate refusing it is not this crate's cliff.
            continue;
        };

        let cost = route_cost(&url);
        assert!(
            cost < bound,
            "{name}: routing {} bytes took {cost:?}",
            raw.len()
        );
    }
}

/// A small deterministic generator, so the loop below is the same loop on
/// every machine and a failure can be replayed from its seed.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % (n as u64)) as usize
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> &'a T {
        &from[self.below(from.len())]
    }
}

/// One random url. Three in four are shaped like urls with hostile parts and
/// the rest are noise, so both the parser's accept path and the bytes it
/// never expected are covered.
fn random_url(rng: &mut XorShift, out: &mut String) {
    out.clear();

    const SCHEMES: [&str; 12] = [
        "http://",
        "https://",
        "HTTP://",
        "ftp://",
        "file://",
        "data:",
        "javascript:",
        "mailto:",
        "",
        "//",
        "http:",
        "blob:",
    ];
    const HOST_PARTS: [&str; 28] = [
        "example",
        "www",
        "a",
        "xn--",
        "xn--bcher-kva",
        ".",
        "..",
        "-",
        "_",
        "com",
        "co.uk",
        "[::1]",
        "[::]",
        "[::1%25eth0]",
        "192.0.2.1",
        "255.255.255.255",
        "0x7f.1",
        ":8080",
        ":0",
        ":65535",
        ":65536",
        "user:pass@",
        "@",
        "\u{4f60}\u{597d}",
        "%20",
        "%FF",
        "localhost",
        "co",
    ];
    const PATH_PARTS: [&str; 30] = [
        "/",
        "//",
        "/.",
        "/..",
        "/...",
        ".",
        "..",
        "a",
        "index.html",
        "feed.xml",
        "x.json",
        "hero.png",
        "a-b-c",
        "a_b_c",
        "12345",
        "f47ac10b58cc4372a5670e02b2c3d479",
        "%",
        "%FF",
        "%2F",
        "%C0%AF",
        " ",
        "\t",
        "\u{0}",
        "\u{65e5}",
        "\u{fffd}",
        "\u{1f600}",
        ".tar.gz",
        "v1.2.3",
        "a.HTML",
        "\\",
    ];
    const QUERY_PARTS: [&str; 24] = [
        "?",
        "&",
        "=",
        "a",
        "page",
        "id",
        "q",
        "utm_source",
        "utm_",
        "gclid",
        "token",
        "lang",
        "format",
        "sort",
        "1",
        "%FF",
        "%",
        "\u{43a}",
        "#",
        "==",
        "&&",
        "?&",
        "=&",
        "x=1",
    ];

    if rng.below(4) != 0 {
        out.push_str(rng.pick::<&str>(&SCHEMES));

        for _ in 0..rng.below(8) {
            out.push_str(rng.pick::<&str>(&HOST_PARTS));
        }

        for _ in 0..rng.below(12) {
            out.push_str(rng.pick::<&str>(&PATH_PARTS));
        }

        for _ in 0..rng.below(12) {
            out.push_str(rng.pick::<&str>(&QUERY_PARTS));
        }

        return;
    }

    let len = rng.below(64);

    for _ in 0..len {
        match rng.below(4) {
            0 => out.push(rng.next() as u8 as char),
            1 => {
                // Any scalar value at all, surrogates skipped.
                if let Some(ch) = char::from_u32((rng.next() % 0x11_0000) as u32) {
                    out.push(ch);
                }
            }
            2 => out.push(*rng.pick(&[
                '/', '.', '?', '&', '=', '#', '%', ':', '@', '[', ']', '-', '_', '~', '+', ' ',
            ])),
            _ => out.push((b'a' + rng.below(26) as u8) as char),
        }
    }
}

#[test]
fn a_few_hundred_thousand_random_urls_route_without_a_panic_and_in_time() {
    const ROUNDS: usize = 300_000;

    let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
    let mut raw = String::new();
    let mut parsed = 0usize;
    let mut routed = Duration::ZERO;
    let memories = [
        SiteMemory::cold(),
        SiteMemory {
            observations: u32::MAX,
            success_rate: f32::NAN,
            streak: i16::MIN,
            last_status: StatusClass::Blocked,
        },
        SiteMemory {
            observations: 3,
            success_rate: 0.9,
            streak: 3,
            last_status: StatusClass::Ok,
        },
        SiteMemory {
            observations: 1,
            success_rate: 0.0,
            streak: -1,
            last_status: StatusClass::Empty,
        },
        SiteMemory {
            observations: 9,
            success_rate: 0.0,
            streak: -9,
            last_status: StatusClass::RateLimited,
        },
    ];

    for round in 0..ROUNDS {
        random_url(&mut rng, &mut raw);

        let Ok(url) = Url::parse(&raw) else {
            continue;
        };

        parsed += 1;

        let need = NEEDS[round % NEEDS.len()];
        let memory = &memories[round % memories.len()];
        let mut input = RouteInput::new(&url, need).with_memory(memory);

        if round % 3 == 0 {
            input = input.at_hour((round % 256) as u8);
        }

        let started = Instant::now();
        let first = answer(&input);
        routed += started.elapsed();

        let second = answer(&input);
        assert_eq!(first, second, "two runs disagree for {raw:?}");

        // A route the client has to be able to act on.
        assert!(matches!(
            first.decision.wait(),
            Wait::Now | Wait::Settled { .. }
        ));
    }

    // A generator whose output `Url` mostly refuses would pass while
    // routing almost nothing.
    assert!(
        parsed > ROUNDS / 4,
        "only {parsed} of {ROUNDS} random urls parsed"
    );

    // Routing has to stay cheap enough to run on every request. A hundred
    // microseconds a call in a debug build is well past anything the featurizer
    // does and well short of anything that has gone wrong.
    let per_call = routed / (parsed as u32);
    assert!(
        per_call < Duration::from_micros(100),
        "routing averaged {per_call:?} over {parsed} urls"
    );
}
