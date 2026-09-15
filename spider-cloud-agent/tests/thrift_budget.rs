//! What the thrift layer saves, measured against the fixture corpus.
//!
//! The fixtures in `tests/fixtures/thrift/` hold one real page in every shape
//! the service can return it, and a six page crawl of one site. The tests below
//! build the response the API would send for a given [`Plan`], measure it, and
//! compare it against the response a caller gets when they ask for nothing in
//! particular, which is everything.
//!
//! The ratios are asserted in narrow bands rather than as single numbers,
//! because a byte either side of a fixture edit should not fail a build and a
//! row of the plan table going wrong should. Both directions are checked: a
//! need that suddenly returns more fails, and so does one that returns less
//! than the plan can account for.

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

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use spider_cloud_agent::params::{ReturnFormat, ReturnFormatHandling};
use spider_cloud_agent::policy::{Ladder, Step};
use spider_cloud_agent::thrift::plan::Endpoint;
use spider_cloud_agent::thrift::trim::{truncate_to_tokens, TrimSettings, Trimmer};
use spider_cloud_agent::thrift::{approx_tokens, max_tokens, Plan, OPTIONAL_RETURNS};
use spider_cloud_agent::{Need, RequestParams, TokenBudget};

// ---------------------------------------------------------------------------
// the corpus
// ---------------------------------------------------------------------------

/// One page of the corpus, in every shape the service can return it.
struct Fixture {
    raw: String,
    clean_html: String,
    text: String,
    markdown: String,
    text_with_furniture: String,
    markdown_with_furniture: String,
    metadata: Value,
    links: Vec<String>,
    fields: Map<String, Value>,
}

fn corpus() -> Fixture {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/thrift/product_page.json"
    ))
    .expect("the product page fixture");
    let value: Value = serde_json::from_str(&raw).expect("the fixture parses");

    Fixture {
        raw: text_at(&value, "raw"),
        clean_html: text_at(&value, "clean_html"),
        text: text_at(&value, "text"),
        markdown: text_at(&value, "markdown"),
        text_with_furniture: text_at(&value, "text_with_furniture"),
        markdown_with_furniture: text_at(&value, "markdown_with_furniture"),
        metadata: value["metadata"].clone(),
        links: value["links"]
            .as_array()
            .expect("links")
            .iter()
            .map(|link| link.as_str().expect("a link").to_string())
            .collect(),
        fields: value["fields"].as_object().expect("fields").clone(),
    }
}

fn text_at(value: &Value, key: &str) -> String {
    value[key].as_str().expect("a string").to_string()
}

/// The pages of the crawl fixture, as markdown.
fn crawl() -> Vec<String> {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/thrift/crawl_widgets.json"
    ))
    .expect("the crawl fixture");
    let value: Value = serde_json::from_str(&raw).expect("the fixture parses");

    value
        .as_array()
        .expect("an array of pages")
        .iter()
        .map(|page| text_at(page, "markdown"))
        .collect()
}

// ---------------------------------------------------------------------------
// what the service would send back for a plan
// ---------------------------------------------------------------------------

