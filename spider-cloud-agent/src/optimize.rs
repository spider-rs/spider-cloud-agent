//! The parameter optimizer, on the first attempt of an operation.
//!
//! The router picks the mode, the pool and the wait, and the need's plan
//! switches off what the caller did not ask for. After both, and before the
//! first call, an [`Optimizer`] lists the valid edits to the learnable request
//! fields, scores them through a [`Scorer`], and picks at most one set that
//! clears the [`Gate`]. With [`NoModel`], the only scorer shipped, every
//! request is kept.
//!
//! What happens to the pick depends on the [`ApplyMode`]. In
//! [`ApplyMode::Shadow`] nothing is written and the request goes out exactly as
//! it would with no optimizer. In [`ApplyMode::Apply`] the edits are written
//! onto every field the caller left unset.
//!
//! Three rules hold whatever the scorer says.
//!
//! - A caller who fixed the mode, the pool or the country is not asked about at
//!   all, the same guard the explorer keeps, and a field the caller set is never
//!   edited.
//! - Only the first attempt is edited. The escalation ladder is untouched: a
//!   rung writes over the mode, the pool and the wait as it always has, and
//!   leaves a `network_blacklist` edit where it was.
//! - Nothing is switched on to observe a page. A blacklist edit needs a
//!   resource summary, and that comes from a [`ResourceSource`] the caller or a
//!   collector supplied. `event_tracker` is never set on the caller's behalf,
//!   because it changes the size and the cost of the answer.
//!
//! # Comparison rows
//!
//! With a [`ComparisonRecorder`] set, every operation the optimizer was asked
//! about writes one row when its walk settles, whether it was accepted or
//! stopped. A walk cut short by the wall, the budget or a failed connection
//! writes none, because what it cost is not known. A row holds numbers, buckets
//! and labels only: no url, no host and no body. See
//! [`spider_optimize::ComparisonRow`].
//!
//! ```no_run
//! # fn run() -> std::io::Result<()> {
//! use spider_cloud_agent::optimize::{JsonlComparisonRecorder, NoModel, Optimizer};
//! use spider_cloud_agent::Spider;
//!
//! let spider = Spider::builder()
//!     .optimizer(Optimizer::shadow(NoModel))
//!     .comparison_recorder(JsonlComparisonRecorder::create("pairs.jsonl")?.with_salt(7))
//!     .build();
//! # let _ = spider;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use spider_optimize::{
    choose, comparison_row, featurize_edit, generate, Arm, Candidate, ComparisonRow, Context,
    EditDescriptor, EditFeatures, Key, MemoryState, Observation, Params, MULTIPLIERS,
};
use spider_route::features::extension_of;
use spider_route::{
    featurize, host_shape, registrable_domain, AttemptOutcome, DeclaredNeed, ExtClass,
    FeatureVector, ProxyPool, RequestMode, RouteDecision, RouteInput, StatusClass,
};
use url::Url;

pub use spider_optimize::{
    summarize, Choice, EditSet, Gate, NoModel, Reason, ResourceSummary, Schema, Scorer,
};

use crate::client::{Spider, SpiderBuilder};
use crate::credits::Credits;
use crate::ops::Call;
use crate::params::{RequestParams, Timeout, WaitFor};
use crate::policy::{Budget, Next};
use crate::response::{Attempt, Pages};

/// Whether a chosen edit is written onto the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// Score and record, and send the request unchanged.
    Shadow,
    /// Write the chosen edits onto the fields the caller left unset.
    Apply,
}

/// Which day a comparison row belongs to, as a count the caller chooses.
///
/// Rows are split into training and test sets by day, and neither this crate
/// nor the optimizer reads a clock. With none set, every row is day zero.
pub trait Clock: Send + Sync {
    /// Today, as the caller counts days.
    fn day(&self) -> u32;
}

/// What a previous rendered attempt loaded on a site, for a blacklist edit.
///
/// The client never asks the service for this. A caller or a collector that
/// asked for `event_tracker` on its own requests summarizes what came back
/// with [`summarize`] and hands it over here.
pub trait ResourceSource: Send + Sync {
    /// The summary for the page about to be fetched, when there is one.
    fn resources(&self, url: &Url) -> Option<ResourceSummary>;
}

/// A scorer, the thresholds it has to clear, and what to do with its pick.
///
/// Cheap to clone: the scorer and the sources are shared.
#[derive(Clone)]
pub struct Optimizer {
    scorer: Arc<dyn Scorer>,
    gate: Gate,
    mode: ApplyMode,
    schema: Schema,
    clock: Option<Arc<dyn Clock>>,
    resources: Option<Arc<dyn ResourceSource>>,
}

impl fmt::Debug for Optimizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Optimizer")
            .field("model", &self.scorer.version())
            .field("gate", &self.gate)
            .field("mode", &self.mode)
            .field("clock", &self.clock.is_some())
            .field("resources", &self.resources.is_some())
            .finish()
    }
}

impl Optimizer {
    /// An optimizer with this scorer, these thresholds and this mode.
    pub fn new(scorer: impl Scorer + 'static, gate: Gate, mode: ApplyMode) -> Optimizer {
        Optimizer {
            scorer: Arc::new(scorer),
            gate,
            mode,
            schema: Schema::v1(),
            clock: None,
            resources: None,
        }
    }

