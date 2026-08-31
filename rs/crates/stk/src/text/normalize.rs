//! Text normalisation shared by every matcher in the project.
//!
//! The same track arrives spelled several different ways: a streaming export
//! gives `DECO*27` and `弱虫モンブラン (sasakure.UK Remix)`, the ripped file on
//! disk is `(06) [DECO-27] 弱虫モンブラン.flac`, and a Soulseek peer offers
//! `06. DECO_27 - Yowamushi Montblanc.flac`. Normalising away featured-artist
//! credits, bracketed suffixes, punctuation and case is what makes those three
//! comparable at all.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Python's `str.isalnum()`: categories L, Nd, Nl and No.
///
/// Neither `\w` nor `\p{Word}` can be used here. Rust's `\w` additionally
/// matches `\p{M}` -- 10,549 combining marks that Python folds to a space
/// instead -- and every comparison in the project runs through [`normalize`], so
/// the difference would shift matching scores globally rather than fail visibly.
/// `scripts/word_chars.py` generates the fixture holding this to Python's answer.
const ALNUM: &str = r"\p{L}\p{Nd}\p{Nl}\p{No}";

/// Python's `\s`, which matches U+001C to U+001F. Rust's does not: those four
/// are not `White_Space`.
const SPACE: &str = r"\s\x1C-\x1F";

/// The fallback [`sanitize_folder_name`] uses when nothing survives.
pub const UNKNOWN_ARTIST: &str = "Unknown Artist";

/// Whether `c` is whitespace to Python. See [`SPACE`].
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

static FEATURED_CREDIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\(feat\.[^)]*\)|feat\.[^,(\[]*").unwrap());
static BRACKETED_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\(\[][^\)\]]*[\)\]]").unwrap());

/// Punctuation, which is `[^\w\s]|_` in the Python.
///
/// Underscore is a word character to a regex engine but stands in for a stripped
/// symbol in filenames, so `DECO_27` has to fold to the same form as `DECO*27`.
/// Leaving it out of [`ALNUM`] rather than alternating on it gives the same set:
/// `_` is in none of L, Nd, Nl, No or whitespace.
static PUNCTUATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("[^{ALNUM}{SPACE}]")).unwrap());
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("[{SPACE}]+")).unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("[{ALNUM}_]+")).unwrap());
static LEADING_TRACK_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^[{SPACE}]*[\(\[]?\d{{1,3}}(?:[.\-]\d{{1,3}})?[\)\]]?[{SPACE}]*[-.\u{{2013}}\u{{2014}}_]?[{SPACE}]*"
    ))
    .unwrap()
});
static INVALID_FILENAME_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("[<>:\"/|?*\\x00-\\x1F]").unwrap());

fn separator() -> String {
    format!(r"[{SPACE}]*[-\u{{2013}}\u{{2014}}_:.|]*[{SPACE}]*")
}

fn nfkc(text: &str) -> String {
    text.nfkc().collect()
}

/// Reduce a name to the lowercase, punctuation-free form used for comparison.
#[must_use]
pub fn normalize(text: &str) -> String {
    let folded = nfkc(text);
    let folded = FEATURED_CREDIT.replace_all(&folded, " ");
    let folded = BRACKETED_SUFFIX.replace_all(&folded, " ");
    let folded = PUNCTUATION.replace_all(&folded, " ");
    WHITESPACE
        .replace_all(&folded, " ")
        .trim_matches(is_space)
        .to_lowercase()
}

/// Lowercase `text` with punctuation turned to spaces, keeping every word.
///
/// Unlike [`normalize`], this preserves bracketed content and featured credits.
/// Those are noise when comparing two titles but are exactly the signal when
/// scanning a path for warnings: "Roki (Live).flac" says what it is only inside
/// the brackets.
#[must_use]
pub fn flatten(text: &str) -> String {
    let folded = nfkc(text);
    let folded = PUNCTUATION.replace_all(&folded, " ");
    WHITESPACE
        .replace_all(&folded, " ")
        .trim_matches(is_space)
        .to_lowercase()
}

/// Drop a leading track number such as `01 -`, `(06)` or `15.`.
#[must_use]
pub fn strip_track_number(name: &str) -> String {
    let stripped = LEADING_TRACK_NUMBER.replacen(name, 1, "");
    let trimmed = stripped.trim_matches(is_space);
    if trimmed.is_empty() {
        name.trim_matches(is_space).to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Remove a leading artist name from a filename stem, if present.
///
/// Matching on the artist's tokens rather than on the literal folder name lets a
/// `DECO_27` folder strip the prefix from `DECO*27 - Roki`: the two spellings
/// differ only in punctuation, and a literal-length slice would cut the title in
/// the wrong place whenever the two spellings differ in length.
#[must_use]
pub fn strip_artist_prefix(stem: &str, artist: &str) -> String {
    let name = strip_track_number(&nfkc(stem));
    let folded = nfkc(artist).replace('_', " ").to_lowercase();
    let tokens: Vec<&str> = WORD.find_iter(&folded).map(|m| m.as_str()).collect();

    // Shorter token runs are tried after the full name so that a "ryo (supercell)"
    // folder strips both "ryo (supercell) - Melt" and "ryo - Melt".
    for count in (1..=tokens.len()).rev() {
        let Some(end) = prefix_end(&name, &tokens[..count]) else {
            continue;
        };
        if end > 0 && end < name.len() {
            return name[end..].trim_matches(is_space).to_owned();
        }
    }
    name
}

/// Where the artist prefix ends in `name`, as a byte offset, if it is there.
///
/// The Python compares `match.end()` against `len(name)` in characters; both
/// sides here are bytes, and the two agree on whether the match spans the whole
/// string, which is all the comparison is asking.
fn prefix_end(name: &str, tokens: &[&str]) -> Option<usize> {
    // `[\W_]` in the Python: anything outside L, Nd, Nl and No.
    let joined = tokens
        .iter()
        .map(|token| fancy_regex::escape(token).into_owned())
        .collect::<Vec<_>>()
        .join(&format!("[^{ALNUM}]*"));
    let separator = separator();
    // A trailing bracket belongs to the artist ("ryo (supercell)"), not the
    // title. The negative lookahead is the one thing here the `regex` crate
    // cannot express, which is why this pattern goes through fancy-regex.
    let pattern = format!(r"(?i)^{separator}{joined}[\)\]\}}]*(?![{ALNUM}_]){separator}");

    prefix_pattern(&pattern)
        .find(name)
        .ok()
        .flatten()
        .map(|m| m.end())
}

/// Compiled prefix patterns, which repeat for every file in an artist's folder.
fn prefix_pattern(pattern: &str) -> Arc<fancy_regex::Regex> {
    static CACHE: LazyLock<Mutex<HashMap<String, Arc<fancy_regex::Regex>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    // A cache cannot be left inconsistent by a panic elsewhere, so poisoning is
    // recovered from rather than propagated.
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache
        .entry(pattern.to_owned())
        .or_insert_with(|| Arc::new(fancy_regex::Regex::new(pattern).expect("a valid pattern")))
        .clone()
}

/// Make a name safe to use as a directory component on any supported platform.
#[must_use]
pub fn sanitize_folder_name(name: &str, fallback: &str) -> String {
    let cleaned = INVALID_FILENAME_CHARS.replace_all(name, "_");
    let cleaned = cleaned.trim_matches(is_space).trim_end_matches('.');
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned.to_owned()
    }
}
