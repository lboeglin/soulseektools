//! A peer's folder as one review decision; each track still keeps its own candidate row.

use std::collections::{HashMap, HashSet};

use crate::config::RankingSettings;
use crate::errors::Result;
use crate::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use crate::review::store::{ReviewStore, ReviewTrack};
use crate::soulseek::ranking;
use crate::text::normalize::{strip_artist_prefix, strip_track_number, track_number};
use crate::wantlist::TRACK_NUMBER_COLUMNS;

#[derive(Debug, Clone, PartialEq)]
pub struct AlbumTrack {
    pub track_id: i64,
    pub rank: i64,
    pub query: String,
    pub scored: ScoredCandidate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlbumOffer {
    pub username: String,
    pub folder: String,
    pub tracks: Vec<AlbumTrack>,
    pub sacrificed: i64,
}

impl AlbumOffer {
    #[must_use]
    pub fn name(&self) -> &str {
        folder_name(&self.folder)
    }

    #[must_use]
    pub fn covered(&self) -> usize {
        self.tracks.len()
    }

    #[must_use]
    pub fn picks(&self) -> Vec<(i64, i64)> {
        self.tracks
            .iter()
            .map(|track| (track.track_id, track.rank))
            .collect()
    }

    #[must_use]
    pub fn top_picks(&self) -> usize {
        self.tracks.iter().filter(|track| track.rank == 1).count()
    }

    #[must_use]
    pub fn quality(&self) -> String {
        containers(self.tracks.iter().map(|track| &track.scored.candidate))
    }
}

/// Pass tracks awaiting review only, or approving an album would overrule a choice made by hand.
#[must_use]
pub fn offers(tracks: &[ReviewTrack], minimum: usize) -> Vec<AlbumOffer> {
    let mut grouped: HashMap<(&str, &str), AlbumOffer> = HashMap::new();

    for entry in tracks {
        // The maximum, not rank 1: `prefer_one_uploader` may have promoted a
        // lower-scoring candidate to the front.
        let best = entry
            .candidates
            .iter()
            .filter(|scored| !scored.rejected())
            .map(|scored| scored.score)
            .max()
            .unwrap_or(0);
        let query = entry.track.query();
        let mut seen: HashSet<(&str, &str)> = HashSet::new();

        for (index, scored) in entry.candidates.iter().enumerate() {
            if scored.rejected() {
                continue;
            }
            let Some(folder) = folder_of(&scored.candidate) else {
                continue;
            };

            let key = (scored.candidate.username.as_str(), folder);
            // Rows arrive in rank order, so the first file per folder wins.
            if !seen.insert(key) {
                continue;
            }

            let offer = grouped.entry(key).or_insert_with(|| AlbumOffer {
                username: scored.candidate.username.clone(),
                folder: folder.to_owned(),
                tracks: Vec::new(),
                sacrificed: 0,
            });
            offer.sacrificed += best - scored.score;
            offer.tracks.push(AlbumTrack {
                track_id: entry.id,
                rank: i64::try_from(index + 1).unwrap_or(i64::MAX),
                query: query.clone(),
                scored: scored.clone(),
            });
        }
    }

    let mut offers: Vec<AlbumOffer> = grouped
        .into_values()
        .filter(|offer| offer.covered() >= minimum)
        .collect();
    // Deterministic, so an index into the listing stays meaningful.
    offers.sort_by(|left, right| {
        right
            .covered()
            .cmp(&left.covered())
            .then_with(|| left.sacrificed.cmp(&right.sacrificed))
            .then_with(|| left.username.cmp(&right.username))
            .then_with(|| left.folder.cmp(&right.folder))
    });
    offers
}

/// Taken whole: all of its audio and nothing else.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderOffer {
    pub username: String,
    pub folder: String,

    pub files: Vec<ScoredCandidate>,

    /// Non-audio files (`.cue`, `.log`, scans): counted, never queued.
    pub extras: usize,

