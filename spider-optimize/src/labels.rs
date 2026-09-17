//! Whether a candidate arm returned what the baseline arm did.
//!
//! Blocking a stylesheet or a third party can remove content along with the
//! weight, so a candidate that came back faster and cheaper only counts as a
//! success when its content still matches the baseline's. These are the
//! comparisons that decide it. They run where the rows are labelled, never
//! while a request is being scored, and only their numbers reach a row.

use std::cmp::Ordering;

/// How alike two texts are, from zero to one.
///
/// Text is lowercased and split on whitespace, then compared as sets of five
/// word shingles by Jaccard index. When either side has fewer than five words
/// the sets are of single words instead. Two empty texts are identical, and an
/// empty text against a non-empty one scores zero.
pub fn shingle_jaccard(a: &str, b: &str) -> f32 {
    const WIDTH: usize = 5;

    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let a_words: Vec<&str> = a.split_whitespace().collect();
    let b_words: Vec<&str> = b.split_whitespace().collect();

    if a_words.is_empty() && b_words.is_empty() {
        return 1.0;
    }

    let width = if a_words.len() < WIDTH || b_words.len() < WIDTH {
        1
    } else {
        WIDTH
    };
    let a_set = shingles(&a_words, width);
    let b_set = shingles(&b_words, width);

    let (mut i, mut j, mut shared) = (0, 0, 0usize);
    while let (Some(x), Some(y)) = (a_set.get(i), b_set.get(j)) {
        match x.cmp(y) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }

    let union = a_set.len() + b_set.len() - shared;
    if union == 0 {
        return 1.0;
    }
    shared as f32 / union as f32
}

/// The distinct runs of `width` consecutive words, sorted.
fn shingles<'s, 'w>(words: &'s [&'w str], width: usize) -> Vec<&'s [&'w str]> {
    let mut out: Vec<&[&str]> = words.windows(width.max(1)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// How close two byte counts are, from zero to one: the smaller over the
/// larger.
///
/// Symmetric on purpose. Which arm came back smaller is in each arm's own
/// `bytes` column, and this is only the size of the gap. Two empty bodies are
/// identical, and an empty body against a non-empty one scores zero.
pub fn byte_ratio(a: usize, b: usize) -> f32 {
    let (small, large) = if a <= b { (a, b) } else { (b, a) };
    if large == 0 {
        return 1.0;
    }
    (small as f64 / large as f64) as f32
}

/// Whether the candidate arm returned every requested field intact.
///
/// Each requested field must be present and non-empty in `candidate`. Where
/// the baseline has a non-empty value for it, the candidate's value must equal
/// it or contain it, after both are lowercased and their whitespace collapsed
/// to single spaces. Fields the caller did not request are not read. The first
/// value under a name is the one compared.
pub fn fields_ok(
    requested: &[&str],
    baseline: &[(&str, &str)],
    candidate: &[(&str, &str)],
) -> bool {
    requested.iter().all(|name| {
        let Some(got) = lookup(candidate, name).map(normalize) else {
            return false;
        };
        if got.is_empty() {
            return false;
        }
        match lookup(baseline, name).map(normalize) {
            Some(want) if !want.is_empty() => got.contains(&want),
            _ => true,
        }
    })
}

fn lookup<'v>(fields: &[(&str, &'v str)], name: &str) -> Option<&'v str> {
    fields
        .iter()
        .find(|(field, _)| *field == name)
        .map(|(_, value)| *value)
}

fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word.to_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;

    /// An article of distinct sentences with a footer of its own.
    fn article(with_footer: bool) -> String {
        let mut text = String::new();
        for n in 0..60 {
            text.push_str(&format!(
                "Paragraph {n} says the harbour rebuilt pier number {n} after winter storms. "
            ));
        }
        if with_footer {
            text.push_str(
                "Copyright the harbour gazette, all rights reserved, contact us, privacy.",
            );
        }
        text
    }

    #[test]
    fn identical_text_is_one() {
        let text = article(true);
        assert_eq!(shingle_jaccard(&text, &text), 1.0);
        // Case and spacing do not count as differences.
        let shouted = text.to_uppercase().replace(' ', "  \n ");
        assert_eq!(shingle_jaccard(&text, &shouted), 1.0);
        assert_eq!(shingle_jaccard("", "  "), 1.0);
    }

    #[test]
    fn disjoint_text_is_zero() {
        assert_eq!(
            shingle_jaccard(
                &article(false),
                "entirely unrelated words about something else here"
            ),
            0.0
        );
        assert_eq!(shingle_jaccard("one two", "three four"), 0.0);
        assert_eq!(shingle_jaccard("", "words"), 0.0);
    }

    #[test]
    fn a_page_missing_its_footer_stays_above_0_8() {
        let score = shingle_jaccard(&article(true), &article(false));
        assert!(score > 0.8 && score < 1.0, "{score}");

        // Losing most of the article does not.
        let full = article(true);
        let half: String = full
            .split_whitespace()
            .take(120)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(shingle_jaccard(&full, &half) < 0.5);
    }

    #[test]
    fn short_texts_compare_word_sets() {
        assert_eq!(shingle_jaccard("price 20 usd", "usd price 20"), 1.0);
        assert_eq!(shingle_jaccard("a b c d", "a b"), 0.5);
    }

    #[test]
    fn byte_ratio_is_the_smaller_over_the_larger() {
        assert_eq!(byte_ratio(0, 0), 1.0);
        assert_eq!(byte_ratio(100, 0), 0.0);
        assert_eq!(byte_ratio(50, 200), 0.25);
        assert_eq!(byte_ratio(200, 50), 0.25);
        assert!(byte_ratio(usize::MAX, usize::MAX - 1).is_finite());
    }

    #[test]
    fn a_field_dropped_fails() {
        let requested = ["title", "price"];
        let baseline = [("title", "Blue  Kettle"), ("price", "20 USD")];

        assert!(fields_ok(&requested, &baseline, &baseline));
        assert!(fields_ok(
            &requested,
            &baseline,
            &[
                ("title", "blue kettle, 1.7 litres"),
                ("price", " 20   usd ")
            ]
        ));
        assert!(!fields_ok(
            &requested,
            &baseline,
            &[("title", "Blue Kettle")]
        ));
        assert!(!fields_ok(
            &requested,
            &baseline,
            &[("title", "Blue Kettle"), ("price", "   ")]
        ));
        assert!(!fields_ok(
            &requested,
            &baseline,
            &[("title", "Blue Kettle"), ("price", "25 USD")]
        ));
        // A field the baseline also missed only has to be there.
        assert!(fields_ok(&["sku"], &[], &[("sku", "A1")]));
        assert!(fields_ok(&[], &baseline, &[]));
    }
}