/// Bytes the service would send for this plan, given this page.
///
/// The plan decides. Nothing here reads a [`Need`], so a row of the table
/// changing moves this number.
fn wire_bytes(page: &Fixture, plan: &Plan) -> usize {
    let mut body = Map::new();
    body.insert("url".into(), json!("https://example.com/widgets/steel-40"));
    body.insert("status".into(), json!(200));

    let format = plan.return_format.unwrap_or(ReturnFormat::Raw);
    let content = match format {
        ReturnFormat::Empty => None,
        // Stripping the navigation and the footer happens on the service, so
        // whether they are in the response is the readability flag's doing.
        ReturnFormat::Text => Some(json!({ "text": if plan.readability == Some(true) {
            &page.text
        } else {
            &page.text_with_furniture
        } })),
        ReturnFormat::Markdown | ReturnFormat::Commonmark => {
            Some(json!({ "markdown": if plan.readability == Some(true) {
                &page.markdown
            } else {
                &page.markdown_with_furniture
            } }))
        }
        ReturnFormat::Bytes => Some(json!({ "screenshot": vec![137u8, 80, 78, 71] })),
        ReturnFormat::Xml => Some(json!({ "raw": page.raw })),
        ReturnFormat::Raw => {
            // Cleaning is what the difference between these two shapes is, and
            // it happens on the service.
            let markup = if plan.clean_html == Some(true) {
                &page.clean_html
            } else {
                &page.raw
            };
            Some(json!({ "raw": markup }))
        }
        // A format added later is measured as the whole page, which is the
        // reading that cannot flatter the plan table by accident.
        _ => Some(json!({ "raw": page.raw })),
    };
    if let Some(content) = content {
        body.insert("content".into(), content);
    }

    // A field left unset by the plan is one the API fills in on its own.
    if plan.metadata != Some(false) {
        body.insert("metadata".into(), page.metadata.clone());
    }
    if plan.return_page_links != Some(false) {
        body.insert("links".into(), json!(page.links));
    }
    if plan.return_headers != Some(false) {
        body.insert(
            "headers".into(),
            json!({
                "content-type": "text/html; charset=utf-8",
                "cache-control": "public, max-age=300",
                "vary": "Accept-Encoding",
            }),
        );
    }
    if plan.return_cookies != Some(false) {
        body.insert("cookies".into(), json!("basket=; Path=/; HttpOnly"));
    }
    if let Some(map) = &plan.css_extraction_map {
        let mut extracted = Map::new();
        for group in map.values().flatten() {
            if let Some(value) = page.fields.get(&group.name) {
                extracted.insert(group.name.clone(), value.clone());
            }
        }
        body.insert("extracted_content".into(), Value::Object(extracted));
    }
    body.insert(
        "costs".into(),
        json!({ "compute_cost": 0.42, "total_cost": 0.42 }),
    );

    serde_json::to_vec(&Value::Array(vec![Value::Object(body)]))
        .expect("the response serializes")
        .len()
}

/// What a caller gets when they ask for nothing in particular.
fn baseline(page: &Fixture) -> usize {
    wire_bytes(page, &Plan::for_need(&Need::Raw))
}

/// The share of the default response a need brings back.
fn share(page: &Fixture, need: &Need) -> f64 {
    wire_bytes(page, &Plan::for_need(need)) as f64 / baseline(page) as f64
}

// ---------------------------------------------------------------------------
// the numbers the readme quotes
// ---------------------------------------------------------------------------

#[test]
fn every_need_returns_a_measured_share_of_the_default_response() {
    let page = corpus();

    // need, the lowest share that still counts as this row working, and the
    // highest. A row of the plan table breaking moves the number out of the
    // band in one direction or the other.
    let expected: &[(&str, Need, f64, f64)] = &[
        ("text", Need::Text, 0.20, 0.24),
        ("markdown", Need::Markdown, 0.21, 0.25),
        ("links", Need::Links, 0.045, 0.07),
        ("html", Need::Html, 0.45, 0.50),
        ("metadata", Need::Metadata, 0.02, 0.05),
        (
            "fields",
            Need::fields([("price", ".price"), ("title", "h1")]),
            0.012,
            0.028,
        ),
    ];

    for (label, need, low, high) in expected {
        let measured = share(&page, need);
        assert!(
            (*low..=*high).contains(&measured),
            "{label} returned {measured:.4} of the default, expected between {low} and {high}"
        );
    }
}

#[test]
fn asking_for_fields_is_the_cheapest_thing_the_crate_can_ask_for() {
    let page = corpus();
    let fields = share(&page, &Need::fields([("price", ".price"), ("title", "h1")]));

    for other in [Need::Text, Need::Markdown, Need::Html, Need::Metadata] {
        assert!(
            fields < share(&page, &other),
            "{} was cheaper than fields",
            other.label()
        );
    }
    // Fewer than one byte in thirty of what a default request returns.
    assert!(fields < 0.033, "fields returned {fields:.4}");
}

