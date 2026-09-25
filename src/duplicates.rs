//! Finding the same track twice: by title (catches two spellings of an artist)
//! and by audio fingerprint (catches untagged files). Resolving keeps one copy
//! per group and quarantines the rest.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::audio::fingerprint::{self, Fingerprint};
use crate::config::OrganiseSettings;
use crate::errors::Result;
use crate::library::changes::ChangeLog;
use crate::library::index::LibraryIndex;
use crate::library::scan::owning_root;
use crate::matching::{blocking_keys, candidate_pairs, score_track, MatchThresholds};
use crate::models::LibraryEntry;
use crate::organise::{self, Applied, Obstacle, PlannedMove};

pub const DEFAULT_AUDIO_THRESHOLD: f64 = 0.9;
pub const DEFAULT_DURATION_TOLERANCE_SECONDS: f64 = 2.0;

/// Two files that look like the same recording, and the evidence for it.
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicatePair {
    pub left: LibraryEntry,
    pub right: LibraryEntry,
    pub score: f64,
    pub evidence: String,
}

impl DuplicatePair {
    /// The copy worth keeping, all else being equal.
    #[must_use]
    pub fn larger(&self) -> &LibraryEntry {
        if self.left.size >= self.right.size {
            &self.left
        } else {
            &self.right
        }
    }

    #[must_use]
    pub fn smaller(&self) -> &LibraryEntry {
        if self.left.size >= self.right.size {
            &self.right
        } else {
            &self.left
        }
    }
}

/// Pairs whose artist and title match, across the whole library.
#[must_use]
pub fn find_by_title(entries: &[LibraryEntry], thresholds: &MatchThresholds) -> Vec<DuplicatePair> {
    let keyed: Vec<(usize, HashSet<String>)> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (index, blocking_keys(&[&entry.artist, &entry.title])))
        .collect();

    let mut pairs: Vec<DuplicatePair> = candidate_pairs(&keyed)
        .into_iter()
        .filter_map(|(left_index, right_index)| {
            let (left, right) = (&entries[left_index], &entries[right_index]);
            if left.path == right.path {
                return None;
            }
            let score = score_track(
                &left.artist,
                &left.title,
                &right.artist,
                &right.title,
                thresholds,
            )?;
            Some(DuplicatePair {
                left: left.clone(),
                right: right.clone(),
                score: score.combined,
                evidence: format!("artist {:.2}, title {:.2}", score.artist, score.title),
            })
        })
        .collect();

    sort_by_score(&mut pairs);
    pairs
}

/// How a fingerprint is obtained, so the caller supplies the cache.
pub type FingerprintSource<'a> = dyn FnMut(&LibraryEntry) -> Option<Fingerprint> + 'a;

/// Pairs whose audio matches, whatever they are named or tagged.
///
/// Duration is only a prefilter; the fingerprint decides.
pub fn find_by_audio(
    entries: &[LibraryEntry],
    threshold: f64,
    duration_tolerance_seconds: f64,
    fingerprint_of: &mut FingerprintSource<'_>,
) -> Vec<DuplicatePair> {
    let comparable: Vec<&LibraryEntry> = entries
        .iter()
        .filter(|entry| entry.duration_seconds.is_some_and(|seconds| seconds != 0.0))
        .collect();
    let candidates = pairs_within_duration(&comparable, duration_tolerance_seconds);
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut needed: Vec<usize> = candidates
        .iter()
        .flat_map(|(left, right)| [*left, *right])
        .collect();
    needed.sort_unstable();
    needed.dedup();

    let mut computed: HashMap<usize, Fingerprint> = HashMap::new();
    for position in needed {
        if let Some(value) = fingerprint_of(comparable[position]) {
            if !value.is_empty() {
                computed.insert(position, value);
            }
        }
    }

    let mut pairs: Vec<DuplicatePair> = candidates
        .into_iter()
        .filter_map(|(left_index, right_index)| {
            let left_print = computed.get(&left_index)?;
            let right_print = computed.get(&right_index)?;
            let score = fingerprint::similarity(left_print, right_print);
            if score < threshold {
                return None;
            }
            Some(DuplicatePair {
                left: comparable[left_index].clone(),
                right: comparable[right_index].clone(),
                score,
                evidence: format!("chromaprint similarity {score:.2}"),
            })
        })
        .collect();

    sort_by_score(&mut pairs);
    pairs
}

