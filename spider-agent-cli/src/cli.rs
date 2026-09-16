//! The command line, as clap reads it.
//!
//! Help text is copy. It says what a flag does and what it costs, and it names
//! the output contract where a caller would otherwise have to guess.

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Fetch, crawl, search and extract, choosing transport locally and stopping on
/// a budget.
#[derive(Debug, Parser)]
#[command(
    name = "spider-agent",
    version,
    about = "Fetch, crawl, search and extract with Spider Cloud.",
    long_about = "Fetch, crawl, search and extract with Spider Cloud.

Transport is chosen locally before a call goes out, escalation answers the \
status the site returned, and a budget you set stops the run. No large model \
is called and no model key is read.

Results go to stdout and diagnostics go to stderr, so a pipe carries the \
payload and nothing else. --ndjson writes one object per line, flushed as each \
arrives, and that schema is a contract: `spider-agent schema` prints it.

Exit codes: 0 done, 1 failed, 2 usage, 3 auth, 4 budget, 5 the site refused, \
6 transport, 7 output refused.

The key comes from SPIDER_API_KEY, then SPIDER_CLOUD_API_KEY, then \
~/.spider/credentials. There is no flag for it, because an argument is \
readable in the process list.",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Settings every command reads.
    #[command(flatten)]
    pub global: Global,

    /// Addresses to read when no command is named.
    #[command(flatten)]
    pub default: ScrapeArgs,

    /// What to do.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// The commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Read one page or a list of them. What the bare form runs.
    Scrape(ScrapeArgs),
    /// Read one path under the config the service holds for it.
    #[command(long_about = "Read one path under the config the service holds for it.

This is not scrape. Scrape sends an address and the settings you chose. Fetch \
names a host and a path, and the service answers with a config it already \
worked out for that path: which fields to pull, whether the page needs a \
browser, what to wait for.

The first call on a path nobody has asked for yet goes away and works one out, \
which takes far longer than a scrape and can fail while it tries. The query \
string is not part of the target, so a path carrying one reaches the page \
without it. Reach for scrape unless you want the stored config.")]
    Fetch(FetchArgs),
    /// Read a site, following its links.
    Crawl(CrawlArgs),
    /// Pull named fields out of pages. The page body never crosses the wire.
    Extract(ExtractArgs),
    /// Collect the links on a page without paying for its content.
    Links(LinksArgs),
    /// Run a query.
    Search(SearchArgs),
    /// Take a picture of a page. Needs --output or --output-dir.
    Screenshot(ScreenshotArgs),
    /// Convert markup you already hold, without fetching anything.
    Transform(TransformArgs),
    /// Work a goal over one or more addresses until it is met or the budget
    /// stops it.
    Run(RunArgs),
    /// What is left on the account.
    Credits,
    /// The record of past crawls.
    Logs(RowsArgs),
    /// The sites configured on the account.
    #[command(long_about = "The sites configured on the account.

One row per site, with the settings the service holds for it. Columns: \
anti_bot, blacklist, cache, crawl_budget, created_at, domain, fts, \
full_resources, gpt_config, headless, id, last_checked_at, metadata, \
proxy_enabled, return_format, scheme, smart_mode, subdomains, tld, \
updated_at, url, user_id, whitelist.

The columns belong to the service and change without a release of this tool, \
so a row is passed through as it arrived.")]
    Sites(RowsArgs),
    /// The API keys on the account, as metadata. No key is returned.
    #[command(long_about = "The API keys on the account, as metadata.

No key is returned. There is no token, secret or hash column in this reply, \
and token_name is the label you gave a key rather than any part of it. \
Columns: created_at, id, last_used, team_id, team_member_id, token_name, \
updated_at, user_id.

Use it to find a key that has not been used in months, or to count what is \
issued against the account. Rotating or revoking one is done on the \
dashboard, not here.")]
    Keys(RowsArgs),
    /// The account itself: plan limits, totals and billing caps.
    #[command(
        long_about = "The account itself: plan limits, totals and billing caps.

One row. Columns include ai_function_calls_limit, ai_requests_limit, \
ai_tokens_limit, approved_usage, auto_recharge, auto_recharge_threshold, \
billing_allowed, billing_limit, billing_limit_soft, disabled_at, \
disabled_reason, email, email_alert_percent, has_stripe, has_subscription, \
id, provider, total_crawl_count, total_credits_spent, usage and user_role.

For what is left to spend right now, `credits` is the shorter answer."
    )]
    Profile,
    /// Sign in through a browser and store the key.
    Login(LoginArgs),
    /// What transport would be chosen for an address. Local, no call, no spend.
    Route(RouteArgs),
    /// The command tree and the record schema, as JSON.
    Schema,
}

/// Settings every command reads.
#[derive(Debug, Args)]
pub struct Global {
    /// Write one JSON document. Buffers, so nothing is written until the run
    /// ends.
    #[arg(long, global = true, conflicts_with_all = ["ndjson", "format"])]
    pub json: bool,