#[test]
fn a_crawl_loses_most_of_its_bytes_to_the_furniture_it_repeats() {
    let pages = crawl();
    let mut trimmer = Trimmer::new(TrimSettings::default());
    trimmer.learn(&pages);

    let before: usize = pages.iter().map(String::len).sum();
    let after: usize = pages
        .iter()
        .map(|page| trimmer.trim(page, None).text.len())
        .sum();
    let kept = after as f64 / before as f64;

    assert!(
        (0.30..=0.45).contains(&kept),
        "a six page crawl kept {kept:.4} of itself"
    );
    // The nav and the footer are gone from every page, and the part that made
    // each page different is still there.
    for (index, page) in pages.iter().enumerate() {
        let trimmed = trimmer.trim(page, None).text;
        assert!(
            !trimmed.contains("Privacy notice"),
            "page {index}: {trimmed}"
        );
        assert!(!trimmed.contains("Cookie settings"), "page {index}");
        assert!(!trimmed.contains("Sign in"), "page {index}");
    }
    assert!(trimmer.trim(&pages[2], None).text.contains("Nylon"));
    assert!(trimmer.trim(&pages[4], None).text.contains("bronze"));
}

#[test]
fn the_sixty_per_cent_line_decides_what_counts_as_furniture() {
    let pages = crawl();
    let mut trimmer = Trimmer::new(TrimSettings::default());
    trimmer.learn(&pages);

    // The trade banner is on four of the six pages, which is over the line.
    let with_banner = trimmer.trim(&pages[1], None).text;
    assert!(
        !with_banner.contains("Trade accounts"),
        "the banner survived: {with_banner}"
    );
    assert!(!with_banner.contains("Carriage is free"));

    // The clearance notice is on three, which is under it, so it stays.
    assert!(
        with_banner.contains("Clearance"),
        "the clearance notice went: {with_banner}"
    );
    assert!(with_banner.contains("Sold as seen"));
}

#[test]
fn a_repeated_line_that_is_not_part_of_a_block_survives() {
    let pages = crawl();
    let mut trimmer = Trimmer::new(TrimSettings::default());
    trimmer.learn(&pages);

    // This line is on every page of the crawl, but the lines either side of it
    // differ page to page, so it is a sentence they have in common rather than
    // a block of furniture. Dropping it would cost the caller a fact.
    for (index, page) in pages.iter().enumerate() {
        let trimmed = trimmer.trim(page, None).text;
        assert!(
            trimmed.contains("Ships the same day"),
            "page {index} lost a line it was the only one holding in that place"
        );
    }
}

// ---------------------------------------------------------------------------
// the plan table, row by row
// ---------------------------------------------------------------------------

/// One row of the plan table, written out by hand so the crate's own answer has
/// something independent to be compared against.
struct Row {
    need: Need,
    format: Option<ReturnFormat>,
    readability: Option<bool>,
    clean_html: Option<bool>,
    endpoint: Option<Endpoint>,
}

fn row(
    need: Need,
    format: Option<ReturnFormat>,
    readability: Option<bool>,
    clean_html: Option<bool>,
    endpoint: Option<Endpoint>,
) -> Row {
    Row {
        need,
        format,
        readability,
        clean_html,
        endpoint,
    }
}

#[test]
fn each_row_of_the_plan_table_sends_what_it_says_it_sends() {
    use ReturnFormat::{Bytes, Empty, Markdown, Raw, Text};

    let table = [
        row(Need::Text, Some(Text), Some(true), Some(true), None),
        row(Need::Markdown, Some(Markdown), Some(true), Some(true), None),
        row(Need::Html, Some(Raw), None, Some(true), None),
        row(Need::Links, Some(Empty), None, None, Some(Endpoint::Links)),
        row(Need::Metadata, Some(Empty), None, None, None),
        row(
            Need::fields([("price", ".price")]),
            Some(Empty),
            None,
            None,
            None,
        ),
        row(
            Need::screenshot(),
            Some(Bytes),
            None,
            None,
            Some(Endpoint::Screenshot),
        ),
        row(Need::Raw, None, None, None, None),
    ];

    for Row {
        need,
        format,
        readability,
        clean_html,
        endpoint,
    } in table
    {
        let plan = Plan::for_need(&need);
        assert_eq!(plan.return_format, format, "{} format", need.label());
        assert_eq!(
            plan.readability,
            readability,
            "{} readability",
            need.label()
        );
        assert_eq!(plan.clean_html, clean_html, "{} clean_html", need.label());
        assert_eq!(plan.endpoint, endpoint, "{} endpoint", need.label());
    }
}

