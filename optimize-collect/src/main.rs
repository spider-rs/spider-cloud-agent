//! Collects paired comparison rows for `spider-optimize`.
//!
//! For every url: one baseline run, the client's own request with the
//! optimizer keeping it, then one run per entry of the candidate file with that
//! edit set applied through the optimizer, and now and then a second baseline
//! run. Each candidate run and its baseline share a pair nonce, a day and a
//! site key, and are written to `rows.jsonl` together with the labels only the
//! pair can compute. `manifest.json` describes the whole directory. See
//! `README.md` beside this file.
//!
//! Nothing is sent without `--dry-run` against a loopback stub, or `--spend`
//! with a credit cap.

mod pair;
mod scorer;
mod spec;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde_json::Value;
use sha2::{Digest, Sha256};
use spider_cloud_agent::auth::Credentials;
use spider_cloud_agent::optimize::{
    summarize, ApplyMode, Clock, ComparisonRecorder, Gate, NoModel, Optimizer, ResourceSource,
    ResourceSummary,
};
use spider_cloud_agent::params::EventTracker;
use spider_cloud_agent::{Budget, Credits, DeclaredNeed, Need, Spider, StatusClass};
use spider_optimize::Observation;
use url::Url;

use crate::pair::{Content, Nonces};
use crate::scorer::ForcedScorer;

/// Paired comparison rows for spider-optimize.
#[derive(Debug, Parser)]
#[command(name = "optimize-collect", version)]
struct Args {
    /// One url per line. Blank lines and lines starting with `#` are skipped.
    #[arg(long)]
    urls: PathBuf,
    /// The candidate edit sets, as `README.md` documents.
    #[arg(long)]
    candidates: PathBuf,
    /// Where `rows.jsonl` and `manifest.json` go.
    #[arg(long)]
    out: PathBuf,
    /// Eight bytes of site key salt, created at mode 0600 when missing.
    #[arg(long)]
    salt_file: PathBuf,
    /// Send every request to `--base-url`, which must be a loopback address.
    #[arg(long)]
    dry_run: bool,
    /// The stub service a dry run talks to.
    #[arg(long)]
    base_url: Option<Url>,
    /// Send real, billed requests. Needs `--max-credits`.
    #[arg(long)]
    spend: bool,
    /// Stop once this many credits are spent, and cap every run at it.
    #[arg(long)]
    max_credits: Option<f64>,
    /// The share of urls that get a second baseline arm.
    #[arg(long, default_value_t = 0.05)]
    repeat_share: f64,
    /// markdown, text, links, or fields:SELECTORS.json.
    #[arg(long, default_value = "markdown")]
    need: String,
    /// Seed for pair nonces and repeat draws. Random when absent.
    #[arg(long)]
    seed: Option<u64>,
    /// What the service was running, when known.
    #[arg(long)]
    service_revision: Option<String>,
}

/// Why the collector stopped early.
enum Stop {
    /// Bad flags or bad input files. Exit 2.
    Usage(String),
    /// Something failed while running. Exit 1.
    Failed(String),
}

fn usage(message: impl Into<String>) -> Stop {
    Stop::Usage(message.into())
}

fn failed(message: impl Into<String>) -> Stop {
    Stop::Failed(message.into())
}

fn main() -> ExitCode {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2));
        }
    };
    match run(args) {
        Ok(Finish::Complete) => ExitCode::SUCCESS,
        Ok(Finish::Capped) => ExitCode::from(3),
        Err(Stop::Usage(message)) => {
            eprintln!("optimize-collect: {message}");
            ExitCode::from(2)
        }
        Err(Stop::Failed(message)) => {
            eprintln!("optimize-collect: {message}");
            ExitCode::from(1)
        }
    }
}

/// How a run that did not fail ended.
enum Finish {
    Complete,
    Capped,
}

/// Where requests go and with which key.
struct Target {
    key: String,
    base: Option<Url>,
}

