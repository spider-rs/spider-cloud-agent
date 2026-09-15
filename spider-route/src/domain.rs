//! The only place a host string is read.
//!
//! Everything this crate learns from the host of a url is decided here and
//! handed out as [`HostShape`], which is `Copy`, holds no text, and cannot be
//! turned back into a name. Keeping the reads in one file is what makes the
//! privacy claim checkable: a reviewer reads this module and knows the rest of
//! the crate never sees a host. `tests` at the bottom of this file enforces it
//! against the sibling modules.
//!
//! What comes out is a label group, a count of labels in front of the
//! registrable name, and three flags. None of that separates one site from
//! another inside its group, which is the point.

use url::{Host, Url};

/// The label group a host falls into.
///
/// The named entries are a fixed public list. Everything else lands in one of
/// the four groups after them, so an unusual suffix is still a slot we can
/// print in the model card rather than a hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TldSlot {
    /// `com`
    Com,
    /// `net`
    Net,
    /// `org`
    Org,
    /// `io`
    Io,
    /// `co`
    Co,
    /// `ai`
    Ai,
    /// `dev`
    Dev,
    /// `app`
    App,
    /// `edu`
    Edu,
    /// `gov`
    Gov,
    /// `info`
    Info,
    /// `biz`
    Biz,
    /// `me`
    Me,
    /// `tv`
    Tv,
    /// `xyz`
    Xyz,
    /// `shop`
    Shop,
    /// `online`
    Online,
    /// `cloud`
    Cloud,
    /// `uk`
    Uk,
    /// `de`
    De,
    /// `fr`
    Fr,
    /// `jp`
    Jp,
    /// `cn`
    Cn,
    /// `ru`
    Ru,
    /// `br`
    Br,
    /// `in`
    In,
    /// A suffix of two labels, such as a country suffix with a sector label in
    /// front of it. Sites under one of these sit a level deeper than the name
    /// suggests, and the subdomain count has to know that.
    MultiLabel,
    /// One of the suffixes that existed before the name space was opened up,
    /// and not on the named list above.
    OtherGtld,
    /// A two letter country suffix not on the named list above.
    OtherCctld,
    /// A suffix from the opened up name space, not on the named list above.
    NewGtld,
}

/// How many slots [`TldSlot`] needs.
pub const TLD_SLOTS: usize = 30;

impl TldSlot {
    /// Which slot in the label block this group sets.
    pub const fn index(self) -> usize {
        match self {
            TldSlot::Com => 0,
            TldSlot::Net => 1,
            TldSlot::Org => 2,
            TldSlot::Io => 3,
            TldSlot::Co => 4,
            TldSlot::Ai => 5,
            TldSlot::Dev => 6,
            TldSlot::App => 7,
            TldSlot::Edu => 8,
            TldSlot::Gov => 9,
            TldSlot::Info => 10,
            TldSlot::Biz => 11,
            TldSlot::Me => 12,
            TldSlot::Tv => 13,
            TldSlot::Xyz => 14,
            TldSlot::Shop => 15,
            TldSlot::Online => 16,
            TldSlot::Cloud => 17,
            TldSlot::Uk => 18,
            TldSlot::De => 19,
            TldSlot::Fr => 20,
            TldSlot::Jp => 21,
            TldSlot::Cn => 22,
            TldSlot::Ru => 23,
            TldSlot::Br => 24,
            TldSlot::In => 25,
            TldSlot::MultiLabel => 26,
            TldSlot::OtherGtld => 27,
            TldSlot::OtherCctld => 28,
            TldSlot::NewGtld => 29,
        }
    }
}

/// Which kind of literal address a host is, when it is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpKind {
    /// Four numbers.
    V4,
    /// Eight groups.
    V6,
}