#[test]
fn fields_ask_for_no_page_bytes_at_all() {
    let plan = Plan::for_need(&Need::fields([("price", ".price"), ("title", "h1")]));
    assert_eq!(plan.return_format, Some(ReturnFormat::Empty));
    assert!(plan.returns_no_page());

    let mut params = RequestParams::url("https://example.com/widgets/steel-40");
    plan.apply_over(&mut params, &RequestParams::default());
    assert_eq!(
        params.return_format,
        Some(ReturnFormatHandling::Single(ReturnFormat::Empty))
    );
    let map = params.css_extraction_map.expect("an extraction map");
    let names: Vec<&str> = map
        .values()
        .flatten()
        .map(|group| group.name.as_str())
        .collect();
    assert_eq!(names, vec!["price", "title"]);

    // And the response carries the fields and nothing else worth paying for.
    let page = corpus();
    assert!(wire_bytes(&page, &plan) < page.text.len() / 4);
}

#[test]
fn every_need_but_raw_turns_the_optional_return_fields_off() {
    let cases: &[(Need, &[&str])] = &[
        (Need::Text, &[]),
        (Need::Markdown, &[]),
        (Need::Html, &[]),
        (Need::Links, &["return_page_links"]),
        (Need::Metadata, &["metadata"]),
        (Need::fields([("price", ".price")]), &[]),
        (Need::screenshot(), &[]),
    ];

    for (need, asked_for) in cases {
        let mut params = RequestParams::url("https://example.com");
        Plan::for_need(need).apply_over(&mut params, &RequestParams::default());

        let set: BTreeMap<&str, Option<bool>> = BTreeMap::from([
            ("metadata", params.metadata),
            ("return_headers", params.return_headers),
            ("return_cookies", params.return_cookies),
            ("return_page_links", params.return_page_links),
            ("return_json_data", params.return_json_data),
            ("return_embeddings", params.return_embeddings),
        ]);
        assert_eq!(set.len(), OPTIONAL_RETURNS.len());

        for (name, value) in set {
            let wanted = asked_for.contains(&name);
            assert_eq!(
                value,
                Some(wanted),
                "{} left {name} at {value:?}",
                need.label()
            );
        }
    }

    // Raw is the one that leaves the service's own defaults alone.
    let mut params = RequestParams::url("https://example.com");
    Plan::for_need(&Need::Raw).apply_over(&mut params, &RequestParams::default());
    for value in [
        params.metadata,
        params.return_headers,
        params.return_cookies,
        params.return_page_links,
        params.return_json_data,
        params.return_embeddings,
    ] {
        assert_eq!(value, None);
    }
}

#[test]
fn a_setting_the_caller_made_by_hand_beats_the_plan() {
    let mut caller = RequestParams::url("https://example.com");
    caller.return_format = Some(ReturnFormatHandling::Multi(vec![
        ReturnFormat::Markdown,
        ReturnFormat::Raw,
    ]));
    caller.return_headers = Some(true);
    caller.readability = Some(false);

    let mut params = caller.clone();
    Plan::for_need(&Need::Text).apply_over(&mut params, &caller);

    assert_eq!(params.return_format, caller.return_format);
    assert_eq!(params.return_headers, Some(true));
    assert_eq!(params.readability, Some(false));
    // The fields the caller said nothing about are still the plan's.
    assert_eq!(params.clean_html, Some(true));
    assert_eq!(params.return_cookies, Some(false));
}