    /// Write one JSON object per line, flushed as each arrives.
    #[arg(long, global = true, conflicts_with = "format")]
    pub ndjson: bool,

    /// How results are written. Each command has its own default.
    #[arg(long, global = true, value_enum)]
    pub format: Option<Format>,

    /// Write results to this file instead of stdout. A dash is stdout.
    #[arg(short = 'o', long, global = true, value_name = "FILE")]
    pub output: Option<String>,

    /// Write one file per page into this directory, named from the address.
    #[arg(
        short = 'd',
        long,
        global = true,
        value_name = "DIR",
        conflicts_with = "output"
    )]
    pub output_dir: Option<String>,

    /// Append to the output file rather than replacing it.
    #[arg(long, global = true, requires = "output")]
    pub append: bool,

    /// Create the parent directories of the output path.
    #[arg(long, global = true)]
    pub mkdir: bool,

    /// Overwrite a file that is already there.
    #[arg(long, global = true)]
    pub force: bool,

    /// The most the whole run may spend, in credits. The first call on a page
    /// always goes out, because a price is not known until it is asked for.
    /// The cap stops the escalations after it and the pages after that.
    #[arg(long, global = true, value_name = "CREDITS", value_parser = credits)]
    pub budget: Option<f64>,

    /// The most any one page may spend, in credits. Fractions survive here.
    #[arg(long, global = true, value_name = "CREDITS", value_parser = credits)]
    pub max_per_page: Option<f64>,

    /// The most calls one page may take, first attempt and escalations
    /// together.
    #[arg(long, global = true, value_name = "N")]
    pub attempts: Option<u8>,

    /// The longest the run may take, in seconds, waits included.
    #[arg(long, global = true, value_name = "SECONDS")]
    pub wall: Option<u64>,

    /// The most to hand back, in approximate tokens, shared across the pages.
    #[arg(long, global = true, value_name = "N")]
    pub max_tokens: Option<usize>,

    /// Fix how the page is fetched. Left alone, the router picks it.
    #[arg(long, global = true, value_enum)]
    pub mode: Option<Mode>,

    /// Ask for the links on every page as well as its content. They come back
    /// in the same call rather than costing another.
    #[arg(long, global = true)]
    pub with_links: bool,

    /// Fix which pool the request leaves from.
    #[arg(long, global = true, value_enum)]
    pub proxy: Option<Pool>,

    /// The country to appear to be in, as a two letter code.
    #[arg(long, global = true, value_name = "CODE")]
    pub country: Option<String>,

    /// How long one page has to come back, in seconds.
    #[arg(long, global = true, value_name = "SECONDS")]
    pub timeout: Option<u64>,

    /// Keep cookies and headers across the requests made to one site.
    #[arg(long, global = true)]
    pub session: bool,

    /// Print nothing on stderr but failures.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Print a line on stderr for every page and every escalation.
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

/// An amount of credits a caller typed.
///
/// `f64` reads `nan`, `inf` and a negative number as numbers. As a cap, a
/// NaN compares false against everything and a negative is spent before the
/// first page, so each is a cap that does not do what the caller who typed
/// one meant. Refused here, where the answer is a usage failure.
fn credits(text: &str) -> Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("{text} is not a number of credits"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!(
            "{text} is not a number of credits. Give a number that is zero or more."
        ));
    }
    Ok(value)
}

/// How results are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// The content alone, with no wrapper.
    Text,
    /// One JSON document for the whole run.
    Json,
    /// One JSON object per line.
    Ndjson,
}

/// How a page is fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Plain HTTP. Cheapest, and enough for most pages.
    Http,
    /// The service decides per page.
    Smart,
    /// A full browser.
    Browser,
}

/// Which pool a request leaves from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Pool {
    /// The cheaper pool.
    Isp,
    /// Residential addresses, dearer per page.
    Residential,
}

/// What the caller wants back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Goal {
    /// The page as plain text, furniture stripped.
    Text,
    /// The page as markdown.
    Markdown,
    /// The markup, cleaned of scripts, styles and comments.
    Html,
    /// The links, and none of the content.
    Links,
    /// Title, description and the rest of the declared metadata.
    Metadata,
    /// Named fields only. Needs --selectors, and no page bytes cross the wire.
    Fields,
    /// A picture of the page.
    Screenshot,
    /// Whatever the service returns on its own, with nothing switched off.
    Raw,
}

/// Addresses, however they arrive.
#[derive(Debug, Args)]
pub struct Targets {
    /// Addresses to work on.
    #[arg(value_name = "URL")]
    pub urls: Vec<String>,

    /// Read addresses from a file, one per line. Use - for stdin.
    #[arg(long, value_name = "FILE")]
    pub urls_from: Option<String>,
}

/// Read one page or a list of them.
#[derive(Debug, Args)]
pub struct ScrapeArgs {
    /// Where to read from.
    #[command(flatten)]
    pub targets: Targets,

    /// What to ask for. Asking for less is where the saving is.
    #[arg(long, value_enum, default_value = "markdown")]
    pub goal: Goal,

