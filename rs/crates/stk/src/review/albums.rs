//! Album-aware review: a peer's folder as one decision (#6).
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

use std::collections::{HashMap, HashSet};

use crate::models::{Candidate, ScoredCandidate};
use crate::review::store::ReviewTrack;

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
        match self.folder.rfind(['\\', '/']) {
            Some(at) => &self.folder[at + 1..],
            None => &self.folder,
        }
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
    ///
    /// Enough to see that a folder is mixed without restating the judgement the
    /// ranker already made about each file in it.
    #[must_use]
    pub fn quality(&self) -> String {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for track in &self.tracks {
            // `extension` hands back the whole filename when it holds no dot,
            // which `ScoredCandidate::describe` shows as "?" rather than
            // printing a track name in a column of containers.
            let candidate = &track.scored.candidate;
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