/// Everything the featurizer is allowed to know about a host.
///
/// No field can hold text, so no value of this type can carry a name out of
/// this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostShape {
    /// The label group, absent for a literal address and for a url with no
    /// host at all.
    pub tld: Option<TldSlot>,
    /// Labels in front of the registrable name, capped at three.
    pub subdomains: u8,
    /// Whether the first label is the conventional web prefix.
    pub www: bool,
    /// Set when the host is a literal address rather than a name.
    pub ip: Option<IpKind>,
    /// Set when any label is in the encoded form that carries non-ascii text.
    /// Worth a slot because those sites are served differently often enough to
    /// matter, and because leaving it out would push the featurizer to look at
    /// the label itself.
    pub punycode: bool,
}

/// The named suffixes, paired with their group.
const NAMED: [(&str, TldSlot); 26] = [
    ("com", TldSlot::Com),
    ("net", TldSlot::Net),
    ("org", TldSlot::Org),
    ("io", TldSlot::Io),
    ("co", TldSlot::Co),
    ("ai", TldSlot::Ai),
    ("dev", TldSlot::Dev),
    ("app", TldSlot::App),
    ("edu", TldSlot::Edu),
    ("gov", TldSlot::Gov),
    ("info", TldSlot::Info),
    ("biz", TldSlot::Biz),
    ("me", TldSlot::Me),
    ("tv", TldSlot::Tv),
    ("xyz", TldSlot::Xyz),
    ("shop", TldSlot::Shop),
    ("online", TldSlot::Online),
    ("cloud", TldSlot::Cloud),
    ("uk", TldSlot::Uk),
    ("de", TldSlot::De),
    ("fr", TldSlot::Fr),
    ("jp", TldSlot::Jp),
    ("cn", TldSlot::Cn),
    ("ru", TldSlot::Ru),
    ("br", TldSlot::Br),
    ("in", TldSlot::In),
];

/// Suffixes that existed before the name space opened up.
///
/// The split between these and the newer ones is coarse on purpose. It is a
/// rough age signal, not a registry.
const LEGACY_GTLD: [&str; 16] = [
    "mil", "int", "name", "pro", "jobs", "mobi", "tel", "travel", "cat", "asia", "post", "aero",
    "coop", "museum", "arpa", "xxx",
];

/// Second level suffixes common enough to be worth knowing.
///
/// A host under one of these has its registrable name one label further left,
/// and counting subdomains without this list would say every such site has an
/// extra subdomain.
const MULTI_LABEL: [&str; 24] = [
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "net.uk", "com.au", "net.au", "org.au",
    "gov.au", "edu.au", "co.jp", "ne.jp", "or.jp", "go.jp", "ac.jp", "com.br", "com.cn", "com.mx",
    "co.nz", "co.za", "co.in", "com.tr", "co.kr",
];

/// Read the host of a url into the shape the featurizer works from.
///
/// Allocates nothing. Comparison is case insensitive in the few places it has
/// to be, though `Url` has already lowercased a name host by the time it gets
/// here.
pub fn host_shape(url: &Url) -> HostShape {
    match url.host() {
        Some(Host::Ipv4(_)) => HostShape {
            ip: Some(IpKind::V4),
            ..HostShape::default()
        },
        Some(Host::Ipv6(_)) => HostShape {
            ip: Some(IpKind::V6),
            ..HostShape::default()
        },
        Some(Host::Domain(host)) => name_shape(host),
        None => HostShape::default(),
    }
}

/// The name a site is registered under, which is the coarsest key that still
/// tells two sites apart.
///
/// `www.example.com` and `shop.example.com` both answer `example.com`, and
/// `news.example.co.uk` answers `example.co.uk`, because the known pair
/// suffixes are read the same way the featurizer reads them. A literal address
/// and a url with no host answer nothing.
///
/// Borrowed from the url, so this allocates nothing. It exists for a caller
/// that keeps its own per site records: the key stays with the caller, and
/// nothing derived from it reaches a [`crate::FeatureVector`].
pub fn registrable_domain(url: &Url) -> Option<&str> {
    let host = match url.host() {
        Some(Host::Domain(host)) => host,
        _ => return None,
    };

    // The last two dot positions before the suffix, right to left. That is all
    // the decision needs: two labels normally, three when the suffix is a
    // known pair.
    let mut dots = host.rmatch_indices('.').map(|(at, _)| at).skip(1);

    let Some(second) = dots.next() else {
        // Two labels or fewer, so the whole host is as far left as it goes.
        return Some(host);
    };

    let pair = host.get(second.saturating_add(1)..).unwrap_or(host);
    let cut = if MULTI_LABEL
        .iter()
        .any(|known| known.eq_ignore_ascii_case(pair))
    {
        match dots.next() {
            Some(third) => third,
            None => return Some(host),
        }
    } else {
        second
    };

    host.get(cut.saturating_add(1)..)
}

