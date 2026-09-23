//! Domain types shared between the library, artwork, Soulseek and review layers.

use std::fmt;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::library::scan::LOSSLESS_EXTENSIONS;
use crate::matching::NameForms;

/// The metadata fields this project reads or writes, container-independent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackTags {
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub title: String,
    pub genres: Vec<String>,
    pub track_number: Option<i64>,
}

/// The loudness figures a player applies at playback (#7).
///
/// Held as the tags spell them rather than as numbers: an undo has to put back
/// exactly what was there, and a round trip through `f64` would rewrite a
/// foobar2000 `-7.29 dB` as `-7.290000 dB`. Nothing in this project reads these
/// back to make a decision, so there is nothing to parse them for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayGainTags {
    pub track_gain: Option<String>,
    pub track_peak: Option<String>,
    pub album_gain: Option<String>,
    pub album_peak: Option<String>,

    /// The Opus spelling of the same two gains, against a fixed -23 LUFS.
    /// Absent on every other container, and on an Opus file no scanner has
    /// reached.
    pub r128_track_gain: Option<String>,
    pub r128_album_gain: Option<String>,
}

impl ReplayGainTags {
    /// Whether the file is level-matched at all, which is the track gain.
    ///
    /// A peak without a gain says nothing a player can act on, and an album
    /// gain belongs to a record rather than to a file. An Opus file scanned by
    /// a tool that wrote only `R128_TRACK_GAIN` counts: it is level-matched
    /// wherever it is played, and re-measuring it would overwrite a scan.
    #[must_use]
    pub fn is_scanned(&self) -> bool {
        self.track_gain.is_some() || self.r128_track_gain.is_some()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// An embedded cover image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub data: Vec<u8>,
    pub mime: String,
}

impl Artwork {
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// One audio file on disk, with everything needed to match it against another.
///
/// `artist` and `title` are resolved once at index time: tags take priority,
/// falling back to the directory layout and the filename, since a sizeable part
/// of any Soulseek-sourced library arrives untagged.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryEntry {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: f64,
    pub folder_artist: String,
    pub tags: TrackTags,
    pub artist: NameForms,
    pub title: NameForms,
    pub duration_seconds: Option<f64>,
    pub artwork_bytes: u64,

    /// The loudness tags the file carries, new in `library.db` v6 (#7). Read
    /// from the same header parse as everything else here, so counting the
    /// files nothing has level-matched costs no extra I/O.
    pub replay_gain: ReplayGainTags,

    /// Stream properties, new in `library.db` v3 (#2). They come off the same
    /// header read `read_duration` already performs, so storing them costs no
    /// extra I/O -- and without them "every file under 320 kbps" cannot be asked.
    pub codec: String,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
}

impl LibraryEntry {
    #[must_use]
    pub fn has_artwork(&self) -> bool {
        self.artwork_bytes > 0
    }

    #[must_use]
    pub fn is_level_matched(&self) -> bool {
        self.replay_gain.is_scanned()
    }

    /// Decided by the container, the same rule `stk verify` uses to choose
    /// what to inspect.
    #[must_use]
    pub fn is_lossless(&self) -> bool {
        self.path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                LOSSLESS_EXTENSIONS.contains(&format!(".{}", extension.to_lowercase()).as_str())
            })
    }
}

/// A track wanted from a streaming-service export.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_seconds: Option<f64>,

    /// Whether `title` was read off a peer's filename rather than asked for by
    /// name (#13). `stk get --album` has no tracklist and no choice; everything
    /// else in the queue was typed or exported. A guess is weaker evidence than
    /// a tag the peer wrote, and [`crate::retag::plan`] treats it as such.
    pub title_guessed: bool,

    pub source_row: Vec<(String, String)>,
}

