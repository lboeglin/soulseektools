//! Properties that must hold for any input, not only the fixtures named elsewhere.
//!
//! The oracle tests pin behaviour against a private library and skip in CI, so
//! without these the parsers and classifiers are exercised only on the inputs
//! the unit tests happen to name (#19).

mod support;

use std::path::{Component, Path, PathBuf};

use proptest::prelude::*;
use stk::audio::spectral::classify;
use stk::config::OrganiseSettings;
use stk::errors::Error;
use stk::library::changes::{apply_undo, plan_undo, ChangeFilter, ChangeLog};
use stk::library::index::{default_extensions, LibraryIndex};
use stk::library::scan::Excludes;
use stk::library::tags::{self, TagField};
use stk::matching::NameForms;
use stk::models::{LibraryEntry, ReplayGainTags, TrackTags, Verdict};
use stk::organise::{render, Pattern};
use stk::text::normalize::normalize;
use stk::text::romaji::to_romaji;
use stk::wantlist::{read_export, Format, ARTIST_COLUMNS, TITLE_COLUMNS};
use support::{encode, Encoded, Scratch};

/// Text with a fair share of what breaks parsers and paths: separators, dots,
/// quotes, control characters, Japanese, characters whose case mapping or
/// compatibility form changes their length, and anything else Unicode holds.
fn awkward_text() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[/\\\\.:\"'<>|?*,; \\x00-\\x1f]{0,8}",
        "[a-zA-Z0-9 ]{0,12}",
        "[\u{3041}-\u{30fc}\u{4e00}-\u{4e20} ]{0,8}",
        "[a-zA-Z \u{130}\u{131}\u{1e9e}\u{df}\u{3a3}\u{1f88}\u{fb01}\u{2126}\u{212b}\u{301}\u{307}]{0,8}",
    ]
}

mod wantlist {
    use super::*;

    fn header_cell() -> impl Strategy<Value = String> {
        prop_oneof![
            proptest::sample::select(ARTIST_COLUMNS).prop_map(str::to_owned),
            proptest::sample::select(TITLE_COLUMNS).prop_map(str::to_owned),
            awkward_text(),
        ]
    }

    fn table() -> impl Strategy<Value = (Vec<String>, Vec<Vec<String>>)> {
        (1usize..6).prop_flat_map(|width| {
            (
                proptest::collection::vec(header_cell(), width),
                proptest::collection::vec(proptest::collection::vec(awkward_text(), width), 0..6),
            )
        })
    }

    fn written(scratch: &Scratch, header: &[String], rows: &[Vec<String>]) -> PathBuf {
        let path = scratch.join("export.csv");
        let mut writer = csv::Writer::from_path(&path).expect("a writable export");
        writer.write_record(header).expect("a header");
        for row in rows {
            writer.write_record(row).expect("a row");
        }
        writer.flush().expect("a flushed export");
        path
    }

    fn read_or_refuse(path: &Path, format: Option<Format>) -> Result<(), TestCaseError> {
        match read_export(path, format, None) {
            Ok(_) | Err(Error::Config(_)) => Ok(()),
            Err(error) => Err(TestCaseError::fail(format!(
                "{format:?} raised something other than a config error: {error:?}"
            ))),
        }
    }

    proptest! {
        #[test]
        fn any_table_is_read_or_refused_as_a_config_error((header, rows) in table()) {
            let scratch = Scratch::new("properties-wantlist-table");
            let path = written(&scratch, &header, &rows);
            read_or_refuse(&path, None)?;
        }

        #[test]
        fn any_text_is_read_or_refused_as_a_config_error_in_every_format(
            text in awkward_text(),
            format in proptest::sample::select(Format::ALL.to_vec()),
        ) {
            let scratch = Scratch::new("properties-wantlist-text");
            let path = scratch.join("export.txt");
            std::fs::write(&path, &text).expect("a written export");
            read_or_refuse(&path, None)?;
            read_or_refuse(&path, Some(format))?;
        }
    }
}

mod organise {
    use super::*;

    const PLACEHOLDERS: &[&str] = &[
        "{artist}",
        "{album_artist}",
        "{album}",
        "{title}",
        "{track}",
    ];

