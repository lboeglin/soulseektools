//! Scoring search results against the whole remote path, since a live cut is often named only in its folder.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::config::{RankingSettings, UploaderSettings};
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

#[must_use]
pub fn is_audio(candidate: &Candidate) -> bool {
    audio_extensions().contains(candidate.extension().as_str())
}

/// The gap between quality tiers, which every other weight sits inside.
pub const TIER_WEIGHT: i64 = 10_000_000;
const QUALITY_WEIGHT: i64 = 10;
const TITLE_WEIGHT: f64 = 500.0;
const FREE_SLOT_BONUS: i64 = 1000;
const MAX_QUEUE_PENALTY: u32 = 500;
const HIGH_BITRATE: u32 = 256;
const ARTIST_WEIGHT_AGAINST_TITLE: f64 = 0.6;

const REJECTED_SCORE: i64 = -1;

static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\\/]+").unwrap());

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

    // Stable, so ties keep the order the peer returned them in.
    scored.sort_by_key(|scored| std::cmp::Reverse(scored.score));
    if let Some(limit) = limit {
        scored.truncate(limit);
    }
    scored
}

#[must_use]
pub fn score(
    candidate: &Candidate,
    track: &Track,
    artist: &NameForms,
    title: &NameForms,
    wanted: &str,
    options: &RankingSettings,
) -> Option<ScoredCandidate> {
    if !is_audio(candidate) {
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
    // `title_ratio` is in [0, 1], so the product is in [0, 500].
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

/// A term in the wanted name ("Live and Learn") is not held against a result.
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

#[must_use]
pub fn is_lossless(candidate: &Candidate) -> bool {
    LOSSLESS_EXTENSIONS.contains(&format!(".{}", candidate.extension()).as_str())
        || candidate.bit_depth.is_some()
}

#[must_use]
pub fn meets_quality_floor(candidate: &Candidate, options: &RankingSettings) -> bool {
    if is_lossless(candidate) {
        return true;
    }
    if options.require_lossless {
        return false;
    }
    // An unknown bitrate passes: it may be the only copy of a rare track.
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

pub const BELOW_QUALITY_FLAG: &str = "below-quality-limit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderScore {
    pub score: i64,
    pub flags: Vec<String>,
    pub below_quality: usize,
}

/// One karaoke track counts against the whole folder. Quality is the mean, not the sum.
#[must_use]
pub fn score_folder(
    files: &[Candidate],
    options: &RankingSettings,
    wanted: &str,
) -> Option<FolderScore> {
    let audio: Vec<&Candidate> = files.iter().filter(|file| is_audio(file)).collect();
    let first = audio.first()?;

    let paths = audio
        .iter()
        .map(|file| file.remote_path.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let path_text = flatten(&SEPARATORS.replace_all(&paths, " "));

    let rejected = terms_in(&path_text, &options.reject_terms, wanted);
    if !rejected.is_empty() {
        return Some(FolderScore {
            score: REJECTED_SCORE,
            flags: rejected
                .iter()
                .map(|term| format!("reject:{term}"))
                .collect(),
            below_quality: 0,
        });
    }

    let below = audio
        .iter()
        .filter(|file| !meets_quality_floor(file, options))
        .count();
    if below == audio.len() {
        return None;
    }

    let mut total = audio
        .iter()
        .map(|file| quality_score(file, options.prefer_lossless))
        .sum::<i64>()
        / i64::try_from(audio.len()).unwrap_or(1);

    // Availability is per peer: slskd reports it once per response.
    total += if first.free_upload_slots {
        FREE_SLOT_BONUS
    } else {
        0
    };
    total -= i64::from(first.queue_length.min(MAX_QUEUE_PENALTY));

    let flags = terms_in(&path_text, &options.penalise_terms, wanted);
    total -= options.penalty_per_term * i64::try_from(flags.len()).unwrap_or(0);

    Some(FolderScore {
        score: total.max(0),
        flags,
        below_quality: below,
    })
}

fn duration_differs(candidate: &Candidate, track: &Track, tolerance_seconds: f64) -> bool {
    let (Some(offered), Some(wanted)) = (candidate.duration_seconds, track.duration_seconds) else {
        return false;
    };
    if offered == 0.0 || wanted == 0.0 {
        return false;
    }
    (offered - wanted).abs() > tolerance_seconds
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferredUploader {
    pub username: String,
    pub covered: usize,
    pub sacrificed: i64,
}

pub const PREFERRED_FLAG: &str = "preferred-uploader";

/// The window spans only availability and name match. Must run before ranks are stored.
pub fn prefer_one_uploader(
    batch: &mut [Vec<ScoredCandidate>],
    settings: &UploaderSettings,
) -> Option<PreferredUploader> {
    // Counted before reordering, so track order can't change the outcome.
    let mut coverage: HashMap<&str, (usize, i64)> = HashMap::new();
    for list in batch.iter() {
        for (username, sacrifice) in within_window(list, settings) {
            let entry = coverage.entry(username).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += sacrifice;
        }
    }

    let (username, covered, sacrificed) = coverage
        .into_iter()
        .map(|(username, (covered, sacrificed))| (username, covered, sacrificed))
        // Most tracks, then cheapest, then name, for a deterministic pick.
        .max_by(|left, right| {
            left.1
                .cmp(&right.1)
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| right.0.cmp(left.0))
        })?;
    if covered < settings.minimum_coverage {
        return None;
    }
    let username = username.to_owned();

    for list in batch.iter_mut() {
        let Some(position) = best_from(list, &username, settings) else {
            continue;
        };
        let mut promoted = list.remove(position);
        promoted.flags.push(PREFERRED_FLAG.to_owned());
        list.insert(0, promoted);
    }

    Some(PreferredUploader {
        username,
        covered,
        sacrificed,
    })
}

fn within_window<'a>(
    list: &'a [ScoredCandidate],
    settings: &UploaderSettings,
) -> HashMap<&'a str, i64> {
    let mut peers: HashMap<&str, i64> = HashMap::new();
    let Some(best) = list.first().filter(|scored| !scored.rejected()) else {
        return peers;
    };

    for scored in list {
        let sacrifice = best.score - scored.score;
        if scored.rejected() || sacrifice > settings.score_window {
            continue;
        }
        let peer = peers
            .entry(scored.candidate.username.as_str())
            .or_insert(sacrifice);
        *peer = (*peer).min(sacrifice);
    }
    peers
}

fn best_from(
    list: &[ScoredCandidate],
    username: &str,
    settings: &UploaderSettings,
) -> Option<usize> {
    let best = list.first().filter(|scored| !scored.rejected())?.score;
    list.iter().position(|scored| {
        scored.candidate.username == username
            && !scored.rejected()
            && best - scored.score <= settings.score_window
    })
}
