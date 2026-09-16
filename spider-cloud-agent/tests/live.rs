// Integration tests are their own crate root, so the allow blocks inside the
// library's test modules do not reach here. A test that cannot set itself up
// should stop loudly rather than quietly measure nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::string_slice
)]

//! The calls that only the real service can answer.
//!
//! Every test here spends credits, so each one is ignored by default and
//! `scripts/verify-live.sh --release` runs the target with `--ignored`. The fixtures
//! in `tests/fixtures` prove only that the crate reads what it was told the wire
//! sends. Every case below pins a fix that no recorded fixture could have
//! prompted, because the recording was of the wrong shape.
//!
//! One of them, `live_the_service_answers_with_the_field_names_the_request_asked_for`,
//! is red on purpose. It is the only assertion here about the service rather
//! than about the crate, and it stays until the extraction cache starts keying
//! on the selector map.
//!
//! The release script requires a key, an explicit API URL and a service revision.
//! Direct runs without a key fail rather than report success without a request.
//!
//! The target is `example.com` throughout. It is the cheapest page the service
//! can be asked for, it does not change, and it is the only sort of host allowed
//! to appear anywhere in this repo.

use spider_cloud_agent::ops::transform::Document;
use spider_cloud_agent::params::ReturnFormat;
use spider_cloud_agent::policy::Budget;
use spider_cloud_agent::{Body, Credits, Need, Spider};

/// The page every test here asks for.
const TARGET: &str = "https://example.com";

/// A client, or nothing when this machine has no key.
///
/// The key is read out of the environment by the crate's own credential store,
/// which is the same path a caller takes. Nothing here prints it, and nothing
/// here writes it anywhere.
fn spider() -> Option<Spider> {
    if std::env::var("SPIDER_API_KEY").ok()?.trim().is_empty() {
        return None;
    }
    Some(Spider::new().expect("a key was found, so a client builds"))
}

/// An explicitly requested live test must have credentials.
macro_rules! client {
    () => {
        match spider() {
            Some(spider) => spider,
            None => {
                panic!("live tests require a non-empty SPIDER_API_KEY");
            }
        }
    };
}

#[tokio::test]
#[ignore = "calls the live api and spends credits"]
async fn live_the_balance_arrives_as_a_quoted_decimal_and_reads_as_a_number() {
    let spider = client!();

    let credits = spider.credits().await.expect("the balance");

    // The service sends `{"data":{"credits":"15442499.281586"}}`. Reading only
    // JSON numbers here failed against a real account and blamed a missing
    // field, which is the error that sends you looking in the wrong place. A
    // balance of exactly zero would pass every other assertion in this file, so
    // this one asserts the amount is above it.
    assert!(
        credits.0 > 0.0,
        "the balance read as {credits:?}, which is what a misread quoted decimal looks like"
    );
    assert!(credits.0.is_finite(), "the balance read as {credits:?}");
}

#[tokio::test]
#[ignore = "calls the live api and spends credits"]
async fn live_a_plain_fetch_reads_both_status_planes_and_the_content() {
    let spider = client!();

    let page = spider
        .scrape(TARGET)
        .need(Need::Markdown)
        .send()
        .await
        .expect("the page");

    // The call plane and the target plane are separate answers and the crate
    // keeps them apart. A fetch that worked is 200 on both.
    assert!(page.status.is_ok(), "the site answered {:?}", page.status);
    assert_eq!(
        page.attempts[0].api.code(),
        200,
        "the call to the service failed"
    );

    let text = page.text().expect("markdown is text");
    assert!(
        text.contains("Example Domain"),
        "the page came back without its heading: {text:?}"
    );
    assert!(
        page.cost.0 > 0.0,
        "a fetch that cost nothing did not happen"
    );
}

/// The two unit bugs, against the service that has the answer.
///
/// A fetch reports its cost in dollars and the balance is kept in credits, so
/// reading the first as the second understated every cost by a factor of ten
/// thousand and no credit budget could trip. A fixture cannot catch that,
/// because a fixture is only ever as right as whoever wrote it.
#[tokio::test]
#[ignore = "calls the live api and spends credits"]
async fn live_a_reported_cost_is_in_credits_and_a_credit_budget_reaches_the_service() {
    let spider = client!();

    // A float here used to come back as "Deserialization error:
    // max_credits_allowed: invalid type: floating point `30.0`, expected u64",
    // which failed every capped request.
    let page = spider
        .scrape(TARGET)
        .need(Need::Markdown)
        .budget(Budget::default().with_credits(Credits::new(30.0)))
        .send()
        .await
        .expect("a budgeted fetch is not a 400");

    assert!(page.status.is_ok(), "the site answered {:?}", page.status);

    // Measured on 2026-09-15, this page ran 0.027 to 0.191 credits. The window
    // is wide because the price moves, and narrow enough that reading dollars
    // as credits lands four orders of magnitude below it.
    let cost = page.cost.get();
    assert!(
        (0.001..100.0).contains(&cost),
        "a cost of {cost} credits is not a price, it is a unit bug"
    );
}