#[test]
fn the_plan_survives_every_step_of_the_escalation_ladder() {
    let caller = RequestParams::url("https://example.com");
    let plan = Plan::for_need(&Need::Markdown);
    let mut params = caller.clone();
    plan.apply_over(&mut params, &caller);

    let ladder = Ladder::standard();
    let steps: Vec<Step> = ladder.0.clone();
    assert!(steps.len() > 2, "the ladder should have rungs to walk");

    for step in steps {
        // This is the pair the send loop runs: the step patches the request,
        // then the plan is written over what it left.
        step.apply(&mut params);
        plan.apply_over(&mut params, &caller);

        assert_eq!(
            params.return_format,
            Some(ReturnFormatHandling::Single(ReturnFormat::Markdown)),
            "the {} step changed the return format",
            step.label
        );
        assert_eq!(params.metadata, Some(false), "after {}", step.label);
        assert_eq!(
            params.return_embeddings,
            Some(false),
            "after {}",
            step.label
        );
    }
}

#[test]
fn a_step_that_asks_for_the_whole_page_again_does_not_get_it() {
    let caller = RequestParams::url("https://example.com");
    let plan = Plan::for_need(&Need::Markdown);
    let mut params = caller.clone();
    plan.apply_over(&mut params, &caller);

    // A rung added later that reaches for the markup, or a service side
    // default written onto the request by anything else in the loop.
    params.return_format = Some(ReturnFormatHandling::Single(ReturnFormat::Raw));
    params.return_embeddings = Some(true);
    plan.apply_over(&mut params, &caller);

    assert_eq!(
        params.return_format,
        Some(ReturnFormatHandling::Single(ReturnFormat::Markdown))
    );
    assert_eq!(params.return_embeddings, Some(false));
}

// ---------------------------------------------------------------------------
// trimming what still arrives
// ---------------------------------------------------------------------------

#[test]
fn boilerplate_goes_and_the_page_that_carried_it_still_reads() {
    let pages = crawl();
    let mut trimmer = Trimmer::new(TrimSettings::default());
    trimmer.learn(&pages);
    assert!(trimmer.knows_boilerplate());

    let trimmed = trimmer.trim(&pages[0], None).text;
    assert!(trimmed.contains("Steel widget"));
    assert!(trimmed.contains("nine newton metres"));
    assert!(!trimmed.contains("Registered in England"));
    assert!(!trimmed.contains("Delivery is charged at checkout"));

    // A line that belongs to one page only is not furniture, however common
    // its neighbours are.
    assert!(trimmer.trim(&pages[5], None).text.contains("fourteen"));
}

#[test]
fn truncation_lands_on_a_sentence_end_and_says_what_it_dropped() {
    let pages = crawl();
    let (out, dropped) = truncate_to_tokens(&pages[0], 360);

    assert!(dropped > 0);
    let kept = out.split("\n...[truncated").next().expect("the kept text");
    assert!(
        pages[0].starts_with(kept),
        "what was kept is not a prefix of the page"
    );
    let last = kept.trim_end();
    assert!(
        last.ends_with('.') || last.ends_with(')'),
        "cut mid sentence: {:?}",
        &last[last.len().saturating_sub(40)..]
    );
    assert!(out.ends_with(&format!("...[truncated {dropped} tokens]")));
    // The ceiling is the number the budget is enforced in, so it is the one
    // that has to hold.
    assert!(max_tokens(&out) <= 360, "{}", max_tokens(&out));
}

