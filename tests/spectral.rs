//! The transcode and upsample detector, on synthesised signals with known
//! cutoffs and edge shapes.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use rand::rngs::StdRng;
use rand::SeedableRng;
use rand_distr::{Distribution, StandardNormal};
use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use stk::audio::spectral::{
    analyse, average_spectrum_db, band_average, classify, classify_lossy, cliff_steepness,
    find_cutoff, resampled_from, Bands, Claim, Tuning, BAND_WIDTH_HZ, KNOWN_LOSSY_CUTOFFS,
};
use stk::models::Verdict;

const SAMPLE_RATE: u32 = 44100;
const DURATION_SECONDS: f64 = 6.0;
const FFT_SIZE: usize = 8192;
const STEEPNESS_WINDOW_HZ: usize = 800;

fn tuning() -> Tuning {
    Tuning::default()
}

fn noise(sample_rate: u32, seconds: f64) -> Vec<f32> {
    let mut rng = StdRng::seed_from_u64(20_260_830);
    let count = (f64::from(sample_rate) * seconds) as usize;
    (0..count)
        .map(|_| Distribution::<f64>::sample(&StandardNormal, &mut rng) as f32)
        .collect()
}

/// Re-synthesise `samples` with a per-frequency gain applied.
fn shape_spectrum(samples: &[f32], sample_rate: u32, gain: impl Fn(f64) -> f64) -> Vec<f32> {
    let length = samples.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(length);
    let inverse = planner.plan_fft_inverse(length);

    let mut input: Vec<f64> = samples.iter().map(|sample| f64::from(*sample)).collect();
    let mut spectrum: Vec<Complex<f64>> = forward.make_output_vec();
    forward.process(&mut input, &mut spectrum).unwrap();

    for (bin, value) in spectrum.iter_mut().enumerate() {
        let frequency = bin as f64 * f64::from(sample_rate) / length as f64;
        *value *= gain(frequency);
    }

    let mut shaped = inverse.make_output_vec();
    inverse.process(&mut spectrum, &mut shaped).unwrap();

    // The inverse transform is unnormalised, but every caller normalises to the
    // peak anyway, so the scale factor cancels.
    let peak = shaped
        .iter()
        .fold(0.0_f64, |best, value| best.max(value.abs()));
    shaped.iter().map(|value| (value / peak) as f32).collect()
}

/// A lossy encoder's low-pass: full level, then nothing.
fn brickwall(cutoff_hz: f64, sample_rate: u32, seconds: f64) -> Vec<f32> {
    let samples = noise(sample_rate, seconds);
    shape_spectrum(&samples, sample_rate, |frequency| {
        f64::from(u8::from(frequency < cutoff_hz)) + 1e-7
    })
}

/// A mastering roll-off: level falls away smoothly above the knee.
fn gradual_rolloff(knee_hz: f64, db_per_khz: f64, sample_rate: u32) -> Vec<f32> {
    let samples = noise(sample_rate, DURATION_SECONDS);
    shape_spectrum(&samples, sample_rate, |frequency| {
        let above = (frequency - knee_hz).max(0.0) / 1000.0;
        10.0_f64.powf(-db_per_khz * above / 20.0)
    })
}

fn full_band(sample_rate: u32) -> Vec<f32> {
    let samples = noise(sample_rate, DURATION_SECONDS);
    let peak = samples
        .iter()
        .fold(0.0_f32, |best, sample| best.max(sample.abs()));
    samples.iter().map(|sample| sample / peak).collect()
}

fn steepness_of(samples: Vec<f32>) -> f64 {
    let spectrum =
        average_spectrum_db(&[samples], SAMPLE_RATE, FFT_SIZE).expect("an analysable spectrum");
    let bands = band_average(&spectrum.0, &spectrum.1, BAND_WIDTH_HZ);
    let cutoff = find_cutoff(&bands, tuning().noise_floor_db);
    cliff_steepness(&bands, cutoff, STEEPNESS_WINDOW_HZ)
}

#[test]
fn find_cutoff_locates_a_brickwall_within_a_band_width() {
    for cutoff in [16000.0, 18000.0, 19500.0] {
        let samples = brickwall(cutoff, SAMPLE_RATE, DURATION_SECONDS);
        let spectrum =
            average_spectrum_db(&[samples], SAMPLE_RATE, FFT_SIZE).expect("an analysable spectrum");
        let bands = band_average(&spectrum.0, &spectrum.1, BAND_WIDTH_HZ);

        let found = find_cutoff(&bands, tuning().noise_floor_db);
        assert!(
            (found - cutoff).abs() <= 400.0,
            "cutoff {cutoff}: found {found}"
        );
    }
}

#[test]
fn a_silent_file_is_not_mistaken_for_genuine_lossless() {
    // Normalising a silent spectrum to its own peak puts every band at 0 dB,
    // which reads as content all the way to Nyquist unless silence is caught.
    let silence = vec![vec![0.0_f32; SAMPLE_RATE as usize * 2]];

    assert!(average_spectrum_db(&silence, SAMPLE_RATE, FFT_SIZE).is_none());
    assert!(analyse(&silence, SAMPLE_RATE, Claim::Lossless, &tuning()).is_none());
}