impl Track {
    #[must_use]
    pub fn new(artist: &str, title: &str) -> Self {
        Self {
            artist: artist.to_owned(),
            title: title.to_owned(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn query(&self) -> String {
        format!("{} - {}", self.artist, self.title)
    }

    /// The export's value under the first of `columns` its header carries,
    /// matched the way `wantlist::read_export` matches a header: trimmed and
    /// case-insensitive. Empty when the row has the column but nothing in it.
    #[must_use]
    pub fn source_value(&self, columns: &[&str]) -> Option<&str> {
        columns.iter().find_map(|wanted| {
            self.source_row
                .iter()
                .find(|(header, _)| header.trim().eq_ignore_ascii_case(wanted))
                .map(|(_, value)| value.trim())
                .filter(|value| !value.is_empty())
        })
    }
}

/// One file offered by a Soulseek peer, as the search results describe it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Candidate {
    pub username: String,
    pub remote_path: String,
    pub size: u64,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub is_vbr: bool,
    pub free_upload_slots: bool,
    pub queue_length: u32,
    pub duration_seconds: Option<f64>,
}

impl Candidate {
    #[must_use]
    pub fn filename(&self) -> &str {
        Self::split_path(&self.remote_path).1
    }

    /// The remote directory holding the file, which often says what it really is.
    #[must_use]
    pub fn folder(&self) -> &str {
        Self::split_path(&self.remote_path).0
    }

    /// Split on the last separator, as the Python's `rsplit(sep, 1)` does: with
    /// no separator present both halves are the whole path.
    fn split_path(path: &str) -> (&str, &str) {
        match path.rfind(['\\', '/']) {
            Some(at) => (&path[..at], &path[at + 1..]),
            None => (path, path),
        }
    }

    /// The filename's extension, lowercased.
    ///
    /// A name with no dot in it yields the whole name, which is what Python's
    /// `rpartition` returns and what the ranking's extension check then rejects.
    #[must_use]
    pub fn extension(&self) -> String {
        let filename = self.filename();
        match filename.rfind('.') {
            Some(at) => filename[at + 1..].to_lowercase(),
            None => filename.to_lowercase(),
        }
    }
}

/// A candidate with its ranking, and any warnings raised about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredCandidate {
    pub candidate: Candidate,
    pub score: i64,
    pub title_ratio: f64,
    pub flags: Vec<String>,
}

impl ScoredCandidate {
    #[must_use]
    pub fn rejected(&self) -> bool {
        self.score < 0
    }

    #[must_use]
    pub fn describe(&self) -> String {
        let candidate = &self.candidate;
        let extension = candidate.extension();
        let extension = if extension.is_empty() {
            "?"
        } else {
            &extension
        };

        let mut quality = format!("[{extension}");
        match (
            candidate.bit_depth,
            candidate.sample_rate,
            candidate.bitrate,
        ) {
            (Some(depth), Some(rate), _) if depth != 0 && rate != 0 => {
                let khz = f64::from(rate) / 1000.0;
                let _ = write!(quality, ", {khz:.1}kHz/{depth}bit");
            }
            (_, _, Some(bitrate)) if bitrate != 0 => {
                let vbr = if candidate.is_vbr { "VBR" } else { "" };
                let _ = write!(quality, ", {bitrate}kbps{vbr}");
            }
            _ => {}
        }
        quality.push(']');

        let mut parts = vec![candidate.filename().to_owned(), quality];
        parts.push(format!("user={}", candidate.username));
        parts.push(format!("queue={}", candidate.queue_length));
        parts.push(
            if candidate.free_upload_slots {
                "slot=free"
            } else {
                "slot=busy"
            }
            .to_owned(),
        );
        if !self.flags.is_empty() {
            parts.push(format!("flags={}", self.flags.join(",")));
        }
        parts.join(" ")
    }
}

/// Where a wanted track has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrackStatus {
    Pending,
    NoCandidates,
    AwaitingReview,
    Approved,
    Rejected,
    Queued,
    Downloaded,
    Failed,
}

impl TrackStatus {
    pub const ALL: [Self; 8] = [
        Self::Pending,
        Self::NoCandidates,
        Self::AwaitingReview,
        Self::Approved,
        Self::Rejected,
        Self::Queued,
        Self::Downloaded,
        Self::Failed,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::NoCandidates => "no_candidates",
            Self::AwaitingReview => "awaiting_review",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Queued => "queued",
            Self::Downloaded => "downloaded",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for TrackStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TrackStatus {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == text)
            .ok_or_else(|| format!("unknown track status: {text}"))
    }
}

/// Outcome of the authenticity check.
///
/// `Upsampled` is not a transcode: the audio is intact, but it was stretched
/// into a bigger container than it needs -- a 128 kbps MP3 re-encoded at 320,
/// or a 44.1 kHz/16-bit master padded out to 96 kHz/24-bit -- so the peer's
/// claim about it was false (#16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verdict {
    Ok,
    Suspect,
    Fake,
    Upsampled,
    Error,
}

impl Verdict {
    pub const ALL: [Self; 5] = [
        Self::Ok,
        Self::Suspect,
        Self::Fake,
        Self::Upsampled,
        Self::Error,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Suspect => "SUSPECT",
            Self::Fake => "FAKE",
            Self::Upsampled => "UPSAMPLED",
            Self::Error => "ERROR",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Verdict {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|verdict| verdict.as_str() == text)
            .ok_or_else(|| format!("unknown verdict: {text}"))
    }
}

/// What the spectral check concluded about one file, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyResult {
    pub path: PathBuf,
    pub verdict: Verdict,
    pub detail: String,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub codec: String,
    pub cutoff_hz: Option<i64>,
    pub nyquist_hz: Option<i64>,
    pub steepness_db_per_khz: Option<f64>,
}
