//! Scoring Soulseek search results.
//!
//! The scorer this replaces looked only at a result's *filename*, and that was
//! the root cause of most wrong downloads: a file named exactly right is still,
//! often enough, a live recording, an orchestral cover, or a medley track, and
//! none of that is visible from the filename. Only the folder says so --
//! `@@user\Symphony Concert 2019\03 Roki.flac` is unmistakable and was
//! previously invisible.
//!
//! So the whole remote path is scored. Terms are matched against folder and
//! filename together, and a term is ignored when the wanted track's own name
//! contains it: someone asking for "Live and Learn" is not asking to reject
//! every result containing "live".

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::config::RankingSettings;
use crate::library::scan::LOSSLESS_EXTENSIONS;
use crate::matching::{similarity, NameForms};
use crate::models::{Candidate, ScoredCandidate, Track};
use crate::text::normalize::{flatten, strip_track_number};

const LOSSY_EXTENSIONS: &[&str] = &["mp3", "ogg", "m4a", "aac", "opus", "wma"];

fn audio_extensions() -> &'static HashSet<&'static str> {
    static SET: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
        LOSSLESS_EXTENSIONS
            .iter()
            .map(|extension| extension.trim_start_matches('.'))
            .chain(LOSSY_EXTENSIONS.iter().copied())
            .collect()
    });
    &SET
}

const TIER_WEIGHT: i64 = 10_000_000;
const QUALITY_WEIGHT: i64 = 10;
const TITLE_WEIGHT: f64 = 500.0;
const FREE_SLOT_BONUS: i64 = 1000;
const MAX_QUEUE_PENALTY: u32 = 500;
const HIGH_BITRATE: u32 = 256;
const ARTIST_WEIGHT_AGAINST_TITLE: f64 = 0.6;

const REJECTED_SCORE: i64 = -1;

static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\\/]+").unwrap());

/// Score and order candidates, best first, dropping the unusable ones.
#[must_use]
pub fn rank(
    track: &Track,
    candidates: &[Candidate],
    settings: &RankingSettings,
    limit: Option<usize>,
) -> Vec<ScoredCandidate> {
    let wanted = flatten(&format!("{} {}", track.artist, track.title));
    let artist = NameForms::of(&track.artist);
    let title = NameForms::of(&track.title);

    let mut scored: Vec<ScoredCandidate> = candidates
        .iter()
        .filter_map(|candidate| score(candidate, track, &artist, &title, &wanted, settings))
        .collect();

    // A stable sort keeps candidates the scorer could not separate in the order
    // the peer returned them, which is what the Python's `list.sort` does.
    scored.sort_by_key(|scored| std::cmp::Reverse(scored.score));
    if let Some(limit) = limit {
        scored.truncate(limit);
    }
    scored
}

