//! The training tap.
//!
//! A [`Recorder`] sees what the router was given, what it chose, and how that
//! turned out. Rows collected this way are what a future set of weights is fit
//! to, and they are the only rows that describe the service as it behaves
//! today rather than as it behaved when a log was written.
//!
//! # What a row can hold, and why that is structural
//!
//! A recorder is handed a [`FeatureVector`]: a fixed width array of numbers
//! with no heap in it. There is no url in a row, no domain, no host and no page
//! content, and there is no way to put one there short of changing the type in
//! the router crate, which asserts its own size for exactly this reason.
//!
//! That is the property that makes a published model defensible. Every feature
//! slot is named and enumerated in public source, and every value of every slot
//! is shared by a very large number of sites. Do not add an address, a host or
//! a body to what a recorder sees, however convenient it would be for a
//! particular analysis. The analysis is not worth the claim.
//!
//! ```no_run
//! # fn run() -> std::io::Result<()> {
//! use spider_cloud_agent::record::JsonlRecorder;
//! use spider_cloud_agent::Spider;
//!
//! let rows = JsonlRecorder::create("routes.jsonl")?;
//! let spider = Spider::builder().key("...").recorder(rows).build();
//! # let _ = spider;
//! # Ok(())
//! # }
//! ```

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use spider_route::{AttemptOutcome, FeatureVector, RouteDecision, RouteSource, StatusClass};

/// Takes note of one routing decision and how it turned out.
///
/// Called once per routed attempt, after the outcome is known. An
/// implementation runs on the calling task, so it should return quickly or hand
/// the row somewhere else.
pub trait Recorder: Send + Sync {
    /// Record one decision.
    fn observe(&self, features: &FeatureVector, chosen: &RouteDecision, outcome: &AttemptOutcome);
}

impl<T: Recorder + ?Sized> Recorder for Arc<T> {
    fn observe(&self, features: &FeatureVector, chosen: &RouteDecision, outcome: &AttemptOutcome) {
        (**self).observe(features, chosen, outcome);
    }
}

impl<T: Recorder + ?Sized> Recorder for Box<T> {
    fn observe(&self, features: &FeatureVector, chosen: &RouteDecision, outcome: &AttemptOutcome) {
        (**self).observe(features, chosen, outcome);
    }
}

/// The version stamped on every row, so a reader knows what the columns mean.
pub const ROW_VERSION: u32 = 1;

/// Writes one JSON object per line.
///
/// The writer is held by shared reference rather than by mutable reference,
/// which is what lets rows be written from any task without a lock. A
/// [`File`] is the case this was built for: appending to one goes through a
/// single write for a row of this size, so two tasks writing at once produce
/// two whole lines rather than one torn one.
///
/// Anything else that writes through a shared reference works too. A type that
/// needs `&mut` to write does not, on purpose: making one work would mean
/// holding a lock across the write.
pub struct JsonlRecorder<W> {
    writer: W,
}

impl JsonlRecorder<File> {
    /// Append rows to a file, creating it if it is not there.
    pub fn create(path: impl AsRef<Path>) -> std::io::Result<JsonlRecorder<File>> {
        let file = File::options().create(true).append(true).open(path)?;
        Ok(JsonlRecorder::new(file))
    }
}

impl<W> JsonlRecorder<W> {
    /// Write rows to this writer.
    pub fn new(writer: W) -> JsonlRecorder<W> {
        JsonlRecorder { writer }
    }

    /// The writer, back again.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W> std::fmt::Debug for JsonlRecorder<W> {
    /// Says what it is and nothing about where it points. A path is not a
    /// secret, but nothing here needs one to be printed either.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JsonlRecorder")
    }
}

impl<W> Recorder for JsonlRecorder<W>
where
    W: Send + Sync,
    for<'a> &'a W: Write,
{
    fn observe(&self, features: &FeatureVector, chosen: &RouteDecision, outcome: &AttemptOutcome) {
        let mut line = row(features, chosen, outcome);
        line.push('\n');

        // A row that cannot be written is dropped. Collecting training data is
        // not what the caller asked for, and a full disk is not a reason to
        // fail a page that came back fine.
        let mut writer = &self.writer;
        if let Err(error) = writer.write_all(line.as_bytes()) {
            log::debug!("a routing row was not written: {error}");
        }
    }
}