    /// An optimizer that scores and records under the default gate, and
    /// changes nothing.
    pub fn shadow(scorer: impl Scorer + 'static) -> Optimizer {
        Optimizer::new(scorer, Gate::default(), ApplyMode::Shadow)
    }

    /// Whether a pick is written onto the request.
    pub fn mode(&self) -> ApplyMode {
        self.mode
    }

    /// The thresholds a candidate has to clear.
    pub fn gate(&self) -> Gate {
        self.gate
    }

    /// Stamp comparison rows with the day this clock reads.
    pub fn with_clock(mut self, clock: impl Clock + 'static) -> Optimizer {
        self.clock = Some(Arc::new(clock));
        self
    }

    /// Read resource summaries for blacklist edits from here.
    pub fn with_resources(mut self, source: impl ResourceSource + 'static) -> Optimizer {
        self.resources = Some(Arc::new(source));
        self
    }

    /// List the candidates for one request and pick one, writing nothing.
    ///
    /// This is the whole decision the send loop makes, without the loop.
    pub fn decide(&self, ctx: &Context<'_>) -> DecisionLog {
        let candidates = generate(&self.schema, ctx);
        let choice = choose(self.scorer.as_ref(), ctx, &candidates, &self.gate);
        DecisionLog {
            choice,
            candidates: candidates.len().min(usize::from(u8::MAX)) as u8,
            applied: false,
        }
    }
}

/// What the optimizer decided for one request.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionLog {
    /// The pick, or the reason the request was kept.
    pub choice: Choice,
    /// How many candidates were listed, keep included.
    pub candidates: u8,
    /// Whether an edit was written onto the request.
    pub applied: bool,
}

/// Takes one comparison row per operation the optimizer was asked about.
///
/// Called on the calling task when the walk settles, so an implementation
/// should return quickly or hand the row somewhere else.
pub trait ComparisonRecorder: Send + Sync {
    /// One row, as a single line of JSON with no trailing newline.
    fn observe(&self, row: &str);

    /// The salt mixed into every row's site key, so keys from two collectors
    /// cannot be joined. Zero unless the recorder says otherwise.
    fn salt(&self) -> u64 {
        0
    }
}

impl<T: ComparisonRecorder + ?Sized> ComparisonRecorder for Arc<T> {
    fn observe(&self, row: &str) {
        (**self).observe(row);
    }

    fn salt(&self) -> u64 {
        (**self).salt()
    }
}

impl<T: ComparisonRecorder + ?Sized> ComparisonRecorder for Box<T> {
    fn observe(&self, row: &str) {
        (**self).observe(row);
    }

    fn salt(&self) -> u64 {
        (**self).salt()
    }
}

/// Writes one comparison row per line.
///
/// The same shape as [`crate::record::JsonlRecorder`], for the same reason: the
/// writer is held by shared reference, so rows are written from any task with
/// no lock, and appending a row to a [`File`] is one write.
pub struct JsonlComparisonRecorder<W> {
    writer: W,
    salt: u64,
}

impl JsonlComparisonRecorder<File> {
    /// Append rows to a file, creating it if it is not there.
    pub fn create(path: impl AsRef<Path>) -> std::io::Result<JsonlComparisonRecorder<File>> {
        let file = File::options().create(true).append(true).open(path)?;
        Ok(JsonlComparisonRecorder::new(file))
    }
}

impl<W> JsonlComparisonRecorder<W> {
    /// Write rows to this writer, with no salt.
    pub fn new(writer: W) -> JsonlComparisonRecorder<W> {
        JsonlComparisonRecorder { writer, salt: 0 }
    }

    /// Mix this salt into every site key.
    pub fn with_salt(mut self, salt: u64) -> JsonlComparisonRecorder<W> {
        self.salt = salt;
        self
    }

    /// The writer, back again.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W> fmt::Debug for JsonlComparisonRecorder<W> {
    /// Says what it is, and nothing about where it points or its salt.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JsonlComparisonRecorder")
    }
}

impl<W> ComparisonRecorder for JsonlComparisonRecorder<W>
where
    W: Send + Sync,
    for<'a> &'a W: Write,
{
    fn observe(&self, row: &str) {
        let mut line = String::with_capacity(row.len() + 1);
        line.push_str(row);
        line.push('\n');

        // A row that cannot be written is dropped, as a routing row is.
        let mut writer = &self.writer;
        if let Err(error) = writer.write_all(line.as_bytes()) {
            log::debug!("a comparison row was not written: {error}");
        }
    }

    fn salt(&self) -> u64 {
        self.salt
    }
}

/// The optimizer and the row writer a client carries.
#[derive(Clone, Default)]
pub(crate) struct Hooks {
    optimizer: Option<Arc<Optimizer>>,
    comparison: Option<Arc<dyn ComparisonRecorder>>,
}

impl fmt::Debug for Hooks {
    /// Presence only: a scorer or a writer is not something a log needs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let presence = |set: bool| if set { "set" } else { "none" };
        f.debug_struct("Optimize")
            .field("optimizer", &presence(self.optimizer.is_some()))
            .field("comparison", &presence(self.comparison.is_some()))
            .finish()
    }
}