#[test]
fn a_brickwall_is_far_steeper_than_a_rolloff() {
    let wall = steepness_of(brickwall(16000.0, SAMPLE_RATE, DURATION_SECONDS));
    let roll = steepness_of(gradual_rolloff(13000.0, 8.0, SAMPLE_RATE));

    assert!(wall > 25.0, "brickwall steepness {wall}");
    assert!(roll < 25.0, "roll-off steepness {roll}");
}

#[test]
fn the_known_bands_leave_no_gap_a_real_transcode_could_fall_into() {
    for pair in KNOWN_LOSSY_CUTOFFS.windows(2) {
        assert_eq!(pair[0].1, pair[1].0);
    }
}

#[test]
fn a_steep_cutoff_at_an_unknown_frequency_is_only_suspect() {
    let (verdict, detail) = classify(12000.0, 60.0, SAMPLE_RATE, 25.0, 1500);

    assert_eq!(verdict, Verdict::Suspect);
    assert!(detail.contains("inspect manually"));
}

#[test]
fn a_gradual_rolloff_is_accepted_even_when_it_starts_low() {
    // The failure mode the steepness test exists to prevent: a loud master
    // rolling off at 16 kHz is not a transcode.
    let (verdict, detail) = classify(16000.0, 9.0, SAMPLE_RATE, 25.0, 1500);

    assert_eq!(verdict, Verdict::Ok);
    assert!(detail.contains("mastering"));
}

#[test]
fn audio_too_band_limited_to_judge_is_not_called_lossless() {
    // A pure tone, or anything else whose content stops far below every encoder
    // cutoff, carries no evidence either way. Saying it looks genuinely lossless
    // would claim more than the spectrum supports.
    let (verdict, detail) = classify(400.0, 0.0, SAMPLE_RATE, 25.0, 1500);

    assert_eq!(verdict, Verdict::Error);
    assert!(detail.contains("too band-limited to judge"));
}

#[test]
fn a_transcode_is_detected_and_its_source_bitrate_named() {
    for (cutoff, label) in [
        (16000.0, "~128kbps"),
        (18000.0, "~160-192kbps"),
        (19500.0, "~256-320kbps"),
    ] {
        let samples = brickwall(cutoff, SAMPLE_RATE, DURATION_SECONDS);
        let result = analyse(&[samples], SAMPLE_RATE, Claim::Lossless, &tuning())
            .expect("an analysable spectrum");

        assert_eq!(result.verdict, Verdict::Fake, "cutoff {cutoff}");
        assert!(
            result.detail.contains(label),
            "cutoff {cutoff}: {}",
            result.detail
        );
    }
}

#[test]
fn genuine_lossless_audio_passes() {
    let result = analyse(
        &[full_band(SAMPLE_RATE)],
        SAMPLE_RATE,
        Claim::Lossless,
        &tuning(),
    )
    .expect("an analysable spectrum");
    assert_eq!(result.verdict, Verdict::Ok);
}

#[test]
fn a_320kbps_header_over_a_128kbps_wall_is_an_upsample() {
    let (verdict, detail) = classify_lossy(16500.0, 100.0, SAMPLE_RATE, 320, false, 25.0, 1500);

    assert_eq!(verdict, Verdict::Upsampled);
    assert!(detail.contains("320kbps header"), "{detail}");
    assert!(detail.contains("~128kbps source"), "{detail}");
}

#[test]
fn a_wall_at_or_above_where_the_header_bitrate_puts_it_is_accepted() {
    for (cutoff, bitrate) in [
        (16500.0, 128),
        (18700.0, 192),
        (20100.0, 320),
        (19600.0, 256),
        (20100.0, 128),
    ] {
        let (verdict, detail) =
            classify_lossy(cutoff, 100.0, SAMPLE_RATE, bitrate, false, 25.0, 1500);
        assert_eq!(verdict, Verdict::Ok, "{bitrate}kbps at {cutoff}: {detail}");
    }
}

#[test]
fn a_vbr_average_is_discounted_before_it_is_held_to_a_tier() {
    // 260 kbps is a dense V0 encode, whose low-pass sits in the 160-192 band's
    // neighbour rather than at 320's; held to the average undiscounted it would
    // be flagged.
    assert_eq!(
        classify_lossy(18700.0, 100.0, SAMPLE_RATE, 260, true, 25.0, 1500).0,
        Verdict::Ok
    );
    assert_eq!(
        classify_lossy(18700.0, 100.0, SAMPLE_RATE, 260, false, 25.0, 1500).0,
        Verdict::Upsampled
    );
    assert_eq!(
        classify_lossy(16500.0, 100.0, SAMPLE_RATE, 230, true, 25.0, 1500).0,
        Verdict::Upsampled
    );
}