/// The shape of a name host.
fn name_shape(host: &str) -> HostShape {
    let mut labels = 0usize;
    let mut punycode = false;
    let mut first = "";
    let mut last = "";
    let mut second_last = "";

    for label in host.split('.') {
        if labels == 0 {
            first = label;
        }

        if label.len() > 4
            && label
                .get(..4)
                .is_some_and(|p| p.eq_ignore_ascii_case("xn--"))
        {
            punycode = true;
        }

        second_last = last;
        last = label;
        labels += 1;
    }

    let multi = labels >= 3 && is_multi_label(second_last, last);

    let tld = if multi {
        Some(TldSlot::MultiLabel)
    } else {
        Some(classify_suffix(last))
    };

    // Two labels are the registrable name, three when the suffix is a pair.
    // Everything left of that is a subdomain, capped so a long internal name
    // cannot become a near unique number.
    let registrable = if multi { 3 } else { 2 };
    let subdomains = labels.saturating_sub(registrable).min(3) as u8;

    HostShape {
        tld,
        subdomains,
        www: first.eq_ignore_ascii_case("www"),
        ip: None,
        punycode,
    }
}

/// Whether these two labels form one of the known second level suffixes.
fn is_multi_label(second_last: &str, last: &str) -> bool {
    MULTI_LABEL.iter().any(|pair| match pair.split_once('.') {
        Some((head, tail)) => {
            head.eq_ignore_ascii_case(second_last) && tail.eq_ignore_ascii_case(last)
        }
        None => false,
    })
}

