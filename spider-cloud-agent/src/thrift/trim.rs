//! Cutting what the service already sent.
//!
//! The second tier, and the smaller one. By the time text reaches here the
//! request has already been made, so everything this saves is context window
//! rather than credits. It is still worth doing: a crawl of one site repeats
//! its navigation and its footer on every page, and a model pays for that
//! repetition once per page.
//!
//! Five cuts, in this order:
//!
//! 1. blocks that repeat across the pages of one crawl
//! 2. `data:` URIs past a size
//! 3. lines that are nothing but a link
//! 4. runs of blank lines and repeated spaces
//! 5. a token ceiling, landing on a sentence end
//!
//! Boilerplate goes first because it is the only cut that needs more than one
//! page, and the ceiling goes last because it is the only one that has to know
//! the final size.

use std::collections::HashMap;
use std::collections::HashSet;

use crate::thrift::need::Need;
use crate::thrift::tokens::{max_tokens, Ceiling};

/// How much of a crawl a block has to appear in before it counts as furniture.
pub const DEFAULT_BOILERPLATE_SHARE: f32 = 0.6;

/// How many pages a crawl needs before boilerplate removal will run at all.
///
/// Below this a repeat is as likely to be content as furniture, and dropping
/// it would cost the caller something they asked for.
pub const MIN_PAGES_FOR_BOILERPLATE: usize = 3;

/// How long a `data:` URI may be before it is dropped.
///
/// Short ones are icons and spacers that cost little and sometimes carry
/// meaning. Long ones are images inlined into the markup, and they are the
/// single largest thing a page can hand a model that it cannot read.
pub const DEFAULT_DATA_URI_LIMIT: usize = 256;

/// What was cut, and how much of it.
///
/// One of these per cut that actually removed something, so a report says
/// where the reduction came from rather than only that there was one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrimReason {
    /// Blocks that repeat across the pages of one crawl.
    Boilerplate {
        /// How many lines went.
        lines: usize,
        /// How many bytes they were.
        bytes: usize,
    },
    /// Inlined `data:` URIs past the size limit.
    DataUri {
        /// How many were dropped.
        count: usize,
        /// How many bytes they were.
        bytes: usize,
    },
    /// Lines that held a link and nothing else.
    LinkLines {
        /// How many lines went.
        lines: usize,
        /// How many bytes they were.
        bytes: usize,
    },
    /// Runs of blank lines and repeated spaces.
    Whitespace {
        /// How many bytes went.
        bytes: usize,
    },
    /// The end of the text, cut to fit a token ceiling.
    Truncated {
        /// How many tokens were dropped.
        tokens: usize,
    },
}

impl TrimReason {
    /// How many bytes this cut removed. A truncation counts none, because it
    /// is measured in tokens.
    pub fn bytes(&self) -> usize {
        match self {
            TrimReason::Boilerplate { bytes, .. }
            | TrimReason::DataUri { bytes, .. }
            | TrimReason::LinkLines { bytes, .. }
            | TrimReason::Whitespace { bytes } => *bytes,
            TrimReason::Truncated { .. } => 0,
        }
    }
}

/// Text after trimming, with a note of what came out of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trimmed {
    /// What is left.
    pub text: String,
    /// What was cut.
    pub reasons: Vec<TrimReason>,
}

impl Trimmed {
    /// How many bytes were removed, the truncation aside.
    pub fn removed_bytes(&self) -> usize {
        self.reasons.iter().map(TrimReason::bytes).sum()
    }
}

/// Which cuts to make.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TrimSettings {
    /// How long a `data:` URI may be before it goes.
    pub data_uri_limit: usize,
    /// Whether a line that is only a link is dropped. Off when the links are
    /// what was asked for.
    pub drop_link_lines: bool,
    /// Whether runs of blank lines and repeated spaces are collapsed.
    pub collapse_blanks: bool,
    /// How much of a crawl a block must appear in to count as furniture.
    pub boilerplate_share: f32,
    /// How many pages are needed before boilerplate removal runs.
    pub min_pages_for_boilerplate: usize,
}

impl Default for TrimSettings {
    fn default() -> TrimSettings {
        TrimSettings {
            data_uri_limit: DEFAULT_DATA_URI_LIMIT,
            drop_link_lines: true,
            collapse_blanks: true,
            boilerplate_share: DEFAULT_BOILERPLATE_SHARE,
            min_pages_for_boilerplate: MIN_PAGES_FOR_BOILERPLATE,
        }
    }
}

impl TrimSettings {
    /// Settings that fit a need.
    ///
    /// The only thing the need changes is whether link lines survive, and
    /// [`Need::Links`] is the one where they are the point.
    pub fn for_need(need: &Need) -> TrimSettings {
        TrimSettings {
            drop_link_lines: !matches!(need, Need::Links),
            ..TrimSettings::default()
        }
    }

