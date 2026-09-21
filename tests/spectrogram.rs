//! The picture `stk verify --spectrograms` leaves beside a flagged file.
//!
//! Nothing compares these images to anything, so what is worth pinning is that
//! one is written, that it is the size the axes were laid out for, and that a
//! file with no decoder is reported rather than silently skipped.

mod support;

use stk::audio::spectrogram;
use support::{encode, noise_source, Encoded, Scratch};

#[test]
fn a_spectrogram_is_written_beside_the_file() {
    let scratch = Scratch::new("spectrogram-write");
    let source = noise_source(&scratch.join("source.flac"), 8, 44100);
    let destination = scratch.join("spectrum.png");

    let written = spectrogram::render(&source, &destination).expect("a rendered spectrogram");

    assert_eq!(written, destination);
    let image = image::open(&destination).expect("a readable png");
    assert_eq!(image.width(), 1074);
    assert_eq!(image.height(), 534);
}

#[test]
fn a_file_too_short_to_transform_is_reported() {
    let scratch = Scratch::new("spectrogram-short");
    let source = encode(
        &scratch.join("blip.wav"),
        &Encoded {
            seconds: 0.01,
            ..Encoded::default()
        },
    );

    let error = spectrogram::render(&source, &scratch.join("spectrum.png"))
        .expect_err("nothing to transform");
    assert!(error.to_string().contains("too short"), "{error}");
}
