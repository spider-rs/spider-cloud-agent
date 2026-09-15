//! What the router is allowed to look at.
//!
//! [`featurize`] turns a request into a fixed width array of floats. Every
//! slot in that array is named by a constant in this file, every value is a
//! one-hot or a bucket, and the whole table is short enough to print. That is
//! deliberate: the feature to weight table ships publicly with the weights, so
//! a slot that cannot be printed and defended does not belong here.
//!
//! # The rule that shapes all of it
//!
//! No slot may be a function of which site is being fetched. Not the name, not
//! a hash of the name. A hash is not anonymity when the candidate set is a
//! published list of a million names: hash the list, look for the cells that
//! carry weight, and you have read back the sites we crawl. So the host
//! contributes a label group, a subdomain count and three flags, all decided
//! in [`crate::domain`], and nothing else.
//!
//! [`FeatureVector`] holds `[f32; FEATURE_DIM]` and nothing else. It is
//! `Copy`, it owns no heap, and a test asserts its size, so adding a field
//! that could hold a name fails the build rather than a review.
//!
//! Per site adaptivity comes back through [`SiteMemory`], which the caller
//! fills in from its own records. The shipped weights carry "this site has
//! failed twice already". They never carry which site.

use crate::action::{Country, ProxyPool, RequestMode};
use crate::domain::{host_shape, HostShape, IpKind, TLD_SLOTS};
use url::Url;

/// How many slots a feature vector has.
///
/// A round number with room left. The slots in use today stop at
/// [`FEATURES_USED`], and what the rest is held for is written down beside it.
pub const FEATURE_DIM: usize = 256;

/// What the caller wants back, coarsely.
///
/// This mirrors the need the client crate takes, minus the payloads, because
/// the router only cares which kind of answer is wanted and never what is in
/// it. A field selector is a strong signal about cost and a weak one about
/// difficulty, which is why it is one slot rather than a description.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum DeclaredNeed {
    /// The page as plain text.
    Text,
    /// The page as markdown.
    #[default]
    Markdown,
    /// The markup.
    Html,
    /// The links and none of the content.
    Links,
    /// The title, description and the rest of the declared metadata.
    Metadata,
    /// Named values pulled out of the page.
    Fields,
    /// A picture of the page.
    Screenshot,
    /// Whatever the service returns on its own.
    Raw,
}

impl DeclaredNeed {
    /// Which slot in the need block this need sets.
    const fn index(self) -> usize {
        match self {
            DeclaredNeed::Text => 0,
            DeclaredNeed::Markdown => 1,
            DeclaredNeed::Html => 2,
            DeclaredNeed::Links => 3,
            DeclaredNeed::Metadata => 4,
            DeclaredNeed::Fields => 5,
            DeclaredNeed::Screenshot => 6,
            DeclaredNeed::Raw => 7,
        }
    }
}

/// How the last attempt against a site ended, in the coarsest terms that still
/// change what to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum StatusClass {
    /// Nothing has been tried yet.
    #[default]
    Unknown,
    /// The page came back and had content.
    Ok,
    /// The page came back with nothing in it. The most common quiet failure
    /// and the one worth its own slot.
    Empty,
    /// The request was rejected as malformed.
    BadRequest,
    /// The page is behind a sign in.
    NeedsLogin,
    /// The site refused the fetch.
    Blocked,
    /// There is no such page.
    NotFound,
    /// The site asked for a slower pace.
    RateLimited,
    /// The site failed on its own side.
    ServerError,
}

impl StatusClass {
    /// Which slot in the memory block this class sets.
    const fn index(self) -> usize {
        match self {
            StatusClass::Unknown => 0,
            StatusClass::Ok => 1,
            StatusClass::Empty => 2,
            StatusClass::BadRequest => 3,
            StatusClass::NeedsLogin => 4,
            StatusClass::Blocked => 5,
            StatusClass::NotFound => 6,
            StatusClass::RateLimited => 7,
            StatusClass::ServerError => 8,
        }
    }

    /// Whether this class says the site turned the fetch away rather than
    /// failing to serve it.
    pub const fn is_refusal(self) -> bool {
        matches!(self, StatusClass::Blocked | StatusClass::NeedsLogin)
    }
}

/// What the caller knows about this site from its own history.
///
/// The caller keeps this, keyed however it likes. The router reads it and
/// forgets it. That split is what lets per site adaptivity exist without a
/// single site name reaching the weights.
// Not `non_exhaustive`: the caller fills every field of this from its own
// records, so it has to be writable as a literal from outside the crate.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SiteMemory {
    /// How many attempts the caller has recorded. Zero is the normal case for
    /// a new site and has its own slot, so cold start is a value rather than a
    /// gap.
    pub observations: u32,
    /// A decaying average of how often attempts succeeded, from zero to one.
    pub success_rate: f32,
    /// Consecutive outcomes of the same kind. Positive counts successes,
    /// negative counts failures.
    pub streak: i16,
    /// How the last attempt ended.
    pub last_status: StatusClass,
}

impl SiteMemory {
    /// A memory of nothing, which is what a site that has never been fetched
    /// gets.
    pub const fn cold() -> SiteMemory {
        SiteMemory {
            observations: 0,
            success_rate: 0.0,
            streak: 0,
            last_status: StatusClass::Unknown,
        }
    }

    /// How many failures in a row, zero when the last attempt worked.
    pub const fn failure_streak(self) -> u16 {
        if self.streak < 0 {
            self.streak.unsigned_abs()
        } else {
            0
        }
    }

    /// Whether there is enough here to act on. One attempt is an anecdote.
    pub const fn is_informative(self) -> bool {
        self.observations >= 2
    }
}

/// Settings the caller fixed itself.
///
/// A pin is not a hint. Whatever is set here is what the first attempt uses,
/// and the router says so by reporting [`crate::RouteSource::Caller`].
// Not `non_exhaustive`, for the same reason as `SiteMemory`: this is the
// caller's own input and a literal is the natural way to write it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CallerPins<'a> {
    /// The fetch mode the caller fixed.
    pub mode: Option<RequestMode>,
    /// The address pool the caller fixed.
    pub proxy: Option<ProxyPool>,
    /// The country the caller fixed.
    pub country: Option<&'a Country>,
}

