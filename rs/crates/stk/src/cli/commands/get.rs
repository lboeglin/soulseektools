//! `stk get` -- fetch one track by name, without a CSV in sight.
//!
//! The batch flow exists for building a library from an export. This is the
//! other half: you want one song, you know what it is called, and setting up a
//! wantlist to get it would be absurd. Same search, same ranking, same
//! verification -- just asked for directly and answered in one command.

use std::io::BufRead as _;

use clap::Args;
use serde_json::json;

use crate::cli::context::{table, AppContext};
use crate::config::SlskdSettings;
use crate::errors::Result;
use crate::models::{ScoredCandidate, Track};
use crate::soulseek::base::SoulseekClient;
use crate::soulseek::ranking;
use crate::soulseek::slskd::{HttpTransport, SlskdClient};
use crate::{fetch, text::normalize::normalize};

/// The en and em dashes are deliberate: a track name pasted out of a browser or
/// a music player usually carries one rather than a plain hyphen.
pub const SEPARATORS: &[&str] = &[" - ", " \u{2013} ", " \u{2014} "];

#[derive(Args, Debug)]
pub struct GetArgs {
    /// What to find: "Artist - Title", or just a title.
    pub query: String,

    /// The artist, if not given in the query.
    #[arg(short = 'a', long)]
    pub artist: Option<String>,

    /// Accept only lossless results.
    #[arg(long)]
    pub lossless: bool,

    /// Reject lossy results below this bitrate.
    #[arg(long)]
    pub min_bitrate: Option<u32>,

    /// Take the best candidate without asking.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Choose a candidate but do not download it yet.
    #[arg(long)]
    pub queue_only: bool,

    /// How many candidates to offer.
    #[arg(short = 'n', long, default_value_t = 5)]
    pub limit: usize,
}

/// Split `artist - title` into its parts, tolerating either being absent.
///
/// A query with no separator is treated as a title: searching for a title alone
/// works, searching for a bare artist does not.
#[must_use]
pub fn split_query(query: &str) -> (String, String) {
    for separator in SEPARATORS {
        if let Some((artist, title)) = query.split_once(separator) {
            return (artist.trim().to_owned(), title.trim().to_owned());
        }
    }
    (String::new(), query.trim().to_owned())
}

/// # Panics
///
/// If the track just added cannot be found again, which would mean the review
/// store lost a row between two statements in the same connection.
pub fn run(context: &AppContext, args: &GetArgs) -> Result<i32> {
    let (parsed_artist, title) = split_query(&args.query);
    let track = Track::new(args.artist.as_deref().unwrap_or(&parsed_artist), &title);

    if track.title.is_empty() {
        eprintln!("error: nothing to search for");
        eprintln!("hint: try: stk get \"DECO*27 - Roki\"");
        return Ok(1);
    }

    let mut options = context.settings().ranking.clone();
    options.require_lossless = args.lossless || options.require_lossless;
    options.min_bitrate = args.min_bitrate.or(options.min_bitrate);

    let settings: &SlskdSettings = &context.settings().slskd;
    let client = SlskdClient::new(settings.clone(), HttpTransport::new(settings)?);

    context.say(&format!("Searching for {} ...", track.query()));
    let candidates = client.search(
        &normalize(&format!("{} {}", track.artist, track.title)),
        settings.search_timeout_seconds,
        settings.search_response_limit,
    )?;
    let scored = ranking::rank(&track, &candidates, &options, Some(args.limit));

    if context.as_json {
        context.emit(&payload(&track, &scored));
        return Ok(0);
    }

    if scored.is_empty() {
        let qualifier = if args.lossless || args.min_bitrate.is_some() {
            " with those quality limits"
        } else {
            ""
        };
        context.say(&format!(
            "Nothing usable found for {}{qualifier}",
            track.query()
        ));
        return Ok(1);
    }

    print_candidates(context, &scored);
    let Some(choice) = (if args.yes { Some(1) } else { ask(scored.len()) }) else {
        context.say("Nothing downloaded.");
        return Ok(0);
    };

    let mut store = context.open_review()?;
    store.add(std::slice::from_ref(&track))?;
    let query = track.query();
    let entry = store
        .tracks(&[], None)?
        .into_iter()
        .find(|entry| entry.track.query() == query)
        .expect("the track just added");
    store.set_candidates(entry.id, &scored)?;
    store.approve(entry.id, i64::try_from(choice).unwrap_or(1))?;

    if args.queue_only {
        context.say(&format!(
            "Approved candidate {choice}. Run stk fetch download."
        ));
        return Ok(0);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let outcomes = fetch::enqueue_approved(&mut store, &client, None, now)?;

    if let Some(failure) = outcomes.iter().find(|outcome| !outcome.queued) {
        let message = if failure.error.is_empty() {
            "could not queue the download"
        } else {
            &failure.error
        };
        eprintln!("error: {message}");
        return Ok(1);
    }

    context.say(&format!(
        "Queued candidate {choice}. Run stk fetch download --collect to record it once it finishes."
    ));
    Ok(0)
}

fn print_candidates(context: &AppContext, scored: &[ScoredCandidate]) {
    let mut rows = Vec::new();
    for (index, candidate) in scored.iter().enumerate() {
        rows.push(vec![(index + 1).to_string(), candidate.describe()]);
        rows.push(vec![String::new(), candidate.candidate.remote_path.clone()]);
        if !candidate.flags.is_empty() {
            rows.push(vec![
                String::new(),
                format!("warnings: {}", candidate.flags.join(", ")),
            ]);
        }
    }
    context.say(&table(&["#", "Detail"], &rows));
}

/// Ask which candidate to take, or none.
fn ask(count: usize) -> Option<usize> {
    println!("Take which? [1-{count}, or n for none] (1): ");
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return None;
    }

    let answer = answer.trim().to_lowercase();
    if answer.is_empty() {
        return Some(1);
    }
    if matches!(answer.as_str(), "n" | "no" | "none" | "q") {
        return None;
    }
    match answer.parse::<usize>() {
        Ok(chosen) if (1..=count).contains(&chosen) => Some(chosen),
        _ => {
            println!("Not one of the offered candidates.");
            None
        }
    }
}

fn payload(track: &Track, scored: &[ScoredCandidate]) -> serde_json::Value {
    json!({
        "query": track.query(),
        "candidates": scored
            .iter()
            .enumerate()
            .map(|(index, candidate)| json!({
                "rank": index + 1,
                "score": candidate.score,
                "username": candidate.candidate.username,
                "remote_path": candidate.candidate.remote_path,
                "size": candidate.candidate.size,
                "flags": candidate.flags,
            }))
            .collect::<Vec<_>>(),
    })
}