#[test]
fn a_token_budget_is_shared_out_in_proportion_to_what_each_page_costs() {
    let pages = crawl();
    let costs: Vec<usize> = pages.iter().map(|page| approx_tokens(page)).collect();

    let total: usize = costs.iter().sum();
    let budget = TokenBudget::total(total / 2);
    let shares = budget.split(&costs);

    assert_eq!(shares.iter().sum::<usize>(), total / 2);
    // A page that costs more gets more, in the same order.
    let mut by_cost: Vec<usize> = (0..costs.len()).collect();
    by_cost.sort_by_key(|index| costs[*index]);
    for pair in by_cost.windows(2) {
        // A token either way is the rounding that makes the shares add up.
        assert!(
            shares[pair[0]] <= shares[pair[1]] + 1,
            "the cheaper page got the larger share"
        );
    }
    // Each share is within a token of half its page, which is what
    // proportional means when the total is half the corpus.
    for (share, cost) in shares.iter().zip(&costs) {
        let expected = cost / 2;
        assert!(
            share.abs_diff(expected) <= 2,
            "{share} is not close to {expected}"
        );
    }
}

#[test]
fn a_crawl_that_fits_is_not_cut_for_the_sake_of_it() {
    let pages = crawl();
    let costs: Vec<usize> = pages.iter().map(|page| approx_tokens(page)).collect();
    let total: usize = costs.iter().sum();

    assert_eq!(TokenBudget::total(total).split(&costs), costs);
    assert_eq!(TokenBudget::total(total * 2).split(&costs), costs);
    assert_eq!(TokenBudget::unlimited().split(&costs), costs);

    // One token short of fitting, and every page gives up a little rather than
    // one page giving up everything.
    let tight = TokenBudget::total(total - 1).split(&costs);
    assert_eq!(tight.iter().sum::<usize>(), total - 1);
    for (share, cost) in tight.iter().zip(&costs) {
        assert!(*share <= *cost);
        assert!(share * 100 >= cost * 95, "{share} is far short of {cost}");
    }
}

#[test]
#[ignore = "prints the measured numbers for the readme"]
fn measured_numbers() {
    let page = corpus();
    let base = baseline(&page);
    println!("baseline bytes {base}");
    for need in [
        Need::Text,
        Need::Markdown,
        Need::Html,
        Need::Metadata,
        Need::fields([("price", ".price"), ("title", "h1")]),
        Need::Links,
    ] {
        let bytes = wire_bytes(&page, &Plan::for_need(&need));
        println!(
            "{:9} {:6} bytes  {:.4} of default  saves {:.1}%",
            need.label(),
            bytes,
            bytes as f64 / base as f64,
            100.0 - 100.0 * bytes as f64 / base as f64
        );
    }
    let pages = crawl();
    let mut trimmer = Trimmer::new(TrimSettings::default());
    trimmer.learn(&pages);
    let before: usize = pages.iter().map(String::len).sum();
    let after: usize = pages.iter().map(|p| trimmer.trim(p, None).text.len()).sum();
    println!(
        "crawl {before} -> {after} bytes, saves {:.1}%",
        100.0 - 100.0 * after as f64 / before as f64
    );
    let t_in: usize = pages.iter().map(|p| approx_tokens(p)).sum();
    let t_out: usize = pages
        .iter()
        .map(|p| approx_tokens(&trimmer.trim(p, None).text))
        .sum();
    println!(
        "crawl tokens {t_in} -> {t_out}, saves {:.1}%",
        100.0 - 100.0 * t_out as f64 / t_in as f64
    );
}

// ---------------------------------------------------------------------------
// what a budget is worth on a page that is not English
// ---------------------------------------------------------------------------