/// Score one candidate, or return `None` when it is not worth offering.
#[must_use]
pub fn score(
    candidate: &Candidate,
    track: &Track,
    artist: &NameForms,
    title: &NameForms,
    wanted: &str,
    options: &RankingSettings,
) -> Option<ScoredCandidate> {
    if !audio_extensions().contains(candidate.extension().as_str()) {
        return None;
    }

    let filename = candidate.filename();
    let stem = strip_track_number(filename.rsplit_once('.').map_or(filename, |(stem, _)| stem));
    let name = NameForms::of(&stem);
    let title_ratio = similarity(title, &name);
    let artist_ratio = similarity(artist, &name);

    if title_ratio.max(artist_ratio * ARTIST_WEIGHT_AGAINST_TITLE) < options.min_title_ratio {
        return None;
    }

    if !meets_quality_floor(candidate, options) {
        return None;
    }

    // Flattened, not normalised: a bracketed "(Live)" is the whole point here.
    let path_text = flatten(&SEPARATORS.replace_all(&candidate.remote_path, " "));
    let rejected = terms_in(&path_text, &options.reject_terms, wanted);
    if !rejected.is_empty() {
        return Some(ScoredCandidate {
            candidate: candidate.clone(),
            score: REJECTED_SCORE,
            title_ratio,
            flags: rejected
                .iter()
                .map(|term| format!("reject:{term}"))
                .collect(),
        });
    }

    let mut total = quality_score(candidate, options.prefer_lossless);
    total += if candidate.free_upload_slots {
        FREE_SLOT_BONUS
    } else {
        0
    };
    total -= i64::from(candidate.queue_length.min(MAX_QUEUE_PENALTY));
    // `title_ratio` is in [0, 1], so the product is in [0, 500]; the cast is
    // Python's `int()`, truncating toward zero.
    #[allow(clippy::cast_possible_truncation)]
    {
        total += (title_ratio * TITLE_WEIGHT) as i64;
    }

    let mut flags = terms_in(&path_text, &options.penalise_terms, wanted);
    total -= options.penalty_per_term * i64::try_from(flags.len()).unwrap_or(0);

    if duration_differs(candidate, track, options.duration_tolerance_seconds) {
        total -= options.penalty_per_term;
        flags.push("duration".to_owned());
    }

    Some(ScoredCandidate {
        candidate: candidate.clone(),
        score: total.max(0),
        title_ratio,
        flags,
    })
}

/// Terms appearing in the remote path but not in the track's own name.
///
/// A term the caller actually asked for is not evidence against a result: a
/// wantlist entry for "Live and Learn" must not reject every path saying "live".
#[must_use]
pub fn terms_in(path_text: &str, terms: &[String], wanted: &str) -> Vec<String> {
    terms
        .iter()
        .filter_map(|term| {
            let normalised = flatten(term);
            let keep = !normalised.is_empty()
                && path_text.contains(&normalised)
                && !wanted.contains(&normalised);
            keep.then_some(term.clone())
        })
        .collect()
}

/// Whether a result is lossless, by container or by carrying a bit depth.
#[must_use]
pub fn is_lossless(candidate: &Candidate) -> bool {
    LOSSLESS_EXTENSIONS.contains(&format!(".{}", candidate.extension()).as_str())
        || candidate.bit_depth.is_some()
}

fn meets_quality_floor(candidate: &Candidate, options: &RankingSettings) -> bool {
    if is_lossless(candidate) {
        return true;
    }
    if options.require_lossless {
        return false;
    }
    // A result with no bitrate reported has not failed the floor -- nothing is
    // known about it, and dropping it would hide the only copy of a rare track.
    match (options.min_bitrate, candidate.bitrate) {
        (Some(floor), Some(bitrate)) if floor != 0 && bitrate != 0 => bitrate >= floor,
        _ => true,
    }
}

fn quality_score(candidate: &Candidate, prefer_lossless: bool) -> i64 {
    let (tier, quality) = if is_lossless(candidate) {
        (
            3_i64,
            i64::from(candidate.sample_rate.unwrap_or(44100))
                * i64::from(candidate.bit_depth.unwrap_or(16)),
        )
    } else if let Some(bitrate) = candidate.bitrate.filter(|rate| *rate != 0) {
        (
            if bitrate >= HIGH_BITRATE { 2 } else { 1 },
            i64::from(bitrate),
        )
    } else {
        (0, 0)
    };

    let tier = if prefer_lossless {
        tier * TIER_WEIGHT
    } else {
        0
    };
    tier + quality * QUALITY_WEIGHT
}

/// Whether the offered file is a different length from the track wanted.
///
/// Catches an edit, an extended mix or an entirely different song sharing a
/// title, which no amount of name matching can distinguish.
fn duration_differs(candidate: &Candidate, track: &Track, tolerance_seconds: f64) -> bool {
    let (Some(offered), Some(wanted)) = (candidate.duration_seconds, track.duration_seconds) else {
        return false;
    };
    if offered == 0.0 || wanted == 0.0 {
        return false;
    }
    (offered - wanted).abs() > tolerance_seconds
}