/// Best score first, then by path, so equal scores come back in a stable order.
fn sort_by_score(pairs: &mut [DuplicatePair]) {
    pairs.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.larger().path.cmp(&right.larger().path))
            .then_with(|| left.smaller().path.cmp(&right.smaller().path))
    });
}

fn pairs_within_duration(entries: &[&LibraryEntry], tolerance_seconds: f64) -> Vec<(usize, usize)> {
    let duration = |index: usize| entries[index].duration_seconds.unwrap_or(0.0);
    let mut ordered: Vec<usize> = (0..entries.len()).collect();
    ordered.sort_by(|left, right| {
        duration(*left)
            .partial_cmp(&duration(*right))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut pairs = Vec::new();
    for (offset, left) in ordered.iter().enumerate() {
        for right in &ordered[offset + 1..] {
            // The list is sorted, so the first entry out of tolerance ends the run.
            if duration(*right) - duration(*left) > tolerance_seconds {
                break;
            }
            pairs.push(if left < right {
                (*left, *right)
            } else {
                (*right, *left)
            });
        }
    }
    pairs
}

/// A [`FingerprintSource`] that consults the index before computing, and stores
/// what it computed.
pub fn cached_fingerprints(
    index: &mut LibraryIndex,
) -> impl FnMut(&LibraryEntry) -> Option<Fingerprint> + '_ {
    move |entry: &LibraryEntry| {
        if let Ok(Some(cached)) = index.fingerprint(&entry.path, entry.mtime) {
            return Some(cached);
        }
        let value = fingerprint::fingerprint(&entry.path).ok()?;
        let _ = index.store_fingerprint(&entry.path, entry.mtime, &value);
        Some(value)
    }
}

/// Which copy of a duplicated track stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Keep {
    Largest,
    /// A lossless container, then the largest.
    ///
    /// The default, because the MP3 of a FLAC can be the larger file.
    #[default]
    Lossless,
    /// The copy in the library first, as far as its `mtime` says.
    Oldest,
    /// The copy with more of artist, album, title and artwork filled in.
    Tagged,
}

impl Keep {
    pub const ALL: [Self; 4] = [Self::Largest, Self::Lossless, Self::Oldest, Self::Tagged];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Largest => "largest",
            Self::Lossless => "lossless",
            Self::Oldest => "oldest",
            Self::Tagged => "tagged",
        }
    }

    /// What decides between two copies, most important first.
    ///
    /// Every policy ends on the same tie-break, so a group resolves the same
    /// way on every run.
    fn criteria(self) -> &'static [Criterion] {
        use Criterion::{Lossless, Mtime, Path, Size, Tags};
        match self {
            Self::Largest => &[Size, Lossless, Path],
            Self::Lossless => &[Lossless, Size, Path],
            Self::Oldest => &[Mtime, Lossless, Size, Path],
            Self::Tagged => &[Tags, Lossless, Size, Path],
        }
    }
}

impl fmt::Display for Keep {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Keep {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|keep| keep.as_str() == text)
            .ok_or_else(|| {
                format!(
                    "unknown keep policy: {text}; known: {}",
                    Self::ALL.map(Self::as_str).join(", ")
                )
            })
    }
}

#[derive(Debug, Clone, Copy)]
enum Criterion {
    Lossless,
    Size,
    Mtime,
    Tags,
    Path,
}

/// The fields `tagged` counts, out of this many.
const TAG_FIELDS: usize = 4;

fn tag_count(entry: &LibraryEntry) -> usize {
    [&entry.tags.artist, &entry.tags.album, &entry.tags.title]
        .iter()
        .filter(|value| !value.is_empty())
        .count()
        + usize::from(entry.has_artwork())
}

