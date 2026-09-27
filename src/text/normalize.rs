//! Normalisation shared by every matcher, so `DECO*27`, `[DECO-27]` and `DECO_27` compare equal.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Word characters: categories L, Nd, Nl and No. Not `\w`, which also takes
/// combining marks (`\p{M}`); those count as punctuation here.
const ALNUM: &str = r"\p{L}\p{Nd}\p{Nl}\p{No}";

/// Whitespace, including the separator controls U+001C to U+001F that `\s`
/// leaves out.
const SPACE: &str = r"\s\x1C-\x1F";

pub const UNKNOWN_ARTIST: &str = "Unknown Artist";

pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

static FEATURED_CREDIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\(feat\.[^)]*\)|feat\.[^,(\[]*").unwrap());
static BRACKETED_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\(\[][^\)\]]*[\)\]]").unwrap());

/// Punctuation, including underscore, which stands in for a stripped symbol in
/// filenames (`DECO_27` for `DECO*27`).
static PUNCTUATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("[^{ALNUM}{SPACE}]")).unwrap());
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("[{SPACE}]+")).unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("[{ALNUM}_]+")).unwrap());
static LEADING_TRACK_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^[{SPACE}]*[\(\[]?(?<disc>\d{{1,3}})(?:[.\-](?<track>\d{{1,3}}))?[\)\]]?[{SPACE}]*[-.\u{{2013}}\u{{2014}}_]?[{SPACE}]*"
    ))
    .unwrap()
});
/// Includes the backslash, or an artist name could create a nested directory.
static INVALID_FILENAME_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[<>:"/\\|?*\x00-\x1F]"#).unwrap());

fn separator() -> String {
    format!(r"[{SPACE}]*[-\u{{2013}}\u{{2014}}_:.|]*[{SPACE}]*")
}

fn nfkc(text: &str) -> String {
    text.nfkc().collect()
}

/// Lowercased first: `İ` lowercases to `i` plus a combining dot, which must then be stripped.
#[must_use]
pub fn normalize(text: &str) -> String {
    let folded = nfkc(text).to_lowercase();
    let folded = FEATURED_CREDIT.replace_all(&folded, " ");
    let folded = BRACKETED_SUFFIX.replace_all(&folded, " ");
    let folded = PUNCTUATION.replace_all(&folded, " ");
    WHITESPACE
        .replace_all(&folded, " ")
        .trim_matches(is_space)
        .to_owned()
}

/// Unlike [`normalize`], keeps brackets and featured credits, where a path says "(Live)".
#[must_use]
pub fn flatten(text: &str) -> String {
    let folded = nfkc(text);
    let folded = PUNCTUATION.replace_all(&folded, " ");
    WHITESPACE
        .replace_all(&folded, " ")
        .trim_matches(is_space)
        .to_lowercase()
}

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

/// `1-05` is track five of disc one.
#[must_use]
pub fn track_number(name: &str) -> Option<i64> {
    let numbered = LEADING_TRACK_NUMBER.captures(name)?;
    numbered
        .name("track")
        .or_else(|| numbered.name("disc"))
        .and_then(|digits| digits.as_str().parse().ok())
}

/// Matched on the artist's tokens, so a `DECO_27` folder strips `DECO*27 - `.
#[must_use]
pub fn strip_artist_prefix(stem: &str, artist: &str) -> String {
    let name = strip_track_number(&nfkc(stem));
    let folded = nfkc(artist).replace('_', " ").to_lowercase();
    let tokens: Vec<&str> = WORD.find_iter(&folded).map(|m| m.as_str()).collect();

    // Shorter runs after the full name, so "ryo (supercell)" also strips "ryo - Melt".
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

fn prefix_end(name: &str, tokens: &[&str]) -> Option<usize> {
    let joined = tokens
        .iter()
        .map(|token| fancy_regex::escape(token).into_owned())
        .collect::<Vec<_>>()
        .join(&format!("[^{ALNUM}]*"));
    let separator = separator();
    // A trailing bracket belongs to the artist ("ryo (supercell)"). fancy-regex
    // for the negative lookahead.
    let pattern = format!(r"(?i)^{separator}{joined}[\)\]\}}]*(?![{ALNUM}_]){separator}");

    prefix_pattern(&pattern)
        .find(name)
        .ok()
        .flatten()
        .map(|m| m.end())
}

fn prefix_pattern(pattern: &str) -> Arc<fancy_regex::Regex> {
    static CACHE: LazyLock<Mutex<HashMap<String, Arc<fancy_regex::Regex>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache
        .entry(pattern.to_owned())
        .or_insert_with(|| Arc::new(fancy_regex::Regex::new(pattern).expect("a valid pattern")))
        .clone()
}

/// The first well-formed code in `text`, uppercase and without hyphens:
/// country, registrant, year and designation, twelve characters in all.
/// A tag may hold several, separated by `;`, `/` or `,`.
#[must_use]
pub fn isrc(text: &str) -> Option<String> {
    text.split([';', '/', ','])
        .map(|code| {
            code.chars()
                .filter(|c| *c != '-' && !c.is_whitespace())
                .collect::<String>()
                .to_uppercase()
        })
        .find(|code| {
            let bytes = code.as_bytes();
            bytes.len() == 12
                && bytes[..2].iter().all(u8::is_ascii_uppercase)
                && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
                && bytes[5..].iter().all(u8::is_ascii_digit)
        })
}

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
