//! What the library looked like at one moment, so maintenance can be measured.
//!
//! `stk library stats` answers "what is in there now", which says nothing about
//! whether the tagging, the artwork and the fake-lossless hunt are gaining on
//! the problem or losing to it (#3). A snapshot per scan turns the same numbers
//! into a series.
//!
//! Counting is a pure function over indexed entries and verdict counts, so what
//! a figure means is tested directly rather than through a database.

use std::collections::BTreeMap;

use crate::library::scan::LOSSLESS_EXTENSIONS;
use crate::models::{LibraryEntry, Verdict};

/// One measurement of the library.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LibrarySnapshot {
    pub taken_at: f64,
    pub total: usize,
    pub tagged: usize,
    pub with_artwork: usize,
    pub with_genre: usize,
    pub lossless: usize,
    pub verified_clean: usize,
    pub flagged: usize,
}

/// The counted part of a snapshot, which is what decides whether it is new.
///
/// `taken_at` is deliberately outside it: two scans an hour apart that found
/// exactly the same library are one measurement, not two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub tagged: usize,
    pub with_artwork: usize,
    pub with_genre: usize,
    pub lossless: usize,
    pub verified_clean: usize,
    pub flagged: usize,
}

impl LibrarySnapshot {
    #[must_use]
    pub fn counts(&self) -> Counts {
        Counts {
            total: self.total,
            tagged: self.tagged,
            with_artwork: self.with_artwork,
            with_genre: self.with_genre,
            lossless: self.lossless,
            verified_clean: self.verified_clean,
            flagged: self.flagged,
        }
    }
}

/// Count what an indexed library holds, at `taken_at`.
///
/// `lossless` is decided by the container, the same rule `stk verify` uses to
/// choose what to analyse, so the two figures describe one set of files.
/// `flagged` is everything `verify` doubted -- FAKE and SUSPECT together --
/// because a library owner chasing them down treats them the same way.
#[must_use]
pub fn measure(
    entries: &[LibraryEntry],
    verdicts: &BTreeMap<Verdict, usize>,
    taken_at: f64,
) -> LibrarySnapshot {
    let count = |verdict: Verdict| verdicts.get(&verdict).copied().unwrap_or(0);
    LibrarySnapshot {
        taken_at,
        total: entries.len(),
        tagged: entries
            .iter()
            .filter(|entry| !entry.tags.title.is_empty())
            .count(),
        with_artwork: entries.iter().filter(|entry| entry.has_artwork()).count(),
        with_genre: entries
            .iter()
            .filter(|entry| !entry.tags.genres.is_empty())
            .count(),
        lossless: entries.iter().filter(|entry| is_lossless(entry)).count(),
        verified_clean: count(Verdict::Ok),
        flagged: count(Verdict::Fake) + count(Verdict::Suspect),
    }
}

fn is_lossless(entry: &LibraryEntry) -> bool {
    entry
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            LOSSLESS_EXTENSIONS.contains(&format!(".{}", extension.to_lowercase()).as_str())
        })
}

/// The day a measurement was taken, as `YYYY-MM-DD`.
///
/// A scan records at most one measurement per set of counts, so the axis a
/// trend is read along is which day the library looked like that, not which
/// second.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn date_of(taken_at: f64) -> String {
    // Bounded by the timestamps SQLite holds -- seconds since 1970, nowhere
    // near the edge of an `i64` count of days.
    let days = (taken_at / 86_400.0).floor() as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's `civil_from_days`, which is the whole of what one column of
/// one report needs. `chrono` would be a dependency for exactly this.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (
        year_of_era + era * 400 + i64::from(month <= 2),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}
