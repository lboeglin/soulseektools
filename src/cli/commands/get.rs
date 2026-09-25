//! `stk get`: fetch tracks, or with `--album` a whole folder, by name.

use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;

use crate::cli::context::{table, AppContext};
use crate::config::SlskdSettings;
use crate::errors::Error;
use crate::errors::Result;
use crate::fetch;
use crate::models::{ScoredCandidate, Track};
use crate::review::albums::{self, FolderOffer};
use crate::review::picker;
use crate::soulseek::base::SoulseekClient;
use crate::soulseek::ranking;
use crate::soulseek::slskd::{HttpTransport, SlskdClient};
use crate::text::normalize::{flatten, normalize};
use crate::wantlist::split_query;

// Independent CLI flags, not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Args, Debug)]
pub struct GetArgs {
    /// What to find: "Artist - Title", or just a title. Repeatable.
    pub query: Vec<String>,

    /// Read one "Artist - Title" per line from a file, or from stdin with `-`.
    #[arg(long = "from", value_name = "PATH")]
    pub from: Option<PathBuf>,

    /// The artist, if not given in the query.
    #[arg(short = 'a', long)]
    pub artist: Option<String>,

    /// Take a peer's whole folder, starting from an album name.
    #[arg(long)]
    pub album: bool,

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

    /// How many candidates to offer, or folders under `--album`.
    #[arg(short = 'n', long, default_value_t = 5)]
    pub limit: usize,
}

/// Read the wanted tracks from the arguments and from `--from`.
///
/// A list is one `Artist - Title` per line. Blank lines and `#` comments are
/// skipped, so a hand-kept file can carry notes.
pub fn wanted_tracks(args: &GetArgs) -> Result<Vec<Track>> {
    let mut lines: Vec<String> = args.query.clone();

    if let Some(source) = &args.from {
        let text = if source == Path::new("-") {
            let mut buffer = String::new();
            std::io::stdin()
                .lock()
                .read_to_string(&mut buffer)
                .map_err(|error| Error::Config(format!("cannot read stdin: {error}")))?;
            buffer
        } else {
            std::fs::read_to_string(source).map_err(|error| {
                Error::Config(format!("cannot read {}: {error}", source.display()))
            })?
        };
        lines.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned),
        );
    }

    Ok(lines
        .iter()
        .map(|line| {
            let (parsed_artist, title) = split_query(line);
            // `--artist` applies to every line.
            Track::new(args.artist.as_deref().unwrap_or(&parsed_artist), &title)
        })
        .filter(|track| !track.title.is_empty())
        .collect())
}

