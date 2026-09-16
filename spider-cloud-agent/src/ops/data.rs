//! Reads against the account rather than against a site.
//!
//! These endpoints answer about your own records: what is left on the balance,
//! what has been crawled, what a stored table holds. None of them fetches a
//! page, so none of them escalates and none of them costs credits.

use std::future::Future;
use std::time::Duration;

use crate::client::Spider;
use crate::credits::Credits;
use crate::error::Error;
use crate::ops::{out_of_time, within, Deadline};
use crate::transport::{route, Reply};
use crate::Result;

/// How long an account read may take unless the client's wall is shorter.
///
/// A balance or a page of the crawl record is a database read, and one that
/// has not answered in a minute is not going to. The page operations have a
/// longer default of their own, because a crawl runs for as long as the site
/// is large.
pub(crate) const DEFAULT_READ_WALL: Duration = Duration::from_secs(60);

/// One account read, held to the client's wall and judged on the call plane.
///
/// These reads take no builder budget, so the wall on the client is what
/// bounds them: [`DEFAULT_READ_WALL`], or the client's own wall when that is
/// shorter, and nothing at all only when the client was built with
/// `without_wall`. A read against a service that accepted the request and
/// went quiet used to wait for as long as the socket stayed open.
pub(crate) async fn read_under_wall<T, F, D>(spider: &Spider, call: F, decode: D) -> Result<T>
where
    F: Future<Output = Result<Reply>>,
    D: FnOnce(&Reply) -> Result<T>,
{
    let deadline = Deadline::new(spider.read_wall)?;
    let reply = within(deadline, call)
        .await
        .ok_or_else(|| out_of_time(Vec::new()))??
        .into_result()?;
    let value = decode(&reply)?;
    if deadline.expired() {
        return Err(out_of_time(Vec::new()));
    }
    Ok(value)
}

/// The balance, read out of whichever shape the reply used.
///
/// The service sends `{"data": {"credits": "1234.5"}}`, with the amount as a
/// quoted decimal rather than a number, which is how it avoids losing precision
/// on a value it bills against. Bare numbers and the unwrapped object are read
/// too, because earlier guesses at this shape are in the wild.
pub(crate) fn credits_from(reply: &Reply) -> Result<Credits> {
    let value: serde_json::Value = reply.json()?;
    if let Some(credits) = read_credits(&value) {
        return Ok(credits);
    }
    if let Some(credits) = value.get("data").and_then(read_credits) {
        return Ok(credits);
    }
    Err(Error::Decode(serde::de::Error::custom(
        "the balance reply carried no credits field",
    )))
}

fn read_credits(value: &serde_json::Value) -> Option<Credits> {
    if let Some(credits) = as_amount(value) {
        return Some(credits);
    }
    if let serde_json::Value::Array(items) = value {
        return items.first().and_then(read_credits);
    }
    for key in ["credits", "credits_remaining", "balance"] {
        if let Some(credits) = value.get(key).and_then(as_amount) {
            return Some(credits);
        }
    }
    None
}

/// A credit amount, whether it arrived as a number or as a quoted decimal.
///
/// The live service quotes it. Reading only numbers here failed against the
/// real account with a message that blamed a missing field, which is the worst
/// kind of error: it names the wrong cause.
fn as_amount(value: &serde_json::Value) -> Option<Credits> {
    if let Some(number) = value.as_f64() {
        return Some(Credits(number));
    }
    value.as_str()?.trim().parse::<f64>().ok().map(Credits)
}

/// A read of the crawl record.
///
/// Built by [`Spider::crawl_logs`]. Rows come back as JSON, because the columns
/// belong to the service and change without a release of this crate.
#[derive(Debug)]
pub struct CrawlLogs<'a> {
    spider: &'a Spider,
    limit: Option<u32>,
    page: Option<u32>,
}

impl<'a> CrawlLogs<'a> {
    pub(crate) fn new(spider: &'a Spider) -> CrawlLogs<'a> {
        CrawlLogs {
            spider,
            limit: None,
            page: None,
        }
    }

    /// How many rows to read.
    pub fn limit(mut self, rows: u32) -> CrawlLogs<'a> {
        self.limit = Some(rows);
        self
    }

    /// Which page of rows to read, counting from one.
    pub fn page(mut self, page: u32) -> CrawlLogs<'a> {
        self.page = Some(page);
        self
    }

    /// Read the rows.
    pub async fn send(self) -> Result<Vec<serde_json::Value>> {
        let query = paging(self.limit, self.page);
        let pairs: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        read_under_wall(
            self.spider,
            self.spider.raw().get_with_limit(
                route::DATA_CRAWL_LOGS,
                &[],
                &pairs,
                self.spider.response_limit,
            ),
            rows,
        )
        .await
    }
}

/// A read of one stored table.
///
/// Built by [`Spider::table`].
#[derive(Debug)]
pub struct Table<'a> {
    spider: &'a Spider,
    name: String,
    limit: Option<u32>,
    page: Option<u32>,
}