    /// Make no cuts at all, which is what [`Need::Raw`] gets.
    pub fn untouched() -> TrimSettings {
        TrimSettings {
            data_uri_limit: usize::MAX,
            drop_link_lines: false,
            collapse_blanks: false,
            boilerplate_share: 1.1,
            min_pages_for_boilerplate: usize::MAX,
        }
    }
}

/// The cuts, plus whatever it has learned about one crawl.
///
/// Trimming one page needs no state. Removing boilerplate needs every page of
/// the crawl, so it is learned once with [`Trimmer::learn`] and then applied to
/// each page in turn.
#[derive(Debug, Clone, Default)]
pub struct Trimmer {
    settings: TrimSettings,
    furniture: HashSet<u64>,
    repeated_lines: HashSet<u64>,
}

impl Trimmer {
    /// A trimmer with these settings and nothing learned yet.
    pub fn new(settings: TrimSettings) -> Trimmer {
        Trimmer {
            settings,
            furniture: HashSet::new(),
            repeated_lines: HashSet::new(),
        }
    }

    /// A trimmer that fits a need.
    pub fn for_need(need: &Need) -> Trimmer {
        match need {
            Need::Raw => Trimmer::new(TrimSettings::untouched()),
            other => Trimmer::new(TrimSettings::for_need(other)),
        }
    }

    /// The settings in force.
    pub fn settings(&self) -> &TrimSettings {
        &self.settings
    }

    /// Learn what repeats across the pages of one crawl.
    ///
    /// A block counts as furniture when its lines appear on more than
    /// [`TrimSettings::boilerplate_share`] of the pages and they appear next to
    /// each other on those pages. Both halves matter. Line frequency on its own
    /// drops a sentence that two product pages happen to share; adjacency on
    /// its own drops the first line of real content that follows a menu.
    pub fn learn<S: AsRef<str>>(&mut self, pages: &[S]) {
        self.furniture.clear();
        self.repeated_lines.clear();

        if pages.len() < self.settings.min_pages_for_boilerplate {
            return;
        }

        let mut line_pages: HashMap<u64, usize> = HashMap::new();
        let mut pair_pages: HashMap<u64, usize> = HashMap::new();

        for page in pages {
            let lines = meaningful_lines(page.as_ref());
            let mut seen_lines: HashSet<u64> = HashSet::new();
            let mut seen_pairs: HashSet<u64> = HashSet::new();

            for hash in &lines {
                if seen_lines.insert(*hash) {
                    *line_pages.entry(*hash).or_insert(0) += 1;
                }
            }
            for window in lines.windows(2) {
                let [first, second] = window else { continue };
                let pair = mix(*first, *second);
                if seen_pairs.insert(pair) {
                    *pair_pages.entry(pair).or_insert(0) += 1;
                }
            }
        }

        let floor = (pages.len() as f32 * self.settings.boilerplate_share).floor() as usize;
        let keep_above = |count: usize| count > floor;

        self.repeated_lines = line_pages
            .into_iter()
            .filter(|(_, count)| keep_above(*count))
            .map(|(hash, _)| hash)
            .collect();
        self.furniture = pair_pages
            .into_iter()
            .filter(|(_, count)| keep_above(*count))
            .map(|(hash, _)| hash)
            .collect();
    }

    /// Whether anything was learned worth dropping.
    pub fn knows_boilerplate(&self) -> bool {
        !self.furniture.is_empty()
    }

    /// Trim one page, to a token ceiling when there is one.
    pub fn trim(&self, text: &str, max_tokens: Option<usize>) -> Trimmed {
        let mut reasons = Vec::new();
        let mut out = text.to_string();

        if self.knows_boilerplate() {
            let (next, lines, bytes) = self.strip_furniture(&out);
            if lines > 0 {
                reasons.push(TrimReason::Boilerplate { lines, bytes });
                out = next;
            }
        }

        if self.settings.data_uri_limit < usize::MAX {
            let (next, count, bytes) = drop_data_uris(&out, self.settings.data_uri_limit);
            if count > 0 {
                reasons.push(TrimReason::DataUri { count, bytes });
                out = next;
            }
        }

        if self.settings.drop_link_lines {
            let (next, lines, bytes) = drop_link_only_lines(&out);
            if lines > 0 {
                reasons.push(TrimReason::LinkLines { lines, bytes });
                out = next;
            }
        }

        if self.settings.collapse_blanks {
            let before = out.len();
            let collapsed = collapse_whitespace(&out);
            if collapsed.len() < before {
                reasons.push(TrimReason::Whitespace {
                    bytes: before - collapsed.len(),
                });
            }
            out = collapsed;
        }

        if let Some(ceiling) = max_tokens {
            let (next, dropped) = truncate_to_tokens(&out, ceiling);
            if dropped > 0 {
                reasons.push(TrimReason::Truncated { tokens: dropped });
                out = next;
            }
        }

        Trimmed { text: out, reasons }
    }