/// # Panics
///
/// If a track just added cannot be found again, which would mean the review
/// store lost a row between two statements in the same connection.
pub fn run(context: &AppContext, args: &GetArgs) -> Result<i32> {
    if args.album {
        return album(context, args);
    }

    let tracks = wanted_tracks(args)?;
    if tracks.is_empty() {
        eprintln!("error: nothing to search for");
        eprintln!("hint: try: stk get \"DECO*27 - Roki\", or --from list.txt");
        return Ok(1);
    }

    // One track is interactive; several, or a list, are a batch.
    if tracks.len() > 1 || args.from.is_some() {
        return batch(context, args, &tracks);
    }

    let track = tracks.into_iter().next().expect("exactly one track");
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

/// Take a peer's folder as one decision.
///
/// With no tracklist, the folder is what is scored and taken; each audio file
/// becomes an approved review row. Only one album at a time, since each is an
/// interactive decision.
fn album(context: &AppContext, args: &GetArgs) -> Result<i32> {
    if args.from.is_some() || args.query.len() != 1 {
        eprintln!("error: --album takes exactly one album name");
        eprintln!("hint: try: stk get --album \"DECO*27 - Ghost Rule EP\"");
        return Ok(1);
    }

    let (parsed_artist, name) = split_query(&args.query[0]);
    let artist = args.artist.as_deref().unwrap_or(&parsed_artist);
    if name.is_empty() {
        eprintln!("error: nothing to search for");
        eprintln!("hint: try: stk get --album \"DECO*27 - Ghost Rule EP\"");
        return Ok(1);
    }

    let mut options = context.settings().ranking.clone();
    options.require_lossless = args.lossless || options.require_lossless;
    options.min_bitrate = args.min_bitrate.or(options.min_bitrate);

    let settings: &SlskdSettings = &context.settings().slskd;
    let client = SlskdClient::new(settings.clone(), HttpTransport::new(settings)?);

    let query = format!("{artist} {name}");
    context.say(&format!("Searching for {} ...", query.trim()));
    let candidates = client.search(
        &normalize(&query),
        settings.search_timeout_seconds,
        settings.search_response_limit,
    )?;

    let mut offers = albums::folders(
        &candidates,
        &options,
        &flatten(&query),
        context.settings().review.album_minimum_tracks,
    );
    offers.truncate(args.limit);

    if context.as_json {
        context.emit(&album_payload(&query, &offers));
        return Ok(0);
    }

    if offers.is_empty() {
        let qualifier = if args.lossless || args.min_bitrate.is_some() {
            " with those quality limits"
        } else {
            ""
        };
        context.say(&format!("No folder found for {}{qualifier}", query.trim()));
        return Ok(1);
    }

    print_folders(context, &offers);
    let Some(choice) = (if args.yes { Some(1) } else { ask(offers.len()) }) else {
        context.say("Nothing downloaded.");
        return Ok(0);
    };
    let Some(offer) = choice.checked_sub(1).and_then(|at| offers.get(at)) else {
        context.say("Nothing downloaded.");
        return Ok(0);
    };

    let mut store = context.open_review()?;
    let outcome = albums::queue_folder(&mut store, &albums::album_rows(offer, artist, &name))?;
    for query in &outcome.skipped {
        context.say(&format!("Left alone, already on its way: {query}"));
    }
    if outcome.queued == 0 {
        context.say("Nothing left to queue from that folder.");
        return Ok(0);
    }
    context.say(
        "Titles are read off the filenames, so a download keeps the peer's own where it has one.",
    );

    if args.queue_only {
        context.say(&format!(
            "Approved {} file(s) from {}'s \"{}\". Run stk fetch download.",
            outcome.queued,
            offer.username,
            offer.name()
        ));
        return Ok(0);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let queued = fetch::enqueue_approved(&mut store, &client, None, now)?;

    if let Some(failure) = queued.iter().find(|outcome| !outcome.queued) {
        let message = if failure.error.is_empty() {
            "could not queue the download"
        } else {
            &failure.error
        };
        eprintln!("error: {message}");
        return Ok(1);
    }

    context.say(&format!(
        "Queued {} file(s) from {}'s \"{}\". \
         Run stk fetch download --collect to record them once they finish.",
        outcome.queued,
        offer.username,
        offer.name()
    ));
    Ok(0)
}

/// The album listing's table rows, pure so the rendering can be tested.
#[must_use]
pub fn folder_rows(offers: &[FolderOffer]) -> Vec<Vec<String>> {
    offers
        .iter()
        .enumerate()
        .map(|(index, offer)| {
            vec![
                (index + 1).to_string(),
                offer.username.clone(),
                offer.name().to_owned(),
                offer.count().to_string(),
                offer.quality(),
                if offer.free_slots() { "free" } else { "busy" }.to_owned(),
                offer.queue_length().to_string(),
            ]
        })
        .collect()
}

/// What is wrong with a folder, as one line, or nothing when nothing is.
#[must_use]
pub fn folder_warnings(offer: &FolderOffer) -> String {
    let mut warnings = offer.flags.clone();
    if offer.below_quality > 0 {
        warnings.push(format!(
            "{} file(s) below the quality limit",
            offer.below_quality
        ));
    }
    warnings.join(", ")
}

/// Show each folder as a row, then its full remote path and what is inside it.
///
/// The files are listed because the folder is taken whole.
fn print_folders(context: &AppContext, offers: &[FolderOffer]) {
    context.say(&table(
        &["#", "Peer", "Album", "Files", "Quality", "Slot", "Queue"],
        &folder_rows(offers),
    ));
    for (index, offer) in offers.iter().enumerate() {
        context.say(&format!("\n{}. {}", index + 1, offer.folder));
        let warnings = folder_warnings(offer);
        if !warnings.is_empty() {
            context.say(&format!("   warnings: {warnings}"));
        }
        for file in &offer.files {
            context.say(&format!("   {}", file.describe()));
        }
        if offer.extras > 0 {
            context.say(&format!("   + {} other file(s), not queued", offer.extras));
        }
    }
}

fn album_payload(query: &str, offers: &[FolderOffer]) -> serde_json::Value {
    json!({
        "query": query.trim(),
        "albums": offers
            .iter()
            .enumerate()
            .map(|(index, offer)| json!({
                "index": index + 1,
                "username": offer.username,
                "folder": offer.folder,
                "album": offer.name(),
                "score": offer.score,
                "files": offer.count(),
                "extras": offer.extras,
                "quality": offer.quality(),
                "free_slots": offer.free_slots(),
                "queue_length": offer.queue_length(),
                "flags": offer.flags,
                "below_quality": offer.below_quality,
                "paths": offer
                    .files
                    .iter()
                    .map(|file| json!({
                        "remote_path": file.candidate.remote_path,
                        "size": file.candidate.size,
                        "flags": file.flags,
                    }))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    })
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
///
/// Parsed like the review picker's answers; `r` and `u` are refused. An
/// exhausted stdin declines rather than taking the default, so nothing
/// downloads unattended.
fn ask(count: usize) -> Option<usize> {
    println!("Take which? [1-{count}, or n for none] (1): ");
    let mut answer = String::new();
    match std::io::stdin().lock().read_line(&mut answer) {
        Ok(0) | Err(_) => return None,
        Ok(_) => {}
    }

    match picker::parse(&answer, count) {
        Some(picker::Decision::Approve(chosen)) => usize::try_from(chosen).ok(),
        Some(picker::Decision::Skip | picker::Decision::Quit) => None,
        Some(picker::Decision::Reject | picker::Decision::Undo) | None => {
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

/// Queue several tracks at once.
///
/// A list goes straight into the review queue, with no diff against the
/// library. `--yes` takes the top pick for each; otherwise they wait for
/// `stk review pick`.
fn batch(context: &AppContext, args: &GetArgs, tracks: &[Track]) -> Result<i32> {
    let mut settings = context.settings().clone();
    settings.ranking.require_lossless = args.lossless || settings.ranking.require_lossless;
    settings.ranking.min_bitrate = args.min_bitrate.or(settings.ranking.min_bitrate);
    settings.review.candidates_per_track = args.limit.clamp(1, 10);

    let mut store = context.open_review()?;
    let added = store.add(tracks)?;
    context.say(&format!(
        "{} track(s) wanted, {added} newly queued. Searching ...",
        tracks.len()
    ));

    let client = SlskdClient::new(settings.slskd.clone(), HttpTransport::new(&settings.slskd)?);
    let outcomes = fetch::search_pending(&mut store, &client, &settings, None)?;

    if context.as_json {
        context.emit(&serde_json::json!({
            "wanted": tracks.len(),
            "queued": added,
            "searched": outcomes
                .iter()
                .map(|outcome| serde_json::json!({
                    "track_id": outcome.track_id,
                    "query": outcome.query,
                    "found": outcome.found,
                    "error": outcome.error,
                }))
                .collect::<Vec<_>>(),
        }));
        return Ok(0);
    }

    let rows: Vec<Vec<String>> = outcomes
        .iter()
        .map(|outcome| {
            let found = if outcome.error.is_empty() {
                outcome.found.to_string()
            } else {
                "--".to_owned()
            };
            vec![found, outcome.query.clone(), outcome.error.clone()]
        })
        .collect();
    context.say(&table(&["Found", "Track", "Note"], &rows));

    let with_candidates = outcomes.iter().filter(|outcome| outcome.found > 0).count();
    if !args.yes {
        context.say(&format!(
            "{with_candidates} of {} track(s) have candidates. \
             Run stk review pick to choose, then stk fetch download.",
            outcomes.len()
        ));
        return Ok(0);
    }

    let mut approved = 0;
    for outcome in &outcomes {
        if outcome.found > 0 && store.approve(outcome.track_id, 1).is_ok() {
            approved += 1;
        }
    }

    if args.queue_only {
        context.say(&format!(
            "Approved the best candidate for {approved} track(s). Run stk fetch download."
        ));
        return Ok(0);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let queued = fetch::enqueue_approved(&mut store, &client, None, now)?;
    let sent = queued.iter().filter(|outcome| outcome.queued).count();

    context.say(&format!(
        "Queued {sent} of {approved} download(s). \
         Run stk fetch download --collect to record them once they finish."
    ));
    Ok(0)
}
