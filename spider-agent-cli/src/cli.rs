//! The command line, as clap reads it.
//!
//! Help text is copy. It says what a flag does and what it costs, and it names
//! the output contract where a caller would otherwise have to guess.

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Shared by help and schema so callers see the same resolution order.
pub const KEY_RESOLUTION: &str = "The key comes from SPIDER_API_KEY, then SPIDER_CLOUD_API_KEY, then the keychain, when the binary was built with the keyring feature, then ~/.spider/credentials.";

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

There is no flag for the key, because an argument is \
readable in the process list.",
    after_long_help = KEY_RESOLUTION,
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
    /// Store a provider fallback that every page request carries.
    #[command(
        long_about = "Store a provider fallback that every page request carries.

The request parameter router names an outside scraping provider and how it \
takes part: fallback puts it behind Spider's own fetch, tried only when that \
fails; first puts it in front, and needs a provider and its key; off keeps the \
request away from outside providers. funding own allows only routes your own \
keys pay for. Providers that take one token: firecrawl, zenrows, scrapingbee, \
scraperapi, brightdata, zyte, apify, crawlbase, diffbot. oxylabs and \
dataforseo take a username and password as credentials instead.

The config lives in ~/.spider/router.json, readable by its owner only. Every \
command that fetches pages sends it unless the request already names a router. \
Pass --no-router, or set SPIDER_AGENT_NO_ROUTER to any value, to skip it for \
one run."
    )]
    Router(RouterArgs),
    /// What transport would be chosen for an address. Local, no call, no spend.
    Route(RouteArgs),
    /// The command tree and the record schema, as JSON.
    Schema,
    /// Install the newest release now instead of on a later run.
    #[command(
        long_about = "Install the newest release now instead of on a later run.

Once a day, a run checks for a newer release in the background, downloads it, \
checks it against the release's SHA256SUMS.txt and the minisign signature \
on that file, and leaves it beside the \
binary. The next run moves it into place and carries on under the new \
version. This command does all of that at once and says what it did on \
stderr.

Exit codes: 0 installed or already the newest, 1 the release was refused or \
the install failed, 2 self update is turned off, 6 the release host could not \
be reached, 7 this binary belongs to cargo, a package manager, or a directory \
this user cannot write, so it is left alone.

Set SPIDER_AGENT_NO_UPDATE to any value, or pass --no-update, to turn self \
update off, including the background check and an update already waiting. \
With CI set, runs skip the background check and the waiting update, and this \
command still works."
    )]
    Update,
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
    #[arg(long, global = true, value_name = "MODE", value_parser = fetch_modes())]
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

    /// Do not check for, download or install a new release on this run. The
    /// same as setting SPIDER_AGENT_NO_UPDATE.
    #[arg(long, global = true)]
    pub no_update: bool,

    /// Send no stored provider fallback on this run. The same as setting
    /// SPIDER_AGENT_NO_ROUTER.
    #[arg(long, global = true)]
    pub no_router: bool,

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
pub(crate) fn credits(text: &str) -> Result<f64, String> {
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

/// A `--mode` value.
///
/// Two flags are spelled `--mode`: the global one, for how a page is fetched,
/// and the one on `router set`, for how a provider takes part. clap keeps one
/// slot per name and hands a value on `router set` up to the global slot, so
/// both flags parse into this one type, and each accepts only its own names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Plain HTTP. Cheapest, and enough for most pages.
    Http,
    /// The service decides per page.
    Smart,
    /// A full browser.
    Browser,
    /// A provider behind Spider's own fetch, tried only when that fails.
    Fallback,
    /// A provider in front of Spider's own fetch.
    First,
    /// No outside provider.
    Off,
}

/// The global `--mode`: how a page is fetched.
fn fetch_modes() -> impl clap::builder::TypedValueParser<Value = Mode> {
    use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};
    PossibleValuesParser::new([
        PossibleValue::new("http").help("Plain HTTP. Cheapest, and enough for most pages"),
        PossibleValue::new("smart").help("The service decides per page"),
        PossibleValue::new("browser").help("A full browser"),
    ])
    .try_map(|name| match name.as_str() {
        "http" => Ok(Mode::Http),
        "smart" => Ok(Mode::Smart),
        "browser" => Ok(Mode::Browser),
        other => Err(format!("{other} is not a fetch mode")),
    })
}