    /// Drop the lines that belong to a repeated block.
    fn strip_furniture(&self, text: &str) -> (String, usize, usize) {
        // Position in the page, and the hash of the line at it. Blank lines are
        // left out, because the pairs were learned over the lines that carry
        // something.
        let carried: Vec<(usize, u64)> = text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                let normal = normalize(line);
                (!normal.is_empty()).then(|| (index, hash(&normal)))
            })
            .collect();

        let mut furniture_lines: HashSet<usize> = HashSet::new();
        for (position, (index, this)) in carried.iter().enumerate() {
            if !self.repeated_lines.contains(this) {
                continue;
            }
            let after_a_repeat = position
                .checked_sub(1)
                .and_then(|earlier| carried.get(earlier))
                .is_some_and(|(_, before)| self.furniture.contains(&mix(*before, *this)));
            let before_a_repeat = carried
                .get(position + 1)
                .is_some_and(|(_, after)| self.furniture.contains(&mix(*this, *after)));

            if after_a_repeat || before_a_repeat {
                furniture_lines.insert(*index);
            }
        }

        let mut kept = String::with_capacity(text.len());
        let mut dropped_lines = 0usize;
        let mut dropped_bytes = 0usize;
        for (index, line) in text.lines().enumerate() {
            if furniture_lines.contains(&index) {
                dropped_lines += 1;
                dropped_bytes += line.len() + 1;
                continue;
            }
            kept.push_str(line);
            kept.push('\n');
        }

        (kept, dropped_lines, dropped_bytes)
    }
}

/// The hashes of a page's non-blank lines, in order.
fn meaningful_lines(text: &str) -> Vec<u64> {
    text.lines()
        .map(normalize)
        .filter(|line| !line.is_empty())
        .map(|line| hash(&line))
        .collect()
}

/// A line with its spacing made uniform, so the same menu entry hashes the same
/// on two pages that indent it differently.
fn normalize(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// FNV-1a. A string hash with no dependency and no cryptographic claim: the
/// worst a collision does here is drop a line that was not furniture.
fn hash(text: &str) -> u64 {
    let mut value: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x1000_0000_01b3);
    }
    value
}

/// Combine two line hashes into a hash for the pair.
fn mix(first: u64, second: u64) -> u64 {
    let mut value = first ^ 0x9e37_79b9_7f4a_7c15;
    value = value.wrapping_mul(0x1000_0000_01b3);
    value ^= second;
    value.wrapping_mul(0x1000_0000_01b3)
}

/// Collapse runs of blank lines to one, and repeated spaces to one.
///
/// Trailing spaces go as well, which markdown uses for a hard line break. That
/// is a deliberate loss: nothing reading this text renders it.
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    let mut wrote_anything = false;

    for line in text.lines() {
        let squeezed = normalize(line);
        if squeezed.is_empty() {
            blank_run += 1;
            continue;
        }
        if wrote_anything {
            out.push('\n');
            if blank_run > 0 {
                out.push('\n');
            }
        }
        out.push_str(&squeezed);
        wrote_anything = true;
        blank_run = 0;
    }

    out
}

/// Drop `data:` URIs longer than `limit`, leaving a note of the size.
///
/// Returns the text, how many went, and how many bytes they were.
pub fn drop_data_uris(text: &str, limit: usize) -> (String, usize, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0usize;
    let mut bytes = 0usize;
    let mut rest = text;

    while let Some(start) = rest.find("data:") {
        let (before, from_start) = rest.split_at(start);
        let end = from_start
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>' | '<'))
            .unwrap_or(from_start.len());
        let (uri, after) = from_start.split_at(end);

        out.push_str(before);
        if uri.len() > limit {
            count += 1;
            bytes += uri.len();
            out.push_str(&format!("data:[dropped {} bytes]", uri.len()));
        } else {
            out.push_str(uri);
        }
        rest = after;
    }
    out.push_str(rest);

    (out, count, bytes)
}

