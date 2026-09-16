//! Counting tokens without a tokenizer, and splitting a budget across pages.
//!
//! There are two numbers here and they answer different questions. A real
//! tokenizer would answer both, and costs a model file and a dependency, which
//! is a poor trade for a crate whose job is to send fewer bytes.
//!
//! [`approx_tokens`] is what a page probably costs. It is what the savings in
//! [`ThriftReport`](crate::thrift::ThriftReport) are measured in, and it is
//! allowed to be wrong in either direction.
//!
//! [`max_tokens`] is what a page cannot cost more than. Every ceiling a caller
//! sets is enforced in this number, so a budget picked to fit a context window
//! holds even on input the estimate reads badly. It runs high on purpose:
//! three to four and a half times a byte-pair count on English prose, and two
//! to three times on Chinese. A caller who set a budget would rather get less
//! text back than lose the request at the model.
//!
//! Both were measured against cl100k_base and o200k_base over 376 samples:
//! English prose, markup, JSON, URLs, code, Chinese, Japanese, Korean, Arabic,
//! Hebrew, Hindi, Thai, Greek, Cyrillic, emoji, and random hex, base64, digits,
//! mixed case and whitespace built to beat them. [`max_tokens`] read low on
//! none of them. There is no proof behind that, only the corpus.
//!
//! The old estimate was characters over four with a correction for
//! punctuation. It read a Chinese page at a quarter of its cost, because
//! `char::is_alphanumeric` counts an ideograph as one word character and a
//! tokenizer spends one to two tokens on it. A crawler that met a Chinese page
//! and trusted the number overflowed by four times.

/// Characters per token in running text, for the estimate.
const CHARS_PER_TOKEN: usize = 4;

/// Punctuation characters per token, for the estimate. Tokenizers split most
/// of them off on their own, but runs like `",` or `...` come back together.
/// Two thirds of a character fits the measured rate better than a half.
const PUNCT_TOKENS_NUMERATOR: usize = 2;
const PUNCT_TOKENS_DENOMINATOR: usize = 3;

/// Tokens a plain word costs per character, in the ceiling. A word the
/// tokenizer has never seen breaks into pieces of one to two characters. Three
/// quarters covered every run of letters in the corpus, including 2,000 random
/// ones; two thirds did not.
const WORD_CEILING_NUMERATOR: usize = 3;
const WORD_CEILING_DENOMINATOR: usize = 4;

/// Roughly what a string costs a model, in tokens.
///
/// An estimate, and wrong in both directions. On the corpus it lands within
/// about a tenth of a byte-pair count on English prose, markdown, JSON and
/// code. It reads high on Chinese and Thai, up to twice, and low on a page of
/// nothing but hex hashes, down to a third. Use [`max_tokens`] for anything
/// that must not overflow.
///
/// ```
/// use spider_cloud_agent::thrift::approx_tokens;
///
/// assert_eq!(approx_tokens(""), 0);
/// assert!(approx_tokens("Hello world.") < approx_tokens("Hello world. Again."));
/// ```
pub fn approx_tokens(text: &str) -> usize {
    let mut word_chars = 0usize;
    let mut punct_chars = 0usize;
    let mut wide = 0usize;

    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            word_chars += 1;
        } else if c.is_ascii() {
            if !c.is_whitespace() {
                punct_chars += 1;
            }
        } else {
            // A tokenizer works on UTF-8 bytes, and outside ASCII it spends
            // close to a token on each of them. One short of the byte count is
            // the rate that fits Han, kana, Hangul, Cyrillic, Greek, Arabic,
            // Hebrew, Devanagari, Thai and emoji at once.
            wide += c.len_utf8().saturating_sub(1);
        }
    }

    word_chars.div_ceil(CHARS_PER_TOKEN)
        + (punct_chars * PUNCT_TOKENS_NUMERATOR).div_ceil(PUNCT_TOKENS_DENOMINATOR)
        + wide
}