/// One page in each shape the estimator used to read wrong, with the higher of
/// the cl100k_base and o200k_base counts beside it.
///
/// The numbers came from tiktoken. They are here rather than in a comment
/// because a formula change that loses the margin should fail a build, not a
/// caller's request.
const MEASURED: &[(&str, &str, usize)] = &[
    (
        "chinese prose",
        "蜘蛛云是一个网页抓取服务。它可以把任何网页转换成结构化的数据，供大型语言模型使用。\
         我们提供多种返回格式，包括纯文本、Markdown、清理过的 HTML 以及页面链接列表。",
        150,
    ),
    (
        "japanese prose",
        "ウェブページを取得して、必要な部分だけを返すサービスです。大規模言語モデルに渡す前に、\
         ナビゲーションやフッターなどの繰り返される部分を取り除きます。",
        119,
    ),
    (
        "korean prose",
        "웹 페이지를 가져와서 필요한 부분만 돌려주는 서비스입니다. 반환 형식은 텍스트, \
         마크다운, 정리된 HTML, 링크 목록 중에서 고를 수 있습니다.",
        99,
    ),
    (
        "chinese markup",
        "<div class=\"product\"><h1 class=\"title\">不锈钢部件，四十毫米</h1>\
         <span class=\"price\">￥89.00</span></div>",
        75,
    ),
    (
        "arabic prose",
        "خدمة تجلب صفحات الويب وتعيد فقط ما طلبته. الصيغ المتاحة هي النص العادي وماركداون.",
        79,
    ),
    (
        "hindi prose",
        "यह सेवा वेब पेज लाती है और केवल वही लौटाती है जो आपने माँगा था।",
        95,
    ),
    ("thai prose", "บริการนี้ดึงหน้าเว็บและส่งคืนเฉพาะสิ่งที่คุณขอ", 63),
    ("emoji", "🚀🔥✨🎉💡📦🧪🛠️🌍🐍⚡️🎯", 47),
    ("one family emoji", "👨‍👩‍👧‍👦", 14),
    (
        "a page of hashes",
        "a3f9c2e1b7d4508f6a2c9e3b1d7f4085c6a29e3b1d7f4085c6a29e3b1d7f4085",
        54,
    ),
    (
        "a base64 image",
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQ==",
        35,
    ),
    ("acronyms", "HTML XML JSON API URL HTTP TLS SDK CLI", 21),
    (
        "english prose",
        "The quick brown fox jumps over the lazy dog.",
        10,
    ),
];

#[test]
fn the_ceiling_covers_every_page_shape_a_crawler_meets() {
    for (label, text, charged) in MEASURED {
        let ceiling = max_tokens(text);
        assert!(
            ceiling >= *charged,
            "{label}: the ceiling read {ceiling} for text a tokenizer charges {charged} for"
        );
    }
}

#[test]
fn a_chinese_page_no_longer_reads_at_a_quarter_of_what_it_costs() {
    // The bug worth publishing a fix for. Every character of this page is
    // alphanumeric to Rust, so the old estimate divided the lot by four: it
    // read 46 tokens for a page cl100k_base charges 150 for, and a caller who
    // set a budget to fit a context window overflowed it by three times.
    let (_, page, charged) = MEASURED[0];
    assert!(max_tokens(page) >= charged);
    assert!(
        approx_tokens(page) * 4 >= charged,
        "the estimate is still reading a quarter of the cost: {}",
        approx_tokens(page)
    );
}

#[test]
fn the_estimate_never_reads_above_the_ceiling() {
    let pages = crawl();
    let corpus = corpus();
    let texts: Vec<&str> = MEASURED
        .iter()
        .map(|(_, text, _)| *text)
        .chain(pages.iter().map(String::as_str))
        .chain([
            corpus.raw.as_str(),
            corpus.markdown.as_str(),
            "",
            " ",
            "\n\n",
        ])
        .collect();

    for text in texts {
        assert!(
            approx_tokens(text) <= max_tokens(text),
            "the ceiling read below the estimate on {text:?}"
        );
    }
}

#[test]
fn what_comes_back_fits_the_ceiling_whatever_the_page_is_written_in() {
    for (label, text, _) in MEASURED {
        let whole = max_tokens(text);
        for ceiling in 1..=whole.saturating_add(4) {
            let (out, _) = truncate_to_tokens(text, ceiling);
            assert!(
                max_tokens(&out) <= ceiling,
                "{label} at a ceiling of {ceiling} came back costing {}",
                max_tokens(&out)
            );
        }
    }
}

