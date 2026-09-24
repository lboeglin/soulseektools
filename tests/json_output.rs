//! The shape of every `--json` document, pinned so a script depending on one
//! learns about a renamed field from this suite rather than from breaking.
//!
//! Each command runs as the real binary, because a document is only usable if
//! it is all that reaches stdout -- which the in-process harness in `cli.rs`
//! cannot capture. Each runs against its own empty library, since an empty
//! result is exactly where a command is most tempted to print nothing at all.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use stk::models::{Candidate, ScoredCandidate, Track};
use stk::review::store::ReviewStore;
use support::{transcode_to_flac, Scratch};

/// What a document is at its top level.
enum Shape {
    /// A list of rows, each carrying these keys when there are any rows.
    Rows(&'static [&'static str]),
    /// An object with exactly these keys.
    Object(&'static [&'static str]),
}

/// A home with an empty library, and nothing listening where slskd would be.
struct Home {
    scratch: Scratch,
}

impl Home {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let root = scratch.join("Music");
        std::fs::create_dir_all(&root).expect("a library root");
        let quoted = |path: PathBuf| toml::Value::String(path.display().to_string()).to_string();
        std::fs::write(
            scratch.join("stk.toml"),
            format!(
                "[library]\nroot = {}\n\n[index]\npath = {}\n\n[review]\npath = {}\n\n\
                 [genres]\nmap_path = {}\n\n[slskd]\nurl = \"http://127.0.0.1:9\"\n",
                quoted(root),
                quoted(scratch.join("library.db")),
                quoted(scratch.join("review.db")),
                quoted(scratch.join("genres.toml")),
            ),
        )
        .expect("a written config");
        std::fs::write(scratch.join("genres.toml"), "").expect("an empty genre map");
        std::fs::write(
            scratch.join("export.csv"),
            "Artist Name(s),Track Name\nZUN,Native Faith\n",
        )
        .expect("a written export");
        std::fs::write(scratch.join("sheet.csv"), "path,artist,title\n").expect("a written sheet");
        std::fs::write(
            scratch.join("legacy.csv"),
            "artist,title,cand1_user,cand1_path,approve\n\
             ZUN,Native Faith,peer,@@peer\\ZUN\\Native Faith.flac,1\n",
        )
        .expect("a written legacy review");
        Self { scratch }
    }

    /// Two tracks one peer holds in a single folder, waiting to be reviewed.
    fn with_album_in_review(self) -> Self {
        let mut store =
            ReviewStore::open(&self.scratch.join("review.db")).expect("the review queue");
        for title in ["Roki", "Ghost Rule"] {
            store
                .add(&[Track::new("DECO*27", title)])
                .expect("an added track");
            let track_id = store
                .tracks(&[], None)
                .expect("tracks")
                .into_iter()
                .find(|entry| entry.track.title == title)
                .expect("the track just added")
                .id;
            store
                .set_candidates(
                    track_id,
                    &[ScoredCandidate {
                        candidate: Candidate {
                            username: "bulk".to_owned(),
                            remote_path: format!(r"@@bulk\Music\Best Of\{title}.flac"),
                            size: 30_000_000,
                            bit_depth: Some(16),
                            sample_rate: Some(44100),
                            free_upload_slots: true,
                            ..Candidate::default()
                        },
                        score: 100,
                        title_ratio: 0.99,
                        flags: Vec::new(),
                    }],
                )
                .expect("recorded candidates");
        }
        self
    }

    fn path(&self) -> &Path {
        self.scratch.path()
    }

    /// Run `stk --json` with `arguments`, handing back the exit code, stdout and stderr.
    fn stk(&self, arguments: &[&str]) -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stk"));
        command
            .current_dir(self.path())
            .arg("--config")
            .arg(self.scratch.join("stk.toml"))
            .arg("--json")
            .args(arguments);
        for (name, _) in std::env::vars() {
            if name.starts_with("STK_") || name.ends_with("_API_KEY") || name.ends_with("_TOKEN") {
                command.env_remove(name);
            }
        }
        let output = command.output().expect("stk runs");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

fn sorted(keys: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut keys: Vec<String> = keys.into_iter().collect();
    keys.sort();
    keys
}