impl SpiderBuilder {
    /// Run this optimizer on the first attempt of every operation.
    ///
    /// Nothing runs unless you set one. A request that fixes the mode, the pool
    /// or the country is never passed to it.
    pub fn optimizer(mut self, optimizer: Optimizer) -> SpiderBuilder {
        self.optimize.optimizer = Some(Arc::new(optimizer));
        self
    }

    /// Where to write a comparison row for every optimized operation.
    ///
    /// A row holds no address: see [`crate::optimize`].
    pub fn comparison_recorder(
        mut self,
        recorder: impl ComparisonRecorder + 'static,
    ) -> SpiderBuilder {
        self.optimize.comparison = Some(Arc::new(recorder));
        self
    }
}

impl Spider {
    /// The optimizer operations consult, when one was set.
    pub fn optimizer(&self) -> Option<&Optimizer> {
        self.optimize.optimizer.as_deref()
    }

    /// Where comparison rows are written, when anywhere.
    pub fn comparison_recorder(&self) -> Option<&dyn ComparisonRecorder> {
        self.optimize.comparison.as_deref()
    }
}

/// What the send loop keeps between the decision and the row.
///
/// Everything a row needs that the request will not hold by the time the walk
/// settles, because the ladder writes over it.
pub(crate) struct Pending {
    log: DecisionLog,
    arm: Arm,
    need: DeclaredNeed,
    ext: ExtClass,
    tld: u8,
    memory: MemoryState,
    routed: RouteDecision,
    edit: Option<EditDescriptor>,
    edit_feats: EditFeatures,
    base: FeatureVector,
    pinned: (u32, u64),
    multiplier: f32,
    site: u64,
    day: u32,
}

/// Ask the optimizer about the request as it is about to go out, and in
/// [`ApplyMode::Apply`] write its pick.
///
/// `None`, with the optimizer never called, when there is no optimizer or the
/// caller pinned the mode, the pool or the country.
pub(crate) fn decide(
    call: &mut Call<'_>,
    input: &RouteInput<'_>,
    caller: &RequestParams,
    routed: &RouteDecision,
) -> Option<Pending> {
    let optimizer = call.spider.optimize.optimizer.clone()?;
    if input.pins.any() {
        return None;
    }

    let observation = Observation {
        memory: input.memory.copied(),
        resources: optimizer
            .resources
            .as_ref()
            .and_then(|source| source.resources(input.url)),
        last_status: input
            .memory
            .map_or(StatusClass::Unknown, |memory| memory.last_status),
    };
    let ctx = Context {
        url: input.url,
        need: input.need,
        current: &call.params,
        caller,
        routed,
        observation: &observation,
        multiplier_cap: multiplier_cap(&call.budget),
    };
    let candidates = generate(&optimizer.schema, &ctx);
    let choice = choose(
        optimizer.scorer.as_ref(),
        &ctx,
        &candidates,
        &optimizer.gate,
    );

    // The row describes the candidate that was picked, applied or not.
    let described = match &choice {
        Choice::Apply { edits, .. } => candidates
            .iter()
            .find(|candidate| &candidate.edits == edits)
            .cloned()
            .unwrap_or_else(|| Candidate::keep(&ctx)),
        Choice::Keep(_) => Candidate::keep(&ctx),
    };
    let edit_feats = featurize_edit(&ctx, &described);
    let edit = EditDescriptor::describe(&described.edits, &observation);
    let mut unpinned = RouteInput::new(input.url, input.need);
    if let Some(memory) = input.memory {
        unpinned = unpinned.with_memory(memory);
    }
    let base = featurize(&unpinned);

    let mut applied = false;
    if let (ApplyMode::Apply, Choice::Apply { edits, .. }) = (optimizer.mode, &choice) {
        applied = edits.apply(&mut call.params, caller).written > 0;
    }

    let arm = match (optimizer.mode, applied) {
        (ApplyMode::Shadow, _) => Arm::Shadow,
        (ApplyMode::Apply, true) => Arm::Candidate,
        (ApplyMode::Apply, false) => Arm::Baseline,
    };
    Some(Pending {
        log: DecisionLog {
            choice,
            candidates: candidates.len().min(usize::from(u8::MAX)) as u8,
            applied,
        },
        arm,
        need: input.need,
        ext: extension_of(input.url),
        tld: host_shape(input.url)
            .tld
            .map_or(u8::MAX, |slot| slot.index() as u8),
        memory: MemoryState::of(input.memory),
        routed: routed.clone(),
        edit,
        edit_feats,
        base,
        pinned: pinned_mask(caller),
        multiplier: described.multiplier,
        site: site_hash(input.url),
        day: optimizer.clock.as_ref().map_or(0, |clock| clock.day()),
    })
}