impl<'a> CallerPins<'a> {
    /// Nothing fixed, which is the usual case.
    pub const fn none() -> CallerPins<'a> {
        CallerPins {
            mode: None,
            proxy: None,
            country: None,
        }
    }

    /// Whether the caller fixed anything at all.
    pub const fn any(&self) -> bool {
        self.mode.is_some() || self.proxy.is_some() || self.country.is_some()
    }
}

/// One request, as the router sees it.
///
/// The url arrives parsed. Parsing allocates and routing does not, so the
/// caller pays that cost once where it already has to.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct RouteInput<'a> {
    /// The address to fetch.
    pub url: &'a Url,
    /// What the caller wants back.
    pub need: DeclaredNeed,
    /// Settings the caller fixed itself.
    pub pins: CallerPins<'a>,
    /// What the caller remembers about this site, when it remembers anything.
    pub memory: Option<&'a SiteMemory>,
    /// The hour of the day at the caller, from zero to twenty three.
    ///
    /// Refusal rates move with the clock, and two floats is a cheap way to say
    /// so. This crate reads no clock of its own: a caller that cares sets it,
    /// and a caller that does not gets the unknown slot.
    pub hour: Option<u8>,
}

impl<'a> RouteInput<'a> {
    /// A request with nothing fixed and nothing remembered.
    pub const fn new(url: &'a Url, need: DeclaredNeed) -> RouteInput<'a> {
        RouteInput {
            url,
            need,
            pins: CallerPins::none(),
            memory: None,
            hour: None,
        }
    }

    /// Fix some settings.
    pub const fn with_pins(mut self, pins: CallerPins<'a>) -> RouteInput<'a> {
        self.pins = pins;
        self
    }

    /// Supply what the caller remembers about this site.
    pub const fn with_memory(mut self, memory: &'a SiteMemory) -> RouteInput<'a> {
        self.memory = Some(memory);
        self
    }

    /// Say what time it is where the caller is.
    pub const fn at_hour(mut self, hour: u8) -> RouteInput<'a> {
        self.hour = Some(hour);
        self
    }
}

/// What the last path segment looks like.
///
/// The classes say how a path was generated, which correlates with what serves
/// it. A run of hex is a record identifier from a store, a run of words with
/// hyphens is an article slug from a publishing system, and those two are
/// served by different software more often than not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum SegmentClass {
    /// The path ends at a directory, so there is no last segment.
    #[default]
    Empty,
    /// Digits only.
    Digits,
    /// Letters only.
    Alpha,
    /// Letters and digits, no separators.
    AlphaNum,
    /// Words joined with hyphens.
    Hyphenated,
    /// Words joined with underscores.
    Underscored,
    /// A long run of hex, with or without the dashes an identifier carries.
    HexIdentifier,
    /// Carries escaped characters.
    Escaped,
    /// Anything else.
    Other,
}

impl SegmentClass {
    /// Which slot in the segment block this class sets.
    const fn index(self) -> usize {
        match self {
            SegmentClass::Empty => 0,
            SegmentClass::Digits => 1,
            SegmentClass::Alpha => 2,
            SegmentClass::AlphaNum => 3,
            SegmentClass::Hyphenated => 4,
            SegmentClass::Underscored => 5,
            SegmentClass::HexIdentifier => 6,
            SegmentClass::Escaped => 7,
            SegmentClass::Other => 8,
        }
    }
}

/// What kind of file the path names, if it names one.
///
/// This is the single most useful pre-call feature, because most of these
/// classes settle the question on their own: a rendered fetch cannot change
/// the bytes of a file that was never markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ExtClass {
    /// No extension. Most pages.
    #[default]
    None,
    /// Markup, including the extensions template engines leave behind.
    Markup,
    /// Structured data by extension.
    Xml,
    /// A json document.
    Json,
    /// A syndication document.
    Feed,
    /// Plain text or markdown.
    Text,
    /// Separated values.
    Csv,
    /// A pdf.
    Pdf,
    /// A word processor or spreadsheet document.
    Office,
    /// An image.
    Image,
    /// Audio or video.
    Media,
    /// An archive.
    Archive,
    /// Script, style or a compiled asset.
    Asset,
    /// An extension we do not know.
    Other,
}

impl ExtClass {
    /// Which slot in the extension block this class sets.
    const fn index(self) -> usize {
        match self {
            ExtClass::None => 0,
            ExtClass::Markup => 1,
            ExtClass::Xml => 2,
            ExtClass::Json => 3,
            ExtClass::Feed => 4,
            ExtClass::Text => 5,
            ExtClass::Csv => 6,
            ExtClass::Pdf => 7,
            ExtClass::Office => 8,
            ExtClass::Image => 9,
            ExtClass::Media => 10,
            ExtClass::Archive => 11,
            ExtClass::Asset => 12,
            ExtClass::Other => 13,
        }
    }

    /// Whether a rendered fetch could possibly change what comes back.
    ///
    /// False for every class whose bytes are fixed before any script could
    /// run. This is what lets the heuristic send a feed or a data file down
    /// the cheap path with no hedging.
    pub const fn can_need_rendering(self) -> bool {
        matches!(self, ExtClass::None | ExtClass::Markup | ExtClass::Other)
    }
}

/// What a query key is for, judged from the key alone.
///
/// Keys are named by the software that serves the page, so they say something
/// about it. Values are the user's business and are never read: a search term
/// or an identifier is exactly the kind of thing that could make a slot
/// personal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeyClass {
    /// Paging through a list.
    Pagination,
    /// Naming one record.
    Identifier,
    /// A search.
    Search,
    /// Campaign tagging, added by whoever linked here rather than by the site.
    Tracking,
    /// A session or a token.
    Session,
    /// Language or region.
    Locale,
    /// Asking for a particular output format.
    Format,
    /// Sorting or filtering a list.
    Filter,
    /// A single character key, the mark of a hand rolled query string.
    SingleChar,
    /// A key that is only digits.
    Numeric,
}

/// How many slots [`KeyClass`] needs.
const KEY_CLASSES: usize = 10;

impl KeyClass {
    /// Which slot in the key block this class sets.
    const fn index(self) -> usize {
        match self {
            KeyClass::Pagination => 0,
            KeyClass::Identifier => 1,
            KeyClass::Search => 2,
            KeyClass::Tracking => 3,
            KeyClass::Session => 4,
            KeyClass::Locale => 5,
            KeyClass::Format => 6,
            KeyClass::Filter => 7,
            KeyClass::SingleChar => 8,
            KeyClass::Numeric => 9,
        }
    }
}