#[tokio::test]
#[ignore = "calls the live api and spends credits"]
async fn live_named_selectors_come_back_in_the_body_and_not_in_the_metadata() {
    let spider = client!();

    let page = spider
        .scrape(TARGET)
        .need(Need::fields([("heading", "h1"), ("paragraph", "p")]))
        .send()
        .await
        .expect("the page");

    // Extractions arrive in a top level `css_extracted` field. The metadata
    // block has an `extracted_data` field that stays null, which is the obvious
    // place to look and the wrong one. This is the assertion that keeps the
    // transport reading the right one.
    let Body::Fields(fields) = &page.body else {
        panic!("the extractions did not reach the body: {:?}", page.body);
    };
    assert!(
        !fields.is_empty(),
        "an extraction request came back with no fields at all"
    );
    assert!(
        fields
            .values()
            .any(|value| value.as_array().is_some_and(|matches| !matches.is_empty())),
        "every field came back empty: {fields:?}"
    );
    // Which names arrive is the service's half of this and it is currently
    // broken. That is asserted on its own below, so a failure there cannot be
    // mistaken for the transport reading the wrong field.
    if let Some(metadata) = &page.metadata {
        assert!(
            metadata.extracted_data.is_none(),
            "extracted_data started carrying the fields, so the transport can read it"
        );
    }
}

#[tokio::test]
#[ignore = "calls the live api, spends credits, and fails until the service is fixed"]
async fn live_the_service_answers_with_the_field_names_the_request_asked_for() {
    let spider = client!();

    let page = spider
        .scrape(TARGET)
        .need(Need::fields([("heading", "h1"), ("paragraph", "p")]))
        .send()
        .await
        .expect("the page");

    let Body::Fields(fields) = &page.body else {
        panic!("the extractions did not reach the body: {:?}", page.body);
    };

    // This one is about the service and not about the crate. The extraction
    // result for an address is cached without the selector map in the key, so a
    // request gets back whatever map was last computed for that address. Five
    // of eight identical requests came back under names the request never sent,
    // one set of which this account had never sent at all. Reproduced with curl
    // on 2026-09-15, so nothing in this crate is involved.
    //
    // Nothing the client can do fixes it, and a client that papered over it
    // would hand a caller fields under names they cannot look up.
    let asked: Vec<&str> = vec!["heading", "paragraph"];
    let got: Vec<&str> = fields.keys().map(String::as_str).collect();
    assert_eq!(
        got, asked,
        "the service answered under names the request never sent, which is the \
         extraction cache ignoring the selector map"
    );
    assert!(
        fields["heading"].to_string().contains("Example Domain"),
        "the heading selector matched nothing: {:?}",
        fields["heading"]
    );
}

#[tokio::test]
#[ignore = "calls the live api and spends credits"]
async fn live_a_request_that_declined_a_body_does_not_climb_the_ladder() {
    let spider = client!();

    let page = spider
        .scrape(TARGET)
        .need(Need::Metadata)
        .send()
        .await
        .expect("the metadata");

    // A need that asks for metadata sends `return_format=empty`, so the page
    // arrives with nothing in it on purpose. Reading that as a blank page sent
    // the walk up the whole ladder against a request that had already
    // succeeded, and billed every rung. The attempt count is the assertion that
    // matters: the result alone looked fine while this bug was live.
    assert_eq!(
        page.attempt_count(),
        1,
        "one call was enough and the client made {}: {:?}",
        page.attempt_count(),
        page.attempts
    );
    assert!(!page.escalated());

    let metadata = page.metadata.as_ref().expect("the metadata block");
    assert_eq!(metadata.domain.as_deref(), Some("example.com"));
    assert_eq!(metadata.pathname.as_deref(), Some("/"));
    assert!(page.is_blank(), "a body arrived that nobody asked for");
}

#[tokio::test]
#[ignore = "calls the live api and spends credits"]
async fn live_the_links_endpoint_answers_in_one_call() {
    let spider = client!();

    let outcome = spider.links(TARGET).send().await.expect("the links");

    // This endpoint returns addresses and no page, which read as a blank page
    // and sent the walk up the whole ladder. Measured on 2026-09-15 before the
    // fix: five attempts, 164 s and 0.4446 credits, ending in an error, for
    // links the first attempt had returned in 820 ms for 0.0087. The count is
    // the assertion, because the links came back either way.
    assert_eq!(
        outcome.attempts.len(),
        1,
        "one call was enough and the client made {}: {:?}",
        outcome.attempts.len(),
        outcome.attempts
    );
    assert!(
        !outcome.value.is_empty(),
        "the links endpoint answered with no addresses at all"
    );
}

#[tokio::test]
#[ignore = "calls the live api and spends credits"]
async fn live_a_converted_document_reads_back_as_text() {
    let spider = client!();

    let page = spider
        .transform(vec![Document::html("<h1>Title</h1><p>Body text.</p>")])
        .format(ReturnFormat::Markdown)
        .send()
        .await
        .expect("the converted document");

    // The endpoint answers `{"content":["# Title\nBody text."]}`, one string per
    // document sent. Read as a page body that list asked for bytes, so every
    // transform came back as `invalid type: string, expected u8` and the
    // endpoint could not be used at all.
    assert_eq!(
        page.text(),
        Some("# Title\nBody text."),
        "the conversion came back as {:?}",
        page.body
    );
}

#[tokio::test]
#[ignore = "calls the live api and spends credits"]
async fn live_a_screenshot_arrives_as_a_picture_and_not_as_base64() {
    let spider = client!();

    let page = spider.screenshot(TARGET).send().await.expect("the picture");

    // Asked for nothing in particular this endpoint sends the image as base64
    // text, so the body was a 22,960 character string and a caller writing it
    // to a file got base64 in something named `.png`. Asking for bytes is what
    // makes it arrive as bytes, which costs no dependency and no decoding.
    let Body::Screenshot(image) = &page.body else {
        panic!("a picture arrived as {:?}", page.body);
    };
    assert_eq!(
        &image[..4.min(image.len())],
        b"\x89PNG",
        "what came back is not a png"
    );
}