/// Write the comparison row, once the walk has settled.
///
/// Does nothing unless a recorder is set, the optimizer was asked about this
/// request, and the policy accepted or stopped. `result` is the last attempt as
/// `routing::outcome_of` read it, so the status class reads both planes the
/// way the router's own rows do.
pub(crate) fn compare(
    call: &Call<'_>,
    pending: Option<&Pending>,
    next: &Next,
    result: &AttemptOutcome,
    attempts: &[Attempt],
    pages: &Pages,
) {
    let (Some(pending), Some(recorder)) = (pending, call.spider.comparison_recorder()) else {
        return;
    };
    if !matches!(next, Next::Accept | Next::Stop(_)) {
        return;
    }
    // The reason and the counts only. An applied edit can name a blocked
    // resource, and that stays out of a log line as it stays out of a row.
    let log = &pending.log;
    log::debug!(
        "optimizer: {} candidates, applied {}, kept for {:?}",
        log.candidates,
        log.applied,
        match &log.choice {
            Choice::Keep(reason) => Some(*reason),
            Choice::Apply { .. } => None,
        }
    );

    let credits: f64 = attempts
        .iter()
        .map(|attempt| attempt.cost.get())
        .filter(|cost| cost.is_finite() && *cost > 0.0)
        .sum();
    let elapsed: Duration = attempts.iter().map(|attempt| attempt.elapsed).sum();
    let (pinned, pinned_hi) = pending.pinned;

    let row = comparison_row(&ComparisonRow {
        // Pairs are matched by the collector that sent both arms.
        pair: 0,
        arm: pending.arm,
        day: pending.day,
        domain_key: pending.site ^ recorder.salt(),
        need: pending.need,
        ext: pending.ext,
        tld: pending.tld,
        memory: pending.memory,
        routed: &pending.routed,
        edit: pending.edit,
        pinned,
        pinned_hi,
        success: result.success,
        status: result.status,
        millis: elapsed.as_millis().min(u128::from(u32::MAX)) as u32,
        bytes: result.bytes,
        credits,
        attempts: attempts.len().min(usize::from(u8::MAX)) as u8,
        multiplier: pending.multiplier,
        fields_requested: fields_requested(&call.params),
        fields_present: fields_present(pages),
        // Labels relative to the other arm of a pair are the collector's.
        content_ok: None,
        fields_ok: None,
        shingle_jaccard: None,
        byte_ratio: None,
        base: &pending.base,
        edit_feats: &pending.edit_feats,
    });
    recorder.observe(&row);
}

/// The dearest multiplier the budget allows, priced the way the explorer
/// prices its arms: [`Budget::floor`] times the multiplier, against both caps.
///
/// The dearest row of [`MULTIPLIERS`] when there is no credit cap, and zero
/// when not even a plain fetch fits.
pub(crate) fn multiplier_cap(budget: &Budget) -> f32 {
    let basis = Budget::floor(Credits::ZERO).get();
    MULTIPLIERS
        .iter()
        .map(|(_, _, _, multiplier)| *multiplier)
        .filter(|multiplier| {
            budget
                .allows_spend(Credits::ZERO, Credits(basis * f64::from(*multiplier)))
                .is_ok()
        })
        .fold(0.0, f32::max)
}

/// Which keys the caller set, as bitmasks over [`Key::index`]: the first 32
/// keys, then the rest.
///
/// Read off the caller's own snapshot, the same rule `routing::pins_from`
/// follows, so a value the router, the plan or an edit wrote is not counted.
pub(crate) fn pinned_mask(caller: &RequestParams) -> (u32, u64) {
    let mut low = 0u32;
    let mut high = 0u64;
    for key in Key::ALL {
        if !caller.is_set(*key) {
            continue;
        }
        let at = key.index();
        if at < 32 {
            low |= 1 << at;
        } else if at < 96 {
            high |= 1 << (at - 32);
        }
    }
    (low, high)
}