/// The feature vector.
///
/// Fixed width, `Copy`, no heap. It cannot hold a url. That is the privacy
/// contract, and `feature_vector_holds_no_text` in this file is what keeps it
/// true when someone reaches for a field with a name in it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FeatureVector {
    bits: [f32; FEATURE_DIM],
}

impl Default for FeatureVector {
    /// Written out rather than derived: the standard library stops deriving
    /// `Default` for arrays well short of this one.
    fn default() -> FeatureVector {
        FeatureVector::zeroed()
    }
}

impl FeatureVector {
    /// Every slot zero.
    pub const fn zeroed() -> FeatureVector {
        FeatureVector {
            bits: [0.0; FEATURE_DIM],
        }
    }

    /// The slots, in order.
    pub const fn as_slice(&self) -> &[f32] {
        &self.bits
    }

    /// One slot, or `None` past the end.
    pub fn get(&self, slot: usize) -> Option<f32> {
        self.bits.get(slot).copied()
    }

    /// Set one slot. Out of range does nothing, because a feature layout bug
    /// should not take down a caller's process.
    fn set(&mut self, slot: usize, value: f32) {
        if let Some(cell) = self.bits.get_mut(slot) {
            *cell = value;
        }
    }

    /// Set the slot at `base + offset` of a one-hot block.
    fn one_hot(&mut self, base: usize, offset: usize) {
        self.set(base + offset, 1.0);
    }
}

// The layout. Each block is a base and a width, and the bases are derived from
// the widths so a block cannot silently overlap its neighbour. The order is
// url shape, then host, then what the caller asked for, then what the caller
// remembers.

/// Path depth, bucketed: 0, 1, 2, 3, 4, 5, 6 or more.
pub const PATH_DEPTH: usize = 0;
const PATH_DEPTH_W: usize = 7;

/// Path length in bytes, bucketed: to 16, 32, 64, 128, 256, past 256.
pub const PATH_LEN: usize = PATH_DEPTH + PATH_DEPTH_W;
const PATH_LEN_W: usize = 6;

/// What the last path segment looks like. See [`SegmentClass`].
pub const SEGMENT: usize = PATH_LEN + PATH_LEN_W;
const SEGMENT_W: usize = 9;

/// The path ends in a slash.
pub const TRAILING_SLASH: usize = SEGMENT + SEGMENT_W;
const TRAILING_SLASH_W: usize = 1;

/// What kind of file the path names. See [`ExtClass`].
pub const EXTENSION: usize = TRAILING_SLASH + TRAILING_SLASH_W;
const EXTENSION_W: usize = 14;

/// How many query parameters, bucketed: 0, 1, 2, 3, 4 to 6, 7 or more.
pub const QUERY_ARITY: usize = EXTENSION + EXTENSION_W;
const QUERY_ARITY_W: usize = 6;

/// Which kinds of query key are present. See [`KeyClass`]. Not one-hot: a url
/// can carry several kinds at once.
pub const QUERY_KEYS: usize = QUERY_ARITY + QUERY_ARITY_W;
const QUERY_KEYS_W: usize = KEY_CLASSES;

/// The longest query key, bucketed: to 2, to 6, to 12, past 12.
pub const QUERY_KEY_LEN: usize = QUERY_KEYS + QUERY_KEYS_W;
const QUERY_KEY_LEN_W: usize = 4;

/// Labels in front of the registrable name: 0, 1, 2, 3 or more.
pub const SUBDOMAINS: usize = QUERY_KEY_LEN + QUERY_KEY_LEN_W;
const SUBDOMAINS_W: usize = 4;

/// The first label is the conventional web prefix.
pub const WWW: usize = SUBDOMAINS + SUBDOMAINS_W;
const WWW_W: usize = 1;

/// The host is a literal address: first slot four numbers, second slot eight
/// groups.
pub const IP_LITERAL: usize = WWW + WWW_W;
const IP_LITERAL_W: usize = 2;

/// A label is in the encoded form that carries non-ascii text.
pub const PUNYCODE: usize = IP_LITERAL + IP_LITERAL_W;
const PUNYCODE_W: usize = 1;

/// Four flags about the request itself: secure scheme, a port that is not the
/// default, credentials in the url, a fragment.
pub const URL_FLAGS: usize = PUNYCODE + PUNYCODE_W;
const URL_FLAGS_W: usize = 4;

/// The label group, one-hot. See [`crate::domain::TldSlot`].
pub const TLD: usize = URL_FLAGS + URL_FLAGS_W;
const TLD_W: usize = TLD_SLOTS;

/// How well known the site is, in five bands plus unknown.
///
/// Always zero today, and deliberately still here. The popularity filters are
/// about a megabyte and they arrive with the weights, not before them, because
/// a crate that answers from rules alone should not carry a megabyte it never
/// reads. The slots exist so the layout does not shift when they land, which
/// would invalidate every weight trained against it.
pub const POPULARITY: usize = TLD + TLD_W;
const POPULARITY_W: usize = 6;

/// What the caller asked for, one-hot. See [`DeclaredNeed`].
pub const NEED: usize = POPULARITY + POPULARITY_W;
const NEED_W: usize = 8;

/// What the caller fixed itself: any pin at all, then the fixed mode one-hot,
/// then the fixed pool one-hot, then a fixed country.
pub const PINS: usize = NEED + NEED_W;
const PINS_W: usize = 8;

/// The hour of the day as a pair of waves, then a slot for not knowing.
///
/// Two waves rather than twenty four slots because the hours either side of
/// one another behave alike, and because midnight is next to eleven at night.
pub const HOUR: usize = PINS + PINS_W;
const HOUR_W: usize = 3;

/// How many attempts the caller has recorded: none, 1 to 2, 3 to 9, 10 to 49,
/// 50 or more. The first slot is cold start, which is the normal case.
pub const MEM_COUNT: usize = HOUR + HOUR_W;
const MEM_COUNT_W: usize = 5;

/// How often attempts succeeded, in five bands.
pub const MEM_SUCCESS: usize = MEM_COUNT + MEM_COUNT_W;
const MEM_SUCCESS_W: usize = 5;