/// One row, as a JSON object.
///
/// Written by hand rather than derived, so that adding a field is a deliberate
/// edit in this function and shows up in review as one.
fn row(features: &FeatureVector, chosen: &RouteDecision, outcome: &AttemptOutcome) -> String {
    let slots = features.as_slice();
    let mut out = String::with_capacity(slots.len() * 4 + 256);

    out.push_str("{\"v\":");
    out.push_str(&ROW_VERSION.to_string());
    out.push_str(",\"mode\":\"");
    out.push_str(chosen.mode().as_str());
    out.push_str("\",\"proxy\":\"");
    out.push_str(chosen.proxy().as_str());
    out.push_str("\",\"wait_ms\":");
    out.push_str(&chosen.wait().millis().to_string());
    out.push_str(",\"start_rung\":");
    out.push_str(&chosen.start_rung.to_string());
    out.push_str(",\"source\":\"");
    out.push_str(source_label(chosen.source));
    out.push_str("\",\"confidence\":");
    out.push_str(&number(chosen.confidence));
    out.push_str(",\"success\":");
    out.push_str(if outcome.success { "true" } else { "false" });
    out.push_str(",\"status\":\"");
    out.push_str(status_label(outcome.status));
    out.push_str("\",\"bytes\":");
    out.push_str(&outcome.bytes.to_string());
    out.push_str(",\"millis\":");
    out.push_str(&outcome.millis.to_string());
    out.push_str(",\"features\":[");

    for (at, value) in slots.iter().enumerate() {
        if at > 0 {
            out.push(',');
        }
        out.push_str(&number(*value));
    }

    out.push_str("]}");
    out
}

/// A float as JSON, with the ones that JSON cannot hold written as zero.
fn number(value: f32) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "0".to_string()
    }
}

/// The name a decision's source is written under.
fn source_label(source: RouteSource) -> &'static str {
    match source {
        RouteSource::Heuristic => "heuristic",
        RouteSource::Model => "model",
        RouteSource::Caller => "caller",
        RouteSource::Memory => "memory",
        RouteSource::Explore => "explore",
        // The router crate may name a source this version has not heard of.
        _ => "other",
    }
}

