//! Drawing a spectrogram for `verify --spectrograms`.
// Bin indices, pixel coordinates and sample counts are cast between `usize`,
// `u32` and `f64` throughout, all bounded by an image a thousand pixels wide.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::path::{Path, PathBuf};

use realfft::RealFftPlanner;

use crate::audio::decode::Audio;
use crate::errors::{Error, Result};

const FFT_SIZE: usize = 2048;
const WIDTH: usize = 1024;
const HEIGHT: usize = 512;
const LEFT: usize = 44;
const BOTTOM: usize = 16;
const MARGIN: usize = 6;
const GLYPH_PITCH: usize = 4;
/// Relative to the file's own peak, so a quiet transfer is as legible.
const RANGE_DB: f64 = 100.0;

pub fn render(path: &Path, destination: &Path) -> Result<PathBuf> {
    let mut audio = Audio::open(path)?;
    let sample_rate = audio.stream().sample_rate;
    let channels = (audio.stream().channels as usize).max(1);

    let mut mono: Vec<f32> = Vec::new();
    audio.frames(&mut |frames: &[f32]| {
        mono.extend(
            frames
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
        Ok(())
    })?;

    if mono.len() < FFT_SIZE {
        return Err(Error::Audio(format!(
            "{}: too short to draw a spectrum",
            path.display()
        )));
    }

    let columns = columns(&mono);
    let image = draw(
        &columns,
        sample_rate,
        mono.len() as f64 / f64::from(sample_rate),
    );
    image
        .save(destination)
        .map_err(|error| Error::Audio(format!("{}: {error}", destination.display())))?;
    Ok(destination.to_path_buf())
}

fn columns(mono: &[f32]) -> Vec<Vec<f64>> {
    let window = hann(FFT_SIZE);
    let mut planner = RealFftPlanner::<f64>::new();
    let plan = planner.plan_fft_forward(FFT_SIZE);
    let mut input = plan.make_input_vec();
    let mut output = plan.make_output_vec();

    // The last window has to end at the last sample, so the step is over what
    // is left after it rather than over the whole track.
    let span = (mono.len() - FFT_SIZE) as f64;
    let mut columns: Vec<Vec<f64>> = Vec::with_capacity(WIDTH);
    for column in 0..WIDTH {
        let start = (span * column as f64 / (WIDTH - 1) as f64) as usize;
        for (slot, (sample, weight)) in input
            .iter_mut()
            .zip(mono[start..start + FFT_SIZE].iter().zip(&window))
        {
            *slot = f64::from(*sample) * weight;
        }
        if plan.process(&mut input, &mut output).is_err() {
            break;
        }
        columns.push(output.iter().map(|bin| bin.norm()).collect());
    }

    let peak = columns
        .iter()
        .flatten()
        .copied()
        .fold(f64::MIN_POSITIVE, f64::max);
    for column in &mut columns {
        for value in column.iter_mut() {
            *value = 20.0 * (*value / peak).max(1e-12).log10();
        }
    }
    columns
}

/// The symmetric Hann window, as `spectral.rs` uses.
fn hann(size: usize) -> Vec<f64> {
    let denominator = (size - 1) as f64;
    (0..size)
        .map(|n| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / denominator).cos())
        .collect()
}

fn draw(columns: &[Vec<f64>], sample_rate: u32, seconds: f64) -> image::RgbImage {
    let width = (LEFT + WIDTH + MARGIN) as u32;
    let height = (MARGIN + HEIGHT + BOTTOM) as u32;
    let mut image = image::RgbImage::from_pixel(width, height, image::Rgb([12, 12, 16]));

    let bins = columns.first().map_or(0, Vec::len);
    for (x, column) in columns.iter().enumerate() {
        for y in 0..HEIGHT {
            // Row zero is the top of the image and the top of the spectrum.
            let bin = (HEIGHT - 1 - y) * (bins - 1) / (HEIGHT - 1);
            let level = ((column[bin] + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0);
            image.put_pixel(
                (LEFT + x) as u32,
                (MARGIN + y) as u32,
                image::Rgb(colour(level)),
            );
        }
    }

    label_frequencies(&mut image, sample_rate);
    label_time(&mut image, seconds);
    image
}

fn colour(level: f64) -> [u8; 3] {
    const STOPS: [[f64; 3]; 5] = [
        [0.0, 0.0, 0.05],
        [0.15, 0.05, 0.45],
        [0.65, 0.10, 0.40],
        [0.98, 0.55, 0.10],
        [1.0, 1.0, 0.95],
    ];

    let position = level.clamp(0.0, 1.0) * (STOPS.len() - 1) as f64;
    let lower = (position as usize).min(STOPS.len() - 2);
    let fraction = position - lower as f64;
    let mut pixel = [0_u8; 3];
    for channel in 0..3 {
        let value =
            STOPS[lower][channel] + (STOPS[lower + 1][channel] - STOPS[lower][channel]) * fraction;
        pixel[channel] = (value * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    pixel
}

fn label_frequencies(image: &mut image::RgbImage, sample_rate: u32) {
    let nyquist = f64::from(sample_rate) / 2.0;
    let mut khz = 0;
    while f64::from(khz) * 1000.0 <= nyquist {
        let y = MARGIN + HEIGHT
            - 1
            - (f64::from(khz) * 1000.0 / nyquist * (HEIGHT - 1) as f64) as usize;
        for x in LEFT - 4..LEFT {
            image.put_pixel(x as u32, y as u32, image::Rgb([150, 150, 160]));
        }
        text(image, 2, y.saturating_sub(2), &format!("{khz}k"));
        khz += 2;
    }
}

fn label_time(image: &mut image::RgbImage, seconds: f64) {
    for quarter in 0..=4 {
        let x = LEFT + (WIDTH - 1) * quarter / 4;
        for y in MARGIN + HEIGHT..MARGIN + HEIGHT + 3 {
            image.put_pixel(x as u32, y as u32, image::Rgb([150, 150, 160]));
        }
        // Centred on the tick, and pulled back inside the image at the end of
        // the axis, where a centred label would run off the edge.
        let label = format!("{:.0}s", seconds * quarter as f64 / 4.0);
        let width = label.len() * GLYPH_PITCH;
        let left = x
            .saturating_sub(width / 2)
            .min(LEFT + WIDTH + MARGIN - width);
        text(image, left, MARGIN + HEIGHT + 5, &label);
    }
}

/// A 3x5 pixel glyph per character the axes use, rather than a font dependency.
fn glyph(character: char) -> [u8; 5] {
    match character {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'k' => [0b100, 0b101, 0b110, 0b101, 0b101],
        's' => [0b011, 0b100, 0b010, 0b001, 0b110],
        _ => [0; 5],
    }
}

fn text(image: &mut image::RgbImage, left: usize, top: usize, value: &str) {
    for (index, character) in value.chars().enumerate() {
        let origin = left + index * GLYPH_PITCH;
        for (row, bits) in glyph(character).iter().enumerate() {
            for column in 0..3 {
                if bits & (0b100 >> column) == 0 {
                    continue;
                }
                let (x, y) = ((origin + column) as u32, (top + row) as u32);
                if x < image.width() && y < image.height() {
                    image.put_pixel(x, y, image::Rgb([190, 190, 200]));
                }
            }
        }
    }
}