fn expected(keys: &[&str]) -> Vec<String> {
    sorted(keys.iter().map(|key| (*key).to_owned()))
}

fn check(arguments: &[&str], exit: i32, shape: &Shape) {
    let home = Home::new(&format!(
        "json-{}",
        arguments.join("-").replace(['.', ' '], "_")
    ));
    let (code, stdout, stderr) = home.stk(arguments);
    let named = arguments.join(" ");

    assert_eq!(code, exit, "stk --json {named} exited {code}\n{stderr}");
    let document: Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!("stk --json {named} printed no document ({error}):\n{stdout:?}\n{stderr}")
    });

    match (shape, &document) {
        (Shape::Rows(keys), Value::Array(rows)) => {
            for row in rows {
                let Value::Object(row) = row else {
                    panic!("stk --json {named} has a row that is not an object: {row}");
                };
                assert_eq!(
                    sorted(row.keys().cloned()),
                    expected(keys),
                    "stk --json {named}"
                );
            }
        }
        (Shape::Object(keys), Value::Object(object)) => {
            assert_eq!(
                sorted(object.keys().cloned()),
                expected(keys),
                "stk --json {named}"
            );
        }
        _ => panic!("stk --json {named} printed the wrong kind of document: {document}"),
    }
}

const DOCTOR: &[&str] = &["name", "status", "detail", "ok"];
const TREND: &[&str] = &[
    "taken_at",
    "total",
    "tagged",
    "with_artwork",
    "with_genre",
    "lossless",
    "verified_clean",
    "flagged",
    "with_replay_gain",
];
const SEARCHED: &[&str] = &["track_id", "query", "found", "error", "preferred_uploader"];
const SETTINGS: &[&str] = &[
    "library", "index", "review", "matching", "slskd", "fetch", "ranking", "uploader", "verify",
    "loudness", "artwork", "identify", "genres", "organise", "dupes", "wantlist",
];

/// Every command whose document can be produced with no slskd to talk to, the
/// exit code it gives on an empty library, and the shape of what it prints.
///
/// Three cannot: `fetch download`, and `get` for one track or one album, each
/// talk to slskd before there is anything to report, and fail with a message
/// on stderr and exit 1 when it is not there. `review pick` refuses `--json`
/// outright, since it asks a question per track.
const DOCUMENTS: &[(&[&str], i32, Shape)] = &[
    (&["config", "show"], 0, Shape::Object(SETTINGS)),
    (&["config", "path"], 0, Shape::Object(&["path"])),
    (
        &["config", "init", "--path", "written.toml"],
        0,
        Shape::Object(&["written"]),
    ),
    (&["doctor"], 0, Shape::Rows(DOCTOR)),
    (
        &["library", "scan"],
        0,
        Shape::Object(&[
            "roots",
            "added",
            "updated",
            "unchanged",
            "removed",
            "failed",
            "total",
        ]),
    ),
    (
        &["library", "stats"],
        0,
        Shape::Object(&[
            "total",
            "tagged",
            "with_artwork",
            "with_genre",
            "with_replay_gain",
            "artists",
        ]),
    ),
    (&["library", "trend"], 0, Shape::Rows(TREND)),
    (&["verify"], 0, Shape::Rows(&[])),
    (&["verify", "--changed"], 0, Shape::Rows(&[])),
    (
        &["loudness"],
        0,
        Shape::Object(&[
            "planned",
            "skipped",
            "partial_albums",
            "applied",
            "written",
            "failed",
        ]),
    ),
    (&["dupes", "titles"], 0, Shape::Rows(&[])),
    (&["dupes", "content"], 0, Shape::Rows(&[])),
    (
        &["dupes", "resolve"],
        0,
        Shape::Object(&[
            "keep",
            "min_score",
            "quarantine",
            "groups",
            "skipped",
            "applied",
            "failed",
        ]),
    ),
    (&["art", "check"], 0, Shape::Rows(&[])),
    (&["art", "fetch"], 0, Shape::Rows(&[])),
    (
        &["genre", "check"],
        0,
        Shape::Object(&["genres", "untagged", "unmapped_artists"]),
    ),
    (&["tags", "dump"], 0, Shape::Rows(&[])),
    (
        &["tags", "apply", "sheet.csv"],
        0,
        Shape::Object(&["planned", "skipped", "applied", "failed"]),
    ),
    (
        &["tags", "identify"],
        0,
        Shape::Object(&[
            "identified",
            "unmatched",
            "planned",
            "skipped",
            "applied",
            "failed",
        ]),
    ),
    (
        &["organise"],
        0,
        Shape::Object(&["planned", "blocked", "applied", "failed"]),
    ),
    (&["playlist", "export", "export.csv"], 0, Shape::Rows(&[])),
    (&["playlist", "genre", "Touhou"], 0, Shape::Rows(&[])),
    (&["playlist", "artist", "ZUN"], 0, Shape::Rows(&[])),
    (
        &["wantlist", "diff", "export.csv"],
        0,
        Shape::Object(&["total", "missing", "owned", "queued", "seen_before"]),
    ),
    (&["review", "list"], 0, Shape::Object(&["counts", "tracks"])),
    (&["review", "albums"], 0, Shape::Rows(&[])),
    (
        &["review", "import", "legacy.csv"],
        0,
        Shape::Object(&["added", "approvals"]),
    ),
    (&["fetch", "search"], 0, Shape::Rows(SEARCHED)),
    (
        &["sync", "export.csv", "--yes"],
        1,
        Shape::Object(&[
            "applied", "scan", "diff", "search", "pick", "download", "organise", "rescan",
        ]),
    ),
    (&["changes", "list"], 0, Shape::Rows(&[])),
    (&["changes", "undo", "--last"], 0, Shape::Rows(&[])),
];