    pub score: i64,

    pub flags: Vec<String>,

    /// A warning, not a filter, since the folder arrives whole.
    pub below_quality: usize,
}

impl FolderOffer {
    #[must_use]
    pub fn name(&self) -> &str {
        folder_name(&self.folder)
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.files.len()
    }

    #[must_use]
    pub fn quality(&self) -> String {
        containers(self.files.iter().map(|file| &file.candidate))
    }

    /// slskd reports availability per response, so any file answers for the folder.
    #[must_use]
    pub fn free_slots(&self) -> bool {
        self.files
            .first()
            .is_some_and(|file| file.candidate.free_upload_slots)
    }

    #[must_use]
    pub fn queue_length(&self) -> u32 {
        self.files
            .first()
            .map_or(0, |file| file.candidate.queue_length)
    }

    #[must_use]
    pub fn rejected(&self) -> bool {
        self.score < 0
    }
}

/// `wanted` is the normalised album query, so a term the caller asked for isn't held against it.
#[must_use]
pub fn folders(
    candidates: &[Candidate],
    options: &RankingSettings,
    wanted: &str,
    minimum: usize,
) -> Vec<FolderOffer> {
    let mut grouped: HashMap<(String, String), Vec<Candidate>> = HashMap::new();
    for candidate in candidates {
        let Some(folder) = folder_of(candidate) else {
            continue;
        };
        grouped
            .entry((candidate.username.clone(), folder.to_owned()))
            .or_default()
            .push(candidate.clone());
    }

    let mut offers: Vec<FolderOffer> = grouped
        .into_iter()
        .filter_map(|((username, folder), files)| {
            let audio: Vec<&Candidate> = files
                .iter()
                .filter(|file| ranking::is_audio(file))
                .collect();
            if audio.len() < minimum {
                return None;
            }
            let scored = ranking::score_folder(&files, options, wanted)?;
            let extras = files.len() - audio.len();
            let offered = audio
                .into_iter()
                .map(|candidate| {
                    let mut flags = scored.flags.clone();
                    if !ranking::meets_quality_floor(candidate, options) {
                        flags.push(ranking::BELOW_QUALITY_FLAG.to_owned());
                    }
                    ScoredCandidate {
                        candidate: candidate.clone(),
                        // The folder's score: the folder is what was judged.
                        score: scored.score,
                        title_ratio: 0.0,
                        flags,
                    }
                })
                .collect();
            Some(FolderOffer {
                username,
                folder,
                files: offered,
                extras,
                score: scored.score,
                flags: scored.flags,
                below_quality: scored.below_quality,
            })
        })
        .collect();

    offers.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.count().cmp(&left.count()))
            .then_with(|| left.username.cmp(&right.username))
            .then_with(|| left.folder.cmp(&right.folder))
    });
    offers
}