    fn component() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                proptest::sample::select(PLACEHOLDERS).prop_map(str::to_owned),
                "[a-zA-Z0-9 _()\\[\\]-]{1,4}",
            ],
            1..4,
        )
        .prop_map(|segments| segments.concat())
    }

    /// Any pattern the parser accepts, built from the documented placeholders.
    fn pattern() -> impl Strategy<Value = Pattern> {
        (
            proptest::collection::vec(component(), 1..4),
            any::<prop::sample::Index>(),
        )
            .prop_map(|(mut components, at)| {
                let at = at.index(components.len());
                components[at].push_str("{title}");
                components.join("/")
            })
            .prop_filter_map("a pattern the parser accepts", |text| {
                Pattern::parse(&text).ok()
            })
    }

    fn tags() -> impl Strategy<Value = TrackTags> {
        (
            awkward_text(),
            awkward_text(),
            awkward_text(),
            awkward_text(),
            proptest::option::of(any::<i64>()),
        )
            .prop_map(
                |(artist, album_artist, album, title, track_number)| TrackTags {
                    artist,
                    album_artist,
                    album,
                    title,
                    track_number,
                    ..TrackTags::default()
                },
            )
    }

    fn entry(tags: TrackTags) -> LibraryEntry {
        LibraryEntry {
            path: PathBuf::from("song.flac"),
            size: 1,
            mtime: 1.0,
            folder_artist: String::new(),
            artist: NameForms::of(&tags.artist),
            title: NameForms::of(&tags.title),
            duration_seconds: None,
            artwork_bytes: 0,
            tags,
            codec: String::new(),
            bitrate: None,
            sample_rate: None,
            bit_depth: None,
            replay_gain: ReplayGainTags::default(),
        }
    }

    proptest! {
        #[test]
        fn a_rendered_path_stays_inside_the_root(pattern in pattern(), tags in tags()) {
            let Some(relative) = render(&pattern, &entry(tags), &OrganiseSettings::default()) else {
                return Ok(());
            };

            prop_assert!(relative.is_relative(), "{}", relative.display());
            for part in relative.components() {
                let Component::Normal(name) = part else {
                    return Err(TestCaseError::fail(format!(
                        "{part:?} in {}",
                        relative.display()
                    )));
                };
                let name = name.to_string_lossy();
                prop_assert!(!name.is_empty(), "{}", relative.display());
                prop_assert!(!name.contains(['/', '\\']), "{}", relative.display());
            }
        }
    }
}

mod names {
    use super::*;

    fn words() -> impl Strategy<Value = Vec<String>> {
        proptest::collection::vec(
            prop_oneof![
                "[a-zA-Z0-9]{1,8}",
                "[\u{3041}-\u{3093}\u{30a1}-\u{30f3}\u{4e00}-\u{4e20}]{1,4}",
            ],
            1..5,
        )
    }

    /// The same name as an export, a filename and a peer might each spell it.
    fn respelled(words: &[String], separator: &str, suffix: &str) -> String {
        format!("{}{suffix}", words.join(separator).to_uppercase())
    }

    fn separator() -> impl Strategy<Value = &'static str> {
        proptest::sample::select(&[" ", "  ", " - ", "_", "*", "!", ", ", " / "][..])
    }

    fn suffix() -> impl Strategy<Value = &'static str> {
        proptest::sample::select(
            &[
                "",
                " (Remix)",
                " [Live]",
                " (feat. Hatsune Miku)",
                " feat. GUMI",
            ][..],
        )
    }

    proptest! {
        #[test]
        fn normalising_twice_changes_nothing(text in awkward_text()) {
            let once = normalize(&text);
            prop_assert_eq!(normalize(&once), once);
        }

        #[test]
        fn romaji_is_already_normalised(text in awkward_text()) {
            let romaji = to_romaji(&text);
            prop_assert_eq!(normalize(&romaji), romaji);
        }

        #[test]
        fn respelling_a_name_does_not_change_its_forms(
            words in words(),
            separator in separator(),
            suffix in suffix(),
        ) {
            let plain = words.join(" ");
            let other = respelled(&words, separator, suffix);
            prop_assert_eq!(normalize(&other), normalize(&plain));
            prop_assert_eq!(to_romaji(&normalize(&other)), to_romaji(&normalize(&plain)));
        }
    }
}