fn run(args: Args) -> Result<Finish, Stop> {
    let target = target(&args)?;
    if !(0.0..=1.0).contains(&args.repeat_share) {
        return Err(usage("--repeat-share must be between 0 and 1"));
    }
    let urls = read_urls(&args.urls)?;
    let candidates = spec::parse(&read_text(&args.candidates)?)
        .map_err(|why| usage(format!("{}: {why}", args.candidates.display())))?;
    let (need, requested) = need_of(&args.need)?;

    let salt = salt(&args.salt_file)?;
    let seed = match args.seed {
        Some(seed) => seed,
        None => u64::from_le_bytes(random_bytes()?),
    };

    std::fs::create_dir_all(&args.out)
        .map_err(|error| failed(format!("cannot create {}: {error}", args.out.display())))?;
    let rows_path = args.out.join("rows.jsonl");
    // Collected rows may have cost money, so an old file is never overwritten.
    let rows = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&rows_path)
        .map_err(|error| usage(format!("cannot create {}: {error}", rows_path.display())))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| failed(format!("cannot start the runtime: {error}")))?;

    let mut collector = Collector {
        target,
        need,
        requested,
        salt: u64::from_le_bytes(salt),
        cap: args.max_credits,
        nonces: Nonces::new(seed),
        rows,
        written: 0,
        pairs: 0,
        days: None,
        spent: 0.0,
    };
    let finish = runtime.block_on(collector.collect(&urls, &candidates, args.repeat_share));

    let manifest = manifest(&collector, &args, &salt);
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|error| failed(format!("cannot write the manifest: {error}")))?;
    std::fs::write(args.out.join("manifest.json"), text + "\n")
        .map_err(|error| failed(format!("cannot write the manifest: {error}")))?;

    let finish = finish?;
    if matches!(finish, Finish::Capped) {
        eprintln!(
            "optimize-collect: stopped at the credit cap, {:.4} credits spent, {} rows in {} pairs",
            collector.spent, collector.written, collector.pairs
        );
    } else {
        eprintln!(
            "optimize-collect: {} rows in {} pairs, {:.4} credits spent",
            collector.written, collector.pairs, collector.spent
        );
    }
    Ok(finish)
}

/// Check the mode flags, and find the key only once they allow a request.
fn target(args: &Args) -> Result<Target, Stop> {
    match (args.dry_run, args.spend) {
        (false, false) => Err(usage(
            "refusing to start: pass --dry-run with a loopback --base-url, or --spend with --max-credits",
        )),
        (true, true) => Err(usage("--dry-run and --spend cannot be used together")),
        (true, false) => {
            let base = args
                .base_url
                .clone()
                .ok_or_else(|| usage("--dry-run needs --base-url"))?;
            if !is_loopback(&base) {
                return Err(usage(
                    "--dry-run only talks to a loopback --base-url such as http://127.0.0.1:PORT",
                ));
            }
            check_cap(args.max_credits)?;
            Ok(Target {
                // The stub never checks it, and no real key is read for a dry run.
                key: "dry-run".to_string(),
                base: Some(base),
            })
        }
        (false, true) => {
            if args.max_credits.is_none() {
                return Err(usage("--spend needs --max-credits"));
            }
            check_cap(args.max_credits)?;
            if args.base_url.is_some() {
                return Err(usage("--base-url is for --dry-run only"));
            }
            let credentials = Credentials::resolve()
                .ok_or_else(|| usage("--spend found no API key in the usual places"))?;
            Ok(Target {
                key: credentials.into_key(),
                base: None,
            })
        }
    }
}

fn check_cap(cap: Option<f64>) -> Result<(), Stop> {
    match cap {
        Some(cap) if !(cap.is_finite() && cap > 0.0) => {
            Err(usage("--max-credits must be a positive number"))
        }
        _ => Ok(()),
    }
}

fn is_loopback(base: &Url) -> bool {
    match base.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn read_text(path: &Path) -> Result<String, Stop> {
    std::fs::read_to_string(path)
        .map_err(|error| usage(format!("cannot read {}: {error}", path.display())))
}

/// The urls, with the line each came from. A bad line is named by number.
fn read_urls(path: &Path) -> Result<Vec<(usize, Url)>, Stop> {
    let mut out = Vec::new();
    for (at, line) in read_text(path)?.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let url = Url::parse(line)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .ok_or_else(|| {
                usage(format!(
                    "{} line {}: not an http address",
                    path.display(),
                    at + 1
                ))
            })?;
        out.push((at + 1, url));
    }
    if out.is_empty() {
        return Err(usage(format!("{} lists no urls", path.display())));
    }
    Ok(out)
}