/// `router set --mode`: how a provider takes part.
fn router_modes() -> impl clap::builder::TypedValueParser<Value = Mode> {
    use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};
    PossibleValuesParser::new([
        PossibleValue::new("fallback")
            .help("Behind Spider's own fetch, tried only when that fails"),
        PossibleValue::new("first")
            .help("In front of Spider's own fetch. Needs a provider and its key"),
        PossibleValue::new("off").help("No outside provider"),
    ])
    .try_map(|name| match name.as_str() {
        "fallback" => Ok(Mode::Fallback),
        "first" => Ok(Mode::First),
        "off" => Ok(Mode::Off),
        other => Err(format!("{other} is not a router mode")),
    })
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
    #[arg(long, value_enum)]
    pub goal: Option<Goal>,

    /// Named fields, as a JSON object. Use - for stdin. Implies --goal fields.
    #[arg(long, value_name = "FILE")]
    pub selectors: Option<String>,

    /// Follow up to this many same-host links found on the pages already read.
    #[arg(long, value_name = "N")]
    pub expand: Option<usize>,

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

/// The stored provider fallback.
#[derive(Debug, Args)]
pub struct RouterArgs {
    /// What to do with it.
    #[command(subcommand)]
    pub command: RouterCommand,
}

/// What `router` does.
#[derive(Debug, Subcommand)]
pub enum RouterCommand {
    /// Store or change the fallback. A flag you do not pass keeps its stored
    /// value.
    Set(RouterSetArgs),
    /// Print the stored fallback, with every key and setting value redacted.
    Show,
    /// Delete the stored fallback.
    Clear,
}

/// Store or change the provider fallback.
#[derive(Debug, Args)]
pub struct RouterSetArgs {
    /// The provider, by name. A name this version does not know is stored with
    /// a note, because providers are added without a release.
    #[arg(long, value_name = "NAME")]
    pub provider: Option<String>,

    /// How the provider takes part.
    #[arg(long, value_name = "MODE", value_parser = router_modes())]
    pub mode: Option<Mode>,

    /// Which routes may be used: any, or only those your own keys pay for.
    #[arg(long, value_enum)]
    pub funding: Option<Funding>,

    /// Read the provider token from stdin. SPIDER_ROUTER_TOKEN works too.
    #[arg(long, conflicts_with = "no_token")]
    pub token_stdin: bool,

    /// Refused: a key on the command line lands in shell history. Use
    /// --token-stdin or SPIDER_ROUTER_TOKEN.
    #[arg(long, value_name = "VALUE", num_args = 0..=1, hide = true)]
    pub token: Option<Option<String>>,

    /// Remove the stored token.
    #[arg(long)]
    pub no_token: bool,

    /// A credential, as NAME=VALUE for a value that is not secret, or
    /// NAME-stdin to read the value from stdin. Repeat for more.
    #[arg(long, value_name = "NAME=VALUE|NAME-stdin")]
    pub credential: Vec<String>,

    /// Remove a stored credential. Repeat for more.
    #[arg(long, value_name = "NAME")]
    pub no_credential: Vec<String>,

    /// A provider setting, as PROVIDER.KEY=VALUE. A value that reads as JSON is
    /// stored as JSON, anything else as text. Applied only on routes your own
    /// key pays for. Repeat for more.
    #[arg(long, value_name = "PROVIDER.KEY=VALUE")]
    pub option: Vec<String>,

    /// Remove a stored provider setting. Repeat for more.
    #[arg(long, value_name = "PROVIDER.KEY")]
    pub no_option: Vec<String>,
}

/// Which routes a request may be paid through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Funding {
    /// Any route.
    Any,
    /// Only routes your own keys pay for.
    Own,
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