#[test]
fn a_lossy_claim_is_judged_by_its_bitrate_not_as_a_transcode() {
    let samples = vec![brickwall(16000.0, SAMPLE_RATE, DURATION_SECONDS)];
    let claimed = |bitrate_kbps| {
        analyse(
            &samples,
            SAMPLE_RATE,
            Claim::Lossy {
                bitrate_kbps,
                is_vbr: false,
            },
            &tuning(),
        )
        .expect("an analysable spectrum")
        .verdict
    };

    assert_eq!(claimed(128), Verdict::Ok);
    assert_eq!(claimed(320), Verdict::Upsampled);
}

/// Bands across a whole spectrum, each at the level `shape` gives its lower edge.
fn shaped_bands(sample_rate: u32, shape: impl Fn(f64) -> f64) -> Bands {
    let lower_edges_hz: Vec<f64> = (0..(sample_rate as usize / 2).div_ceil(BAND_WIDTH_HZ))
        .map(|band| (band * BAND_WIDTH_HZ) as f64)
        .collect();
    let db = lower_edges_hz.iter().map(|edge| shape(*edge)).collect();
    Bands { lower_edges_hz, db }
}

/// A sharp resampler: full level, then silence from `wall_hz` up.
fn walled(sample_rate: u32, wall_hz: f64) -> Bands {
    shaped_bands(
        sample_rate,
        |edge| if edge < wall_hz { -20.0 } else { -140.0 },
    )
}

#[test]
fn every_rate_that_could_have_left_an_edge_is_named() {
    let between = resampled_from(&walled(96000, 23850.0), 96000, &tuning()).expect("an edge");
    let high = resampled_from(&walled(192_000, 47850.0), 192_000, &tuning()).expect("an edge");

    assert_eq!(between.from_hz, vec![44100, 48000]);
    assert_eq!(high.from_hz, vec![96000]);
}

#[test]
fn a_soft_resamplers_tail_past_the_nyquist_is_found_at_the_deeper_floor() {
    // ffmpeg's default: a gentle slope through the old Nyquist that the
    // ordinary floor calls a roll-off, then a steep drop to silence 3 kHz on.
    let soft = shaped_bands(96000, |edge| match edge {
        edge if edge < 21000.0 => -20.0,
        edge if edge < 24500.0 => -20.0 - (edge - 21000.0) / 1000.0 * 12.0,
        edge if edge < 25500.0 => -62.0 - (edge - 24500.0) / 1000.0 * 60.0,
        _ => -140.0,
    });

    let resampled = resampled_from(&soft, 96000, &tuning()).expect("an edge");

    assert!(resampled.edge_hz > 24000.0, "{resampled:?}");
    assert!(resampled.from_hz.contains(&44100), "{resampled:?}");
}

#[test]
fn a_hi_res_master_fading_into_its_converters_noise_is_not_an_upsample() {
    // Genuine 96 kHz recordings of material with nothing above 22 kHz exist:
    // the content rolls off, and the converter's noise carries on to Nyquist.
    let genuine = shaped_bands(96000, |edge| {
        (-20.0 - (edge - 15000.0).max(0.0) / 1000.0 * 6.0).max(-75.0)
    });

    assert_eq!(resampled_from(&genuine, 96000, &tuning()), None);
}

#[test]
fn only_a_hi_res_claim_is_judged_by_its_spectrum() {
    // A 48 kHz file shows too little past 44.1 kHz's Nyquist to tell a
    // resampler from a mastering low-pass, and 48 kHz is no inflated claim.
    assert_eq!(
        resampled_from(&walled(48000, 21300.0), 48000, &tuning()),
        None
    );
    assert_eq!(
        resampled_from(&walled(44100, 16000.0), 44100, &tuning()),
        None
    );

    let samples = brickwall(21300.0, 48000, DURATION_SECONDS);
    let result =
        analyse(&[samples], 48000, Claim::Lossless, &tuning()).expect("an analysable spectrum");
    assert_ne!(result.verdict, Verdict::Upsampled, "{}", result.detail);
}

#[test]
fn a_hi_res_wall_in_a_lossy_band_is_still_a_transcode() {
    // A lossy source resampled up is worse than a lossless one: it keeps FAKE.
    let samples = brickwall(19800.0, 96000, DURATION_SECONDS);
    let result =
        analyse(&[samples], 96000, Claim::Lossless, &tuning()).expect("an analysable spectrum");

    assert_eq!(result.verdict, Verdict::Fake, "{}", result.detail);
}

#[test]
fn a_hi_res_file_resampled_from_44100_is_detected() {
    let samples = brickwall(22050.0, 96000, DURATION_SECONDS);
    let result =
        analyse(&[samples], 96000, Claim::Lossless, &tuning()).expect("an analysable spectrum");

    assert_eq!(result.verdict, Verdict::Upsampled, "{}", result.detail);
    assert!(result.detail.contains("44100Hz"), "{}", result.detail);
    assert!(
        result.detail.contains("upsampled, not hi-res"),
        "{}",
        result.detail
    );
}