/// What a string cannot cost a model more than, in tokens.
///
/// No sample in the corpus tokenized above this, and it is the number every
/// [`TokenBudget`] is enforced in. It is not a proof, and it cannot be one
/// without a tokenizer: `the` and `a3f9c2e1` hold the same class of character
/// and cost nine times apart. The claim it does make is narrower and testable.
/// Over 376 samples across ten scripts, plus random hex, base64, digits, mixed
/// case and whitespace written to beat it, nothing read low.
///
/// The price is that it reads high, three to four times a byte-pair count on
/// English prose. Budget accordingly, or use [`approx_tokens`] and accept that
/// a page can come back over the line.
///
/// ```
/// use spider_cloud_agent::thrift::{approx_tokens, max_tokens};
///
/// assert_eq!(max_tokens(""), 0);
/// // The two never cross.
/// assert!(approx_tokens("蜘蛛云是一个网页抓取服务。") <= max_tokens("蜘蛛云是一个网页抓取服务。"));
/// ```
pub fn max_tokens(text: &str) -> usize {
    let mut ceiling = Ceiling::new();
    for c in text.chars() {
        ceiling.push(c);
    }

    ceiling.tokens()
}

/// The ceiling for a string being read one character at a time.
///
/// Split out because the truncation walk needs to know, at every character,
/// what it has spent so far. After each [`Ceiling::push`], [`Ceiling::tokens`]
/// is the ceiling for the prefix read so far, so a walk can stop the moment it
/// goes over and be sure of everything behind it.
///
/// Whether a character starts a new run or continues the one open is the whole
/// of the logic. Costing a run rather than a character is what separates
/// `the` from `a3f9c2e1`, which hold the same characters and cost nine times
/// apart.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Ceiling {
    /// Tokens for the runs already closed.
    settled: usize,
    /// The run still open.
    run: Run,
}

/// The run of characters a [`Ceiling`] is in the middle of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Run {
    #[default]
    None,
    /// ASCII letters and digits.
    Word {
        chars: usize,
        has_letter: bool,
        has_digit: bool,
        /// A capital after the first character. `HTML` and `iPhone` break into
        /// pieces the way a hash does; `Hello` does not.
        inner_capital: bool,
    },
    /// Whitespace, which the tokenizer folds into the word after it.
    Space {
        chars: usize,
        first: char,
        /// One run of the same character merges. A mix of tabs, newlines and
        /// spaces does not.
        uniform: bool,
    },
}

impl Ceiling {
    pub(crate) fn new() -> Ceiling {
        Ceiling::default()
    }

    /// Read one more character.
    pub(crate) fn push(&mut self, c: char) {
        match (&mut self.run, classify(c)) {
            (
                Run::Word {
                    chars,
                    has_letter,
                    has_digit,
                    inner_capital,
                },
                Class::Word,
            ) => {
                *inner_capital |= c.is_ascii_uppercase();
                *has_letter |= c.is_ascii_alphabetic();
                *has_digit |= c.is_ascii_digit();
                *chars += 1;
            }
            (
                Run::Space {
                    chars,
                    first,
                    uniform,
                },
                Class::Space,
            ) => {
                *uniform &= c == *first;
                *chars += 1;
            }
            (_, class) => {
                // The run just ended because a character of another kind
                // arrived, which is exactly the case where a trailing space
                // folds into what follows it.
                self.settled += self.run.tokens(true);
                self.run = match class {
                    Class::Word => Run::Word {
                        chars: 1,
                        has_letter: c.is_ascii_alphabetic(),
                        has_digit: c.is_ascii_digit(),
                        inner_capital: false,
                    },
                    Class::Space => Run::Space {
                        chars: 1,
                        first: c,
                        uniform: true,
                    },
                    Class::Other => {
                        // ASCII punctuation is a token each. Outside ASCII the
                        // tokenizer falls back to bytes, so a character cannot
                        // cost more than it has.
                        self.settled += c.len_utf8();
                        Run::None
                    }
                };
            }
        }
    }

    /// The ceiling for everything read so far.
    pub(crate) fn tokens(&self) -> usize {
        // The open run is costed as though the text ended here, which is the
        // dearer reading: whitespace only gives a character back to the word
        // that follows it, and there is no word yet.
        self.settled + self.run.tokens(false)
    }
}

impl Run {
    /// What this run costs. `folded` says a non-space character follows it.
    fn tokens(&self, folded: bool) -> usize {
        match *self {
            Run::None => 0,
            Run::Word {
                chars,
                has_letter,
                has_digit,
                inner_capital,
            } => {
                let plain = has_letter && !has_digit && !inner_capital;
                if plain {
                    (chars * WORD_CEILING_NUMERATOR)
                        .div_ceil(WORD_CEILING_DENOMINATOR)
                        .max(1)
                } else {
                    chars
                }
            }
            Run::Space {
                chars,
                first: _,
                uniform,
            } => {
                // The last space of a run joins the word after it and costs
                // nothing of its own.
                let paid = chars.saturating_sub(usize::from(folded));
                if uniform {
                    paid.div_ceil(2)
                } else {
                    paid
                }
            }
        }
    }
}

