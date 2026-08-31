//! Romaji transliteration, used to compare Japanese titles with romanised filenames.
//!
//! A streaming export lists `千本桜` while the file on disk is `Senbonzakura.flac`;
//! no amount of string similarity relates those two without transliterating one of
//! them first. Artist readings are less trustworthy than title readings -- aliases
//! frequently use non-standard readings -- which is why callers weight an artist
//! match lower than a title match.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use regex::Regex;

use crate::text::normalize::{is_space, normalize};

static JAPANESE_CHARACTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[\u{3040}-\u{309f}\u{30a0}-\u{30ff}\u{3400}-\u{4dbf}\u{4e00}-\u{9fff}\u{f900}-\u{faff}\u{ff66}-\u{ff9f}]",
    )
    .unwrap()
});

/// Report whether transliteration could change anything.
#[must_use]
pub fn contains_japanese(text: &str) -> bool {
    JAPANESE_CHARACTER.is_match(text)
}

/// Return the normalised Hepburn romanisation of `text`.
///
/// Text without Japanese characters is returned normalised and otherwise
/// untouched, so this is safe to call on every name unconditionally.
#[must_use]
pub fn to_romaji(text: &str) -> String {
    if let Some(cached) = cache().get(text) {
        return cached.clone();
    }

    let converted = if contains_japanese(text) {
        normalize(&transliterate(text))
    } else {
        normalize(text)
    };

    cache().insert(text.to_owned(), converted.clone());
    converted
}

/// Romanise each whitespace-separated run on its own, discarding the word
/// boundaries the transliterator introduces.
///
/// pykakasi hands back segments which the Python joins with nothing, so the only
/// spaces in its output are the ones the input already had. The `kakasi` crate
/// returns one string with a space at every segment boundary instead --
/// `bakenohana` against `bake no hana` -- which is 57 of the 434 oracle rows.
/// Converting run by run and rejoining reproduces the Python exactly, without
/// discarding the spaces that were genuinely in the name.
fn transliterate(text: &str) -> String {
    text.split(is_space)
        .filter(|run| !run.is_empty())
        .map(|run| {
            kakasi::convert(with_known_readings(run))
                .romaji
                .split(is_space)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Compounds the `kakasi` crate reads character by character, and their reading.
///
/// pykakasi's dictionary carries name readings this crate's does not: it gives
/// 初音 as はつおと rather than はつね, so Hatsune Miku romanises as
/// "hatsuotomiku" and stops matching a file named "Hatsune Miku". 重音 and 音楽
/// resolve correctly, so this is a gap in the dictionary rather than in the
/// segmenting, and there is nothing to reach from outside the crate but the
/// input.
///
/// Every entry here is a row `oracle/romaji.csv` disagreed on. Two are also
/// simply right -- 初音 and 鏡音 are Hatsune and Kagamine. The third is not:
/// pykakasi reads 優曇華 as うどんはな where the word is うどんげ, and the entry
/// reproduces that, because the oracle is the specification and a "corrected"
/// reading would move scores the Python build never moved.
const KNOWN_READINGS: &[(&str, &str)] =
    &[("初音", "はつね"), ("鏡音", "かがみね"), ("優曇", "うどん")];

/// Rewrite any [`KNOWN_READINGS`] compound to its kana before transliteration.
fn with_known_readings(run: &str) -> String {
    if !KNOWN_READINGS
        .iter()
        .any(|(compound, _)| run.contains(compound))
    {
        return run.to_owned();
    }

    let mut patched = run.to_owned();
    for (compound, reading) in KNOWN_READINGS {
        patched = patched.replace(compound, reading);
    }
    patched
}

/// Transliteration is the most expensive step of a scan and repeats heavily
/// across an artist's folder, so results are kept.
fn cache() -> MutexGuard<'static, HashMap<String, String>> {
    static CACHE: LazyLock<Mutex<HashMap<String, String>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    // A cache cannot be left inconsistent by a panic elsewhere, so poisoning is
    // recovered from rather than propagated.
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}