/// Which group a single suffix label belongs to.
fn classify_suffix(last: &str) -> TldSlot {
    for (name, slot) in NAMED {
        if last.eq_ignore_ascii_case(name) {
            return slot;
        }
    }

    if last.len() == 2 && last.bytes().all(|b| b.is_ascii_alphabetic()) {
        return TldSlot::OtherCctld;
    }

    if LEGACY_GTLD
        .iter()
        .any(|name| last.eq_ignore_ascii_case(name))
    {
        return TldSlot::OtherGtld;
    }

    TldSlot::NewGtld
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

    fn shape(url: &str) -> HostShape {
        host_shape(&Url::parse(url).unwrap())
    }

    #[test]
    fn named_suffix_wins() {
        assert_eq!(shape("https://example.com/").tld, Some(TldSlot::Com));
        assert_eq!(shape("https://example.dev/").tld, Some(TldSlot::Dev));
    }

    #[test]
    fn unnamed_suffixes_fall_into_groups() {
        assert_eq!(shape("https://example.sn/").tld, Some(TldSlot::OtherCctld));
        assert_eq!(
            shape("https://example.museum/").tld,
            Some(TldSlot::OtherGtld)
        );
        assert_eq!(
            shape("https://example.plumbing/").tld,
            Some(TldSlot::NewGtld)
        );
    }

    #[test]
    fn a_paired_suffix_does_not_inflate_the_subdomain_count() {
        let plain = shape("https://www.example.com/");
        let paired = shape("https://www.example.co.uk/");

        assert_eq!(paired.tld, Some(TldSlot::MultiLabel));
        assert_eq!(paired.subdomains, plain.subdomains);
        assert_eq!(paired.subdomains, 1);
    }

    #[test]
    fn subdomains_are_counted_and_capped() {
        assert_eq!(shape("https://example.com/").subdomains, 0);
        assert_eq!(shape("https://a.example.com/").subdomains, 1);
        assert_eq!(shape("https://a.b.example.com/").subdomains, 2);
        assert_eq!(shape("https://a.b.c.d.e.example.com/").subdomains, 3);
    }

    #[test]
    fn literal_addresses_have_no_suffix() {
        let four = shape("http://192.0.2.10:8080/api");
        assert_eq!(four.ip, Some(IpKind::V4));
        assert_eq!(four.tld, None);

        let six = shape("http://[2001:db8::1]/api");
        assert_eq!(six.ip, Some(IpKind::V6));
        assert_eq!(six.tld, None);
    }

    #[test]
    fn encoded_labels_are_flagged_without_being_read() {
        let encoded = shape("https://xn--bcher-kva.example.com/");
        assert!(encoded.punycode);
        assert_eq!(encoded.tld, Some(TldSlot::Com));
        assert!(!shape("https://bucher.example.com/").punycode);
    }

    #[test]
    fn a_non_ascii_host_does_not_split_a_character() {
        // `Url` encodes this before we see it, and the check above reads four
        // ascii bytes with `get`, so neither path can cut a character in half.
        let shape = shape("https://\u{4f60}\u{597d}.com/");
        assert!(shape.punycode);
        assert_eq!(shape.tld, Some(TldSlot::Com));
    }

    #[test]
    fn the_host_is_read_in_this_module_and_nowhere_else() {
        // The privacy contract is structural, so it is checked structurally.
        // Anything that pulls the host out of a url has to live here, where a
        // reviewer looking for it will find all of it.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let reads = ["host_str(", ".host()", ".domain()", "Host::Domain"];

        let mut checked = 0;
        let mut this_file_reads_the_host = false;

        for entry in std::fs::read_dir(&dir).expect("the source directory is readable") {
            let path = entry.expect("a readable directory entry").path();

            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }

            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("a source file has a name")
                .to_string();
            let body = std::fs::read_to_string(&path).expect("a source file is readable");
            let hits: Vec<&str> = reads
                .iter()
                .copied()
                .filter(|read| body.contains(read))
                .collect();

            if name == "domain.rs" {
                this_file_reads_the_host = !hits.is_empty();
            } else {
                assert!(
                    hits.is_empty(),
                    "{name} reads the host directly ({hits:?}). Move it into domain.rs \
                     and hand out a HostShape instead."
                );
            }

            checked += 1;
        }

        // A directory read that returned nothing would pass every assertion
        // above without checking anything.
        assert!(
            checked >= 5,
            "expected to scan the whole crate, scanned {checked} files"
        );
        assert!(
            this_file_reads_the_host,
            "domain.rs no longer reads the host, so the scan above proves nothing"
        );
    }

    #[test]
    fn the_registrable_name_drops_subdomains_and_keeps_pair_suffixes() {
        let cases = [
            ("https://example.com/a", Some("example.com")),
            ("https://www.example.com/a", Some("example.com")),
            ("https://shop.eu.example.com/a", Some("example.com")),
            ("https://example.co.uk/a", Some("example.co.uk")),
            ("https://news.example.co.uk/a", Some("example.co.uk")),
            ("https://deep.news.example.co.uk/a", Some("example.co.uk")),
            ("https://example/a", Some("example")),
            ("https://192.0.2.1/a", None),
            ("https://[2001:db8::1]/a", None),
        ];

        for (raw, expected) in cases {
            let url = Url::parse(raw).unwrap();
            assert_eq!(registrable_domain(&url), expected, "{raw}");
        }
    }

    #[test]
    fn two_sites_under_one_name_share_a_key_and_two_names_do_not() {
        let a = Url::parse("https://www.example.com/one").unwrap();
        let b = Url::parse("https://api.example.com/two").unwrap();
        let c = Url::parse("https://example.org/one").unwrap();

        assert_eq!(registrable_domain(&a), registrable_domain(&b));
        assert_ne!(registrable_domain(&a), registrable_domain(&c));
    }
}