/// The three kinds of character the ceiling counts separately.
enum Class {
    Word,
    Space,
    Other,
}

fn classify(c: char) -> Class {
    if c.is_ascii_alphanumeric() {
        Class::Word
    } else if c.is_ascii_whitespace() {
        Class::Space
    } else {
        // Whitespace outside ASCII, the ideographic space above all, is
        // costed by its bytes like any other wide character. Folded into a
        // space run it would cost half a token, which is less than the
        // estimate charges for it, and the ceiling must never read below
        // the estimate.
        Class::Other
    }
}

/// How many tokens a run of text may cost.
///
/// `total` is the ceiling for a whole operation, and `per_page` is the ceiling
/// for any one page in it. Either can stand alone. With both set, a page gets
/// the smaller of its share of the total and its own cap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenBudget {
    /// The most any single page may cost.
    pub per_page: Option<usize>,
    /// The most the whole operation may cost.
    pub total: Option<usize>,
}

impl TokenBudget {
    /// No ceiling at all.
    pub const fn unlimited() -> TokenBudget {
        TokenBudget {
            per_page: None,
            total: None,
        }
    }

    /// A ceiling on the whole operation.
    pub const fn total(tokens: usize) -> TokenBudget {
        TokenBudget {
            per_page: None,
            total: Some(tokens),
        }
    }

    /// A ceiling on each page.
    pub const fn per_page(tokens: usize) -> TokenBudget {
        TokenBudget {
            per_page: Some(tokens),
            total: None,
        }
    }

    /// Add a per-page ceiling to this budget.
    pub const fn and_per_page(mut self, tokens: usize) -> TokenBudget {
        self.per_page = Some(tokens);
        self
    }

    /// Whether anything is capped.
    pub const fn is_unlimited(&self) -> bool {
        self.per_page.is_none() && self.total.is_none()
    }

    /// Split the total across pages in proportion to what each one costs.
    ///
    /// `costs` is what each page costs now, in tokens. The result is what each
    /// one may keep, in the same order. A set that already fits inside the
    /// total is left alone, so nothing is cut for the sake of it. Past that,
    /// every page is cut by the same proportion: a page twice the size of
    /// another keeps twice as much of the budget, not twice as much of itself.
    ///
    /// With no total set, each page gets its own cost or the per-page ceiling,
    /// whichever is smaller. The returned shares always sum to at most the
    /// total.
    pub fn split(&self, costs: &[usize]) -> Vec<usize> {
        let capped: Vec<usize> = costs
            .iter()
            .map(|cost| match self.per_page {
                Some(cap) => (*cost).min(cap),
                None => *cost,
            })
            .collect();

        let Some(total) = self.total else {
            return capped;
        };

        // Summed and multiplied in a wider type. A cost is a page's token
        // count and a total is a caller's number, and the product of the two
        // does not have to fit in a usize. A wrapped sum reads as already
        // fitting and hands back every page uncut.
        let weight: u128 = capped.iter().map(|cost| *cost as u128).sum();
        if weight <= total as u128 {
            return capped;
        }

        // Hand every page its share of the total by weight, then give the
        // remainder to the pages that lost the most to rounding, so the shares
        // add up to the total exactly rather than a few short.
        let mut shares: Vec<usize> = Vec::with_capacity(capped.len());
        let mut remainders: Vec<(u128, usize)> = Vec::with_capacity(capped.len());
        for (index, cost) in capped.iter().enumerate() {
            let exact = *cost as u128 * total as u128;
            // The quotient is at most the total, so it fits.
            shares.push(usize::try_from(exact / weight).unwrap_or(total));
            remainders.push((exact % weight, index));
        }

        let mut spare = total.saturating_sub(shares.iter().sum::<usize>());
        remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, index) in remainders {
            if spare == 0 {
                break;
            }
            if let Some(share) = shares.get_mut(index) {
                *share += 1;
                spare -= 1;
            }
        }

        shares
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

    #[test]
    fn nothing_costs_nothing() {
        assert_eq!(approx_tokens(""), 0);
        assert_eq!(max_tokens(""), 0);
    }

    #[test]
    fn the_estimate_grows_with_the_text() {
        let short = "The quick brown fox.";
        let long = "The quick brown fox jumps over the lazy dog, twice, and then rests.";
        assert!(approx_tokens(long) > approx_tokens(short));
        // Doubling the text roughly doubles the count.
        let doubled = format!("{long} {long}");
        let ratio = approx_tokens(&doubled) as f64 / approx_tokens(long) as f64;
        assert!((1.9..=2.1).contains(&ratio), "ratio was {ratio}");
    }

