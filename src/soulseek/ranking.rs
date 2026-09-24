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

/// Whether a result is audio at all, by its container.
///
/// The extension check [`score`] opens with, on its own: a folder holds a
/// `.cue`, a `.log` and a scan alongside the music, and grouping one has to
/// tell them apart before it counts what is in it.
#[must_use]
pub fn is_audio(candidate: &Candidate) -> bool {
    audio_extensions().contains(candidate.extension().as_str())
}

/// The gap between quality tiers, which every other weight sits inside.
///
/// Public so `config` can bound the uploader window against it rather than
/// repeating the number.
pub const TIER_WEIGHT: i64 = 10_000_000;
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

/// Whether a result clears `--lossless` and `--min-bitrate`.
#[must_use]
pub fn meets_quality_floor(candidate: &Candidate, options: &RankingSettings) -> bool {
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

/// The flag a file carries when it is the part of its folder missing the floor.
pub const BELOW_QUALITY_FLAG: &str = "below-quality-limit";

/// What a peer's folder is worth, and what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderScore {
    pub score: i64,
    /// The reject and penalise terms its paths carry.
    pub flags: Vec<String>,
    /// How many of its audio files miss `--lossless` or `--min-bitrate`.
    pub below_quality: usize,
}

/// Score a peer's folder the way [`score`] scores a file, minus the name match.
///
/// An album search knows no tracklist, so there is no title to hold a filename
/// against -- the one half of the scoring that cannot make the move. What does
/// survive is the half that reads the whole remote path, which is the half that
/// catches a live set or a karaoke rip, and here it is read across every file
/// at once: a folder is taken whole or not at all, so one `Karaoke` track
/// inside it is evidence about all of them.
///
/// Quality is the mean of what the files are worth rather than the sum, or a
/// folder padded with bonus tracks would outrank a better rip of the record.
///
/// `None` means nothing in the folder clears the quality floor. A folder where
/// only some files fail keeps its place and carries a warning instead: dropping
/// the files that fail would offer a folder that is not the one on the peer's
/// disk, and the one on the peer's disk is what would arrive.
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

    // Availability belongs to the peer rather than to the file: slskd spreads
    // one response's free slot and queue length across every file in it.
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

/// The peer a batch settled on, and what preferring them cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferredUploader {
    pub username: String,
    /// Tracks whose first candidate is now theirs.
    pub covered: usize,
    /// Total score given up across those tracks.
    pub sacrificed: i64,
}

/// The flag a promoted candidate carries, so the reordering is not silent.
pub const PREFERRED_FLAG: &str = "preferred-uploader";

/// Prefer one peer across a whole batch, where doing so costs almost nothing.
///
/// Soulseek's cost is per queue position, not per file: eight tracks from one
/// peer is one wait, one rip and one tagging convention, where eight peers is
/// eight of each, any of which can stall on its own.
///
/// The trade is only made where that peer's own candidate is already close. The
/// window is narrower than every structural gap in the score -- a quality tier,
/// a content penalty, a sample rate -- and wider than the availability band, so
/// a preference can reorder candidates differing in free slots, queue length
/// and name match, and can never swap a lossless file for a lossy one, take a
/// live version, or accept a worse master.
///
/// Each list is reordered in place, best first. This must run *before* the
/// ranks are written: `set_candidates` numbers them densely from the order it
/// is handed, and reordering a stored list would repoint an existing approval
/// at a different file. The one exception is a stalled peer's group, whose
/// approvals are discarded anyway: see `ReviewStore::reorder_untried` (#17).
pub fn prefer_one_uploader(
    batch: &mut [Vec<ScoredCandidate>],
    settings: &UploaderSettings,
) -> Option<PreferredUploader> {
    // Counted against the un-reordered lists, so the outcome cannot depend on
    // the order the tracks happen to arrive in.
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
        // Most tracks first; then the cheapest; then by name, so one batch
        // always settles on the same peer.
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

/// Each peer offering this track a candidate close enough to count, and what
/// taking their best would give up.
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

/// Where this peer's best in-window candidate sits in the list.
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