#[test]
fn a_page_with_no_whitespace_in_it_still_comes_back_with_something_in_it() {
    // Minified markup has no spaces, no full stops and no wide characters, so
    // the walk finds no boundary of any kind. Handing the caller an empty
    // string and a report of a perfect saving would cost them the page.
    let minified = "<div class=\"a\"><span id=\"b\">12.50</span><span id=\"c\">GBP</span></div>";
    let (out, dropped) = truncate_to_tokens(minified, max_tokens(minified) / 2);

    assert!(dropped > 0);
    let kept = out.split("\n...[truncated").next().expect("the kept text");
    assert!(!kept.is_empty(), "the whole page was thrown away");
    assert!(minified.starts_with(kept));
}

#[test]
fn a_cut_never_lands_between_a_character_and_a_mark_that_leans_on_it() {
    // Decomposed Japanese writes が as か plus a combining voiced sound mark,
    // and か is a character the trimmer is otherwise happy to break after.
    // Cutting there changes the syllable rather than shortening the page.
    let cases = [
        "\u{304b}\u{3099}\u{3093}\u{305f}\u{3099}\u{3080}\u{3000}\u{30cf}\u{309a}\u{30f3}\u{30bf}\u{3099}",
        "\u{ff76}\u{ff9e}\u{ff9d}\u{ff80}\u{ff9e}\u{ff91}",
        "Shipped 👨\u{200d}👩\u{200d}👧\u{200d}👦 and 🧑\u{1f3fd}\u{200d}🚒 today, twice.",
        "\u{3030}\u{fe0f}\u{3030}\u{fe0f}\u{3030}\u{fe0f} waves",
    ];

    for text in cases {
        for ceiling in 1..=(max_tokens(text) + 4) {
            let (out, _) = truncate_to_tokens(text, ceiling);
            let kept = out.split("\n...[truncated").next().expect("the kept text");
            let orphaned = text
                .get(kept.len()..)
                .and_then(|rest| rest.chars().next())
                .map(hangs_on_what_came_before)
                .unwrap_or(false);
            assert!(
                !orphaned,
                "a ceiling of {ceiling} cut {text:?} in front of a combining mark"
            );
        }
    }
}

/// The marks the trimmer must never cut in front of, written out here so the
/// test checks the rule rather than the implementation of the rule.
fn hangs_on_what_came_before(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036f}'
        | '\u{200d}'
        | '\u{3099}'..='\u{309a}'
        | '\u{fe00}'..='\u{fe0f}'
        | '\u{ff9e}'..='\u{ff9f}'
        | '\u{1f3fb}'..='\u{1f3ff}'
    )
}

#[test]
fn a_crawl_of_chinese_pages_stays_inside_the_total_it_was_given() {
    // The failure the crate would have shipped: six pages of Chinese, a total
    // a caller sized to a context window, and a set of shares worked out from
    // an estimate that read a quarter of the truth.
    let pages: Vec<String> = (0..6)
        .map(|n| {
            format!(
                "第{n}页。蜘蛛云是一个网页抓取服务。它可以把任何网页转换成结构化的数据，\
                 供大型语言模型使用。我们提供多种返回格式，包括纯文本和 Markdown。"
            )
        })
        .collect();

    let costs: Vec<usize> = pages.iter().map(|page| max_tokens(page)).collect();
    let total = costs.iter().sum::<usize>() / 2;
    let shares = TokenBudget::total(total).split(&costs);

    let spent: usize = pages
        .iter()
        .zip(&shares)
        .map(|(page, share)| max_tokens(&truncate_to_tokens(page, *share).0))
        .sum();

    assert!(spent <= total, "{spent} tokens against a total of {total}");
}

#[test]
fn a_budget_a_caller_set_holds_on_a_page_of_nothing_but_ideographs() {
    // cl100k_base charges two tokens for each of these and the old estimate
    // charged a quarter of one, so a ceiling of 100 bought 1600 real tokens.
    let page = "漢".repeat(200);
    assert!(max_tokens(&page) >= 400, "{}", max_tokens(&page));

    let (out, dropped) = truncate_to_tokens(&page, 100);
    assert!(dropped > 0);
    assert!(max_tokens(&out) <= 100);
    assert!(out.chars().filter(|c| *c == '漢').count() <= 60);
}