    /// Named fields to pull out, as a JSON object of name to selector. Use -
    /// for stdin. Implies --goal fields.
    #[arg(long, value_name = "FILE")]
    pub selectors: Option<String>,
}

/// Read one path under the config the service holds for it.
#[derive(Debug, Args)]
pub struct FetchArgs {
    /// The host, with no scheme. Example: example.com
    #[arg(value_name = "DOMAIN")]
    pub domain: String,

    /// The path on that host. Defaults to the root.
    #[arg(value_name = "PATH", default_value = "/")]
    pub path: String,

    /// What to ask for. Defaults to raw, which leaves the stored config to
    /// decide. Naming anything else argues with it and usually wins.
    #[arg(long, value_enum, default_value = "raw")]
    pub goal: Goal,
}

/// Fetch a site, following its links.
#[derive(Debug, Args)]
pub struct CrawlArgs {
    /// Where to start.
    #[command(flatten)]
    pub targets: Targets,

    /// What to ask for on every page.
    #[arg(long, value_enum, default_value = "markdown")]
    pub goal: Goal,

    /// Named fields to pull off every page. Use - for stdin.
    #[arg(long, value_name = "FILE")]
    pub selectors: Option<String>,

    /// How many pages the crawl may visit. A crawl with no limit reads the
    /// whole site.
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,

    /// How many links deep from the starting page the crawl may go.
    #[arg(long, value_name = "N")]
    pub depth: Option<u32>,
}

/// Pull named fields out of pages.
#[derive(Debug, Args)]
pub struct ExtractArgs {
    /// Which pages to read.
    #[command(flatten)]
    pub targets: Targets,

    /// A JSON object of field name to selector, or to a list of selectors
    /// tried in order. A selector starting with a slash is read as XPath. Use
    /// - for stdin.
    #[arg(long, value_name = "FILE", required = true)]
    pub selectors: String,

    /// The path the fields apply to. Worth setting on a crawl, where a listing
    /// page and a product page hold different fields.
    #[arg(long, value_name = "PATH")]
    pub at: Option<String>,
}

/// Collect the links on a page.
#[derive(Debug, Args)]
pub struct LinksArgs {
    /// Which pages to read.
    #[command(flatten)]
    pub targets: Targets,
}

/// Run a query.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// What to search for.
    #[arg(value_name = "QUERY", required = true)]
    pub query: Vec<String>,

    /// How many results to work through.
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,

    /// Read the page behind every result. One fetch per result, so a query
    /// returning fifty of them is fifty fetches.
    #[arg(long)]
    pub fetch_pages: bool,
}

/// Take a picture of a page.
#[derive(Debug, Args)]
pub struct ScreenshotArgs {
    /// Which pages to shoot.
    #[command(flatten)]
    pub targets: Targets,
}

/// Convert markup you already hold.
#[derive(Debug, Args)]
pub struct TransformArgs {
    /// The markup to convert. Use - for stdin.
    #[arg(long, value_name = "FILE", required = true)]
    pub input: String,

    /// Where the markup came from, so links resolve.
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,

    /// The shape to convert into.
    #[arg(long = "to", value_enum, default_value = "markdown")]
    pub to: Goal,

    /// Strip navigation, adverts and the rest of the furniture first.
    #[arg(long)]
    pub readability: bool,
}

/// Work a goal over addresses until it is met or the budget stops it.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Where to start.
    #[command(flatten)]
    pub targets: Targets,

    /// What counts as done for every address.
    #[arg(long, value_enum, default_value = "markdown")]
    pub goal: Goal,

    /// Named fields, as a JSON object. Use - for stdin. Implies --goal fields.
    #[arg(long, value_name = "FILE")]
    pub selectors: Option<String>,

    /// Follow up to this many same-host links found on the pages already read.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub expand: usize,

    /// Stop after this many pages, however they were reached.
    #[arg(long, value_name = "N")]
    pub max_pages: Option<usize>,

    /// Read goal, addresses and caps from a JSON file. Use - for stdin.
    /// Anything named on the command line wins.
    #[arg(long, value_name = "FILE")]
    pub plan: Option<String>,
}

/// Read rows off the account.
#[derive(Debug, Args)]
pub struct RowsArgs {
    /// How many rows to read.
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,

    /// Which page of rows, counting from one.
    #[arg(long, value_name = "N")]
    pub page: Option<u32>,
}

impl RowsArgs {
    /// Paging for a read that answers with one row, where a limit and a page
    /// would mean nothing.
    pub const fn none() -> RowsArgs {
        RowsArgs {
            limit: None,
            page: None,
        }
    }
}

/// Sign in through a browser.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Print the key instead of storing it. It goes to stdout and nowhere
    /// else, so redirect it or do not use it.
    #[arg(long)]
    pub print: bool,
}

/// Ask what transport would be chosen.
#[derive(Debug, Args)]
pub struct RouteArgs {
    /// The addresses to decide about.
    #[command(flatten)]
    pub targets: Targets,

    /// What the request would ask for, which changes the answer.
    #[arg(long, value_enum, default_value = "markdown")]
    pub goal: Goal,
}
