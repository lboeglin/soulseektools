//! The image `stk verify --spectrograms` writes beside a flagged file.

mod support;

use stk::audio::spectrogram;
use support::{noise_source, Scratch};

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