mod spectral {
    use super::*;

    fn judged(cutoff: f64, steepness: f64, rate: u32, steep: f64, margin: u32) -> Verdict {
        classify(cutoff, steepness, rate, steep, margin).0
    }

    proptest! {
        #[test]
        fn any_measurement_gets_one_verdict_and_a_reason(
            cutoff in 0.0f64..200_000.0,
            steepness in 0.0f64..1_000.0,
            rate in 8_000u32..=384_000,
            steep in 1.0f64..200.0,
            margin in 0u32..4_000,
        ) {
            let (verdict, detail) = classify(cutoff, steepness, rate, steep, margin);
            prop_assert!(Verdict::ALL.contains(&verdict));
            prop_assert!(!detail.is_empty());
        }

        #[test]
        fn a_steeper_edge_never_clears_a_file(
            cutoff in 0.0f64..200_000.0,
            gentle in 0.0f64..1_000.0,
            extra in 0.0f64..1_000.0,
            rate in 8_000u32..=384_000,
            steep in 1.0f64..200.0,
            margin in 0u32..4_000,
        ) {
            let before = judged(cutoff, gentle, rate, steep, margin);
            let after = judged(cutoff, gentle + extra, rate, steep, margin);
            if before != Verdict::Ok {
                prop_assert_eq!(after, before);
            }
        }
    }
}

mod undo {
    use super::*;

    /// A tag value as a tag editor would leave it: no surrounding whitespace,
    /// which `TagField::set` trims, and no `;`, which separates genres.
    fn value() -> impl Strategy<Value = String> {
        "[^;\\x00-\\x1f]{0,16}".prop_map(|text| text.trim().to_owned())
    }

    fn tags() -> impl Strategy<Value = TrackTags> {
        (
            value(),
            value(),
            value(),
            value(),
            proptest::collection::vec(value(), 0..3),
            proptest::option::of(1i64..1000),
        )
            .prop_map(
                |(artist, album_artist, album, title, genres, track_number)| TrackTags {
                    artist,
                    album_artist,
                    album,
                    title,
                    genres: genres
                        .into_iter()
                        .filter(|genre| !genre.is_empty())
                        .collect(),
                    track_number,
                },
            )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        #[test]
        fn undoing_a_logged_write_restores_the_tags_it_started_from(
            start in tags(),
            field in proptest::sample::select(TagField::ALL.to_vec()),
            written in value(),
        ) {
            let scratch = Scratch::new("properties-undo");
            let root = scratch.join("Music");
            let track = encode(&root.join("track.flac"), &Encoded::default());
            tags::write_tags(&track, &start).expect("the starting tags");
            let started = tags::read_tags(&track).expect("readable tags");

            let mut index = LibraryIndex::open(&scratch.join("library.db")).expect("an index");
            index
                .refresh(
                    std::slice::from_ref(&root),
                    default_extensions(),
                    &Excludes::default(),
                    false,
                    None,
                )
                .expect("a refresh");

            let log = ChangeLog::open(&index, "tags apply", 100.0).expect("a run");
            let mut after = started.clone();
            field.set(&mut after, &written);
            tags::write_tags(&track, &after).expect("a tag write");
            log.tag(&mut index, &track, field, &started, &after).expect("a logged change");

            let change = index
                .changes(&ChangeFilter::default())
                .expect("the logged rows")
                .remove(0);
            let plan = plan_undo(&index, change).expect("a plan");
            prop_assert!(plan.ready(), "{:?}", plan.blocked);
            let undo = ChangeLog::open(&index, "changes undo", 200.0).expect("a run");
            apply_undo(&mut index, &plan, &undo, std::slice::from_ref(&root)).expect("an undo");

            prop_assert_eq!(tags::read_tags(&track).expect("readable tags"), started);
        }
    }
}
