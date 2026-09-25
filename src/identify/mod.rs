//! Identifying a file from its fingerprint through AcoustID, and choosing which
//! release to tag it with. Below the confidence threshold nothing is proposed:
//! a wrongly tagged file is worse than an untagged one.

pub mod acoustid;

use std::fmt::Write as _;
use std::path::Path;

use crate::audio::fingerprint::Fingerprint;
use crate::config::IdentifySettings;
use crate::errors::Result;
use crate::identify::acoustid::{credited, AcoustId, Hit, Recording, Release};
use crate::library::tags::TagField;
use crate::matching::{similarity, NameForms};
use crate::models::{LibraryEntry, TrackTags};
use crate::services::ServiceHttp;
use crate::tagsheet::{Cell, TagEdit};

/// A confident identification, and the release it was resolved against.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub score: f64,
    pub recording_id: String,
    pub release_id: String,
    pub year: Option<u32>,
    /// What the file should say. Genres are carried over untouched, because
    /// `genre apply` owns them.
    pub proposed: TrackTags,
    /// The rule that chose this release over the others.
    pub reason: String,
}

/// What a lookup came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Match(Match),
    /// The service answered, and nothing in the answer was good enough.
    NoMatch(String),
    /// The file could not be fingerprinted, or the service could not be asked.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Identification {
    pub entry: LibraryEntry,
    pub outcome: Outcome,
}

/// Look every entry up, one request each.
///
/// A failed print or request is reported against its file, not fatal.
pub fn identify(
    http: &dyn ServiceHttp,
    client: &AcoustId,
    entries: &[LibraryEntry],
    prints: Vec<Result<Fingerprint>>,
    settings: &IdentifySettings,
) -> Vec<Identification> {
    entries
        .iter()
        .zip(prints)
        .map(|(entry, print)| {
            let outcome = match print.and_then(|print| client.lookup(http, &print)) {
                Ok(hits) => choose(&hits, entry, settings),
                Err(error) => Outcome::Failed(error.to_string()),
            };
            Identification {
                entry: entry.clone(),
                outcome,
            }
        })
        .collect()
}

/// Decide what one file is, from what AcoustID said about it.
///
/// Only the best-scoring track counts, and only above the threshold. Among
/// its recordings and their releases, the first rule that separates two
/// candidates decides, in this order:
///
/// 1. a release whose title matches the file's album tag;
/// 2. a release whose title matches the folder the file is in;
/// 3. a recording credited to the artist the file is filed under -- one print
///    can be linked to a cover as well as the original, and the folder says
///    which was meant;
/// 4. the earliest release, undated ones last;
/// 5. the original over a compilation, when both came out the same day;
/// 6. the release id, so a tie is the same tie on every run.
#[must_use]
pub fn choose(hits: &[Hit], entry: &LibraryEntry, settings: &IdentifySettings) -> Outcome {
    let Some(best) = hits.first() else {
        return Outcome::NoMatch("unknown to AcoustID".to_owned());
    };
    if best.score < settings.min_score {
        return Outcome::NoMatch(format!("no confident match (best {:.2})", best.score));
    }

    let mut candidates: Vec<Candidate<'_>> = best
        .recordings
        .iter()
        .flat_map(|recording| {
            recording.releases.iter().map(move |release| Candidate {
                recording,
                release,
                rank: Rank::of(recording, release, entry, settings.name_match_threshold),
            })
        })
        .collect();
    if candidates.is_empty() {
        return Outcome::NoMatch(
            "the fingerprint is known, but no MusicBrainz release is linked to it".to_owned(),
        );
    }
    candidates.sort_by(|left, right| left.rank.cmp(&right.rank));

    let chosen = &candidates[0];
    let mut reason = chosen.rank.reason(
        candidates.get(1).map(|runner_up| &runner_up.rank),
        &candidates,
    );
    // Flagged, not refused: the file's artist tag may be the wrong one. An
    // untagged file's folder isn't worth contradicting.
    if chosen.rank.artist == Fit::Differs && !entry.tags.artist.is_empty() {
        let _ = write!(reason, "; not credited to {}", entry.tags.artist);
    }

    let (recording, release) = (chosen.recording, chosen.release);
    let artist = credited(&recording.artists);
    let album_artist = credited(&release.artists);
    let proposed = TrackTags {
        album_artist: if album_artist == artist {
            String::new()
        } else {
            album_artist
        },
        artist,
        album: release.title.clone(),
        title: release
            .track
            .as_ref()
            .map_or_else(|| recording.title.clone(), |track| track.title.clone()),
        track_number: release.track.as_ref().and_then(|track| track.position),
        genres: entry.tags.genres.clone(),
    };

    Outcome::Match(Match {
        score: best.score,
        recording_id: recording.id.clone(),
        release_id: release.id.clone(),
        year: release.date.map(|date| date.year),
        proposed,
        reason,
    })
}