#[test]
fn every_command_prints_its_document_on_an_empty_library() {
    for (arguments, exit, shape) in DOCUMENTS {
        check(arguments, *exit, shape);
    }
}

#[test]
fn verify_fails_a_fake_file_under_json_as_it_does_without() {
    // The document was emitted and the run returned 0 before the FAKE check,
    // so a script reading --json saw success where a person saw failure.
    let home = Home::new("json-verify-fake");
    let faked = transcode_to_flac(&home.scratch, "fake", "128k");
    let target = faked.display().to_string();

    let (code, stdout, stderr) = home.stk(&["verify", &target]);
    let rows: Value = serde_json::from_str(&stdout).expect("a document");

    assert_eq!(code, 1, "{stderr}");
    assert_eq!(rows[0]["verdict"], "FAKE", "{rows}");
}

const DECIDED: &[&str] = &["id", "artist", "title", "status", "detail", "approved_rank"];

fn decided(home: &Home, arguments: &[&str]) -> Value {
    let (code, stdout, stderr) = home.stk(arguments);
    assert_eq!(code, 0, "stk --json {}\n{stderr}", arguments.join(" "));
    let row: Value = serde_json::from_str(&stdout).expect("a document");
    let keys = row.as_object().expect("an object").keys().cloned();
    assert_eq!(sorted(keys), expected(DECIDED), "{row}");
    row
}

#[test]
fn a_decision_on_the_review_queue_prints_the_track_as_it_now_stands() {
    let home = Home::new("json-review-decisions").with_album_in_review();

    let approved = decided(&home, &["review", "approve", "1", "1"]);
    assert_eq!(approved["status"], "approved", "{approved}");
    assert_eq!(approved["approved_rank"], 1, "{approved}");

    let rejected = decided(&home, &["review", "reject", "2", "--reason", "live cut"]);
    assert_eq!(rejected["status"], "rejected", "{rejected}");

    let reset = decided(&home, &["review", "reset", "2"]);
    assert_eq!(reset["status"], "pending", "{reset}");
}

#[test]
fn approving_an_album_prints_every_track_it_took() {
    let home = Home::new("json-review-album").with_album_in_review();

    let (code, stdout, stderr) = home.stk(&["review", "approve-album", "1"]);
    let album: Value = serde_json::from_str(&stdout).expect("a document");

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(album["username"], "bulk", "{album}");
    assert_eq!(album["album"], "Best Of", "{album}");
    assert_eq!(
        album["approved"].as_array().map(Vec::len),
        Some(2),
        "{album}"
    );
}
