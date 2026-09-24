//! The three hot paths behind the slow commands, timed on synthesised audio.
//!
//! `verify` spends its time in `spectral::analyse`, `dupes content` and
//! `tags identify` in `fingerprint::fingerprint`, and every name comparison in
//! `matching::score_track`. Run `cargo bench` before and after a change to any
//! of them, so a regression shows up as a number rather than a feeling. CI does
//! not run these; it only builds them.

#[path = "../tests/support/mod.rs"]
mod support;

use criterion::{criterion_group, criterion_main, Criterion};
use stk::audio::decode::{probe_windows, Audio};
use stk::audio::fingerprint::fingerprint;
use stk::audio::spectral::{analyse, Claim};
use stk::config::VerifySettings;
use stk::matching::{score_track, MatchThresholds, NameForms};
use support::{noise_source, Scratch};

/// Long enough for `verify` to take all four of its probe windows, as it does
/// on a real track; a shorter file falls back to one.
const TRACK_SECONDS: u32 = 120;

fn matching(criterion: &mut Criterion) {
    let thresholds = MatchThresholds::default();
    let pairs = [
        (
            "latin",
            ("DECO*27", "Roki"),
            ("DECO_27", "06. Roki (feat. Hatsune Miku)"),
        ),
        (
            "japanese",
            (
                "\u{7a32}\u{8449}\u{66c7}",
                "\u{30ed}\u{30b9}\u{30c8}\u{30ef}\u{30f3}\u{306e}\u{53f7}\u{54ed}",
            ),
            ("Inabakumori", "Lost One no Goukoku"),
        ),
        ("mismatch", ("ZUN", "Native Faith"), ("Ado", "Usseewa")),
    ];

    let mut group = criterion.benchmark_group("score_track");
    for (name, (left_artist, left_title), (right_artist, right_title)) in pairs {
        let forms = (
            NameForms::of(left_artist),
            NameForms::of(left_title),
            NameForms::of(right_artist),
            NameForms::of(right_title),
        );
        group.bench_function(name, |bencher| {
            bencher.iter(|| score_track(&forms.0, &forms.1, &forms.2, &forms.3, &thresholds));
        });
    }
    group.finish();
}

fn fingerprinting(criterion: &mut Criterion) {
    let scratch = Scratch::new("bench-fingerprint");
    let track = noise_source(&scratch.join("noise.flac"), TRACK_SECONDS, 44100);

    criterion.bench_function("fingerprint a 120s flac", |bencher| {
        bencher.iter(|| fingerprint(&track).expect("a fingerprint"));
    });
}

fn spectral(criterion: &mut Criterion) {
    let scratch = Scratch::new("bench-spectral");
    let track = noise_source(&scratch.join("noise.flac"), TRACK_SECONDS, 44100);
    let settings = VerifySettings::default();

    let mut audio = Audio::open(&track).expect("a decodable fixture");
    let stream = audio.stream().clone();
    let chunks: Vec<Vec<f32>> = probe_windows(
        stream.duration_seconds,
        settings.probe_seconds,
        settings.probe_count,
    )
    .into_iter()
    .map(|(start, length)| audio.window(start, length).expect("a window"))
    .collect();
    let tuning = settings.tuning();

    criterion.bench_function("analyse verify's windows", |bencher| {
        bencher.iter(|| analyse(&chunks, stream.sample_rate, Claim::Lossless, &tuning));
    });
}

criterion_group!(benches, matching, fingerprinting, spectral);
criterion_main!(benches);