/// The need, and the field names a fields need asks for.
///
/// A selectors file is a JSON object from field name to CSS selector.
fn need_of(text: &str) -> Result<(Need, Vec<String>), Stop> {
    match text {
        "markdown" => Ok((Need::Markdown, Vec::new())),
        "text" => Ok((Need::Text, Vec::new())),
        "links" => Ok((Need::Links, Vec::new())),
        other => {
            let Some(path) = other.strip_prefix("fields:") else {
                return Err(usage(format!(
                    "--need {other:?} is not markdown, text, links or fields:SELECTORS.json"
                )));
            };
            let selectors: std::collections::BTreeMap<String, String> =
                serde_json::from_str(&read_text(Path::new(path))?).map_err(|error| {
                    usage(format!(
                        "{path}: not an object of name to selector: {error}"
                    ))
                })?;
            if selectors.is_empty() {
                return Err(usage(format!("{path}: names no fields")));
            }
            let names = selectors.keys().cloned().collect();
            Ok((Need::fields(selectors), names))
        }
    }
}

fn declared(need: &Need) -> DeclaredNeed {
    match need {
        Need::Text => DeclaredNeed::Text,
        Need::Links => DeclaredNeed::Links,
        Need::Fields(_) => DeclaredNeed::Fields,
        _ => DeclaredNeed::Markdown,
    }
}

/// Eight bytes from the system's random source.
fn random_bytes() -> Result<[u8; 8], Stop> {
    let mut bytes = [0u8; 8];
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| failed(format!("cannot read /dev/urandom: {error}")))?;
    Ok(bytes)
}