/// Drop lines that carry a link and nothing else.
///
/// A bare address, a markdown link on its own, or either of those as a single
/// list item. A line with a link and a sentence around it stays.
pub fn drop_link_only_lines(text: &str) -> (String, usize, usize) {
    let mut out = String::with_capacity(text.len());
    let mut lines = 0usize;
    let mut bytes = 0usize;

    for line in text.lines() {
        if is_link_only(line) {
            lines += 1;
            bytes += line.len() + 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    (out, lines, bytes)
}

/// Whether a line is a link and nothing else.
fn is_link_only(line: &str) -> bool {
    let body = line
        .trim()
        .trim_start_matches(['-', '*', '+'])
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches(['.', ')'])
        .trim();

    if body.is_empty() {
        return false;
    }

    if body.starts_with("http://") || body.starts_with("https://") {
        return !body.contains(char::is_whitespace);
    }

    // A markdown link, `[label](target)`, with nothing either side of it.
    if let Some(rest) = body.strip_prefix('[') {
        if let Some((label, target)) = rest.split_once("](") {
            if let Some(target) = target.strip_suffix(')') {
                return !label.contains(['[', ']']) && !target.contains(['(', ')']);
            }
        }
    }

    false
}

/// Cut text down to a token ceiling, landing on a sentence end.
///
/// Returns the text and how many tokens were dropped. The marker that says so
/// is counted against the ceiling, so what comes back fits the number that was
/// asked for rather than that number plus an apology.
///
/// With no sentence end inside the ceiling, the cut lands on a word boundary,
/// and with neither, on the last character that fits. Minified markup has no
/// whitespace in it at all, and handing that caller an empty string would cost
/// them the page. A ceiling too small even for the marker returns nothing at
/// all, because there is nothing honest to return.
///
/// The ceiling is enforced in [`max_tokens`], not in
/// [`approx_tokens`](crate::thrift::approx_tokens). A caller who sized a
/// budget to a context window gets less text than the window would hold rather
/// than more.
pub fn truncate_to_tokens(text: &str, ceiling: usize) -> (String, usize) {
    let total = max_tokens(text);
    if total <= ceiling {
        return (text.to_string(), 0);
    }

    // The marker's own cost, allowing for the digits of a number we do not
    // know yet.
    let marker_cost = approx_marker_tokens(total);
    let room = ceiling.saturating_sub(marker_cost);
    if room == 0 {
        return (String::new(), total);
    }

    let cut = floor_to_character(text, last_boundary_within(text, room));
    if cut == 0 {
        return (String::new(), total);
    }

    // `cut` is a character boundary twice over: the walk counts characters, and
    // the floor above settles it again. Asking for the slice rather than taking
    // it means a third mistake would cost a few bytes rather than the caller's
    // process.
    let Some(kept) = text.get(..cut) else {
        return (String::new(), total);
    };
    let kept = kept.trim_end();
    let dropped = total.saturating_sub(max_tokens(kept));
    let mut out = String::with_capacity(kept.len() + 32);
    out.push_str(kept);
    out.push_str(&format!("\n...[truncated {dropped} tokens]"));

    (out, dropped)
}

/// What the truncation marker costs, in tokens.
fn approx_marker_tokens(total: usize) -> usize {
    max_tokens(&format!("\n...[truncated {total} tokens]"))
}

/// The nearest character boundary at or before `at`.
///
/// Every cut this module makes is worked out by walking characters, so this
/// should never move anything. It is here because the cost of being wrong is a
/// panic in someone else's program, and the cost of being sure is a handful of
/// comparisons on text that is about to be thrown away anyway.
fn floor_to_character(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Whether a character ends a sentence in a script that puts a space after it.
fn ends_a_sentence(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

/// Whether a character ends a sentence on its own, with no space after it.
///
/// The wide stops. Chinese and Japanese text runs the next sentence straight
/// on, so waiting for a space here would mean never finding a sentence end in
/// a whole page of it.
fn ends_a_wide_sentence(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '…' | '．')
}

/// Whether text may be cut after this character even without a space.
///
/// Han, kana and the full width punctuation that goes with them are written
/// without spaces between words, so whitespace is no guide to where a cut is
/// safe. Emoji are deliberately not in here: a skin tone or a joiner follows
/// its base character, and cutting between them makes nonsense of both.
fn breaks_without_a_space(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303f}'
        | '\u{3040}'..='\u{30ff}'
        | '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{f900}'..='\u{faff}'
        | '\u{ff00}'..='\u{ffef}'
    )
}

/// Whether a character only means anything attached to the one before it.
///
/// Nothing may be cut in front of one of these. The kana voiced sound marks
/// are the reason: decomposed Japanese writes `\u{304c}` as `\u{304b}` plus
/// `\u{3099}`, and `\u{304b}` is a character this module is otherwise happy
/// to break after. Cutting there changes the syllable rather than shortening
/// the text. The variation selectors and the joiner do the same to emoji, and
/// the combining marks do it to every script that uses them.
fn hangs_on_the_character_before(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036f}'      // combining diacritics
        | '\u{0483}'..='\u{0489}'
        | '\u{0591}'..='\u{05bd}'
        | '\u{0610}'..='\u{061a}'
        | '\u{064b}'..='\u{065f}'
        | '\u{0e31}' | '\u{0e34}'..='\u{0e3a}' | '\u{0e47}'..='\u{0e4e}'
        | '\u{0900}'..='\u{0903}' | '\u{093a}'..='\u{094f}'
        | '\u{1ab0}'..='\u{1aff}'
        | '\u{1dc0}'..='\u{1dff}'
        | '\u{200d}'                // zero width joiner
        | '\u{20d0}'..='\u{20f0}'
        | '\u{3099}'..='\u{309a}'   // kana voiced sound marks
        | '\u{fe00}'..='\u{fe0f}'   // variation selectors
        | '\u{fe20}'..='\u{fe2f}'
        | '\u{ff9e}'..='\u{ff9f}'   // halfwidth kana voiced sound marks
        | '\u{1f3fb}'..='\u{1f3ff}' // skin tones
        | '\u{e0100}'..='\u{e01ef}'
    )
}

