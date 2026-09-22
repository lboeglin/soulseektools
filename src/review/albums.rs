//! Album-aware review: a peer's folder as one decision (#6, #13).
//!
//! Soulseek's real unit is the folder, not the track. One peer holding a whole
//! album means one queue position, one consistent rip and consistent tags, where
//! twelve tracks from twelve strangers is twelve of each, any of which can stall
//! on its own. The folder was present at every stage and discarded at every one:
//! `ranking` scores the whole remote path -- that is the core of its scoring --
//! but never parses the folder out of it.
//!
//! An offer is derived rather than stored. `candidates` already carries
//! `username` and `remote_path`, so the grouping key costs no column. And
//! because every covered track keeps its own candidate row, an approved album is
//! still one `remote_path` per track: the folder is a unit of *decision*, not of
//! transfer, which is what leaves `fetch::collect`, `fetch::downloaded_path` and
//! the stall retry untouched by it.
//!
//! [`offers`] comes at that from the queue: here are tracks, which folder covers
//! several of them. [`folders`] comes at it from the other end, for `stk get
//! --album` (#13): here is a folder, and nothing is known about what is inside
//! it beyond the filenames, because an album asked for by name brings no
//! tracklist with it. The offer that results is taken whole and becomes rows of
//! exactly the shape above.

use std::collections::{HashMap, HashSet};

use crate::config::RankingSettings;
use crate::errors::Result;
use crate::models::{Candidate, ScoredCandidate, Track, TrackStatus};
use crate::review::store::{ReviewStore, ReviewTrack};
use crate::soulseek::ranking;
use crate::text::normalize::{strip_artist_prefix, track_number};
use crate::wantlist::TRACK_NUMBER_COLUMNS;

/// One queued track a folder can supply, and the rank holding it.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumTrack {
    pub track_id: i64,
    pub rank: i64,
    pub query: String,
    pub scored: ScoredCandidate,
}

/// A peer's folder, and the queued tracks it can supply from it.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumOffer {
    pub username: String,
    pub folder: String,
    pub tracks: Vec<AlbumTrack>,
    /// Total score given up against each track's own best, counted in the same
    /// currency as [`crate::soulseek::ranking::PreferredUploader::sacrificed`].
    pub sacrificed: i64,
}

impl AlbumOffer {
    /// The folder's last component, which is the album rather than the path.
    #[must_use]
    pub fn name(&self) -> &str {
        folder_name(&self.folder)
    }

    #[must_use]
    pub fn covered(&self) -> usize {
        self.tracks.len()
    }

    /// Every track's choice, as [`crate::review::store::ReviewStore::approve_all`]
    /// takes them.
    #[must_use]
    pub fn picks(&self) -> Vec<(i64, i64)> {
        self.tracks
            .iter()
            .map(|track| (track.track_id, track.rank))
            .collect()
    }

    /// How many covered tracks this folder already holds the first choice for.
    ///
    /// The listing's cost signal. Unlike `prefer_one_uploader`, which reorders
    /// candidates unseen and so needs a score window to bound what it may
    /// trade, an offer is shown before it is taken -- but "this folder is
    /// nobody's first choice" is the part a table of names does not otherwise
    /// say.
    #[must_use]
    pub fn top_picks(&self) -> usize {
        self.tracks.iter().filter(|track| track.rank == 1).count()
    }

    /// The containers on offer, commonest first: `2 flac, 1 mp3`.
    #[must_use]
    pub fn quality(&self) -> String {
        containers(self.tracks.iter().map(|track| &track.scored.candidate))
    }
}