struct Candidate<'a> {
    recording: &'a Recording,
    release: &'a Release,
    rank: Rank,
}

/// Whether a name the file carries agrees with one the release carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    Matches,
    Differs,
}

impl Fit {
    fn of(matches: bool) -> Self {
        if matches {
            Self::Matches
        } else {
            Self::Differs
        }
    }
}

/// Where a release stands under each rule, field order being rule order, so
/// the derived ordering puts the best candidate first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    album: Fit,
    folder: Fit,
    artist: Fit,
    date: (u32, u32, u32),
    compilation: bool,
    id: String,
}

/// A release with no date sorts after every dated one.
const UNDATED: (u32, u32, u32) = (u32::MAX, u32::MAX, u32::MAX);

impl Rank {
    fn of(recording: &Recording, release: &Release, entry: &LibraryEntry, threshold: f64) -> Self {
        let release_title = NameForms::of(&release.title);
        let named = |text: &str| similarity(&NameForms::of(text), &release_title) >= threshold;
        let credited_as = |wanted: &NameForms| {
            let joined = NameForms::of(&credited(&recording.artists));
            similarity(wanted, &joined) >= threshold
                || recording
                    .artists
                    .iter()
                    .any(|credit| similarity(wanted, &NameForms::of(&credit.name)) >= threshold)
        };
        Self {
            album: Fit::of(named(&entry.tags.album)),
            folder: Fit::of(named(&folder_name(&entry.path))),
            artist: Fit::of(credited_as(&entry.artist)),
            date: release
                .date
                .map_or(UNDATED, |date| (date.year, date.month, date.day)),
            compilation: release.compilation,
            id: release.id.clone(),
        }
    }

    /// Equal under every rule but the last.
    fn ties_with(&self, other: &Self) -> bool {
        (
            self.album,
            self.folder,
            self.artist,
            self.date,
            self.compilation,
        ) == (
            other.album,
            other.folder,
            other.artist,
            other.date,
            other.compilation,
        )
    }

    /// The first rule on which this rank beats the runner-up, in words.
    fn reason(&self, runner_up: Option<&Self>, all: &[Candidate<'_>]) -> String {
        let Some(other) = runner_up else {
            return "the only release".to_owned();
        };
        let dated = || match self.date {
            UNDATED => "undated".to_owned(),
            (year, 0, _) => year.to_string(),
            (year, month, 0) => format!("{year}-{month:02}"),
            (year, month, day) => format!("{year}-{month:02}-{day:02}"),
        };
        if self.album != other.album {
            "the release matches the file's album tag".to_owned()
        } else if self.folder != other.folder {
            "the release matches the folder name".to_owned()
        } else if self.artist != other.artist {
            "the recording is credited to the file's artist".to_owned()
        } else if self.date != other.date {
            format!("the earliest of {} releases ({})", all.len(), dated())
        } else if self.compilation != other.compilation {
            format!("the original over a compilation, both {}", dated())
        } else {
            format!(
                "one of {} releases dated {}, taken by id",
                all.iter()
                    .filter(|candidate| candidate.rank.ties_with(self))
                    .count(),
                dated()
            )
        }
    }
}

/// The name of the folder a file sits in, which for a download is usually the
/// album the peer had it filed under.
fn folder_name(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The matches as rows for [`crate::tagsheet::plan_changes`], so the plan, the
/// stale check and the write are the ones `tags apply` has.
///
/// A field the release can't fill is left as it is, not blanked.
#[must_use]
pub fn edits(identifications: &[Identification]) -> Vec<TagEdit> {
    identifications
        .iter()
        .filter_map(|identification| match &identification.outcome {
            Outcome::Match(found) => Some(TagEdit {
                path: identification.entry.path.clone(),
                cells: [
                    TagField::Artist,
                    TagField::AlbumArtist,
                    TagField::Album,
                    TagField::Title,
                    TagField::TrackNumber,
                ]
                .into_iter()
                .map(|field| {
                    let value = field.value(&found.proposed);
                    let cell = if value.is_empty() {
                        Cell::Untouched
                    } else {
                        Cell::Set(value)
                    };
                    (field, cell)
                })
                .collect(),
            }),
            Outcome::NoMatch(_) | Outcome::Failed(_) => None,
        })
        .collect()
}