impl<'a> Table<'a> {
    pub(crate) fn new(spider: &'a Spider, name: impl Into<String>) -> Table<'a> {
        Table {
            spider,
            name: name.into(),
            limit: None,
            page: None,
        }
    }

    /// How many rows to read.
    pub fn limit(mut self, rows: u32) -> Table<'a> {
        self.limit = Some(rows);
        self
    }

    /// Which page of rows to read, counting from one.
    pub fn page(mut self, page: u32) -> Table<'a> {
        self.page = Some(page);
        self
    }

    /// Read the rows.
    pub async fn send(self) -> Result<Vec<serde_json::Value>> {
        let query = paging(self.limit, self.page);
        let pairs: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        read_under_wall(
            self.spider,
            self.spider.raw().get_with_limit(
                route::DATA_TABLE,
                &[&self.name],
                &pairs,
                self.spider.response_limit,
            ),
            rows,
        )
        .await
    }
}

fn paging(limit: Option<u32>, page: Option<u32>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(limit) = limit {
        out.push(("limit", limit.to_string()));
    }
    if let Some(page) = page {
        out.push(("page", page.to_string()));
    }
    out
}

/// Rows out of a reply, whether the service sent a bare list or wrapped it.
///
/// A read that answers with one thing rather than a list wraps it the same way:
/// `/data/profiles` sends `{"data": {...}}` where the crawl record sends
/// `{"data": [...]}`. Both unwrap to rows, so a caller gets the columns either
/// way instead of one row whose only column is `data`.
fn rows(reply: &Reply) -> Result<Vec<serde_json::Value>> {
    let value: serde_json::Value = reply.json()?;
    Ok(match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Null => Vec::new(),
        ref object => match object.get("data") {
            Some(serde_json::Value::Array(items)) => items.clone(),
            Some(inner @ serde_json::Value::Object(_)) => vec![inner.clone()],
            _ => vec![value],
        },
    })
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
    use crate::status::ApiStatus;
    use crate::transport::RateLimit;
    use bytes::Bytes;
    use std::time::Duration;

    fn reply(body: &str) -> Reply {
        Reply {
            status: ApiStatus::new(200),
            rate_limit: RateLimit::default(),
            retry_after: None,
            elapsed: Duration::from_millis(10),
            content_type: Some("application/json".into()),
            body: Bytes::from(body.to_string()),
        }
    }

    #[test]
    fn a_balance_reads_from_every_shape_the_service_uses() {
        assert_eq!(credits_from(&reply("1250.5")).unwrap(), Credits(1250.5));

        // The shape the live service actually sends, confirmed against a real
        // account on 2026-09-15. The amount is quoted, not a number.
        assert_eq!(
            credits_from(&reply(r#"{"data":{"credits":"1250.5","team_id":null}}"#)).unwrap(),
            Credits(1250.5)
        );
        assert_eq!(
            credits_from(&reply(r#"{"credits":"40"}"#)).unwrap(),
            Credits(40.0)
        );
        assert_eq!(
            credits_from(&reply(r#"{"credits":40}"#)).unwrap(),
            Credits(40.0)
        );
        assert_eq!(
            credits_from(&reply(r#"{"data":{"credits":7}}"#)).unwrap(),
            Credits(7.0)
        );
        assert_eq!(
            credits_from(&reply(r#"[{"credits":3}]"#)).unwrap(),
            Credits(3.0)
        );
    }

    #[test]
    fn a_balance_reply_with_no_number_is_an_error_rather_than_zero() {
        assert!(credits_from(&reply(r#"{"note":"nothing here"}"#)).is_err());
    }

    #[test]
    fn rows_read_wrapped_and_unwrapped() {
        assert_eq!(rows(&reply("[{\"a\":1},{\"a\":2}]")).unwrap().len(), 2);
        assert_eq!(rows(&reply("{\"data\":[{\"a\":1}]}")).unwrap().len(), 1);
        assert_eq!(rows(&reply("null")).unwrap().len(), 0);
    }

    /// The shape `/data/profiles` sends, confirmed against a real account on
    /// 2026-09-15: one object under `data` rather than a list. Passing the
    /// wrapper through as the row gave a row whose only column was `data`.
    #[test]
    fn a_single_object_under_data_is_one_row_with_its_own_columns() {
        let read = rows(&reply(r#"{"data":{"email":"a@b.c","usage":3}}"#)).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0]["usage"], serde_json::json!(3));
        assert!(
            read[0].get("data").is_none(),
            "the wrapper was passed through as the row: {}",
            read[0]
        );
    }

    /// A reply that is an object and carries no `data` is still one row, which
    /// is how a shape nobody planned for reaches the caller rather than
    /// vanishing.
    #[test]
    fn an_object_with_no_data_key_is_still_one_row() {
        let read = rows(&reply(r#"{"teams":[]}"#)).unwrap();
        assert_eq!(read.len(), 1);
        assert!(read[0].get("teams").is_some());
    }

    #[test]
    fn paging_only_sends_what_was_asked_for() {
        assert!(paging(None, None).is_empty());
        assert_eq!(paging(Some(10), None), vec![("limit", "10".to_string())]);
        assert_eq!(
            paging(Some(10), Some(2)),
            vec![("limit", "10".to_string()), ("page", "2".to_string())]
        );
    }
}