/// A 64 bit FNV-1a of the registrable name, case folded, or zero when the
/// address names no site. Salted by the recorder before it reaches a row.
fn site_hash(url: &Url) -> u64 {
    let Some(name) = registrable_domain(url) else {
        return 0;
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.as_bytes() {
        hash ^= u64::from(byte.to_ascii_lowercase());
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// How many named fields the request asked for, across every path.
fn fields_requested(params: &RequestParams) -> u8 {
    let count: usize = params
        .css_extraction_map
        .as_ref()
        .map_or(0, |map| map.values().map(Vec::len).sum());
    count.min(usize::from(u8::MAX)) as u8
}

/// How many fields came back with something in them, on the first page served.
fn fields_present(pages: &Pages) -> u8 {
    let count = pages
        .first_ok()
        .and_then(|page| page.body.fields())
        .map_or(0, |fields| {
            fields.values().filter(|value| has_value(value)).count()
        });
    count.min(usize::from(u8::MAX)) as u8
}

/// Whether an extracted value holds anything.
fn has_value(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(text) => !text.trim().is_empty(),
        serde_json::Value::Array(items) => items.iter().any(has_value),
        serde_json::Value::Object(map) => map.values().any(has_value),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => true,
    }
}

/// Plain field access on the request body, for the optimizer.
///
/// No read allocates. [`Key::WaitIdleMillis`] stands for `wait_for`: it is set
/// when any wait is, and reads a number only for an idle network wait.
impl Params for RequestParams {
    fn is_set(&self, key: Key) -> bool {
        match key {
            Key::Url => self.url.is_some(),
            Key::Request => self.request.is_some(),
            Key::Proxy => self.proxy.is_some(),
            Key::CountryCode => self.country_code.is_some(),
            Key::RemoteProxy => self.remote_proxy.is_some(),
            Key::Stealth => self.stealth.is_some(),
            Key::Fingerprint => self.fingerprint.is_some(),
            Key::UserAgent => self.user_agent.is_some(),
            Key::Viewport => self.viewport.is_some(),
            Key::Locale => self.locale.is_some(),
            Key::Cookies => self.cookies.is_some(),
            Key::Headers => self.headers.is_some(),
            Key::Encoding => self.encoding.is_some(),
            Key::Storageless => self.storageless.is_some(),
            Key::Session => self.session.is_some(),
            Key::RedirectPolicy => self.redirect_policy.is_some(),
            Key::RequestTimeout => self.request_timeout.is_some(),
            Key::ServiceWorkerEnabled => self.service_worker_enabled.is_some(),
            Key::PreserveHost => self.preserve_host.is_some(),
            Key::Delay => self.delay.is_some(),
            Key::ConcurrencyLimit => self.concurrency_limit.is_some(),
            Key::Wayback => self.wayback.is_some(),
            Key::Limit => self.limit.is_some(),
            Key::Depth => self.depth.is_some(),
            Key::Budget => self.budget.is_some(),
            Key::Blacklist => self.blacklist.is_some(),
            Key::Whitelist => self.whitelist.is_some(),
            Key::Subdomains => self.subdomains.is_some(),
            Key::Tld => self.tld.is_some(),
            Key::ExternalDomains => self.external_domains.is_some(),
            Key::Sitemap => self.sitemap.is_some(),
            Key::SitemapOnly => self.sitemap_only.is_some(),
            Key::SitemapPath => self.sitemap_path.is_some(),
            Key::RespectRobots => self.respect_robots.is_some(),
            Key::LinkRewrite => self.link_rewrite.is_some(),
            Key::CrawlTimeout => self.crawl_timeout.is_some(),
            Key::RunInBackground => self.run_in_background.is_some(),
            Key::WaitIdleMillis => self.wait_for.is_some(),
            Key::Scroll => self.scroll.is_some(),
            Key::ExecutionScripts => self.execution_scripts.is_some(),
            Key::AutomationScripts => self.automation_scripts.is_some(),
            Key::EvaluateOnNewDocument => self.evaluate_on_new_document.is_some(),
            Key::DisableIntercept => self.disable_intercept.is_some(),
            Key::FullResources => self.full_resources.is_some(),
            Key::BlockAds => self.block_ads.is_some(),
            Key::BlockAnalytics => self.block_analytics.is_some(),
            Key::BlockStylesheets => self.block_stylesheets.is_some(),
            Key::DisableFirstPartyStylesheets => self.disable_first_party_stylesheets.is_some(),
            Key::DisableFirstPartyJavascript => self.disable_first_party_javascript.is_some(),
            Key::DisableFirstPartyVisuals => self.disable_first_party_visuals.is_some(),
            Key::NetworkWhitelist => self.network_whitelist.is_some(),
            Key::NetworkBlacklist => self.network_blacklist.is_some(),
            Key::EventTracker => self.event_tracker.is_some(),
            Key::ReturnFormat => self.return_format.is_some(),
            Key::RootSelector => self.root_selector.is_some(),
            Key::ExcludeSelector => self.exclude_selector.is_some(),
            Key::FilterOutputImages => self.filter_output_images.is_some(),
            Key::FilterOutputSvg => self.filter_output_svg.is_some(),
            Key::FilterOutputMainOnly => self.filter_output_main_only.is_some(),
            Key::MaxSize => self.max_size.is_some(),
            Key::Readability => self.readability.is_some(),
            Key::CleanHtml => self.clean_html.is_some(),
            Key::CssExtractionMap => self.css_extraction_map.is_some(),
            Key::ChunkingAlg => self.chunking_alg.is_some(),
            Key::Metadata => self.metadata.is_some(),
            Key::ReturnEmbeddings => self.return_embeddings.is_some(),
            Key::ReturnHeaders => self.return_headers.is_some(),
            Key::ReturnCookies => self.return_cookies.is_some(),
            Key::ReturnPageLinks => self.return_page_links.is_some(),
            Key::ReturnJsonData => self.return_json_data.is_some(),
            Key::Text => self.text.is_some(),
            Key::Cache => self.cache.is_some(),
            Key::Webhooks => self.webhooks.is_some(),
            Key::DataConnectors => self.data_connectors.is_some(),
            Key::Router => self.router.is_some(),
            Key::ProviderOptions => self.provider_options.is_some(),
            Key::MaxCreditsAllowed => self.max_credits_allowed.is_some(),
            Key::MaxCreditsPerPage => self.max_credits_per_page.is_some(),
            Key::DisableHints => self.disable_hints.is_some(),
            Key::SkipConfigChecks => self.skip_config_checks.is_some(),
            // A key a later schema adds, which this version has no field for.
            _ => false,
        }
    }

    fn request(&self) -> Option<RequestMode> {
        self.request
    }

    fn proxy(&self) -> Option<ProxyPool> {
        self.proxy
    }

    fn idle_wait_millis(&self) -> Option<u32> {
        let timeout = self.wait_for.as_ref()?.idle_network?.timeout;
        let millis = timeout
            .secs
            .saturating_mul(1_000)
            .saturating_add(u64::from(timeout.nanos / 1_000_000));
        Some(millis.min(u64::from(u32::MAX)) as u32)
    }

    fn flag(&self, key: Key) -> Option<bool> {
        match key {
            Key::Stealth => self.stealth,
            Key::Fingerprint => self.fingerprint,
            Key::Storageless => self.storageless,
            Key::Session => self.session,
            Key::ServiceWorkerEnabled => self.service_worker_enabled,
            Key::PreserveHost => self.preserve_host,
            Key::Wayback => self.wayback,
            Key::Subdomains => self.subdomains,
            Key::Tld => self.tld,
            Key::Sitemap => self.sitemap,
            Key::SitemapOnly => self.sitemap_only,
            Key::RespectRobots => self.respect_robots,
            Key::RunInBackground => self.run_in_background,
            Key::DisableIntercept => self.disable_intercept,
            Key::FullResources => self.full_resources,
            Key::BlockAds => self.block_ads,
            Key::BlockAnalytics => self.block_analytics,
            Key::BlockStylesheets => self.block_stylesheets,
            Key::DisableFirstPartyStylesheets => self.disable_first_party_stylesheets,
            Key::DisableFirstPartyJavascript => self.disable_first_party_javascript,
            Key::DisableFirstPartyVisuals => self.disable_first_party_visuals,
            Key::FilterOutputImages => self.filter_output_images,
            Key::FilterOutputSvg => self.filter_output_svg,
            Key::FilterOutputMainOnly => self.filter_output_main_only,
            Key::Readability => self.readability,
            Key::CleanHtml => self.clean_html,
            Key::Metadata => self.metadata,
            Key::ReturnEmbeddings => self.return_embeddings,
            Key::ReturnHeaders => self.return_headers,
            Key::ReturnCookies => self.return_cookies,
            Key::ReturnPageLinks => self.return_page_links,
            Key::ReturnJsonData => self.return_json_data,
            Key::DisableHints => self.disable_hints,
            Key::SkipConfigChecks => self.skip_config_checks,
            _ => None,
        }
    }

    fn list(&self, key: Key) -> Option<&[String]> {
        match key {
            Key::Blacklist => self.blacklist.as_deref(),
            Key::Whitelist => self.whitelist.as_deref(),
            Key::ExternalDomains => self.external_domains.as_deref(),
            Key::NetworkWhitelist => self.network_whitelist.as_deref(),
            Key::NetworkBlacklist => self.network_blacklist.as_deref(),
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
        self.wait_for = Some(WaitFor::idle_network(Timeout::from_millis(u64::from(
            millis,
        ))));
    }

    fn set_flag(&mut self, key: Key, value: bool) {
        let slot = match key {
            Key::DisableIntercept => &mut self.disable_intercept,
            Key::FullResources => &mut self.full_resources,
            Key::BlockAds => &mut self.block_ads,
            Key::BlockAnalytics => &mut self.block_analytics,
            Key::BlockStylesheets => &mut self.block_stylesheets,
            _ => return,
        };
        *slot = Some(value);
    }

    fn set_list(&mut self, key: Key, list: Option<Vec<String>>) {
        if key == Key::NetworkBlacklist {
            self.network_blacklist = list;
        }
    }
}

#[cfg(test)]
mod tests {
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
    use super::*;
    use crate::params::{
        Cache, ChunkingAlg, ChunkingKind, Country, CrawlBudget, CssExtractionMap, EventTracker,
        LinkRewriteRule, RedirectPolicy, ReturnFormat, Router, SelectorGroup, Viewport,
        WebAutomation, WebhookSettings, WholeCredits,
    };
    use spider_optimize::schema::{Kind, MODES, PROXIES, WAIT_BUCKETS};
    use std::collections::BTreeMap;

    /// Every field set. There is no `..Default` here, so a field added to the
    /// struct and not to this list fails to compile.
    fn populated() -> RequestParams {
        let mut css = CssExtractionMap::new();
        css.insert("/".into(), vec![SelectorGroup::css("title", ["h1"])]);
        RequestParams {
            url: Some("https://example.com/".into()),
            request: Some(RequestMode::Browser),
            proxy: Some(ProxyPool::Residential),
            country_code: Country::new("de"),
            remote_proxy: Some("http://example.com:8080".into()),
            stealth: Some(true),
            fingerprint: Some(false),
            user_agent: Some("agent".into()),
            viewport: Some(Viewport::desktop()),
            locale: Some("en-GB".into()),
            cookies: Some("a=b".into()),
            headers: Some(BTreeMap::from([("x".to_string(), "y".to_string())])),
            encoding: Some("utf-8".into()),
            storageless: Some(true),
            session: Some(false),
            redirect_policy: Some(RedirectPolicy::Loose),
            request_timeout: Some(30),
            service_worker_enabled: Some(false),
            preserve_host: Some(true),
            delay: Some(10),
            concurrency_limit: Some(2),
            wayback: Some(true),
            limit: Some(5),
            depth: Some(2),
            budget: Some(CrawlBudget::from([("*".to_string(), 3)])),
            blacklist: Some(vec!["/a".into()]),
            whitelist: Some(vec!["/b".into()]),
            subdomains: Some(true),
            tld: Some(false),
            external_domains: Some(vec!["example.org".into()]),
            sitemap: Some(true),
            sitemap_only: Some(false),
            sitemap_path: Some("sitemap.xml".into()),
            respect_robots: Some(true),
            link_rewrite: Some(LinkRewriteRule::replace("a", "b")),
            crawl_timeout: Some(Timeout::from_secs(5)),
            run_in_background: Some(false),
            wait_for: Some(WaitFor::idle_network(Timeout::from_millis(2_000))),
            scroll: Some(1),
            execution_scripts: Some(BTreeMap::from([("/".to_string(), "1".to_string())])),
            automation_scripts: Some(BTreeMap::from([(
                "/".to_string(),
                vec![WebAutomation::Evaluate("1".into())],
            )])),
            evaluate_on_new_document: Some("1".into()),
            disable_intercept: Some(false),
            full_resources: Some(true),
            block_ads: Some(true),
            block_analytics: Some(false),
            block_stylesheets: Some(true),
            disable_first_party_stylesheets: Some(false),
            disable_first_party_javascript: Some(true),
            disable_first_party_visuals: Some(false),
            network_whitelist: Some(vec!["example.com".into()]),
            network_blacklist: Some(vec!["example.net".into()]),
            event_tracker: Some(EventTracker::default()),
            return_format: Some(ReturnFormat::Markdown.into()),
            root_selector: Some("main".into()),
            exclude_selector: Some("nav".into()),
            filter_output_images: Some(true),
            filter_output_svg: Some(false),
            filter_output_main_only: Some(true),
            max_size: Some(1_000),
            readability: Some(false),
            clean_html: Some(true),
            css_extraction_map: Some(css),
            chunking_alg: Some(ChunkingAlg::new(ChunkingKind::BySentence, 2)),
            metadata: Some(true),
            return_embeddings: Some(false),
            return_headers: Some(true),
            return_cookies: Some(false),
            return_page_links: Some(true),
            return_json_data: Some(false),
            text: Some("text".into()),
            cache: Some(Cache::Enabled(true)),
            webhooks: Some(WebhookSettings::new("https://example.com/hook")),
            data_connectors: Some(serde_json::Map::from_iter([(
                "s3".to_string(),
                serde_json::Value::Bool(true),
            )])),
            router: Some(Router {
                mode: Some("off".into()),
                ..Router::default()
            }),
            provider_options: Some(BTreeMap::from([(
                "p".to_string(),
                serde_json::Value::Bool(true),
            )])),
            max_credits_allowed: Some(WholeCredits::floor(Credits::new(10.0))),
            max_credits_per_page: Some(Credits::new(1.0)),
            disable_hints: Some(true),
            skip_config_checks: Some(false),
        }
    }

    fn body(params: &RequestParams) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::to_value(params).unwrap() {
            serde_json::Value::Object(map) => map,
            other => panic!("a request is an object, got {other}"),
        }
    }

    #[test]
    fn is_set_agrees_with_is_some_on_every_field() {
        // Read off the serialized body, which skips exactly the fields that are
        // `None`, so this cannot share a mistake with the match it checks.
        let full = body(&populated());
        assert_eq!(full.len(), Key::ALL.len(), "populated() left a field unset");

        let empty = RequestParams::default();
        for key in Key::ALL {
            assert!(full.contains_key(key.wire()), "{key:?} has no field");
            assert!(populated().is_set(*key), "{key:?} reads unset when set");
            assert!(!empty.is_set(*key), "{key:?} reads set on a default");

            // The same request with this one field taken away.
            let mut without = full.clone();
            without.remove(key.wire());
            let without: RequestParams =
                serde_json::from_value(serde_json::Value::Object(without)).unwrap();
            for other in Key::ALL {
                assert_eq!(
                    without.is_set(*other),
                    other != key,
                    "{other:?} after clearing {key:?}"
                );
            }
        }
    }

    #[test]
    fn every_bool_and_list_field_reads_what_goes_on_the_wire() {
        let schema = Schema::v1();
        let params = populated();
        let full = body(&params);
        let mut bools = 0;
        for spec in schema.specs() {
            let wire = full
                .get(spec.key.wire())
                .and_then(serde_json::Value::as_bool);
            if spec.kind == Kind::Bool {
                bools += 1;
                assert_eq!(params.flag(spec.key), wire, "{:?}", spec.key);
                assert_eq!(RequestParams::default().flag(spec.key), None);
            } else {
                assert_eq!(params.flag(spec.key), None, "{:?}", spec.key);
            }
            if spec.kind == Kind::StringList {
                let wire: Option<Vec<String>> = full
                    .get(spec.key.wire())
                    .map(|value| serde_json::from_value(value.clone()).unwrap());
                assert_eq!(params.list(spec.key).map(<[String]>::to_vec), wire);
            } else {
                assert_eq!(params.list(spec.key), None, "{:?}", spec.key);
            }
        }
        assert!(bools > 20, "only {bools} switches were read");
    }

    #[test]
    fn every_learnable_key_round_trips() {
        let mut covered = 0;
        for spec in Schema::v1().learnable() {
            let key = spec.key;
            let mut params = RequestParams::default();
            match key {
                Key::Request => {
                    for mode in MODES {
                        params.set_request(mode);
                        assert_eq!(params.request(), Some(mode));
                    }
                }
                Key::Proxy => {
                    for pool in PROXIES {
                        params.set_proxy(pool);
                        assert_eq!(params.proxy(), Some(pool));
                    }
                }
                Key::WaitIdleMillis => {
                    for millis in WAIT_BUCKETS.into_iter().chain([1, 60_000]) {
                        params.set_idle_wait(millis);
                        assert_eq!(params.idle_wait_millis(), Some(millis));
                        assert_eq!(
                            params.wait_for,
                            Some(WaitFor::idle_network(Timeout::from_millis(u64::from(
                                millis
                            ))))
                        );
                    }
                    // A wait of another kind is set, and holds no idle figure.
                    params.wait_for = Some(WaitFor::selector("main", Timeout::from_secs(2)));
                    assert!(params.is_set(key));
                    assert_eq!(params.idle_wait_millis(), None);
                }
                Key::NetworkBlacklist => {
                    let list = vec!["a.example".to_string(), "b.example".to_string()];
                    params.set_list(key, Some(list.clone()));
                    assert_eq!(params.list(key), Some(list.as_slice()));
                    params.set_list(key, None);
                    assert_eq!(params.list(key), None);
                    // No other list is written through this.
                    params.set_list(Key::NetworkWhitelist, Some(list));
                    assert_eq!(params.list(Key::NetworkWhitelist), None);
                    params.set_list(key, Some(Vec::new()));
                }
                _ => {
                    assert_eq!(spec.kind, Kind::Bool, "{key:?}");
                    for value in [true, false] {
                        params.set_flag(key, value);
                        assert_eq!(params.flag(key), Some(value), "{key:?}");
                    }
                }
            }
            assert!(params.is_set(key), "{key:?} reads unset after a write");
            // Exactly this key was written.
            for other in Key::ALL {
                assert_eq!(params.is_set(*other), other == &key, "{other:?}");
            }
            covered += 1;
        }
        assert_eq!(covered, 9);

        // A switch nothing learns is not written.
        let mut params = RequestParams::default();
        params.set_flag(Key::Stealth, true);
        assert_eq!(params, RequestParams::default());
    }

    #[test]
    fn the_pinned_mask_names_what_the_caller_set() {
        assert_eq!(pinned_mask(&RequestParams::default()), (0, 0));
        let caller = RequestParams {
            stealth: Some(true),
            skip_config_checks: Some(false),
            ..RequestParams::default()
        };
        assert_eq!(
            pinned_mask(&caller),
            (
                1 << Key::Stealth.index(),
                1 << (Key::SkipConfigChecks.index() - 32)
            )
        );
        let (low, high) = pinned_mask(&populated());
        assert_eq!(low.count_ones() + high.count_ones(), Key::ALL.len() as u32);
    }

    #[test]
    fn the_multiplier_cap_follows_the_budget() {
        let floor = crate::policy::ASSUMED_MINIMUM_COST;
        assert_eq!(multiplier_cap(&Budget::default()), 8.0);
        assert_eq!(multiplier_cap(&Budget::unlimited()), 8.0);
        let two = Budget::default().with_credits(Credits(floor.get() * 2.0));
        assert_eq!(multiplier_cap(&two), 1.5);
        let page = Budget::default().with_per_page_credits(Credits(floor.get() * 4.0));
        assert_eq!(multiplier_cap(&page), 4.0);
        let broke = Budget::default().with_credits(Credits::ZERO);
        assert_eq!(multiplier_cap(&broke), 0.0);
    }

    #[test]
    fn a_site_key_is_the_registrable_name_and_nothing_else() {
        let url = |raw: &str| Url::parse(raw).unwrap();
        assert_eq!(
            site_hash(&url("https://www.example.com/a")),
            site_hash(&url("https://shop.EXAMPLE.com/b"))
        );
        assert_ne!(
            site_hash(&url("https://example.com/")),
            site_hash(&url("https://example.org/"))
        );
        assert_eq!(site_hash(&url("http://192.0.2.1/")), 0);
    }

    #[test]
    fn a_field_counts_as_present_only_with_something_in_it() {
        use serde_json::json;
        for (value, expected) in [
            (json!(null), false),
            (json!(""), false),
            (json!("  "), false),
            (json!([]), false),
            (json!([""]), false),
            (json!({}), false),
            (json!("x"), true),
            (json!(["", "x"]), true),
            (json!(0), true),
            (json!(false), true),
        ] {
            assert_eq!(has_value(&value), expected, "{value}");
        }
        let mut params = RequestParams::default();
        assert_eq!(fields_requested(&params), 0);
        params.css_extraction_map = Some(CssExtractionMap::from([
            (
                "/".to_string(),
                vec![
                    SelectorGroup::css("a", ["h1"]),
                    SelectorGroup::css("b", ["h2"]),
                ],
            ),
            ("/x".to_string(), vec![SelectorGroup::css("c", ["p"])]),
        ]));
        assert_eq!(fields_requested(&params), 3);
    }

    #[test]
    fn debug_prints_presence_and_no_salt() {
        let hooks = Hooks {
            optimizer: Some(Arc::new(Optimizer::shadow(NoModel))),
            comparison: None,
        };
        let printed = format!("{hooks:?}");
        assert!(printed.contains("optimizer: \"set\""), "{printed}");
        assert!(printed.contains("comparison: \"none\""), "{printed}");

        let recorder = JsonlComparisonRecorder::new(Vec::<u8>::new()).with_salt(987_654_321);
        assert_eq!(recorder.salt, 987_654_321);
        assert!(!format!("{recorder:?}").contains("987654321"));
    }
}