/// The salt in the file, or a new one written there at mode 0600.
fn salt(path: &Path) -> Result<[u8; 8], Stop> {
    match std::fs::read(path) {
        Ok(bytes) => <[u8; 8]>::try_from(bytes.as_slice()).map_err(|_| {
            usage(format!(
                "{} holds {} bytes, a salt is exactly 8",
                path.display(),
                bytes.len()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let bytes = random_bytes()?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options
                .open(path)
                .and_then(|mut file| file.write_all(&bytes))
                .map_err(|error| failed(format!("cannot create {}: {error}", path.display())))?;
            Ok(bytes)
        }
        Err(error) => Err(usage(format!("cannot read {}: {error}", path.display()))),
    }
}

/// The first 8 hex characters of the salt's sha256, so two corpora can be told
/// apart by salt without the salt.
fn salt_id(salt: &[u8; 8]) -> String {
    Sha256::digest(salt)
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Days since 2026-01-01, by the wall clock.
fn today() -> u32 {
    const START: u64 = 1_767_225_600;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    (now.saturating_sub(START) / 86_400).min(u64::from(u32::MAX)) as u32
}

/// The day every arm of one url is stamped with.
struct Day(u32);

impl Clock for Day {
    fn day(&self) -> u32 {
        self.0
    }
}

/// Rows handed back down a channel, salted with the collector's salt.
struct Rows {
    sender: Sender<String>,
    salt: u64,
}

impl ComparisonRecorder for Rows {
    fn observe(&self, row: &str) {
        let _ = self.sender.send(row.to_string());
    }

    fn salt(&self) -> u64 {
        self.salt
    }
}

/// What the baseline saw the page load.
struct Seen(ResourceSummary);

impl ResourceSource for Seen {
    fn resources(&self, _url: &Url) -> Option<ResourceSummary> {
        Some(self.0.clone())
    }
}

/// Which arm a run is.
enum Plan<'a> {
    Baseline,
    Candidate {
        scorer: Box<ForcedScorer>,
        observation: &'a Observation,
        blacklist: bool,
    },
}

/// One finished run.
struct Arm {
    /// The row the client wrote, when the walk settled.
    row: Option<Value>,
    /// What came back, when a page was served.
    content: Option<Content>,
    /// The page's resources, when the event tracker reported them.
    resources: Option<ResourceSummary>,
}

struct Collector {
    target: Target,
    need: Need,
    requested: Vec<String>,
    salt: u64,
    cap: Option<f64>,
    nonces: Nonces,
    rows: File,
    written: usize,
    pairs: usize,
    days: Option<(u32, u32)>,
    spent: f64,
}

impl Collector {
    fn capped(&self) -> bool {
        self.cap.is_some_and(|cap| self.spent >= cap)
    }

    async fn collect(
        &mut self,
        urls: &[(usize, Url)],
        candidates: &[spec::Candidate],
        repeat_share: f64,
    ) -> Result<Finish, Stop> {
        for (line, url) in urls {
            if self.capped() {
                return Ok(Finish::Capped);
            }
            let day = today();
            let repeat = self.nonces.unit() < repeat_share;

            let baseline = self.send(url, day, Plan::Baseline).await?;
            let Some(base_row) = baseline.row.clone() else {
                eprintln!("optimize-collect: line {line}: the baseline wrote no row, url skipped");
                continue;
            };
            if self.capped() {
                return Ok(Finish::Capped);
            }
            let observation = Observation {
                memory: None,
                resources: baseline.resources.clone(),
                last_status: StatusClass::Unknown,
            };

            for candidate in candidates {
                let edits = match candidate.resolve(&observation) {
                    Ok(edits) => edits,
                    Err(why) => {
                        eprintln!(
                            "optimize-collect: line {line}: entry {} not run: {why}",
                            candidate.index
                        );
                        continue;
                    }
                };
                let scorer = ForcedScorer::new(url, declared(&self.need), &edits, &observation);
                let arm = self
                    .send(
                        url,
                        day,
                        Plan::Candidate {
                            scorer: Box::new(scorer.clone()),
                            observation: &observation,
                            blacklist: candidate.blacklists(),
                        },
                    )
                    .await?;
                match &arm.row {
                    Some(row) if scorer::row_matches(&scorer, row, &edits, &observation) => {
                        self.write_pair(&base_row, &baseline, &arm, false)?;
                    }
                    Some(_) => eprintln!(
                        "optimize-collect: line {line}: entry {} was not applied as written, pair dropped",
                        candidate.index
                    ),
                    None => eprintln!(
                        "optimize-collect: line {line}: entry {} wrote no row, pair dropped",
                        candidate.index
                    ),
                }
                if self.capped() {
                    return Ok(Finish::Capped);
                }
            }

            if repeat {
                let again = self.send(url, day, Plan::Baseline).await?;
                match &again.row {
                    Some(row) if row["arm"] == "baseline" => {
                        self.write_pair(&base_row, &baseline, &again, true)?;
                    }
                    _ => eprintln!(
                        "optimize-collect: line {line}: the repeat wrote no row, pair dropped"
                    ),
                }
                if self.capped() {
                    return Ok(Finish::Capped);
                }
            }
        }
        Ok(Finish::Complete)
    }

    /// Send one arm and collect what it wrote and what it cost.
    async fn send(&mut self, url: &Url, day: u32, plan: Plan<'_>) -> Result<Arm, Stop> {
        let (sender, receiver): (Sender<String>, Receiver<String>) = channel();
        let recorder = Rows {
            sender,
            salt: self.salt,
        };
        let baseline = matches!(plan, Plan::Baseline);
        let (optimizer, blacklist) = match plan {
            Plan::Baseline => (
                Optimizer::new(NoModel, Gate::default(), ApplyMode::Apply),
                false,
            ),
            Plan::Candidate {
                scorer,
                observation,
                blacklist,
            } => {
                let optimizer = Optimizer::new(*scorer, Gate::default(), ApplyMode::Apply);
                let optimizer = match &observation.resources {
                    Some(summary) => optimizer.with_resources(Seen(summary.clone())),
                    None => optimizer,
                };
                (optimizer, blacklist)
            }
        };

        let mut budget = Budget::default();
        if let Some(cap) = self.cap {
            budget = budget.with_credits(Credits::new(cap));
        }
        let mut builder = Spider::builder()
            .key(self.target.key.clone())
            .budget(budget)
            .optimizer(optimizer.with_clock(Day(day)))
            .comparison_recorder(recorder);
        if let Some(base) = &self.target.base {
            builder = builder.base_url(base.clone());
        }
        let spider = builder
            .build()
            .map_err(|error| failed(format!("cannot build the client: {error}")))?;

        let mut call = spider.scrape(url).need(self.need.clone());
        if baseline {
            // The baseline is the only arm that asks what the page loads, so a
            // blacklist arm has identifiers to name.
            call.params_mut().event_tracker = Some(EventTracker {
                responses: Some(true),
                ..EventTracker::default()
            });
        }
        if blacklist {
            // The optimizer refuses a blacklist edit while the service's own
            // blocking hints are on.
            call.params_mut().disable_hints = Some(true);
        }

        let (content, resources) = match call.send().await {
            Ok(outcome) => {
                self.spent += outcome.cost.get();
                let page = &outcome.value;
                let resources = page.response_map.as_ref().map(|map| resources_of(url, map));
                (Some(Content::of(page, declared(&self.need))), resources)
            }
            Err(error) => {
                self.spent += error.spent().get();
                (None, None)
            }
        };
        let row = receiver
            .try_iter()
            .last()
            .and_then(|row| serde_json::from_str::<Value>(&row).ok())
            .filter(Value::is_object);
        Ok(Arm {
            row,
            content,
            resources,
        })
    }

    /// Label a pair and append both rows.
    fn write_pair(
        &mut self,
        base_row: &Value,
        baseline: &Arm,
        candidate: &Arm,
        repeat: bool,
    ) -> Result<(), Stop> {
        let (mut base, Some(mut cand)) = (base_row.clone(), candidate.row.clone()) else {
            return Ok(());
        };
        let nonce = self.nonces.pair();
        pair::label(
            nonce,
            &mut base,
            &mut cand,
            baseline.content.as_ref(),
            candidate.content.as_ref(),
            &self.requested,
            repeat,
        );
        let mut text = String::new();
        for row in [&base, &cand] {
            let line = serde_json::to_string(row)
                .map_err(|error| failed(format!("cannot write a row: {error}")))?;
            text.push_str(&line);
            text.push('\n');
        }
        self.rows
            .write_all(text.as_bytes())
            .map_err(|error| failed(format!("cannot write rows.jsonl: {error}")))?;
        for row in [&base, &cand] {
            if let Some(day) = row["day"].as_u64() {
                let day = day.min(u64::from(u32::MAX)) as u32;
                self.days = Some(match self.days {
                    Some((low, high)) => (low.min(day), high.max(day)),
                    None => (day, day),
                });
            }
        }
        self.written += 2;
        self.pairs += 1;
        Ok(())
    }
}

/// The resource summary of an event tracker's response map: address to bytes.
fn resources_of(page: &Url, map: &Value) -> ResourceSummary {
    let resources: Vec<(Url, u64)> = map
        .as_object()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(address, bytes)| {
                    let url = Url::parse(address).ok()?;
                    let bytes = bytes.as_f64().filter(|b| b.is_finite() && *b >= 0.0)?;
                    Some((url, bytes as u64))
                })
                .collect()
        })
        .unwrap_or_default();
    summarize(page, &resources)
}