impl Criterion {
    /// `Less` when `left` is the better copy to keep.
    fn compare(self, left: &LibraryEntry, right: &LibraryEntry) -> Ordering {
        match self {
            Self::Lossless => right.is_lossless().cmp(&left.is_lossless()),
            Self::Size => right.size.cmp(&left.size),
            Self::Mtime => left
                .mtime
                .partial_cmp(&right.mtime)
                .unwrap_or(Ordering::Equal),
            Self::Tags => tag_count(right).cmp(&tag_count(left)),
            Self::Path => left.path.cmp(&right.path),
        }
    }

    fn reason(self, loser: &LibraryEntry, keeper: &LibraryEntry) -> String {
        match self {
            Self::Lossless => "lossy, and the kept copy is lossless".to_owned(),
            Self::Size => "smaller".to_owned(),
            Self::Mtime => "newer".to_owned(),
            Self::Tags => format!(
                "{} of {TAG_FIELDS} tag fields filled, against {}",
                tag_count(loser),
                tag_count(keeper)
            ),
            Self::Path => "identical by every measure; the first by path is kept".to_owned(),
        }
    }
}

/// Copies of one track, and the pairs that tied them together.
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateGroup {
    pub entries: Vec<LibraryEntry>,
    pub pairs: Vec<DuplicatePair>,
}

impl DuplicateGroup {
    /// The weakest link holding the group together.
    #[must_use]
    pub fn lowest_score(&self) -> f64 {
        self.pairs
            .iter()
            .map(|pair| pair.score)
            .fold(f64::INFINITY, f64::min)
    }
}

/// Merge pairs into groups of copies of one track.
///
/// Three copies arrive as three pairs; resolving pair by pair could keep and
/// move the same file. A pair both finders report counts once, at its stronger
/// score.
#[must_use]
pub fn group(pairs: impl IntoIterator<Item = DuplicatePair>) -> Vec<DuplicateGroup> {
    let mut strongest: HashMap<(PathBuf, PathBuf), DuplicatePair> = HashMap::new();
    for pair in pairs {
        let key = if pair.left.path <= pair.right.path {
            (pair.left.path.clone(), pair.right.path.clone())
        } else {
            (pair.right.path.clone(), pair.left.path.clone())
        };
        if strongest
            .get(&key)
            .is_none_or(|known| known.score < pair.score)
        {
            strongest.insert(key, pair);
        }
    }

    let mut nodes: HashMap<PathBuf, usize> = HashMap::new();
    let mut entries: Vec<LibraryEntry> = Vec::new();
    let mut parent: Vec<usize> = Vec::new();
    let mut node_of = |entry: &LibraryEntry, parent: &mut Vec<usize>| {
        *nodes.entry(entry.path.clone()).or_insert_with(|| {
            entries.push(entry.clone());
            parent.push(parent.len());
            parent.len() - 1
        })
    };

    let linked: Vec<(usize, DuplicatePair)> = strongest
        .into_values()
        .map(|pair| {
            let left = node_of(&pair.left, &mut parent);
            let right = node_of(&pair.right, &mut parent);
            let (left, right) = (root_of(&mut parent, left), root_of(&mut parent, right));
            parent[right] = left;
            (left, pair)
        })
        .collect();

    let mut groups: HashMap<usize, DuplicateGroup> = HashMap::new();
    for (node, entry) in entries.into_iter().enumerate() {
        let root = root_of(&mut parent, node);
        groups
            .entry(root)
            .or_insert_with(|| DuplicateGroup {
                entries: Vec::new(),
                pairs: Vec::new(),
            })
            .entries
            .push(entry);
    }
    for (node, pair) in linked {
        let root = root_of(&mut parent, node);
        if let Some(group) = groups.get_mut(&root) {
            group.pairs.push(pair);
        }
    }

    let mut groups: Vec<DuplicateGroup> = groups.into_values().collect();
    for group in &mut groups {
        group
            .entries
            .sort_by(|left, right| left.path.cmp(&right.path));
        sort_by_score(&mut group.pairs);
    }
    groups.sort_by(|left, right| left.entries[0].path.cmp(&right.entries[0].path));
    groups
}