/// Failures in a row: none, 1, 2, 3 or more.
pub const MEM_FAIL_STREAK: usize = MEM_SUCCESS + MEM_SUCCESS_W;
const MEM_FAIL_STREAK_W: usize = 4;

/// Successes in a row: none, 1, 2, 3 or more.
pub const MEM_WIN_STREAK: usize = MEM_FAIL_STREAK + MEM_FAIL_STREAK_W;
const MEM_WIN_STREAK_W: usize = 4;

/// How the last attempt ended, one-hot. See [`StatusClass`].
pub const MEM_STATUS: usize = MEM_WIN_STREAK + MEM_WIN_STREAK_W;
const MEM_STATUS_W: usize = 9;

/// Always one, so a model has an intercept.
pub const BIAS: usize = MEM_STATUS + MEM_STATUS_W;
const BIAS_W: usize = 1;

/// The first slot no block claims.
///
/// What is left is held for the two blocks that are not written yet: the
/// signals read off a response, which only the escalation head uses, and the
/// explicit crosses between blocks that give a linear model its curves. Both
/// land with the weights. Leaving the room here means adding them does not
/// move any slot already trained against.
pub const FEATURES_USED: usize = BIAS + BIAS_W;

const _: () = assert!(FEATURES_USED <= FEATURE_DIM);

/// Turn a request into the vector the router reads.
///
/// Allocates nothing, reads no clock and no environment, and gives the same
/// answer for the same input forever. Two urls of the same shape on two
/// different sites come out identical, which is the anonymity claim and is
/// tested as such.
pub fn featurize(input: &RouteInput<'_>) -> FeatureVector {
    let mut out = FeatureVector::zeroed();
    let url = input.url;

    // Path shape.
    let path = url.path();
    let mut depth = 0usize;
    let mut last = "";

    for segment in path.split('/') {
        if !segment.is_empty() {
            depth += 1;
            last = segment;
        }
    }

    out.one_hot(PATH_DEPTH, depth.min(6));
    out.one_hot(PATH_LEN, length_bucket(path.len()));

    let trailing = path.len() > 1 && path.ends_with('/');
    let segment_class = if trailing || last.is_empty() {
        SegmentClass::Empty
    } else {
        segment_class(last)
    };

    out.one_hot(SEGMENT, segment_class.index());

    if trailing {
        out.set(TRAILING_SLASH, 1.0);
    }

    out.one_hot(EXTENSION, extension_class(last).index());

    // Query shape. Keys are read, values never are.
    let mut arity = 0usize;
    let mut longest_key = 0usize;

    for pair in url.query().unwrap_or("").split('&') {
        if pair.is_empty() {
            continue;
        }

        arity += 1;

        let key = match pair.split_once('=') {
            Some((key, _value)) => key,
            None => pair,
        };

        longest_key = longest_key.max(key.chars().count());

        for class in key_classes(key) {
            out.one_hot(QUERY_KEYS, class.index());
        }
    }

    out.one_hot(QUERY_ARITY, arity_bucket(arity));

    if arity > 0 {
        out.one_hot(QUERY_KEY_LEN, key_length_bucket(longest_key));
    }

    // Host shape, decided entirely in `domain`.
    let host: HostShape = host_shape(url);

    out.one_hot(SUBDOMAINS, usize::from(host.subdomains).min(3));

    if host.www {
        out.set(WWW, 1.0);
    }

    match host.ip {
        Some(IpKind::V4) => out.one_hot(IP_LITERAL, 0),
        Some(IpKind::V6) => out.one_hot(IP_LITERAL, 1),
        None => {}
    }

    if host.punycode {
        out.set(PUNYCODE, 1.0);
    }

    if let Some(tld) = host.tld {
        out.one_hot(TLD, tld.index());
    }

    // The request itself.
    if url.scheme() == "https" {
        out.set(URL_FLAGS, 1.0);
    }

    if url.port().is_some() {
        out.set(URL_FLAGS + 1, 1.0);
    }

    if !url.username().is_empty() || url.password().is_some() {
        out.set(URL_FLAGS + 2, 1.0);
    }

    if url.fragment().is_some() {
        out.set(URL_FLAGS + 3, 1.0);
    }

    // Popularity is left at zero until the filters ship. Named here so the
    // block is visible in the layout rather than looking like a gap.
    let _ = POPULARITY;

    // What the caller asked for.
    out.one_hot(NEED, input.need.index());

    if input.pins.any() {
        out.set(PINS, 1.0);
    }

    match input.pins.mode {
        Some(RequestMode::Http) => out.one_hot(PINS, 1),
        Some(RequestMode::Smart) => out.one_hot(PINS, 2),
        Some(RequestMode::Browser) => out.one_hot(PINS, 3),
        _ => {}
    }

    match input.pins.proxy {
        Some(ProxyPool::Isp) => out.one_hot(PINS, 4),
        Some(ProxyPool::Residential) => out.one_hot(PINS, 5),
        _ => {}
    }

    if input.pins.country.is_some() {
        out.one_hot(PINS, 6);
    }

    // Slot 7 of the pin block is spare, so the block stays eight wide when a
    // fourth pin is added.

    match input.hour {
        Some(hour) => {
            let turns = f32::from(hour.min(23)) / 24.0 * std::f32::consts::TAU;
            out.set(HOUR, turns.sin());
            out.set(HOUR + 1, turns.cos());
        }
        None => out.set(HOUR + 2, 1.0),
    }

    // What the caller remembers.
    let memory = input.memory.copied().unwrap_or_else(SiteMemory::cold);

    out.one_hot(MEM_COUNT, observation_bucket(memory.observations));
    out.one_hot(MEM_SUCCESS, success_bucket(memory.success_rate));
    out.one_hot(MEM_FAIL_STREAK, streak_bucket(memory.failure_streak()));
    out.one_hot(
        MEM_WIN_STREAK,
        streak_bucket(memory.streak.max(0).unsigned_abs()),
    );
    out.one_hot(MEM_STATUS, memory.last_status.index());

    out.set(BIAS, 1.0);

    out
}

/// Path length in bytes, bucketed.
const fn length_bucket(len: usize) -> usize {
    match len {
        0..=16 => 0,
        17..=32 => 1,
        33..=64 => 2,
        65..=128 => 3,
        129..=256 => 4,
        _ => 5,
    }
}