/// The title is a guess from the filename, flagged so `fetch download` won't overwrite the peer's.
#[must_use]
pub fn album_rows(offer: &FolderOffer, artist: &str, album: &str) -> Vec<(Track, ScoredCandidate)> {
    let mut rows: Vec<(Track, ScoredCandidate)> = Vec::new();
    for file in &offer.files {
        let filename = file.candidate.filename();
        let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
        // "Kikuo - Kikuo Miku 7 - 11 Astral Travel" names artist, album and number.
        let after_album = strip_album_prefix(&strip_artist_prefix(stem, artist), album);
        let title = strip_track_number(&after_album);
        if title.is_empty() {
            continue;
        }

        let track = Track {
            artist: artist.to_owned(),
            title,
            album: album.to_owned(),
            title_guessed: true,
            // Read back by `retag::RowTags::of` through the column names an
            // export is read with, so a download lands with its number on it.
            source_row: track_number(filename)
                .or_else(|| track_number(&after_album))
                .map(|number| vec![(TRACK_NUMBER_COLUMNS[0].to_owned(), number.to_string())])
                .unwrap_or_default(),
            ..Track::default()
        };
        if rows
            .iter()
            .any(|(queued, _)| queued.query() == track.query())
        {
            continue;
        }
        rows.push((track, file.clone()));
    }
    rows
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedFolder {
    pub queued: usize,
    /// Left alone because a transfer is already in flight or finished.
    pub skipped: Vec<String>,
}

/// One candidate per row, so a file that never arrives just fails.
pub fn queue_folder(
    store: &mut ReviewStore,
    rows: &[(Track, ScoredCandidate)],
) -> Result<QueuedFolder> {
    let tracks: Vec<Track> = rows.iter().map(|(track, _)| track.clone()).collect();
    store.add(&tracks)?;

    // `add` returns a count, not ids, so read them back.
    let queued: HashMap<String, (i64, TrackStatus)> = store
        .tracks(&[], None)?
        .into_iter()
        .map(|entry| (entry.track.query(), (entry.id, entry.status)))
        .collect();

    let mut skipped = Vec::new();
    let mut picks: Vec<(i64, i64)> = Vec::new();
    for (track, file) in rows {
        let query = track.query();
        let Some((id, status)) = queued.get(&query) else {
            continue;
        };
        if matches!(status, TrackStatus::Queued | TrackStatus::Downloaded) {
            skipped.push(query);
            continue;
        }
        store.set_candidates(*id, std::slice::from_ref(file))?;
        picks.push((*id, 1));
    }

    store.approve_all(&picks)?;
    Ok(QueuedFolder {
        queued: picks.len(),
        skipped,
    })
}

const TITLE_SEPARATORS: [char; 6] = [' ', '-', '_', '.', '\u{2013}', '\u{2014}'];

/// Only the whole name: "Ghost Rule" is a title on "Ghost Rule EP", not a prefix.
fn strip_album_prefix(name: &str, album: &str) -> String {
    let rest = name
        .get(..album.len())
        .filter(|head| !album.is_empty() && head.eq_ignore_ascii_case(album))
        .and_then(|_| name.get(album.len()..));
    match rest {
        Some(rest) if rest.starts_with(TITLE_SEPARATORS) => {
            let title = rest.trim_start_matches(TITLE_SEPARATORS);
            if title.is_empty() {
                name
            } else {
                title
            }
        }
        _ => name,
    }
    .to_owned()
}

/// Every number the album name holds appears in the folder's path, read as a
/// number so `03` counts as `3`. Name similarity alone rates "Kikuo Miku 7"
/// the album asked for as "Kikuo Miku 3".
#[must_use]
pub fn names_its_numbers(folder: &str, album: &str) -> bool {
    let numbers = |text: &str| -> Vec<u64> {
        text.split(|c: char| !c.is_ascii_digit())
            .filter_map(|digits| digits.parse().ok())
            .collect()
    };
    let present = numbers(folder);
    numbers(album).iter().all(|number| present.contains(number))
}

fn folder_name(folder: &str) -> &str {
    match folder.rfind(['\\', '/']) {
        Some(at) => &folder[at + 1..],
        None => folder,
    }
}

fn containers<'a>(files: impl Iterator<Item = &'a Candidate>) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for candidate in files {
        // `extension` returns the whole filename when there is no dot.
        let extension = match candidate.filename().rfind('.') {
            Some(_) => candidate.extension(),
            None => "?".to_owned(),
        };
        match counts.iter_mut().find(|(name, _)| *name == extension) {
            Some((_, count)) => *count += 1,
            None => counts.push((extension, 1)),
        }
    }
    counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts
        .iter()
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Needs a separator inside: rules out a bare filename, the share root (`@@peer`) and an empty name.
fn folder_of(candidate: &Candidate) -> Option<&str> {
    let folder = candidate.folder();
    folder.contains(['\\', '/']).then_some(folder)
}