    #[test]
    fn the_estimate_is_close_to_four_characters_a_token_on_prose() {
        let prose = "Spider fetches a page and hands back what you asked for, which is \
                     usually a great deal less than the page itself weighed.";
        let estimate = approx_tokens(prose) as f64;
        let chars = prose.chars().count() as f64;
        let per_token = chars / estimate;
        assert!(
            (3.0..=5.0).contains(&per_token),
            "{per_token} characters a token"
        );
    }

    #[test]
    fn punctuation_costs_more_than_the_same_run_of_letters() {
        let letters = approx_tokens("abcdefgh");
        let punctuation = approx_tokens("{}[]<>()");
        assert!(
            punctuation > letters,
            "{punctuation} should be more than {letters}"
        );
    }

    #[test]
    fn the_ceiling_never_reads_below_the_estimate() {
        for text in [
            "",
            " ",
            "\n\n\n",
            "a",
            "The quick brown fox jumps over the lazy dog.",
            "蜘蛛云是一个网页抓取服务。它把网页转换成结构化的数据。",
            "ウェブページを取得して、必要な部分だけを返すサービスです。",
            "a3f9c2e1b7d4508f6a2c9e3b1d7f4085c6a29e3b1d7f4085",
            "<div class=\"grid\" data-testid=\"x\"><img src=\"data:image/png;base64,iVBOR\"></div>",
            "{\"price\":12.5,\"title\":\"Steel widget\",\"tags\":[\"a\",\"b\"]}",
            "🚀🔥✨ shipped 👨‍👩‍👧‍👦 today",
            "Сервис извлекает веб-страницы и возвращает только то, что вы просили.",
        ] {
            assert!(
                approx_tokens(text) <= max_tokens(text),
                "the ceiling read below the estimate on {text:?}"
            );
        }
    }

    // The numbers in the two tests below were measured against cl100k_base and
    // o200k_base. They are the worst readings in each class, so a formula
    // change that loses the safety margin fails here rather than in a caller's
    // context window.

    #[test]
    fn the_ceiling_covers_what_a_byte_pair_tokenizer_charges() {
        // text, the higher of the cl100k_base and o200k_base counts.
        let measured: &[(&str, usize)] = &[
            ("蜘蛛云是一个网页抓取服务。它可以把任何网页转换成结构化的数据，供大型语言模型使用。", 74),
            ("ウェブページを取得して、必要な部分だけを返すサービスです。", 44),
            ("웹 페이지를 가져와서 필요한 부분만 돌려주는 서비스입니다.", 44),
            ("漢漢漢漢漢漢漢漢漢漢", 20),
            ("Сервис извлекает веб-страницы и возвращает только то, что вы просили.", 40),
            ("यह सेवा वेब पेज लाती है", 39),
            ("🚀🔥✨🎉💡📦🧪🌍🐍⚡", 30),
            ("👨‍👩‍👧‍👦", 14),
            ("a3f9c2e1b7d4508f6a2c9e3b1d7f4085c6a29e3b1d7f4085c6a29e3b1d7f4085", 54),
            ("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ", 20),
            ("HTML XML JSON API URL HTTP TLS SDK CLI", 21),
            ("The quick brown fox jumps over the lazy dog.", 10),
        ];

        for (text, charged) in measured {
            assert!(
                max_tokens(text) >= *charged,
                "the ceiling read {} for text a tokenizer charges {charged} for: {text:?}",
                max_tokens(text)
            );
        }
    }

    #[test]
    fn a_chinese_page_no_longer_reads_at_a_quarter_of_its_cost() {
        // The bug this module was rewritten for. Every character here is
        // alphanumeric to Rust, so the old formula divided them by four and
        // read 25 tokens for a page a tokenizer charges 150 for.
        let page = "蜘蛛云是一个网页抓取服务。它可以把任何网页转换成结构化的数据，\
                    供大型语言模型使用。我们提供多种返回格式，包括纯文本、Markdown、\
                    清理过的 HTML 以及页面链接列表。";
        // cl100k_base charges 150 for this page and o200k_base charges 101.
        assert!(max_tokens(page) >= 150, "{}", max_tokens(page));
        // And the estimate is now in the same order of magnitude rather than a
        // quarter of it.
        assert!(approx_tokens(page) >= 100, "{}", approx_tokens(page));
    }