/// Query parameter count, bucketed.
const fn arity_bucket(count: usize) -> usize {
    match count {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4..=6 => 4,
        _ => 5,
    }
}

/// Longest query key, bucketed.
const fn key_length_bucket(len: usize) -> usize {
    match len {
        0..=2 => 0,
        3..=6 => 1,
        7..=12 => 2,
        _ => 3,
    }
}

/// Recorded attempts, bucketed.
const fn observation_bucket(count: u32) -> usize {
    match count {
        0 => 0,
        1..=2 => 1,
        3..=9 => 2,
        10..=49 => 3,
        _ => 4,
    }
}

/// Success rate, bucketed. Anything outside zero to one is clamped, because a
/// caller's arithmetic is not this crate's to trust.
fn success_bucket(rate: f32) -> usize {
    let rate = rate.clamp(0.0, 1.0);

    if rate < 0.2 {
        0
    } else if rate < 0.4 {
        1
    } else if rate < 0.6 {
        2
    } else if rate < 0.8 {
        3
    } else {
        4
    }
}

/// A run of the same outcome, bucketed.
const fn streak_bucket(streak: u16) -> usize {
    match streak {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 3,
    }
}

/// Classify the last path segment. Reads characters, never bytes, so an
/// escaped or non-ascii path cannot cut a character in half.
pub fn segment_class(segment: &str) -> SegmentClass {
    if segment.contains('%') {
        return SegmentClass::Escaped;
    }

    let mut letters = 0usize;
    let mut digits = 0usize;
    let mut hyphens = 0usize;
    let mut underscores = 0usize;
    let mut hex = 0usize;
    let mut other = 0usize;
    let mut total = 0usize;

    for ch in segment.chars() {
        total += 1;

        if ch.is_ascii_digit() {
            digits += 1;
            hex += 1;
        } else if ch.is_ascii_hexdigit() {
            letters += 1;
            hex += 1;
        } else if ch.is_alphabetic() {
            letters += 1;
        } else if ch == '-' {
            hyphens += 1;
        } else if ch == '_' {
            underscores += 1;
        } else {
            other += 1;
        }
    }

    if total == 0 {
        return SegmentClass::Empty;
    }

    // An identifier out of a store: long, hex, dashes at most. Checked before
    // the word classes so a record id does not read as an article slug.
    if hex + hyphens == total && hex >= 16 && digits > 0 {
        return SegmentClass::HexIdentifier;
    }

    if other > 0 {
        return SegmentClass::Other;
    }

    if digits == total {
        return SegmentClass::Digits;
    }

    if letters == total {
        return SegmentClass::Alpha;
    }

    if hyphens > 0 && underscores == 0 {
        return SegmentClass::Hyphenated;
    }

    if underscores > 0 && hyphens == 0 {
        return SegmentClass::Underscored;
    }

    if hyphens == 0 && underscores == 0 {
        return SegmentClass::AlphaNum;
    }

    SegmentClass::Other
}

/// Classify the extension of the last path segment.
pub fn extension_class(segment: &str) -> ExtClass {
    let ext = match segment.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext,
        _ => return ExtClass::None,
    };

    // A dotted version or a date in a path is not an extension, and neither is
    // a run of anything that is not a short alphanumeric word.
    if ext.chars().count() > 8
        || ext.chars().any(|ch| !ch.is_ascii_alphanumeric())
        || ext.chars().all(|ch| ch.is_ascii_digit())
    {
        return ExtClass::None;
    }

    const TABLE: [(&str, ExtClass); 51] = [
        ("html", ExtClass::Markup),
        ("htm", ExtClass::Markup),
        ("xhtml", ExtClass::Markup),
        ("shtml", ExtClass::Markup),
        ("php", ExtClass::Markup),
        ("asp", ExtClass::Markup),
        ("aspx", ExtClass::Markup),
        ("jsp", ExtClass::Markup),
        ("cfm", ExtClass::Markup),
        ("xml", ExtClass::Xml),
        ("xsl", ExtClass::Xml),
        ("json", ExtClass::Json),
        ("jsonl", ExtClass::Json),
        ("ndjson", ExtClass::Json),
        ("rss", ExtClass::Feed),
        ("atom", ExtClass::Feed),
        ("txt", ExtClass::Text),
        ("md", ExtClass::Text),
        ("rst", ExtClass::Text),
        ("csv", ExtClass::Csv),
        ("tsv", ExtClass::Csv),
        ("pdf", ExtClass::Pdf),
        ("doc", ExtClass::Office),
        ("docx", ExtClass::Office),
        ("xls", ExtClass::Office),
        ("xlsx", ExtClass::Office),
        ("ppt", ExtClass::Office),
        ("pptx", ExtClass::Office),
        ("odt", ExtClass::Office),
        ("jpg", ExtClass::Image),
        ("jpeg", ExtClass::Image),
        ("png", ExtClass::Image),
        ("gif", ExtClass::Image),
        ("webp", ExtClass::Image),
        ("svg", ExtClass::Image),
        ("avif", ExtClass::Image),
        ("ico", ExtClass::Image),
        ("mp4", ExtClass::Media),
        ("webm", ExtClass::Media),
        ("mov", ExtClass::Media),
        ("mp3", ExtClass::Media),
        ("wav", ExtClass::Media),
        ("m3u8", ExtClass::Media),
        ("zip", ExtClass::Archive),
        ("gz", ExtClass::Archive),
        ("tar", ExtClass::Archive),
        ("rar", ExtClass::Archive),
        ("7z", ExtClass::Archive),
        ("js", ExtClass::Asset),
        ("css", ExtClass::Asset),
        ("wasm", ExtClass::Asset),
    ];

    for (name, class) in TABLE {
        if ext.eq_ignore_ascii_case(name) {
            return class;
        }
    }

    ExtClass::Other
}

/// What kind of file a url names, if it names one.
///
/// Reads the path and nothing else, so the heuristic can ask this question
/// without going anywhere near the host.
pub fn extension_of(url: &Url) -> ExtClass {
    extension_class(last_segment(url))
}

/// The last non empty path segment, empty when the path ends at a directory.
fn last_segment(url: &Url) -> &str {
    let path = url.path();

    if path.ends_with('/') {
        return "";
    }

    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
}