/// Every folder one peer could supply at least `minimum` of these tracks from.
///
/// Pass tracks awaiting review only. A track that has already been decided must
/// not be swept back up into an album, or approving one would silently overrule
/// a choice someone made by hand.
#[must_use]
pub fn offers(tracks: &[ReviewTrack], minimum: usize) -> Vec<AlbumOffer> {
    let mut grouped: HashMap<(&str, &str), AlbumOffer> = HashMap::new();

    for entry in tracks {
        // The score the track would have had on its own, which is what taking
        // it from this folder instead is measured against. Taken as a maximum
        // rather than off the head of the list, because `prefer_one_uploader`
        // promotes its peer's candidate to the front before the ranks are
        // written, so rank 1 is not always the highest-scoring row.
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
            // Rows arrive in rank order, so a folder holding two files for one
            // track contributes the one the review listing puts first, and
            // nothing else.
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
    // Most tracks first; then the cheapest; then by name, so one queue always
    // produces the same listing and an index from it stays meaningful.
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

/// A peer's folder as one search turned it up, before anything is queued (#13).
///
/// The sibling of [`AlbumOffer`], approached from the other end. That one
/// starts from tracks already in the queue and asks which folder covers several
/// of them, so every file it names is a file someone asked for. This one starts
/// from a folder and knows nothing about what is inside it beyond the
/// filenames, because `stk get --album` is given an album by name and no
/// tracklist comes with it. The folder is therefore taken whole -- all of its
/// audio, and nothing but its audio.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderOffer {
    pub username: String,
    pub folder: String,

    /// The audio inside it, in the order the peer listed it. Each becomes one
    /// review row's single candidate.
    pub files: Vec<ScoredCandidate>,

    /// What else is inside it: a `.cue`, a `.log`, a scan. Counted so the
    /// listing describes the folder as it is, and never queued -- the album was
    /// asked for, not the uploader's paperwork.
    pub extras: usize,

    pub score: i64,

    /// The reject and penalise terms its paths carry.
    pub flags: Vec<String>,

    /// How many of its files miss `--lossless` or `--min-bitrate`.
    ///
    /// A warning rather than a filter: a folder is taken whole, so hiding the
    /// files that fail would offer a folder that is not the one on the peer's
    /// disk, and the one on the peer's disk is what would arrive.
    pub below_quality: usize,
}

impl FolderOffer {
    /// The folder's last component, which is the album rather than the path.
    #[must_use]
    pub fn name(&self) -> &str {
        folder_name(&self.folder)
    }

    /// How many files would be queued, which is the audio and nothing else.
    #[must_use]
    pub fn count(&self) -> usize {
        self.files.len()
    }

    /// The containers inside it, commonest first: `11 flac, 1 mp3`.
    #[must_use]
    pub fn quality(&self) -> String {
        containers(self.files.iter().map(|file| &file.candidate))
    }

    /// Whether the peer has an upload slot free.
    ///
    /// The peer's, not the file's: slskd spreads one search response's free
    /// slot and queue length across every file it returned, so any file of the
    /// folder answers for all of them.
    #[must_use]
    pub fn free_slots(&self) -> bool {
        self.files
            .first()
            .is_some_and(|file| file.candidate.free_upload_slots)
    }

    /// How many transfers are ahead of this one. See [`Self::free_slots`].
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

/// Every folder a search turned up holding at least `minimum` audio files.
///
/// `wanted` is the flattened album query, as [`crate::soulseek::ranking::rank`]
/// builds it: a term the caller asked for is not evidence against a result.
///
/// Ordered best first, then by the fuller folder, then by peer and path, so one
/// search always produces the same listing and an index read off it keeps
/// naming what it named.
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
                        // The folder's score, because the folder is what was
                        // scored: no file in it was held against a title, and
                        // none of them is taken on its own.
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

/// The review rows one folder becomes: one per audio file, each carrying the
/// title its filename gives and the peer's file as its only candidate.
///
/// The title is the weak part, and cannot be otherwise. A folder is a list of
/// filenames; what one says once the track number and the artist are off it is
/// the only thing there is to call the track. The row records that it is a
/// guess, so `fetch download` does not write it over a title the peer had right.
///
/// Two filenames reducing to the same title would collide on the queue's unique
/// key and the second would silently take the first's row, so the first wins and
/// the rest are dropped.
#[must_use]
pub fn album_rows(offer: &FolderOffer, artist: &str, album: &str) -> Vec<(Track, ScoredCandidate)> {
    let mut rows: Vec<(Track, ScoredCandidate)> = Vec::new();
    for file in &offer.files {
        let filename = file.candidate.filename();
        let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
        let title = strip_artist_prefix(stem, artist);
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

/// What queueing a folder came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedFolder {
    /// Rows now approved, each with the peer's file as its only candidate.
    pub queued: usize,
    /// Queries left alone because a transfer is already in flight or finished.
    pub skipped: Vec<String>,
}

/// Queue a folder's rows, each approved with its single candidate.
///
/// One row per file and one candidate per row, which is what leaves
/// `fetch::collect`, `fetch::downloaded_path` and the stall retry untouched by
/// this: the folder is a unit of decision, not of transfer. With one candidate
/// there is nothing for the retry to promote, so a file that never arrives fails
/// and is `stk get`'s to fetch by name -- the folder was the point, and taking
/// one of its tracks from a stranger instead is a different decision.
///
/// A row already `QUEUED` or `DOWNLOADED` is left alone and named. An album
/// asked for as a whole is not a reason to re-queue a transfer already in flight
/// or to fetch a second copy of a track that is already here.
pub fn queue_folder(
    store: &mut ReviewStore,
    rows: &[(Track, ScoredCandidate)],
) -> Result<QueuedFolder> {
    let tracks: Vec<Track> = rows.iter().map(|(track, _)| track.clone()).collect();
    store.add(&tracks)?;

    // `add` hands back a count rather than ids, and a row may have been in the
    // queue already, so the ids come back off one read of the queue.
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

/// The last component of a remote folder, which is the album rather than the
/// path that leads to it.
fn folder_name(folder: &str) -> &str {
    match folder.rfind(['\\', '/']) {
        Some(at) => &folder[at + 1..],
        None => folder,
    }
}

/// The containers a set of files uses, commonest first: `2 flac, 1 mp3`.
///
/// Enough to see that a folder is mixed without restating the judgement the
/// ranker already made about each file in it.
fn containers<'a>(files: impl Iterator<Item = &'a Candidate>) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for candidate in files {
        // `extension` hands back the whole filename when it holds no dot, which
        // `ScoredCandidate::describe` shows as "?" rather than printing a track
        // name in a column of containers.
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

/// The folder a candidate sits in, or `None` when its path names none.
///
/// The folder has to contain a separator of its own, which rules out all three
/// ways a remote path can fail to name one. `Candidate::split_path` returns the
/// whole path as *both* halves when there is no separator at all, so a bare
/// filename would otherwise group with itself and be offered as an album of one.
/// A file shared at the root of a peer's library yields `@@peer`, which is the
/// share and not a directory within it. A leading separator yields an empty
/// string, which would be offered under a blank name.
fn folder_of(candidate: &Candidate) -> Option<&str> {
    let folder = candidate.folder();
    folder.contains(['\\', '/']).then_some(folder)
}