fn root_of(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

/// A copy that is not kept, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Loser {
    pub entry: LibraryEntry,
    pub reason: String,
}

/// One group decided: the copy that stays and the ones that go.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub keeper: LibraryEntry,
    pub losers: Vec<Loser>,
}

/// Pick the keeper of a group by `keep`, and say why each other copy lost.
#[must_use]
pub fn choose(group: &DuplicateGroup, keep: Keep) -> Resolution {
    let criteria = keep.criteria();
    let mut ranked: Vec<&LibraryEntry> = group.entries.iter().collect();
    ranked.sort_by(|left, right| {
        criteria
            .iter()
            .map(|criterion| criterion.compare(left, right))
            .find(|ordering| ordering.is_ne())
            .unwrap_or(Ordering::Equal)
    });

    let keeper = ranked[0];
    let losers = ranked[1..]
        .iter()
        .map(|loser| Loser {
            entry: (*loser).clone(),
            reason: criteria
                .iter()
                .find(|criterion| criterion.compare(keeper, loser).is_ne())
                .unwrap_or(&Criterion::Path)
                .reason(loser, keeper),
        })
        .collect();

    Resolution {
        keeper: keeper.clone(),
        losers,
    }
}

/// The command a resolve logs its moves under.
pub const RESOLVE_COMMAND: &str = "dupes resolve";

/// Where one run's losers go: beside the index, a directory per run.
///
/// Outside every library root, so a scan never indexes it.
#[must_use]
pub fn quarantine_directory(index: &LibraryIndex, run_id: i64) -> PathBuf {
    index
        .database
        .parent()
        .unwrap_or(Path::new("."))
        .join("quarantine")
        .join(run_id.to_string())
}

/// Where each loser would go, keeping its path below its root.
///
/// Never overwrites: two roots can hold the same relative path.
#[must_use]
pub fn plan_quarantine(
    resolutions: &[Resolution],
    roots: &[PathBuf],
    directory: &Path,
) -> Vec<PlannedMove> {
    let mut claimed: Vec<PathBuf> = Vec::new();
    resolutions
        .iter()
        .flat_map(|resolution| &resolution.losers)
        .map(|loser| {
            let entry = loser.entry.clone();
            let Some(relative) =
                owning_root(&entry.path, roots).and_then(|root| entry.path.strip_prefix(root).ok())
            else {
                return PlannedMove {
                    destination: entry.path.clone(),
                    entry,
                    blocked: Some(Obstacle::NoRoot),
                };
            };

            let target = directory.join(relative);
            let blocked = if claimed
                .iter()
                .any(|path| organise::same_file(path, &target))
            {
                Some(Obstacle::Claimed(target.clone()))
            } else if target.exists() {
                Some(Obstacle::Occupied(target.clone()))
            } else {
                claimed.push(target.clone());
                None
            };
            PlannedMove {
                entry,
                destination: target,
                blocked,
            }
        })
        .collect()
}

/// Move the losers that are ready into quarantine, logging each under `log`.
///
/// The index forgets each file; `changes undo` re-indexes it.
pub fn quarantine(
    index: &mut LibraryIndex,
    log: &ChangeLog,
    ready: &[&PlannedMove],
    roots: &[PathBuf],
    settings: &OrganiseSettings,
) -> Result<Applied> {
    let mut applied = Applied::default();
    for planned in ready {
        let from = &planned.entry.path;
        // Logged before the move; see `ChangeLog::moved`.
        log.moved(index, from, &planned.destination)?;
        match organise::move_file(from, &planned.destination) {
            Ok(()) => {
                index.forget(from)?;
                applied.moved += 1;
                if settings.remove_empty_directories {
                    if let Some(root) = owning_root(from, roots) {
                        applied.pruned += organise::prune_empty(from, root).len();
                    }
                }
            }
            Err(error) => applied.failures.push((from.clone(), error.to_string())),
        }
    }
    Ok(applied)
}
