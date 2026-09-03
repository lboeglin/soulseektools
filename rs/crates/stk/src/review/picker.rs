//! Walking the review queue, one track and one keystroke at a time.
//!
//! Reviewing a wantlist is the only high-volume decision loop in the tool:
//! everything else is a batch job that runs to completion or a report read once,
//! where this is one small judgement per track, each needing its own context.
//! Done through `review list --candidates` and `review approve <id> <rank>` it
//! costs a track id read off a table and typed back in, once per track.
//!
//! This is a prompt loop rather than a full-screen app. A full-screen app would
//! reimplement every command's presentation and give the project a second place
//! where output is decided, against a design where each command has `--json` and
//! a meaningful exit code; a loop keeps the picker to the one thing a table
//! cannot do.
//!
//! Everything except the binding to stdin and stdout lives here, so the whole
//! loop runs against a `Cursor` in a test.

use std::io::{BufRead, Write};

use crate::errors::Result;
use crate::models::TrackStatus;
use crate::review::store::{ReviewStore, ReviewTrack};

/// What one answer at the prompt asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Take this candidate, by the rank the listing gives it.
    Approve(i64),
    Reject,
    Skip,
    Undo,
    Quit,
}

/// Read one answer, against a listing of `count` candidates.
///
/// `None` means the answer named nothing on offer, which is a question to ask
/// again rather than a decision to record. An empty answer takes the first
/// candidate, which is the ranking's own pick and the reason it is ranked --
/// so a listing with nothing in it has no default either, and every answer
/// naming a rank is one nothing can honour.
///
/// "None of these" is [`Decision::Skip`], not [`Decision::Quit`]: `stk get`
/// asks the same question of a single track, where declining it and ending the
/// command are the same act, and someone carrying that habit into a walk of a
/// hundred and thirty-nine means "not this one", never "abandon the rest".
#[must_use]
pub fn parse(answer: &str, count: usize) -> Option<Decision> {
    let answer = answer.trim().to_lowercase();
    if answer.is_empty() {
        return (count > 0).then_some(Decision::Approve(1));
    }
    match answer.as_str() {
        "r" | "reject" => return Some(Decision::Reject),
        "s" | "skip" | "n" | "no" | "none" => return Some(Decision::Skip),
        "u" | "undo" => return Some(Decision::Undo),
        "q" | "quit" => return Some(Decision::Quit),
        _ => {}
    }
    match answer.parse::<usize>() {
        Ok(chosen) if (1..=count).contains(&chosen) => {
            i64::try_from(chosen).ok().map(Decision::Approve)
        }
        _ => None,
    }
}

/// What a run of the picker decided.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// How many tracks were awaiting review when the walk started.
    pub waiting: usize,
    pub approved: usize,
    pub rejected: usize,
    /// Seen and passed over, as against never reached.
    pub skipped: usize,
}

/// Offer every track awaiting review, and record what is chosen for each.
///
/// Only `AwaitingReview` is walked: those are the tracks with candidates to
/// choose between, and returning a dead track to the search queue is a different
/// question from picking one of its results.
///
/// Ranks are passed to the store exactly as the listing shows them. Nothing here
/// renumbers or reorders anything stored, because [`ReviewTrack::approved`]
/// resolves a choice by `rank - 1` into the candidate list and the retry in
/// [`ReviewStore::next_candidate`] assumes the ranks run `1..n`.
pub fn walk(
    store: &mut ReviewStore,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Tally> {
    let queue = store.tracks(&[TrackStatus::AwaitingReview], None)?;
    let mut tally = Tally {
        waiting: queue.len(),
        ..Tally::default()
    };
    // The decisions this run made, so `u` can take one back. Scoped to the run,
    // because that is what makes clearing a candidate's `attempted` mark safe:
    // an approval nothing has queued yet was never really tried.
    let mut decided: Vec<(usize, i64, Decision)> = Vec::new();
    let mut position = 0;

    while let Some(entry) = queue.get(position) {
        present(output, position, queue.len(), entry);
        let count = entry.candidates.len();
        let _ = write!(
            output,
            "Take which? [1-{count}, r reject, s skip, u undo, q quit] (1): "
        );
        let _ = output.flush();

        let Some(answer) = read_answer(input) else {
            // A pipe that has run dry is not agreement. Reading it as the empty
            // answer -- which takes the first candidate -- would walk the whole
            // queue approving everything with nobody at the keyboard.
            let _ = writeln!(output, "\nStopped: nothing left to read.");
            break;
        };

        match parse(&answer, count) {
            None => {
                let _ = writeln!(output, "Not one of the offered candidates.");
            }
            Some(Decision::Quit) => break,
            Some(Decision::Skip) => {
                decided.push((position, entry.id, Decision::Skip));
                tally.skipped += 1;
                position += 1;
            }
            Some(Decision::Reject) => {
                store.reject(entry.id, "")?;
                let _ = writeln!(output, "Rejected.");
                decided.push((position, entry.id, Decision::Reject));
                tally.rejected += 1;
                position += 1;
            }
            Some(Decision::Approve(rank)) => {
                // A rank the store will not take leaves the previous choice
                // standing rather than half-applying it, so say so and offer the
                // same track again instead of passing over it silently.
                if let Err(error) = store.approve(entry.id, rank) {
                    let _ = writeln!(output, "error: {error}");
                    continue;
                }
                let _ = writeln!(output, "Approved candidate {rank}.");
                decided.push((position, entry.id, Decision::Approve(rank)));
                tally.approved += 1;
                position += 1;
            }
            Some(Decision::Undo) => {
                let Some((previous, track_id, what)) = decided.pop() else {
                    let _ = writeln!(output, "Nothing to undo yet.");
                    continue;
                };
                match what {
                    Decision::Approve(_) => {
                        store.undecide(track_id)?;
                        tally.approved -= 1;
                    }
                    Decision::Reject => {
                        store.undecide(track_id)?;
                        tally.rejected -= 1;
                    }
                    _ => tally.skipped -= 1,
                }
                position = previous;
            }
        }
    }

    Ok(tally)
}

/// Show one track and everything found for it.
///
/// The full remote path goes on its own line under each candidate, as `review
/// list --candidates` prints it: the ranking is largely based on it, and it is
/// what reveals a live recording or an orchestral cover behind a filename that
/// looks exactly right. `describe()` carries the flags, so a candidate the batch
/// pass moved to the front says `preferred-uploader` here too.
fn present(output: &mut impl Write, position: usize, total: usize, entry: &ReviewTrack) {
    let _ = writeln!(
        output,
        "\n{}/{total}  {} - {}",
        position + 1,
        entry.track.artist,
        entry.track.title
    );
    for (index, scored) in entry.candidates.iter().enumerate() {
        let _ = writeln!(output, " {}. {}", index + 1, scored.describe());
        let _ = writeln!(output, "    {}", scored.candidate.remote_path);
    }
}

/// One line from the answering end, or `None` once there is nothing left.
fn read_answer(input: &mut impl BufRead) -> Option<String> {
    let mut answer = String::new();
    match input.read_line(&mut answer) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(answer),
    }
}