/// The byte index of the last sentence end that fits inside `room` tokens.
///
/// Falls back to the last place a word or a character ends, and to zero when
/// not even one of those fits.
fn last_boundary_within(text: &str, room: usize) -> usize {
    let mut spent = Ceiling::new();
    let mut sentence_end = 0usize;
    let mut word_end = 0usize;
    let mut character_end = 0usize;

    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        spent.push(c);
        if spent.tokens() > room {
            break;
        }

        let next_index = index + c.len_utf8();
        let next = chars.peek().map(|(_, next)| *next);
        let followed_by_space = next.map(char::is_whitespace).unwrap_or(true);
        // A mark that leans on this character has to travel with it, so none
        // of the three boundaries may land between the two.
        let may_cut_after = !next.map(hangs_on_the_character_before).unwrap_or(false);

        if may_cut_after {
            character_end = next_index;
            if ends_a_wide_sentence(c) || (ends_a_sentence(c) && followed_by_space) {
                sentence_end = next_index;
            }
            if c.is_whitespace() {
                word_end = index;
            } else if breaks_without_a_space(c) {
                word_end = next_index;
            }
        }
    }

    // A sentence end reads best, a word end next. Falling all the way through
    // to the last character that fits is what keeps a page with no whitespace
    // in it from coming back empty.
    if sentence_end > 0 {
        sentence_end
    } else if word_end > 0 {
        word_end
    } else {
        character_end
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
    use crate::thrift::tokens::max_tokens;

    /// Text the crate will meet on a real site and has no business panicking
    /// on: accented Latin, currency symbols, Han, an emoji carrying a skin tone
    /// modifier, and Arabic written right to left.
    const AWKWARD: &[(&str, &str)] = &[
        (
            "accented",
            "Café, naïve, Ærø. Prix réduit pour la rentrée scolaire.",
        ),
        (
            "currency",
            "£19.99, €22.40, ¥3200, ₹1650. Livraison incluse.",
        ),
        (
            "han",
            "钢制垫圈，四十毫米。冷压装配，不要加热外壳。同日发货。",
        ),
        ("emoji", "Ready 👍🏽 to ship 🧑🏾‍🔧 today. Packed 📦 and sealed."),
        (
            "rtl",
            "الشحن في نفس اليوم. السعر يشمل الضريبة. الرجاء الاتصال بنا.",
        ),
        (
            "mixed",
            "Café ☕ 钢制 £19.99 مرحبا 👍🏽. Second sentence, also mixed: naïve ¥3200.",
        ),
    ];

    /// Every awkward string, joined into something page shaped.
    fn awkward_page() -> String {
        let mut out = String::new();
        for (name, line) in AWKWARD {
            out.push_str(name);
            out.push('\n');
            out.push_str(line);
            out.push_str("\n\n");
        }
        out
    }

    /// What was kept, without the marker that says how much went.
    fn kept_part(out: &str) -> &str {
        out.split("\n...[truncated").next().unwrap_or_default()
    }

    /// Whether `kept` is the front of `original`, ending between two characters
    /// rather than inside one.
    ///
    /// Checking that a `String` holds valid UTF-8 proves nothing: the type
    /// cannot hold anything else, and a cut through a character either panics
    /// or comes back empty. The question worth asking is where in the original
    /// the cut landed.
    fn cut_between_characters(original: &str, kept: &str) -> bool {
        original.starts_with(kept) && original.is_char_boundary(kept.len())
    }

    #[test]
    fn truncation_lands_between_characters_whatever_the_script() {
        for (name, line) in AWKWARD {
            // Every ceiling from nothing to more than the line costs, so the
            // cut is asked to land between every pair of characters in turn.
            for ceiling in 0..=(max_tokens(line) + 4) {
                let (out, _) = truncate_to_tokens(line, ceiling);
                let kept = kept_part(&out);
                assert!(
                    cut_between_characters(line, kept),
                    "{name} at {ceiling} cut at byte {} of {line:?}",
                    kept.len()
                );
            }
        }
    }

    #[test]
    fn a_ceiling_large_enough_to_keep_something_keeps_something() {
        // The other half of the boundary test. A cut through a character comes
        // back empty rather than panicking, so a test that only asks whether
        // the result is whole would pass on a trimmer that returns nothing.
        for (name, line) in AWKWARD {
            let ceiling = max_tokens(line);
            let (out, dropped) = truncate_to_tokens(line, ceiling);
            assert_eq!(dropped, 0, "{name} was cut when it fitted");
            assert_eq!(out, *line, "{name}");

            // Long enough that three quarters of it is well clear of what the
            // marker costs, which is the case a caller actually hits.
            let long = [*line; 6].join(" ");
            let ceiling = max_tokens(&long) * 3 / 4;
            let (out, dropped) = truncate_to_tokens(&long, ceiling);
            let kept = kept_part(&out);
            assert!(dropped > 0, "{name} was not cut at all");
            assert!(!kept.is_empty(), "{name} kept nothing");
            assert!(
                kept.len() > long.len() / 2,
                "{name} kept {} bytes",
                kept.len()
            );
            assert!(cut_between_characters(&long, kept), "{name}");
        }
    }

    #[test]
    fn a_cut_never_separates_an_emoji_from_its_modifier() {
        // A skin tone and a joiner follow the character they change. Cutting
        // between them leaves a thumb of the wrong colour and an orphan.
        let line = "Ready 👍🏽 to ship 🧑🏾‍🔧 today. Packed 📦 and sealed, twice over.";
        let follower = |c: char| matches!(c, '\u{1f3fb}'..='\u{1f3ff}' | '\u{200d}' | '\u{fe0f}');

        for ceiling in 0..=(max_tokens(line) + 4) {
            let (out, _) = truncate_to_tokens(line, ceiling);
            let kept = kept_part(&out);
            let Some(next) = line.get(kept.len()..).and_then(|rest| rest.chars().next()) else {
                continue;
            };
            assert!(
                !follower(next),
                "ceiling {ceiling} cut before {next:?}, leaving {kept:?}"
            );
        }
    }

    #[test]
    fn a_page_with_no_spaces_in_it_is_still_cut_somewhere_useful() {
        // Han runs without spaces and ends its sentences with a wide stop, so
        // neither of the rules written for English finds anything at all.
        let line = ["钢制垫圈，四十毫米。冷压装配，不要加热外壳。同日发货，价格含税。"; 8].concat();
        let line = line.as_str();
        let ceiling = max_tokens(line) * 3 / 4;
        let (out, dropped) = truncate_to_tokens(line, ceiling);
        let kept = kept_part(&out);

        assert!(dropped > 0);
        assert!(cut_between_characters(line, kept), "{kept:?}");
        assert!(
            kept.len() > line.len() / 2,
            "kept {} bytes of {}",
            kept.len(),
            line.len()
        );
        assert!(kept.trim_end().ends_with('。'), "{kept:?}");
    }

    #[test]
    fn a_price_in_euros_does_not_take_the_caller_down() {
        // The shape of the bug this guards: a cut whose byte offset lands
        // inside a three byte currency symbol.
        let line = "Price: €22.40 including delivery. Next sentence here.";
        for ceiling in 0..40 {
            let (out, _) = truncate_to_tokens(line, ceiling);
            assert!(
                cut_between_characters(line, kept_part(&out)),
                "ceiling {ceiling}: {out:?}"
            );
        }
        // And enough of it survives to be worth having. The ceiling has to
        // clear what the marker costs before anything of the page fits beside
        // it, and the marker is twenty tokens read the careful way.
        let (out, _) = truncate_to_tokens(line, 40);
        assert!(kept_part(&out).contains('€'), "{out:?}");
    }

    #[test]
    fn every_cut_survives_awkward_text() {
        let page = awkward_page();
        let trimmer = Trimmer::new(TrimSettings::default());

        for ceiling in [None, Some(0), Some(1), Some(8), Some(40), Some(4000)] {
            let trimmed = trimmer.trim(&page, ceiling);
            // Every character that comes out is one that went in, and nothing
            // arrived as a replacement character from a broken decode.
            assert!(!trimmed.text.contains('\u{fffd}'), "ceiling {ceiling:?}");
            for c in trimmed.text.chars().filter(|c| !c.is_whitespace()) {
                assert!(
                    page.contains(c) || ".[]".contains(c) || c.is_ascii_alphanumeric(),
                    "ceiling {ceiling:?} produced {c:?}"
                );
            }
        }

        // The individual cuts, each on its own. None of them may invent or
        // mangle a character.
        for out in [
            collapse_whitespace(&page),
            drop_data_uris(&page, 4).0,
            drop_link_only_lines(&page).0,
        ] {
            assert!(!out.contains('\u{fffd}'));
            for line in out.lines().filter(|line| !line.is_empty()) {
                assert!(page.contains(line), "{line:?} is not from the page");
            }
        }
    }

    #[test]
    fn boilerplate_removal_survives_awkward_text() {
        let bodies = ["钢制垫圈", "黄铜垫圈", "尼龙垫圈", "青铜垫圈"];
        let pages: Vec<String> = bodies
            .iter()
            .map(|body| {
                format!(
                    "مرحبا بكم\nCafé ☕\n£19.99\n\n{body}，同日发货。\n\nالشحن اليوم\nNaïve 👍🏽\n"
                )
            })
            .collect();

        let mut trimmer = Trimmer::new(TrimSettings::default());
        trimmer.learn(&pages);
        for (index, page) in pages.iter().enumerate() {
            let trimmed = trimmer.trim(page, Some(45));
            assert!(!trimmed.text.contains('\u{fffd}'), "page {index}");
            assert!(
                !trimmed.text.is_empty(),
                "page {index} came back with nothing"
            );
        }
        // The furniture around the differing line is gone, in any script.
        let trimmed = trimmer.trim(&pages[0], None).text;
        assert!(trimmed.contains("钢制垫圈"), "{trimmed}");
        assert!(!trimmed.contains("Café"), "{trimmed}");
    }

    #[test]
    fn a_data_uri_next_to_a_multibyte_character_is_cut_cleanly() {
        let long = "z".repeat(400);
        let text = format!("Prix: €22,40 <img src=\"data:image/png;base64,{long}\">。终わり");
        let (out, count, _) = drop_data_uris(&text, 256);
        assert_eq!(count, 1);
        assert!(out.contains("€22,40"));
        assert!(out.contains("。终わり"));
    }

    #[test]
    fn a_link_line_in_another_script_is_still_a_link_line() {
        let text = "[الشحن](https://example.com/shipping)\nCafé ☕ costs £3.\n";
        let (out, lines, _) = drop_link_only_lines(text);
        assert_eq!(lines, 1);
        assert!(out.contains("Café ☕ costs £3."));
    }

    /// A crawl where a menu and a footer wrap a paragraph of real content.
    fn crawl(bodies: &[&str]) -> Vec<String> {
        bodies
            .iter()
            .map(|body| {
                format!(
                    "Example Shop\nHome\nProducts\nAbout\nContact\n\n{body}\n\n\
                     Terms of sale\nPrivacy notice\nCopyright Example Shop\n"
                )
            })
            .collect()
    }

    #[test]
    fn a_menu_and_a_footer_go_and_the_content_stays() {
        let pages = crawl(&[
            "The blue widget is made of steel and weighs two kilos.",
            "The red widget is made of brass and weighs one kilo.",
            "The green widget is made of wood and weighs nothing much.",
            "The black widget is made of stone and weighs rather a lot.",
        ]);
        let mut trimmer = Trimmer::new(TrimSettings::default());
        trimmer.learn(&pages);
        assert!(trimmer.knows_boilerplate());

        let trimmed = trimmer.trim(&pages[0], None);
        assert!(
            trimmed.text.contains("blue widget"),
            "content went: {:?}",
            trimmed.text
        );
        for furniture in [
            "Example Shop",
            "Home",
            "Products",
            "About",
            "Contact",
            "Terms of sale",
            "Privacy notice",
        ] {
            assert!(
                !trimmed.text.contains(furniture),
                "{furniture} survived: {:?}",
                trimmed.text
            );
        }
        assert!(trimmed
            .reasons
            .iter()
            .any(|r| matches!(r, TrimReason::Boilerplate { .. })));
    }

    #[test]
    fn boilerplate_removal_is_most_of_the_reduction_on_a_crawl() {
        let pages = crawl(&[
            "The blue widget is made of steel.",
            "The red widget is made of brass.",
            "The green widget is made of wood.",
            "The black widget is made of stone.",
            "The white widget is made of bone.",
        ]);
        let mut trimmer = Trimmer::new(TrimSettings::default());
        trimmer.learn(&pages);

        let before: usize = pages.iter().map(|p| p.len()).sum();
        let after: usize = pages
            .iter()
            .map(|page| trimmer.trim(page, None).text.len())
            .sum();
        let ratio = after as f64 / before as f64;
        assert!(ratio < 0.35, "kept {ratio} of the crawl");
    }

    #[test]
    fn a_line_on_two_pages_of_five_is_not_furniture() {
        let shared = "Free delivery on orders over fifty pounds.";
        let mut pages = crawl(&[
            "The blue widget is made of steel.",
            "The red widget is made of brass.",
            "The green widget is made of wood.",
            "The black widget is made of stone.",
            "The white widget is made of bone.",
        ]);
        pages[0] = format!("{}\n{shared}\nMore about it.\n", pages[0]);
        pages[1] = format!("{}\n{shared}\nMore about it.\n", pages[1]);

        let mut trimmer = Trimmer::new(TrimSettings::default());
        trimmer.learn(&pages);
        let trimmed = trimmer.trim(&pages[0], None);
        assert!(trimmed.text.contains(shared), "{:?}", trimmed.text);
    }

    #[test]
    fn a_crawl_too_short_to_judge_loses_nothing() {
        let pages = crawl(&["One widget.", "Another widget."]);
        let mut trimmer = Trimmer::new(TrimSettings::default());
        trimmer.learn(&pages);
        assert!(!trimmer.knows_boilerplate());
        assert!(trimmer.trim(&pages[0], None).text.contains("Home"));
    }

    #[test]
    fn the_first_line_of_content_after_a_menu_survives() {
        // Every page opens its content with the same word, which makes the
        // adjacency test the only thing protecting the line.
        let pages = crawl(&[
            "Widget: blue, steel, two kilos.",
            "Widget: red, brass, one kilo.",
            "Widget: green, wood, light.",
            "Widget: black, stone, heavy.",
        ]);
        let mut trimmer = Trimmer::new(TrimSettings::default());
        trimmer.learn(&pages);
        let trimmed = trimmer.trim(&pages[2], None);
        assert!(trimmed.text.contains("green"), "{:?}", trimmed.text);
    }

    #[test]
    fn blank_runs_and_repeated_spaces_collapse() {
        let text = "One.\n\n\n\nTwo    words   here.\n   \n\nThree.   \n";
        let out = collapse_whitespace(text);
        assert_eq!(out, "One.\n\nTwo words here.\n\nThree.");
        assert!(!out.contains("\n\n\n"));
    }

    #[test]
    fn a_long_data_uri_goes_and_a_short_one_stays() {
        let long = "x".repeat(400);
        let text = format!(
            "<img src=\"data:image/png;base64,{long}\"> and <img src=\"data:image/gif;base64,AAAA\">"
        );
        let (out, count, bytes) = drop_data_uris(&text, 256);
        assert_eq!(count, 1);
        assert!(bytes > 400);
        assert!(out.contains("data:[dropped"));
        assert!(out.contains("data:image/gif;base64,AAAA"));
        assert!(out.len() < text.len() / 2);
    }

    #[test]
    fn a_line_that_is_only_a_link_goes_and_a_sentence_with_one_stays() {
        let text = "https://example.com/a\n\
                   - [Contact](https://example.com/contact)\n\
                   See https://example.com/a for the details.\n\
                   Ordinary prose.\n";
        let (out, lines, bytes) = drop_link_only_lines(text);
        assert_eq!(lines, 2);
        assert!(bytes > 0);
        assert!(out.contains("See https://example.com/a for the details."));
        assert!(out.contains("Ordinary prose."));
        assert!(!out.contains("- [Contact]"));
    }

    #[test]
    fn link_lines_survive_when_the_links_are_what_was_asked_for() {
        let trimmer = Trimmer::for_need(&Need::Links);
        let trimmed = trimmer.trim("https://example.com/a\nhttps://example.com/b\n", None);
        assert!(trimmed.text.contains("https://example.com/b"));
        assert!(!trimmer.settings().drop_link_lines);
    }

    #[test]
    fn truncation_lands_on_a_sentence_end() {
        let text = "One sentence here. Two sentences here. Three sentences here. \
                    Four sentences here. Five sentences here. Six sentences here.";
        let (out, dropped) = truncate_to_tokens(text, 60);
        assert!(dropped > 0);
        let body = out
            .split("\n...[truncated")
            .next()
            .expect("the kept text")
            .to_string();
        assert!(body.ends_with('.'), "cut mid sentence: {body:?}");
        assert!(text.starts_with(&body), "kept text is not a prefix");
        assert!(out.contains(&format!("truncated {dropped} tokens")));
    }

    #[test]
    fn what_comes_back_fits_the_ceiling_it_was_given() {
        let text = "One sentence here. Two sentences here. Three sentences here. \
                    Four sentences here. Five sentences here. Six sentences here.";
        for ceiling in [8usize, 12, 20, 30, 40] {
            let (out, _) = truncate_to_tokens(text, ceiling);
            let cost = max_tokens(&out);
            assert!(cost <= ceiling, "{cost} tokens for a ceiling of {ceiling}");
        }
    }

    #[test]
    fn text_that_already_fits_is_returned_untouched() {
        let text = "Short enough.";
        let (out, dropped) = truncate_to_tokens(text, 100);
        assert_eq!(out, text);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn a_need_that_opted_out_loses_nothing() {
        let trimmer = Trimmer::for_need(&Need::Raw);
        let text = "One.\n\n\n\nTwo.\nhttps://example.com/a\n";
        let trimmed = trimmer.trim(text, None);
        assert_eq!(trimmed.text, text);
        assert!(trimmed.reasons.is_empty());
    }

    #[test]
    fn the_reasons_say_where_the_bytes_went() {
        let long = "y".repeat(400);
        let text = format!(
            "One.\n\n\n\nhttps://example.com/a\n<img src=\"data:image/png;base64,{long}\">\n"
        );
        let trimmer = Trimmer::new(TrimSettings::default());
        let trimmed = trimmer.trim(&text, None);
        assert!(trimmed.removed_bytes() > 400);
        assert!(trimmed
            .reasons
            .iter()
            .any(|r| matches!(r, TrimReason::DataUri { .. })));
        assert!(trimmed
            .reasons
            .iter()
            .any(|r| matches!(r, TrimReason::LinkLines { .. })));
        assert_eq!(TrimReason::Truncated { tokens: 4 }.bytes(), 0);
    }
}
