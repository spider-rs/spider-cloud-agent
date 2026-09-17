//! An offline evaluation of the whole decision path over the shipped fixture
//! models.
//!
//! A seeded sweep of a few thousand contexts runs through `generate`,
//! `featurize_edit` and `choose`, once with the fixture artifacts, which must
//! abstain on everything, and once with a scorer that is sure of every edit,
//! which must apply only what `validate` allows and never touch a field the
//! caller set. The sweep has to answer the same way twice and stay cheap, and
//! the trainer's golden cases have to reach the gate as keeps.
//!
//! Everything here is offline: no page, no network, no clock but the one that
//! bounds the timing test.

#![cfg(feature = "embedded-model")]
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

use serde::Deserialize;
use spider_optimize::candidates::price;
use spider_optimize::features::{EDIT_KEY, EDIT_OP};
use spider_optimize::schema::{service_default, MODES, PROXIES};
use spider_optimize::{
    choose, embedded, featurize_edit, generate, summarize, validate, Candidate, Choice, Compact,
    Context, Gate, Input, Key, ModelVersion, Observation, Params, Reason, Schema, Score, Scorer,
    EDIT_DIM, KEEP_CODE,
};
use spider_route::{
    Action, Country, DeclaredNeed, ProxyPool, RequestMode, RouteDecision, RouteSource, SiteMemory,
    StatusClass, FEATURES_USED,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use url::Url;

/// How many contexts the sweep holds.
const SWEEP: usize = 2_000;

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

const CAPS: [f32; 3] = [1.0, 3.0, 8.0];

/// The switches an edit may set, in the order `Request::flags` holds them.
const FLAGS: [Key; 6] = [
    Key::DisableIntercept,
    Key::FullResources,
    Key::BlockAds,
    Key::BlockAnalytics,
    Key::BlockStylesheets,
    Key::DisableHints,
];

/// The fields an edit reads and writes, standing in for the client's request
/// type, which this crate cannot name.
#[derive(Clone, Debug, Default, PartialEq)]
struct Request {
    request: Option<RequestMode>,
    proxy: Option<ProxyPool>,
    country: Option<Country>,
    idle_wait: Option<u32>,
    flags: [Option<bool>; 6],
    network_blacklist: Option<Vec<String>>,
    network_whitelist: Option<Vec<String>>,
}

impl Params for Request {
    fn is_set(&self, key: Key) -> bool {
        match key {
            Key::Request => self.request.is_some(),
            Key::Proxy => self.proxy.is_some(),
            Key::CountryCode => self.country.is_some(),
            Key::WaitIdleMillis => self.idle_wait.is_some(),
            Key::NetworkBlacklist => self.network_blacklist.is_some(),
            Key::NetworkWhitelist => self.network_whitelist.is_some(),
            key => self.flag(key).is_some(),
        }
    }
    fn request(&self) -> Option<RequestMode> {
        self.request
    }
    fn proxy(&self) -> Option<ProxyPool> {
        self.proxy
    }
    fn idle_wait_millis(&self) -> Option<u32> {
        self.idle_wait
    }
    fn flag(&self, key: Key) -> Option<bool> {
        FLAGS
            .iter()
            .position(|k| *k == key)
            .and_then(|at| self.flags[at])
    }
    fn list(&self, key: Key) -> Option<&[String]> {
        match key {
            Key::NetworkBlacklist => self.network_blacklist.as_deref(),
            Key::NetworkWhitelist => self.network_whitelist.as_deref(),
            _ => None,
        }
    }
    fn set_request(&mut self, mode: RequestMode) {
        self.request = Some(mode);
    }
    fn set_proxy(&mut self, pool: ProxyPool) {
        self.proxy = Some(pool);
    }
    fn set_idle_wait(&mut self, millis: u32) {
        self.idle_wait = Some(millis);
    }
    fn set_flag(&mut self, key: Key, value: bool) {
        if let Some(at) = FLAGS.iter().position(|k| *k == key) {
            self.flags[at] = Some(value);
        }
    }
    fn set_list(&mut self, key: Key, list: Option<Vec<String>>) {
        if key == Key::NetworkBlacklist {
            self.network_blacklist = list;
        }
    }
}

/// A small deterministic generator, so the sweep is the same sweep on every
/// machine and a failure can be replayed from its seed.
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

    // Spelled out for text because rustc 1.88, the floor, infers the slice's
    // element as `str` from `push_str` when the generic form is used.
    fn word(&mut self, from: &[&'static str]) -> &'static str {
        from[self.below(from.len())]
    }

    fn coin(&mut self) -> bool {
        self.below(2) == 0
    }
}

/// One random page address: a scheme, a host, a few path segments and a
/// query. Every extension class the router knows shows up in the path parts,
/// and the hosts cover subdomains, a public suffix with two labels, an
/// address and a punycode label.
fn random_url(rng: &mut XorShift) -> Url {
    const SCHEMES: [&str; 3] = ["https://", "http://", "HTTPS://"];
    const HOSTS: [&str; 12] = [
        "example.com",
        "www.example.com",
        "shop.example.co.uk",
        "a.b.c.example.org",
        "example.net:8080",
        "192.0.2.1",
        "[::1]",
        "xn--bcher-kva.example",
        "docs.example.org",
        "cdn.example.net",
        "news.example.co.uk",
        "EXAMPLE.COM",
    ];
    const SEGMENTS: [&str; 16] = [
        "articles",
        "products",
        "api/v2",
        "2024/09/16",
        "u/12345",
        "a-long-story-with-many-words",
        "%E6%97%A5%E6%9C%AC",
        "index",
        "wp-content/uploads",
        "search",
        "blog",
        "docs",
        "assets",
        "files",
        "..",
        "download",
    ];
    const LEAVES: [&str; 22] = [
        "",
        "page.html",
        "feed.xml",
        "data.json",
        "feed.rss",
        "notes.txt",
        "table.csv",
        "paper.pdf",
        "report.docx",
        "hero.png",
        "clip.mp4",
        "bundle.zip",
        "app.js",
        "style.css",
        "font.woff2",
        "archive.tar.gz",
        "image.JPEG",
        "sheet.xlsx",
        "readme.md",
        "atom",
        "item.php",
        "video.webm",
    ];
    const QUERIES: [&str; 12] = [
        "",
        "?page=2",
        "?id=9&sort=asc",
        "?utm_source=x&utm_medium=y",
        "?q=%D0%BA",
        "?token=abc",
        "?format=json",
        "?lang=de&page=1&id=2&x=3",
        "?a=1&a=2&a=3",
        "?",
        "?=&==",
        "?gclid=1",
    ];

    let mut raw = String::new();
    raw.push_str(rng.word(&SCHEMES));
    raw.push_str(rng.word(&HOSTS));
    for _ in 0..rng.below(4) {
        raw.push('/');
        raw.push_str(rng.word(&SEGMENTS));
    }
    raw.push('/');
    raw.push_str(rng.word(&LEAVES));
    raw.push_str(rng.word(&QUERIES));
    if rng.below(8) == 0 {
        raw.push_str("#top");
    }
    Url::parse(&raw).expect("every generated url parses")
}

/// A memory in one of the four states the rows record: cold, thin, warm,
/// steady, each with the status given.
fn memory(rng: &mut XorShift, state: usize, status: StatusClass) -> Option<SiteMemory> {
    let observations = match state {
        0 => return None,
        1 => 1 + rng.below(2) as u32,
        2 => 3 + rng.below(7) as u32,
        _ => 10 + rng.below(200) as u32,
    };
    Some(SiteMemory {
        observations,
        success_rate: rng.below(101) as f32 / 100.0,
        streak: rng.below(21) as i16 - 10,
        last_status: status,
    })
}

/// A resource list with a first party script and a few third party groups,
/// so the summary holds identifiers a blacklist edit can name.
fn resources(rng: &mut XorShift, page: &Url) -> Vec<(Url, u64)> {
    const THIRD: [(&str, &str); 6] = [
        ("tracker-alpha.example", "t.js"),
        ("cdn-beta.example", "lib/big.js"),
        ("ads-gamma.example", "a.gif"),
        ("fonts-delta.example", "f.woff2"),
        ("widget-epsilon.example", "w.js"),
        ("video-zeta.example", "clip.mp4"),
    ];
    let mut out = Vec::new();
    let own = page.join("/static/app.js").unwrap();
    out.push((own, 10_000 + rng.below(90_000) as u64));
    if rng.coin() {
        out.push((page.join("/static/site.css").unwrap(), 5_000));
    }
    let groups = 1 + rng.below(THIRD.len());
    let start = rng.below(THIRD.len());
    for at in 0..groups {
        let (host, path) = THIRD[(start + at) % THIRD.len()];
        let requests = 1 + rng.below(4);
        for n in 0..requests {
            let url = Url::parse(&format!("https://{host}/{n}/{path}")).unwrap();
            out.push((url, 500 + rng.below(400_000) as u64));
        }
    }
    out
}

/// Everything a [`Context`] borrows, owned, plus what the sweep knows about
/// how it was built.
struct Case {
    url: Url,
    need: DeclaredNeed,
    current: Request,
    caller: Request,
    routed: RouteDecision,
    observation: Observation,
    cap: f32,
    /// The caller fixed the mode, the pool or the country.
    pinned: bool,
}

impl Case {
    fn ctx(&self) -> Context<'_> {
        Context {
            url: &self.url,
            need: self.need,
            current: &self.current,
            caller: &self.caller,
            routed: &self.routed,
            observation: &self.observation,
            multiplier_cap: self.cap,
        }
    }
}

/// The seeded sweep. The same seed gives the same list, so a failing index
/// can be replayed.
fn sweep() -> Vec<Case> {
    let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::with_capacity(SWEEP);

    for round in 0..SWEEP {
        let url = random_url(&mut rng);
        let need = NEEDS[round % NEEDS.len()];
        let status = STATUSES[(round / NEEDS.len()) % STATUSES.len()];
        let state = (round / (NEEDS.len() * STATUSES.len())) % 4;
        let cap = CAPS[round % CAPS.len()];

        let mut observation = Observation {
            memory: memory(&mut rng, state, status),
            resources: None,
            last_status: if rng.coin() {
                status
            } else {
                *rng.pick(&STATUSES)
            },
        };
        if round % 2 == 0 {
            observation.resources = Some(summarize(&url, &resources(&mut rng, &url)));
        }

        // What the caller fixed: none, one of the three the gate refuses on,
        // or all three. A switch the caller set is pinned too, but the gate
        // does not refuse on it.
        let mut caller = Request::default();
        match round % 5 {
            0..=2 => {}
            3 => match rng.below(3) {
                0 => caller.request = Some(*rng.pick(&MODES)),
                1 => caller.proxy = Some(*rng.pick(&PROXIES)),
                _ => caller.country = Country::new("de"),
            },
            _ => {
                caller.request = Some(*rng.pick(&MODES));
                caller.proxy = Some(*rng.pick(&PROXIES));
                caller.country = Country::new("us");
            }
        }
        if rng.below(4) == 0 {
            let at = rng.below(5);
            caller.flags[at] = Some(rng.coin());
        }
        if rng.below(10) == 0 {
            caller.network_whitelist = Some(vec!["example.com".into()]);
        }
        let pinned = caller.request.is_some() || caller.proxy.is_some() || caller.country.is_some();

        // The request as the router and the plan left it: the caller's
        // fields as they were, the rest filled in or left to the service.
        let mut current = caller.clone();
        if current.request.is_none() && rng.below(4) != 0 {
            current.request = Some(*rng.pick(&MODES));
        }
        if current.proxy.is_none() && rng.below(3) == 0 {
            current.proxy = Some(*rng.pick(&PROXIES));
        }
        if rng.below(3) == 0 {
            current.idle_wait = Some(*rng.pick(&[0u32, 2_000, 5_000, 10_000]));
        }
        for at in 0..5 {
            if current.flags[at].is_none() && rng.below(4) == 0 {
                current.flags[at] = Some(rng.coin());
            }
        }
        current.flags[5] = Some(round % 4 < 2);
        if rng.below(6) == 0 {
            current.network_blacklist = Some(vec!["stale.example".into()]);
        }

        let source = if pinned {
            RouteSource::Caller
        } else if observation.memory.is_some() {
            RouteSource::Memory
        } else {
            RouteSource::Heuristic
        };
        let routed = RouteDecision::new(Action::default(), source, 0.5);

        out.push(Case {
            url,
            need,
            current,
            caller,
            routed,
            observation,
            cap,
            pinned,
        });
    }

    out
}

/// How many contexts the caller fixed the mode, the pool or the country on.
fn pinned_count(cases: &[Case]) -> usize {
    cases.iter().filter(|case| case.pinned).count()
}

/// Whether an input is keep: no edit key bit is set.
fn is_keep(input: &Input) -> bool {
    (EDIT_KEY..EDIT_OP).all(|slot| input.edit.get(slot) == Some(0.0))
}

const fn score(p_success: f32, latency_ms: f32, credits: f32, support: f32) -> Score {
    Score {
        p_success,
        latency_ms,
        credits,
        support,
    }
}

/// A scorer sure of every edit: cheap, fast and well supported for anything
/// but keep, which it prices dear.
struct Confident;

impl Scorer for Confident {
    fn score(&self, input: &Input) -> Score {
        if is_keep(input) {
            score(0.95, 900.0, 4.0, 50.0)
        } else {
            score(0.99, 100.0, 1.0, 50.0)
        }
    }

    fn version(&self) -> Option<ModelVersion> {
        Some(ModelVersion(1))
    }
}

fn gbdt() -> Compact {
    Compact::from_bytes(include_bytes!("fixtures/gbdt-v1.bin")).unwrap()
}

/// Run the whole path on every case and return each choice with the reasons
/// tallied.
fn run(scorer: &dyn Scorer, cases: &[Case]) -> (Vec<Choice>, BTreeMap<String, usize>) {
    let schema = Schema::v1();
    let gate = Gate::default();
    let mut choices = Vec::with_capacity(cases.len());
    let mut reasons = BTreeMap::new();

    for (at, case) in cases.iter().enumerate() {
        let ctx = case.ctx();
        let cands = generate(&schema, &ctx);
        assert!(cands[0].edits.is_keep(), "case {at}: keep is not first");
        for cand in &cands {
            let feats = featurize_edit(&ctx, cand);
            for (slot, value) in feats.as_slice().iter().enumerate() {
                assert!(
                    value.is_finite() && (-1.0..=1.0).contains(value),
                    "case {at}: slot {slot} is {value} for {:?}",
                    cand.edits
                );
            }
        }
        let choice = choose(scorer, &ctx, &cands, &gate);
        let label = match &choice {
            Choice::Keep(reason) => format!("{reason:?}"),
            Choice::Apply { .. } => "Apply".to_owned(),
        };
        *reasons.entry(label).or_insert(0) += 1;
        choices.push(choice);
    }

    (choices, reasons)
}

/// The index of a mode in the request kind, which is also how heavy it is.
fn mode_rank(mode: Option<RequestMode>) -> usize {
    MODES
        .iter()
        .position(|m| *m == mode.unwrap_or_default())
        .unwrap()
}

/// The index of a pool in the proxy kind, which is also how dear it is.
fn proxy_rank(pool: Option<ProxyPool>) -> usize {
    PROXIES
        .iter()
        .position(|p| *p == pool.unwrap_or_default())
        .unwrap()
}

/// What a request costs as it stands.
fn price_of(request: &Request) -> f32 {
    price(
        request.request.unwrap_or_default(),
        request.proxy.unwrap_or_default(),
        request.idle_wait.unwrap_or(0),
    )
}

#[test]
fn abstention_holds_end_to_end() {
    let cases = sweep();
    assert_eq!(cases.len(), SWEEP);

    let gate = Gate::default();
    for (name, model) in [("mlp", embedded().unwrap()), ("gbdt", gbdt())] {
        // What this test relies on, read off the artifact so it is on
        // record: the support table is not empty, so a score taken without a
        // cell carries no support and the gate has too little evidence; and
        // the floor table abstains on keep and on most codes, with the few
        // sentinel floors the trainer writes for a folded split never above
        // the gate's own success floor, so no floor could ever argue for an
        // edit the gate would refuse.
        assert_eq!(model.score(&Input::default()).support, 0.0, "{name}");
        let floors: Vec<(u8, Option<f32>)> = (0..=u8::MAX)
            .map(|code| (code, model.threshold(code)))
            .filter(|(_, floor)| floor.is_some())
            .collect();
        println!("{name} floors: {floors:?}");
        assert_eq!(model.threshold(KEEP_CODE), None, "{name}: keep has a floor");
        assert!(floors.len() <= 2, "{name}: {floors:?}");
        for (code, floor) in &floors {
            assert!(
                floor.unwrap() <= gate.min_p_success,
                "{name}: code {code} floors at {floor:?}, above the gate"
            );
        }

        let (choices, reasons) = run(&model, &cases);
        println!("{name} reasons over {SWEEP} contexts: {reasons:?}");

        for (at, choice) in choices.iter().enumerate() {
            assert!(
                matches!(choice, Choice::Keep(_)),
                "{name}: case {at} applied an edit from an abstaining artifact: {choice:?}"
            );
        }
        assert!(
            reasons.len() >= 3,
            "{name}: only {} reasons over the sweep: {reasons:?}",
            reasons.len()
        );
        assert_eq!(
            reasons.get("Pinned").copied().unwrap_or(0),
            pinned_count(&cases),
            "{name}: every pinned context is refused as pinned"
        );
    }
}

#[test]
fn a_confident_scorer_applies_only_valid_unpinned_edits() {
    let cases = sweep();
    let schema = Schema::v1();
    let (choices, reasons) = run(&Confident, &cases);
    println!("confident reasons over {SWEEP} contexts: {reasons:?}");

    let mut applied = 0usize;
    let mut refused_pinned = 0usize;
    let mut under_limit = 0usize;

    for (at, (case, choice)) in cases.iter().zip(&choices).enumerate() {
        let ctx = case.ctx();
        match choice {
            Choice::Keep(Reason::Pinned) => {
                assert!(case.pinned, "case {at}: refused as pinned with no pin");
                refused_pinned += 1;
                continue;
            }
            Choice::Keep(reason) => {
                assert!(
                    !case.pinned,
                    "case {at}: a pinned context kept for {reason:?}"
                );
                continue;
            }
            Choice::Apply {
                edits,
                expected,
                baseline,
            } => {
                assert!(!case.pinned, "case {at}: a pinned context was edited");
                assert!(!edits.is_keep(), "case {at}: applied keep");
                assert!(expected.credits < baseline.credits, "case {at}");
                applied += 1;

                // The chosen set is one validate accepts against the same
                // context.
                let mut after = case.current.clone();
                let done = edits.apply(&mut after, &case.caller);
                let cand = Candidate {
                    edits: edits.clone(),
                    multiplier: price_of(&after),
                };
                assert_eq!(
                    validate(&schema, &ctx, &cand),
                    Ok(()),
                    "case {at}: {edits:?}"
                );
                assert_eq!(done.skipped_pinned, 0, "case {at}: {edits:?}");
                assert_eq!(usize::from(done.written), edits.edits().len(), "case {at}");

                // No key the caller set moved, field by field.
                let caller = &case.caller;
                if caller.request.is_some() {
                    assert_eq!(after.request, caller.request, "case {at}");
                }
                if caller.proxy.is_some() {
                    assert_eq!(after.proxy, caller.proxy, "case {at}");
                }
                if caller.idle_wait.is_some() {
                    assert_eq!(after.idle_wait, caller.idle_wait, "case {at}");
                }
                assert_eq!(after.country, caller.country, "case {at}");
                for (slot, key) in FLAGS.iter().enumerate() {
                    if caller.flags[slot].is_some() {
                        assert_eq!(after.flags[slot], caller.flags[slot], "case {at}: {key:?}");
                    }
                }
                if caller.network_whitelist.is_some() || caller.network_blacklist.is_some() {
                    assert_eq!(
                        after.network_blacklist, case.current.network_blacklist,
                        "case {at}"
                    );
                }
                assert_eq!(after.network_whitelist, case.current.network_whitelist);

                // And nothing the edit did not name moved either.
                let before = &case.current;
                for (slot, key) in FLAGS.iter().enumerate() {
                    if edits.get(*key).is_none() {
                        assert_eq!(after.flags[slot], before.flags[slot], "case {at}: {key:?}");
                    }
                }
                if edits.get(Key::Request).is_none() {
                    assert_eq!(after.request, before.request, "case {at}");
                }
                if edits.get(Key::Proxy).is_none() {
                    assert_eq!(after.proxy, before.proxy, "case {at}");
                }
                if edits.get(Key::WaitIdleMillis).is_none() {
                    assert_eq!(after.idle_wait, before.idle_wait, "case {at}");
                }
                if edits.get(Key::NetworkBlacklist).is_none() {
                    assert_eq!(
                        after.network_blacklist, before.network_blacklist,
                        "case {at}"
                    );
                }

                // Never dearer than the budget.
                assert!(
                    price_of(&after) <= case.cap,
                    "case {at}: {} over a cap of {}",
                    price_of(&after),
                    case.cap
                );

                // Never heavier than keep under a rate limit.
                if case.observation.rate_limited() {
                    under_limit += 1;
                    assert!(
                        mode_rank(after.request) <= mode_rank(before.request),
                        "case {at}"
                    );
                    assert!(
                        proxy_rank(after.proxy) <= proxy_rank(before.proxy),
                        "case {at}"
                    );
                    assert!(
                        after.idle_wait.unwrap_or(0) <= before.idle_wait.unwrap_or(0),
                        "case {at}"
                    );
                    assert!(price_of(&after) <= price_of(before), "case {at}");
                    for (slot, key) in FLAGS.iter().enumerate().take(5) {
                        let was = before.flag(*key).unwrap_or(service_default(*key));
                        let now = after.flags[slot].unwrap_or(service_default(*key));
                        let loosened = match key {
                            Key::FullResources | Key::DisableIntercept => now && !was,
                            _ => !now && was,
                        };
                        assert!(!loosened, "case {at}: {key:?} loosened under a limit");
                    }
                }
            }
        }
    }

    assert!(applied >= 100, "only {applied} edits applied");
    assert!(
        under_limit >= 10,
        "only {under_limit} edits applied under a limit"
    );
    assert_eq!(refused_pinned, pinned_count(&cases));
    assert!(pinned_count(&cases) >= 100, "the sweep pinned too little");
}

#[test]
fn decisions_are_deterministic_and_fast() {
    let cases = sweep();
    let mlp = embedded().unwrap();

    let (first, _) = run(&mlp, &cases);
    let (second, _) = run(&mlp, &sweep());
    assert_eq!(
        first, second,
        "the fixture model answered differently twice"
    );
    let (first, _) = run(&Confident, &cases);
    let (second, _) = run(&Confident, &sweep());
    assert_eq!(
        first, second,
        "the confident scorer answered differently twice"
    );

    // A full candidate list on the widest context the sweep offers.
    let schema = Schema::v1();
    let gate = Gate::default();
    let case = cases
        .iter()
        .filter(|case| !case.pinned)
        .max_by_key(|case| generate(&schema, &case.ctx()).len())
        .unwrap();
    let ctx = case.ctx();
    let cands = generate(&schema, &ctx);
    assert!(
        cands.len() >= 16,
        "only {} candidates to score",
        cands.len()
    );

    // The gate's own cost, with a scorer that costs nothing: featurizing and
    // validating every candidate and ranking them. Generous for a debug
    // build on a loaded machine, and still far below what a per-call
    // allocation or a quadratic pass over the candidates would cost.
    const GATE_CALLS: u32 = 10_000;
    let started = Instant::now();
    for _ in 0..GATE_CALLS {
        std::hint::black_box(choose(&Confident, &ctx, &cands, &gate));
    }
    let gate_cost = started.elapsed();
    println!(
        "{GATE_CALLS} choose calls over {} candidates with a fixed scorer took {gate_cost:?}, {:?} each",
        cands.len(),
        gate_cost / GATE_CALLS
    );
    let bound = Duration::from_secs(5);
    assert!(
        gate_cost < bound,
        "{GATE_CALLS} choose calls took {gate_cost:?}, over {bound:?}"
    );

    // The same calls through the MLP fixture, which is three dense nets per
    // candidate and dominates the cost in a debug build: a few milliseconds
    // a call here, so fewer calls and a bound per call with an order of
    // magnitude to spare.
    const MODEL_CALLS: u32 = 500;
    let started = Instant::now();
    for _ in 0..MODEL_CALLS {
        std::hint::black_box(choose(&mlp, &ctx, &cands, &gate));
    }
    let model_cost = started.elapsed();
    let per_call = model_cost / MODEL_CALLS;
    println!(
        "{MODEL_CALLS} choose calls over {} candidates with the mlp fixture took {model_cost:?}, {per_call:?} each",
        cands.len()
    );
    let bound = Duration::from_millis(50);
    assert!(
        per_call < bound,
        "the mlp fixture averaged {per_call:?} a call over {MODEL_CALLS} calls, over {bound:?}"
    );
}

#[derive(Deserialize)]
struct Expected {
    p_success: Option<f32>,
    support: f32,
}

#[derive(Deserialize)]
struct GoldenCase {
    base: Vec<Option<f32>>,
    edit: Vec<Option<f32>>,
    cell: u32,
    expect: Expected,
}

/// A scorer that hands the gate one recorded score for every candidate, with
/// keep priced dear, and honours the artifact's floor table the way a client
/// would: a code with no floor abstains, and a score under its floor abstains.
struct Recorded {
    candidate: Score,
    floor: Option<f32>,
    honour_floor: bool,
}

impl Scorer for Recorded {
    fn score(&self, input: &Input) -> Score {
        // Keep sits exactly on the gate's success floor, so a candidate is
        // never refused for being a point worse than keep, only for being
        // under the floor itself.
        if is_keep(input) {
            return score(0.9, 900.0, 1_000.0, 50.0);
        }
        if !self.honour_floor {
            return self.candidate;
        }
        match self.floor {
            Some(floor) if self.candidate.p_success >= floor => self.candidate,
            _ => Score {
                p_success: f32::NAN,
                ..self.candidate
            },
        }
    }

    fn version(&self) -> Option<ModelVersion> {
        Some(ModelVersion(1))
    }
}

/// How the golden cases of one fixture fell.
#[derive(Debug, Default)]
struct Tally {
    nonfinite: usize,
    unsupported: usize,
    no_floor: usize,
    floored: usize,
    applied: usize,
    applied_without_floor: usize,
}

/// Score every golden case through the reader and hand each score to the
/// gate on one plain context.
fn golden(model: &Compact, text: &str) -> Tally {
    let cases: Vec<GoldenCase> = serde_json::from_str(text).unwrap();
    assert!(cases.len() >= 32);

    // A plain context with a few identifiers, so every kind of edit is on
    // offer and a confident score would be applied.
    let url = Url::parse("https://www.example.com/news/world/a-long-story").unwrap();
    let page: Vec<(Url, u64)> = [
        ("https://www.example.com/app.js", 40_000),
        ("https://tracker-alpha.example/t.js", 90_000),
        ("https://cdn-beta.example/big.js", 300_000),
    ]
    .iter()
    .map(|(url, bytes)| (Url::parse(url).unwrap(), *bytes))
    .collect();
    let case = Case {
        need: DeclaredNeed::Markdown,
        current: Request {
            request: Some(RequestMode::Smart),
            flags: [None, None, None, None, None, Some(true)],
            ..Request::default()
        },
        caller: Request::default(),
        routed: RouteDecision::default(),
        observation: Observation {
            resources: Some(summarize(&url, &page)),
            ..Observation::cold()
        },
        cap: 8.0,
        pinned: false,
        url,
    };
    let ctx = case.ctx();
    let schema = Schema::v1();
    let gate = Gate::default();
    let cands = generate(&schema, &ctx);
    assert!(cands.len() > 10);

    let mut tally = Tally::default();
    for (at, golden) in cases.iter().enumerate() {
        assert_eq!(golden.base.len(), FEATURES_USED);
        assert_eq!(golden.edit.len(), EDIT_DIM);
        let base: Vec<f32> = golden.base.iter().map(|x| x.unwrap_or(f32::NAN)).collect();
        let edit: Vec<f32> = golden.edit.iter().map(|x| x.unwrap_or(f32::NAN)).collect();
        let scored = model.score_slices(&base, &edit, golden.cell);

        // The three ways the trainer's file says abstain: a slot it wrote as
        // null, a cell outside the support table, a code with no floor.
        let has_nonfinite = golden.base.iter().chain(&golden.edit).any(Option::is_none);
        let cell_unsupported = !model.supported(golden.cell);
        let code = (golden.cell & 0xff) as u8;
        let floor = model.threshold(code);

        assert_eq!(
            has_nonfinite,
            golden.expect.p_success.is_none(),
            "case {at}"
        );
        assert_eq!(has_nonfinite, scored.p_success.is_nan(), "case {at}");
        assert_eq!(cell_unsupported, golden.expect.support == 0.0, "case {at}");
        assert_eq!(cell_unsupported, scored.support == 0.0, "case {at}");

        let recorded = Recorded {
            candidate: scored,
            floor,
            honour_floor: true,
        };
        let choice = choose(&recorded, &ctx, &cands, &gate);

        if has_nonfinite {
            tally.nonfinite += 1;
            assert_eq!(choice, Choice::Keep(Reason::NoEvidence), "case {at}");
        } else if cell_unsupported {
            tally.unsupported += 1;
            assert!(
                matches!(
                    choice,
                    Choice::Keep(Reason::NoEvidence | Reason::BelowFloor | Reason::NoGain)
                ),
                "case {at}: {choice:?}"
            );
        } else if let Some(floor) = floor {
            // Finite, supported and floored: the gate's own bar decides, and
            // this score is far cheaper than keep, so the bar is the success
            // floor alone.
            tally.floored += 1;
            if scored.p_success < floor {
                assert_eq!(choice, Choice::Keep(Reason::NoEvidence), "case {at}");
            } else if scored.p_success < gate.min_p_success {
                assert!(
                    matches!(choice, Choice::Keep(Reason::BelowFloor | Reason::NoGain)),
                    "case {at}: {choice:?}"
                );
            } else {
                assert!(
                    matches!(choice, Choice::Apply { .. }),
                    "case {at}: {choice:?}"
                );
            }
        } else {
            tally.no_floor += 1;
            assert_eq!(choice, Choice::Keep(Reason::NoEvidence), "case {at}");
        }
        if matches!(choice, Choice::Apply { .. }) {
            tally.applied += 1;
        }

        // The control: the same score with the floor table ignored. A
        // supported, finite, confident score is then applied whatever its
        // code, so a keep on a code with no floor comes from the abstain
        // handling and not from a score the gate would refuse anyway.
        let ignoring = Recorded {
            honour_floor: false,
            ..recorded
        };
        let control = choose(&ignoring, &ctx, &cands, &gate);
        let confident =
            !has_nonfinite && !cell_unsupported && scored.p_success >= gate.min_p_success;
        if confident {
            tally.applied_without_floor += 1;
            assert!(
                matches!(control, Choice::Apply { .. }),
                "case {at}: {control:?} for {scored:?}"
            );
        } else {
            assert!(matches!(control, Choice::Keep(_)), "case {at}: {control:?}");
        }
    }
    tally
}

#[test]
fn the_golden_cases_agree_with_the_gate() {
    let mlp = golden(
        &embedded().unwrap(),
        include_str!("fixtures/golden-v1.json"),
    );
    println!("mlp golden: {mlp:?}");
    let gbdt = golden(&gbdt(), include_str!("fixtures/golden-gbdt-v1.json"));
    println!("gbdt golden: {gbdt:?}");

    for tally in [&mlp, &gbdt] {
        assert!(tally.nonfinite >= 1, "{tally:?}");
        assert!(tally.unsupported >= 1, "{tally:?}");
        assert!(tally.no_floor >= 1, "{tally:?}");
        assert!(tally.floored >= 1, "{tally:?}");
    }
    // The keeps above are not a gate that never applies. The MLP file holds
    // scores that clear the gate once the abstain marks are set aside, some
    // of them on a code with no floor, and a few that clear every mark and
    // are applied. The GBDT file records no score confident enough for
    // either, so it proves only the abstain side.
    assert!(
        mlp.applied_without_floor > mlp.applied,
        "no golden case was kept by an abstain mark alone: {mlp:?}"
    );
    assert!(mlp.applied >= 1, "{mlp:?}");
    assert_eq!(gbdt.applied, 0, "{gbdt:?}");
}
