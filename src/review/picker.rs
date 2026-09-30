//! Walking the review queue, one track and one keystroke at a time.

use std::io::{BufRead, Write};

use crate::errors::Result;
use crate::models::TrackStatus;
use crate::review::store::{ReviewStore, ReviewTrack};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve(i64),
    Reject,
    Skip,
    Undo,
    Quit,
}

/// `None` means nothing on offer was named. `n` skips this track rather than quitting.
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub waiting: usize,
    pub approved: usize,
    pub rejected: usize,
    /// Seen and passed over, as against never reached.
    pub skipped: usize,
}

/// What `--yes` did with the tracks it was given.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Taken {
    pub approved: Vec<i64>,
    /// Rank 1 carries a warning, so the track waits for `stk review pick`.
    pub held: Vec<ReviewTrack>,
}

/// Approves rank 1 of each track, unless it is flagged and `force` is off.
pub fn take_best(store: &mut ReviewStore, entries: &[ReviewTrack], force: bool) -> Result<Taken> {
    let mut taken = Taken::default();
    for entry in entries {
        let Some(best) = entry.candidates.first() else {
            continue;
        };
        if best.needs_a_look() && !force {
            taken.held.push(entry.clone());
        } else if store.approve(entry.id, 1).is_ok() {
            taken.approved.push(entry.id);
        }
    }
    Ok(taken)
}

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
    // This run's decisions, for `u`. Nothing from this run has been queued, so
    // undoing one can safely clear its `attempted` mark.
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
            // End of input is not an empty answer, or a dry pipe would approve
            // everything.
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
                // A rejected rank leaves the previous choice; ask again.
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

fn read_answer(input: &mut impl BufRead) -> Option<String> {
    let mut answer = String::new();
    match input.read_line(&mut answer) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(answer),
    }
}