/// Which classes a query key belongs to. A key can fall into more than one,
/// and the return is a fixed array so nothing is allocated to say so.
fn key_classes(key: &str) -> impl Iterator<Item = KeyClass> {
    const PAGINATION: [&str; 8] = [
        "page", "p", "pg", "offset", "start", "limit", "per_page", "cursor",
    ];
    const IDENTIFIER: [&str; 8] = ["id", "pid", "sku", "uid", "item", "product", "ref", "asin"];
    const SEARCH: [&str; 6] = ["q", "query", "s", "search", "keyword", "term"];
    const SESSION: [&str; 7] = [
        "token",
        "session",
        "sid",
        "auth",
        "key",
        "signature",
        "nonce",
    ];
    const LOCALE: [&str; 7] = [
        "lang", "locale", "hl", "gl", "country", "region", "currency",
    ];
    const FORMAT: [&str; 5] = ["format", "output", "type", "view", "render"];
    const FILTER: [&str; 8] = [
        "sort", "order", "filter", "category", "tag", "min", "max", "price",
    ];

    let matches = |table: &[&str]| table.iter().any(|name| key.eq_ignore_ascii_case(name));

    let mut found = [None; KEY_CLASSES];
    let mut at = 0usize;
    let mut push = |class: KeyClass| {
        if let Some(cell) = found.get_mut(at) {
            *cell = Some(class);
            at += 1;
        }
    };

    if matches(&PAGINATION) {
        push(KeyClass::Pagination);
    }

    if matches(&IDENTIFIER) {
        push(KeyClass::Identifier);
    }

    if matches(&SEARCH) {
        push(KeyClass::Search);
    }

    // Campaign tags are added by whoever wrote the link, so they say nothing
    // about the site and a lot about the traffic. Worth a slot either way.
    if key.len() > 4 && key.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("utm_"))
        || key.eq_ignore_ascii_case("gclid")
        || key.eq_ignore_ascii_case("fbclid")
        || key.eq_ignore_ascii_case("msclkid")
    {
        push(KeyClass::Tracking);
    }

    if matches(&SESSION) {
        push(KeyClass::Session);
    }

    if matches(&LOCALE) {
        push(KeyClass::Locale);
    }

    if matches(&FORMAT) {
        push(KeyClass::Format);
    }

    if matches(&FILTER) {
        push(KeyClass::Filter);
    }

    if key.chars().count() == 1 {
        push(KeyClass::SingleChar);
    }

    if !key.is_empty() && key.chars().all(|ch| ch.is_ascii_digit()) {
        push(KeyClass::Numeric);
    }

    found.into_iter().flatten()
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

    fn vector(url: &str) -> FeatureVector {
        let parsed = Url::parse(url).unwrap();
        featurize(&RouteInput::new(&parsed, DeclaredNeed::Markdown))
    }

    #[test]
    fn feature_vector_holds_no_text() {
        // The whole privacy argument rests on this. A `String` field would add
        // a pointer, a length and a capacity, and the assertion would fail
        // before anyone had to notice it in review.
        assert_eq!(
            std::mem::size_of::<FeatureVector>(),
            FEATURE_DIM * std::mem::size_of::<f32>()
        );
    }

    #[test]
    fn the_layout_fits_and_the_blocks_do_not_overlap() {
        let bases = [
            ("path depth", PATH_DEPTH, PATH_DEPTH_W),
            ("path length", PATH_LEN, PATH_LEN_W),
            ("segment", SEGMENT, SEGMENT_W),
            ("trailing slash", TRAILING_SLASH, TRAILING_SLASH_W),
            ("extension", EXTENSION, EXTENSION_W),
            ("query arity", QUERY_ARITY, QUERY_ARITY_W),
            ("query keys", QUERY_KEYS, QUERY_KEYS_W),
            ("query key length", QUERY_KEY_LEN, QUERY_KEY_LEN_W),
            ("subdomains", SUBDOMAINS, SUBDOMAINS_W),
            ("www", WWW, WWW_W),
            ("ip literal", IP_LITERAL, IP_LITERAL_W),
            ("punycode", PUNYCODE, PUNYCODE_W),
            ("url flags", URL_FLAGS, URL_FLAGS_W),
            ("tld", TLD, TLD_W),
            ("popularity", POPULARITY, POPULARITY_W),
            ("need", NEED, NEED_W),
            ("pins", PINS, PINS_W),
            ("hour", HOUR, HOUR_W),
            ("memory count", MEM_COUNT, MEM_COUNT_W),
            ("memory success", MEM_SUCCESS, MEM_SUCCESS_W),
            ("memory failures", MEM_FAIL_STREAK, MEM_FAIL_STREAK_W),
            ("memory wins", MEM_WIN_STREAK, MEM_WIN_STREAK_W),
            ("memory status", MEM_STATUS, MEM_STATUS_W),
            ("bias", BIAS, BIAS_W),
        ];

        let mut next = 0usize;

        for (name, base, width) in bases {
            assert_eq!(
                base, next,
                "{name} does not start where the block before it ends"
            );
            next = base + width;
        }

        assert_eq!(next, FEATURES_USED);
        assert_eq!(bases.len(), 24);
        // Room left over, checked against a vector rather than against the
        // constants, which the compiler would fold away.
        let room = vector("https://example.com/").as_slice().len() - next;
        assert!(room > 0, "the layout has filled the vector");
    }

    #[test]
    fn every_block_is_wide_enough_for_what_it_holds() {
        // The bases above are derived from the widths, so a block that is too
        // narrow keeps the layout consistent and quietly writes its last value
        // into the block next door. The vocabularies are what settle the
        // widths, so they are checked against them here.
        assert_eq!(SEGMENT_W, 9);
        assert_eq!(EXTENSION_W, 14);
        assert_eq!(QUERY_KEYS_W, 10);
        assert_eq!(NEED_W, 8);
        assert_eq!(MEM_STATUS_W, 9);
        assert_eq!(TLD_W, 30);

        let widest = [
            ("segment", SegmentClass::Other.index(), SEGMENT_W),
            ("extension", ExtClass::Other.index(), EXTENSION_W),
            ("query keys", KeyClass::Numeric.index(), QUERY_KEYS_W),
            ("need", DeclaredNeed::Raw.index(), NEED_W),
            (
                "memory status",
                StatusClass::ServerError.index(),
                MEM_STATUS_W,
            ),
            ("tld", crate::domain::TldSlot::NewGtld.index(), TLD_W),
        ];

        for (name, last, width) in widest {
            assert_eq!(
                last + 1,
                width,
                "the {name} block is {width} wide and its last value sits at {last}"
            );
        }
    }

    #[test]
    fn a_value_at_the_end_of_a_block_stays_inside_it() {
        // The other half of the check above, from the outside: the last
        // extension class and the first query arity bucket are neighbours, and
        // this is what catches one writing over the other.
        let last_in_block = vector("https://example.com/thing.wobble");

        assert_eq!(
            last_in_block.get(EXTENSION + ExtClass::Other.index()),
            Some(1.0)
        );
        assert_eq!(last_in_block.get(QUERY_ARITY), Some(1.0));
        assert_eq!(last_in_block.get(QUERY_ARITY + 1), Some(0.0));

        let one_query = vector("https://example.com/thing.wobble?a=1");
        assert_eq!(one_query.get(QUERY_ARITY), Some(0.0));
        assert_eq!(one_query.get(QUERY_ARITY + 1), Some(1.0));
    }

    #[test]
    fn the_popularity_block_is_reserved_and_empty() {
        let v = vector("https://www.example.com/a/b/c");

        for slot in POPULARITY..POPULARITY + POPULARITY_W {
            assert_eq!(
                v.get(slot),
                Some(0.0),
                "popularity slot {slot} carries a value, and the filters have not shipped"
            );
        }
    }

    #[test]
    fn everything_past_the_last_block_stays_zero() {
        let v = vector("https://a.b.example.co.uk/x/y/z/page-12.html?q=hi&utm_source=x#frag");

        for slot in FEATURES_USED..FEATURE_DIM {
            assert_eq!(
                v.get(slot),
                Some(0.0),
                "slot {slot} is not claimed by a block"
            );
        }
    }

    /// Sites with nothing in common but the shape of the address they were
    /// asked for. If any pair of these stops colliding, a slot has started to
    /// depend on which site it is, which is the one thing this crate promises
    /// never to do.
    const DISSIMILAR: [&str; 8] = [
        "www.nike.com",
        "www.pfizer.com",
        "www.goldmansachs.com",
        "www.weightwatchers.com",
        "www.nationalgeographic.com",
        "www.bostonglobe.com",
        "www.chess.com",
        "www.tripadvisor.com",
    ];

    /// The same, one label group over.
    const DISSIMILAR_PAIRED_SUFFIX: [&str; 5] = [
        "www.bbc.co.uk",
        "www.hsbc.co.uk",
        "www.rakuten.co.jp",
        "www.woolworths.com.au",
        "www.globo.com.br",
    ];

    #[test]
    fn the_same_shape_on_different_sites_is_the_same_vector() {
        let paths = [
            "/products/air-max-90?color=red&size=10",
            "/",
            "/news/world/a-long-story-about-things",
            "/api/v2/items.json?page=3&limit=50",
            "/assets/hero-image.png",
            "/search?q=something+private&sort=price",
        ];

        let mut compared = 0usize;

        for group in [DISSIMILAR.as_slice(), DISSIMILAR_PAIRED_SUFFIX.as_slice()] {
            for path in paths {
                let first_host = group.first().expect("a non empty group");
                let reference = vector(&format!("https://{first_host}{path}"));

                for host in group.iter().skip(1) {
                    let other = vector(&format!("https://{host}{path}"));

                    assert_eq!(
                        reference, other,
                        "{first_host} and {host} produced different vectors for {path}, \
                         so some slot is reading the site rather than the shape"
                    );
                    compared += 1;
                }
            }
        }

        // A loop that compared nothing would pass.
        assert_eq!(compared, (7 + 4) * 6);
    }

    #[test]
    fn the_shape_itself_still_moves_the_vector() {
        // The counterpart to the test above. If everything collided the
        // anonymity test would pass while the featurizer did nothing.
        let flat = vector("https://www.example.com/");
        let deep = vector("https://www.example.com/a/b/c/d/e/f/g");
        let data = vector("https://www.example.com/a/b/c/d/e/f/g.json");

        assert_ne!(flat, deep);
        assert_ne!(deep, data);
    }

    #[test]
    fn path_depth_and_length_are_bucketed() {
        let v = vector("https://example.com/a/b/c");
        assert_eq!(v.get(PATH_DEPTH + 3), Some(1.0));
        assert_eq!(v.get(PATH_DEPTH + 2), Some(0.0));

        let deep = vector("https://example.com/a/b/c/d/e/f/g/h/i");
        assert_eq!(deep.get(PATH_DEPTH + 6), Some(1.0));
    }

    #[test]
    fn segment_classes_split_the_ways_paths_are_generated() {
        assert_eq!(segment_class("12345"), SegmentClass::Digits);
        assert_eq!(segment_class("about"), SegmentClass::Alpha);
        assert_eq!(segment_class("page2"), SegmentClass::AlphaNum);
        assert_eq!(segment_class("a-long-story"), SegmentClass::Hyphenated);
        assert_eq!(segment_class("a_long_story"), SegmentClass::Underscored);
        assert_eq!(
            segment_class("f47ac10b-58cc-4372-a567-0e02b2c3d479"),
            SegmentClass::HexIdentifier
        );
        assert_eq!(segment_class("caf%C3%A9"), SegmentClass::Escaped);
        assert_eq!(segment_class("a+b"), SegmentClass::Other);
    }

    #[test]
    fn a_non_ascii_segment_is_classified_without_splitting_a_character() {
        // `Url` escapes this on parse, so the feature path sees the escaped
        // form. The classifier is written over characters either way.
        assert_eq!(segment_class("\u{5546}\u{54c1}"), SegmentClass::Alpha);
        let v = vector("https://example.com/\u{5546}\u{54c1}/1");
        assert_eq!(v.get(PATH_DEPTH + 2), Some(1.0));
    }

    #[test]
    fn extensions_are_classified_and_versions_are_not_extensions() {
        assert_eq!(extension_class("index.html"), ExtClass::Markup);
        assert_eq!(extension_class("feed.xml"), ExtClass::Xml);
        assert_eq!(extension_class("items.json"), ExtClass::Json);
        assert_eq!(extension_class("report.PDF"), ExtClass::Pdf);
        assert_eq!(extension_class("hero.png"), ExtClass::Image);
        assert_eq!(extension_class("about"), ExtClass::None);
        assert_eq!(extension_class("v1.2.3"), ExtClass::None);
        assert_eq!(extension_class("archive.tar"), ExtClass::Archive);
        assert_eq!(extension_class("thing.wobble"), ExtClass::Other);
    }

    #[test]
    fn query_keys_are_read_and_values_are_not() {
        let with_value = vector("https://example.com/s?q=a+very+identifying+search");
        let other_value = vector("https://example.com/s?q=b");

        assert_eq!(
            with_value, other_value,
            "two different search terms changed the vector, so a value is being read"
        );
        assert_eq!(
            with_value.get(QUERY_KEYS + KeyClass::Search.index()),
            Some(1.0)
        );
        assert_eq!(
            with_value.get(QUERY_KEYS + KeyClass::SingleChar.index()),
            Some(1.0)
        );
    }

    #[test]
    fn a_key_can_fall_into_several_classes_at_once() {
        let v = vector("https://example.com/l?page=2&utm_source=mail&sort=price&id=9");

        for class in [
            KeyClass::Pagination,
            KeyClass::Tracking,
            KeyClass::Filter,
            KeyClass::Identifier,
        ] {
            assert_eq!(v.get(QUERY_KEYS + class.index()), Some(1.0), "{class:?}");
        }

        assert_eq!(v.get(QUERY_ARITY + 4), Some(1.0));
    }

    #[test]
    fn cold_start_is_a_value_rather_than_a_gap() {
        let v = vector("https://example.com/");

        assert_eq!(v.get(MEM_COUNT), Some(1.0));
        assert_eq!(v.get(MEM_STATUS + StatusClass::Unknown.index()), Some(1.0));
        assert_eq!(v.get(MEM_FAIL_STREAK), Some(1.0));
    }

    #[test]
    fn memory_lands_in_its_own_block() {
        let url = Url::parse("https://example.com/").unwrap();
        let memory = SiteMemory {
            observations: 12,
            success_rate: 0.1,
            streak: -3,
            last_status: StatusClass::Blocked,
        };
        let v = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown).with_memory(&memory));

        assert_eq!(v.get(MEM_COUNT + 3), Some(1.0));
        assert_eq!(v.get(MEM_SUCCESS), Some(1.0));
        assert_eq!(v.get(MEM_FAIL_STREAK + 3), Some(1.0));
        assert_eq!(v.get(MEM_WIN_STREAK), Some(1.0));
        assert_eq!(v.get(MEM_STATUS + StatusClass::Blocked.index()), Some(1.0));
    }

    #[test]
    fn the_hour_is_two_waves_and_an_unknown() {
        let url = Url::parse("https://example.com/").unwrap();
        let unknown = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown));
        assert_eq!(unknown.get(HOUR + 2), Some(1.0));
        assert_eq!(unknown.get(HOUR), Some(0.0));

        let noon = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown).at_hour(12));
        assert_eq!(noon.get(HOUR + 2), Some(0.0));
        assert!(noon.get(HOUR + 1).unwrap_or(0.0) < -0.99);

        // Both waves have to move, or half the clock is not being recorded.
        let dawn = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown).at_hour(6));
        assert!(dawn.get(HOUR).unwrap_or(0.0) > 0.99);
        assert!(dawn.get(HOUR + 1).unwrap_or(1.0).abs() < 0.01);

        let dusk = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown).at_hour(18));
        assert!(dusk.get(HOUR).unwrap_or(0.0) < -0.99);

        // Eleven at night sits next to midnight, which is the reason for waves
        // rather than twenty four slots.
        let late = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown).at_hour(23));
        let midnight = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown).at_hour(0));
        let gap = (late.get(HOUR).unwrap_or(0.0) - midnight.get(HOUR).unwrap_or(0.0)).abs();
        assert!(gap < 0.3, "the clock does not wrap, gap was {gap}");
    }

    #[test]
    fn an_out_of_range_hour_is_clamped_rather_than_wrapping_into_a_panic() {
        let url = Url::parse("https://example.com/").unwrap();
        let v = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown).at_hour(200));
        assert_eq!(v.get(HOUR + 2), Some(0.0));
    }

    #[test]
    fn pins_are_recorded_without_being_obeyed_here() {
        let url = Url::parse("https://example.com/").unwrap();
        let country = Country::new("de").unwrap();
        let pins = CallerPins {
            mode: Some(RequestMode::Browser),
            proxy: Some(ProxyPool::Residential),
            country: Some(&country),
        };
        let v = featurize(&RouteInput::new(&url, DeclaredNeed::Markdown).with_pins(pins));

        assert_eq!(v.get(PINS), Some(1.0));
        assert_eq!(v.get(PINS + 3), Some(1.0));
        assert_eq!(v.get(PINS + 5), Some(1.0));
        assert_eq!(v.get(PINS + 6), Some(1.0));
    }

    #[test]
    fn a_site_with_no_path_is_not_a_trailing_slash_site() {
        let bare = vector("https://example.com");
        let slashed = vector("https://example.com/");

        assert_eq!(bare, slashed);
        assert_eq!(bare.get(TRAILING_SLASH), Some(0.0));
        assert_eq!(bare.get(SEGMENT + SegmentClass::Empty.index()), Some(1.0));
    }

    #[test]
    fn url_flags_read_the_request_and_not_the_site() {
        let plain = vector("http://example.com/a");
        assert_eq!(plain.get(URL_FLAGS), Some(0.0));

        let odd = vector("https://user:pw@example.com:8443/a#top");
        assert_eq!(odd.get(URL_FLAGS), Some(1.0));
        assert_eq!(odd.get(URL_FLAGS + 1), Some(1.0));
        assert_eq!(odd.get(URL_FLAGS + 2), Some(1.0));
        assert_eq!(odd.get(URL_FLAGS + 3), Some(1.0));
    }

    #[test]
    fn the_bias_slot_is_always_set() {
        assert_eq!(vector("https://example.com/").get(BIAS), Some(1.0));
    }
}