/// The name an outcome's status class is written under.
fn status_label(status: StatusClass) -> &'static str {
    match status {
        StatusClass::Unknown => "unknown",
        StatusClass::Ok => "ok",
        StatusClass::Empty => "empty",
        StatusClass::BadRequest => "bad_request",
        StatusClass::NeedsLogin => "needs_login",
        StatusClass::Blocked => "blocked",
        StatusClass::NotFound => "not_found",
        StatusClass::RateLimited => "rate_limited",
        StatusClass::ServerError => "server_error",
        _ => "other",
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
    use spider_route::{featurize, Action, DeclaredNeed, ProxyPool, RequestMode, RouteInput};
    use url::Url;

    fn decision() -> RouteDecision {
        RouteDecision::new(
            Action::new(RequestMode::Browser).with_proxy(ProxyPool::Residential),
            RouteSource::Memory,
            0.8,
        )
        .starting_at(2)
    }

    /// A file of this test's own, removed on the way in and on the way out.
    struct Rows {
        path: std::path::PathBuf,
    }

    impl Rows {
        fn new(name: &str) -> Rows {
            let path = std::env::temp_dir().join(format!(
                "spider-cloud-agent-{name}-{}.jsonl",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&path);
            Rows { path }
        }

        fn recorder(&self) -> JsonlRecorder<File> {
            JsonlRecorder::create(&self.path).expect("a file to write rows to")
        }

        fn read(&self) -> String {
            std::fs::read_to_string(&self.path).unwrap_or_default()
        }
    }

    impl Drop for Rows {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn a_row_is_one_line_of_json_with_every_feature_slot_in_it() {
        let url = Url::parse("https://example.com/a/b").expect("a url");
        let features = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown));
        let line = row(&features, &decision(), &AttemptOutcome::ok(1_234, 567));

        assert!(!line.contains('\n'));
        assert!(line.starts_with("{\"v\":1,"));
        assert!(line.contains("\"mode\":\"browser\""));
        assert!(line.contains("\"proxy\":\"residential\""));
        assert!(line.contains("\"source\":\"memory\""));
        assert!(line.contains("\"start_rung\":2"));
        assert!(line.contains("\"bytes\":1234"));
        assert!(line.contains("\"millis\":567"));

        let values = line
            .split_once("\"features\":[")
            .expect("a feature array")
            .1
            .trim_end_matches("]}")
            .split(',')
            .count();
        assert_eq!(values, features.as_slice().len());
    }

    #[test]
    fn a_recorded_row_holds_no_host() {
        // A name that could not appear by accident: it is not a word, not a
        // suffix in any list, and not a substring of anything this crate
        // writes.
        const DISTINCTIVE: &str = "zqxwvutsrqponmlk";
        let url = Url::parse(&format!(
            "https://shop.{DISTINCTIVE}.com/checkout/{DISTINCTIVE}-item?ref={DISTINCTIVE}"
        ))
        .expect("a url");

        let rows = Rows::new("no-host");
        let features = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown));
        rows.recorder().observe(
            &features,
            &decision(),
            &AttemptOutcome::failed(StatusClass::Blocked, 900),
        );

        let out = rows.read();
        assert!(
            !out.is_empty(),
            "nothing was written, so nothing was checked"
        );
        assert!(
            !out.contains(DISTINCTIVE),
            "the host reached the row: {out}"
        );
        assert!(!out.contains(".com"), "a suffix reached the row: {out}");
        assert!(!out.contains("http"), "the address reached the row: {out}");
        assert!(out.contains("\"status\":\"blocked\""), "{out}");
        assert!(out.ends_with('\n'), "a row has to end its line");
    }

    #[test]
    fn rows_append_rather_than_replace() {
        let rows = Rows::new("append");
        let recorder = rows.recorder();
        let url = Url::parse("https://example.com/a").expect("a url");
        let features = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown));

        for _ in 0..3 {
            recorder.observe(&features, &decision(), &AttemptOutcome::ok(10, 10));
        }

        assert_eq!(rows.read().lines().count(), 3);
    }

    #[test]
    fn a_boxed_recorder_records() {
        let rows = Rows::new("boxed");
        let recorder: Box<dyn Recorder> = Box::new(rows.recorder());
        let url = Url::parse("https://example.com/a").expect("a url");
        let features = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown));

        recorder.observe(&features, &decision(), &AttemptOutcome::ok(10, 10));
        assert_eq!(rows.read().lines().count(), 1);
    }

    #[test]
    fn an_unwritable_row_is_dropped_rather_than_raised() {
        // A handle opened for reading rejects every write, which is the state
        // a recorder must survive: it cannot write, and that is not a reason to
        // fail a page that came back. Opening a directory would do the same on
        // unix and is denied outright on windows, so this takes the portable
        // route of a real file opened read only.
        let path = std::env::temp_dir().join("spider-cloud-agent-unwritable.jsonl");
        File::create(&path).expect("a file to reopen read only");
        let recorder =
            JsonlRecorder::new(File::open(&path).expect("a read only handle on that file"));
        let url = Url::parse("https://example.com/a").expect("a url");
        let features = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown));

        recorder.observe(&features, &decision(), &AttemptOutcome::ok(10, 10));

        // Surviving the write is half of it. The file staying empty is what
        // proves the write was refused, so that a handle which quietly accepted
        // the row could not pass this test by not panicking.
        let written = std::fs::metadata(&path)
            .expect("the file still exists")
            .len();
        let _ = std::fs::remove_file(&path);
        assert_eq!(written, 0, "the read only handle accepted a write");
    }
}