fn manifest(collector: &Collector, args: &Args, salt: &[u8; 8]) -> Value {
    let (day_min, day_max) = collector.days.unwrap_or((0, 0));
    serde_json::json!({
        "schema_version": spider_optimize::SCHEMA_VERSION,
        "feature_version": spider_optimize::row::BASE_FEATURE_VERSION,
        "edit_feature_version": spider_optimize::EDIT_FEATURE_VERSION,
        "edit_dim": spider_optimize::EDIT_DIM,
        "rows": collector.written,
        "pairs": collector.pairs,
        "day_min": day_min,
        "day_max": day_max,
        "collector_rev": option_env!("OPTIMIZE_COLLECT_REV").unwrap_or("unknown"),
        "client_version": env!("CARGO_PKG_VERSION"),
        "service_revision": args.service_revision.as_deref().unwrap_or("unattested"),
        "credits_spent": collector.spent,
        "tau": pair::TAU,
        "salt_id": salt_id(salt),
        "synthetic": false,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_salt_id_is_eight_hex_characters_and_not_the_salt() {
        let salt = [1, 2, 3, 4, 5, 6, 7, 8];
        let id = salt_id(&salt);
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(id, "01020304");
        assert_ne!(salt_id(&[0; 8]), id);
    }

    #[test]
    fn only_a_loopback_base_is_a_dry_run_target() {
        for (base, loopback) in [
            ("http://127.0.0.1:8080", true),
            ("http://[::1]:8080", true),
            ("http://localhost:1", true),
            ("https://api.spider.cloud", false),
            ("http://10.0.0.1", false),
        ] {
            assert_eq!(is_loopback(&Url::parse(base).unwrap()), loopback, "{base}");
        }
    }

    #[test]
    fn the_day_counts_from_2026() {
        assert!(today() >= 258, "2026-09-16 is day 258");
    }
}