    #[test]
    fn whitespace_is_free_between_words_and_costs_something_on_its_own() {
        // A single space joins the word after it, so prose pays nothing for
        // it. A page of nothing but whitespace still costs tokens.
        assert_eq!(max_tokens("a b"), max_tokens("a") + max_tokens("b"));
        assert!(max_tokens("   \n\n\t  \r\n   ") >= 3);
    }

    #[test]
    fn whitespace_outside_ascii_does_not_pull_the_ceiling_under_the_estimate() {
        // The ideographic space is three bytes, which the estimate charges two
        // tokens for. Folded into a space run it cost half a token.
        for text in [
            "\u{3000}",
            "\u{3000}\u{3000}\u{3000}",
            "\u{a0}",
            "\u{2003}\u{2009}",
            "漢\u{3000}漢",
            "a\u{3000}b",
            "\u{feff}\u{3000}",
        ] {
            assert!(
                approx_tokens(text) <= max_tokens(text),
                "the ceiling read {} under the estimate {} on {text:?}",
                max_tokens(text),
                approx_tokens(text)
            );
        }
    }

    #[test]
    fn a_run_that_looks_like_a_hash_costs_more_than_a_run_that_looks_like_a_word() {
        assert!(max_tokens("a3f9c2e1") > max_tokens("elephant"));
        assert!(max_tokens("HTML") > max_tokens("html"));
    }

    #[test]
    fn reading_one_character_at_a_time_costs_what_reading_the_whole_prefix_costs() {
        // The truncation walk stops on this number, so it has to be the
        // ceiling for the text behind it and not an approximation of one.
        let text = "Steel widget, 40mm.  不锈钢部件 🚀 https://example.com/p/89?a=1";
        let mut ceiling = Ceiling::new();
        let mut taken = String::new();
        for c in text.chars() {
            ceiling.push(c);
            taken.push(c);
            assert_eq!(
                ceiling.tokens(),
                max_tokens(&taken),
                "walking {taken:?} disagreed with reading it whole"
            );
        }
    }

    #[test]
    fn a_budget_with_no_total_only_applies_its_page_cap() {
        let budget = TokenBudget::per_page(100);
        assert_eq!(budget.split(&[50, 200, 100]), vec![50, 100, 100]);
        assert!(!budget.is_unlimited());
        assert!(TokenBudget::unlimited().is_unlimited());
    }

    #[test]
    fn a_total_splits_in_proportion_to_what_each_page_costs() {
        let budget = TokenBudget::total(1000);
        let shares = budget.split(&[1000, 2000, 3000]);
        assert_eq!(shares.iter().sum::<usize>(), 1000);
        // A page that is three times the size gets three times the room.
        assert_eq!(shares, vec![167, 333, 500]);
    }

    #[test]
    fn pages_that_already_fit_are_left_alone() {
        let budget = TokenBudget::total(5000);
        assert_eq!(budget.split(&[100, 200]), vec![100, 200]);
    }

    #[test]
    fn a_page_cap_and_a_total_both_apply() {
        let budget = TokenBudget::total(600).and_per_page(200);
        let shares = budget.split(&[1000, 1000, 1000]);
        assert_eq!(shares, vec![200, 200, 200]);
        assert_eq!(shares.iter().sum::<usize>(), 600);
    }

    #[test]
    fn a_split_holds_its_total_on_costs_too_large_to_multiply() {
        // Two costs that overflow when added, let alone when multiplied by
        // the total. A wrapped sum reads as already fitting and hands back
        // shares of nine quintillion against a total of three.
        let budget = TokenBudget::total(3);
        let shares = budget.split(&[usize::MAX / 2, usize::MAX / 2, 4]);
        assert!(
            shares.iter().sum::<usize>() <= 3,
            "shares {shares:?} against a total of 3"
        );

        let budget = TokenBudget::total(usize::MAX / 4);
        let shares = budget.split(&[usize::MAX / 2, usize::MAX / 2]);
        assert!(shares.iter().all(|share| *share <= usize::MAX / 4));
        assert!(shares.iter().sum::<usize>() <= usize::MAX / 4);
    }

    #[test]
    fn splitting_nothing_hands_back_nothing() {
        assert_eq!(TokenBudget::total(100).split(&[]), Vec::<usize>::new());
        assert_eq!(TokenBudget::total(100).split(&[0, 0]), vec![0, 0]);
    }
}
