//! Detecting files whose audio is less than their container claims.
//!
//! Two claims are checked. A lossless file claims its source was lossless: an
//! MP3 re-encoded as FLAC is a transcode, [`Verdict::Fake`]. An MP3 claims its
//! header bitrate: a 128 kbps file re-encoded at 320 is an upsample,
//! [`Verdict::Upsampled`]. Either lie leaves the first encoder's low-pass
//! behind, and that edge is what this module finds. So does a 44.1 kHz master
//! resampled to 96 kHz: nothing was lost, but its content stops dead at the
//! Nyquist of the rate it really has.
//!
//! A genuine CD-sourced lossless file carries content out to near the Nyquist
//! frequency. An MP3 or AAC re-encoded as FLAC keeps the lossy encoder's low-pass
//! baked in, and the *shape* of that edge is what gives it away:
//!
//! - a lossy encoder's cutoff is a brickwall -- tens of dB lost over a few
//!   hundred hertz, landing at one of a handful of bitrate-specific frequencies
//! - a loud modern master rolls off gradually over several kilohertz
//!
//! This distinction is the whole point. "Is there content above 16 kHz" is not a
//! usable test on its own: heavily limited masters roll off early and would be
//! flagged wholesale. Steepness is measured by scanning for the sharpest slope
//! across a narrow window straddling the cutoff, because averaging one wide span
//! dilutes a real, narrow cliff until it looks gradual.
//!
//! Everything here operates on sample slices, so the classifier is testable
//! against synthesised signals with known cutoffs and needs no decoder.
// Sample counts, band indices and frame offsets are cast between `usize`, `f64`
// and the integer widths throughout. Every one of them is bounded by an FFT size
// or a track length -- thousands, not billions -- so the pedantic cast lints
// only ask for `try_from` unwraps around values that cannot overflow.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use realfft::RealFftPlanner;

use crate::models::Verdict;

pub const BAND_WIDTH_HZ: usize = 150;
pub const DEFAULT_FFT_SIZE: usize = 8192;
pub const DEFAULT_NOISE_FLOOR_DB: f64 = -50.0;
pub const DEFAULT_STEEP_DB_PER_KHZ: f64 = 25.0;
pub const DEFAULT_NEAR_NYQUIST_MARGIN_HZ: u32 = 1500;
const STEEPNESS_WINDOW_HZ: usize = 800;

/// Below this averaged magnitude there is no signal to characterise.
///
/// Without the guard, normalising a silent spectrum to its own peak makes every
/// band read as 0 dB, and a silent file is confidently reported as genuinely
/// lossless.
const SILENCE_MAGNITUDE: f64 = 1e-9;

/// Below this, there is no cutoff to judge.
///
/// Every lossy encoder's low-pass sits well above this, so content stopping
/// lower means the file is not ordinary music -- a tone, a field recording, an
/// early transfer -- and saying it looks genuinely lossless would be claiming
/// more than the spectrum supports.
pub const MIN_ANALYSABLE_CUTOFF_HZ: u32 = 8000;

/// Frequency bands where common encoders place their low-pass, what they imply,
/// and the lowest bitrate each band's encodes are made at.
///
/// Measured from LAME stereo transcodes, which land at roughly 16.6, 18.8 and
/// 20.1 kHz. The bands are wider than those nominal values and deliberately
/// contiguous: encoders differ, and band averaging quantises the measured cutoff.
/// A gap between them would only downgrade a real transcode to SUSPECT for being
/// 150 Hz off.
pub const KNOWN_LOSSY_CUTOFFS: &[(u32, u32, &str, u32)] = &[
    (15000, 16800, "~128kbps source", 128),
    (16800, 19000, "~160-192kbps source", 160),
    (19000, 20600, "~256-320kbps source", 256),
];

/// The sample rates a master is made at, and so what a hi-res file can have
/// been resampled from.
pub const STANDARD_SAMPLE_RATES: &[u32] = &[44100, 48000, 88200, 96000, 176_400];

/// How far below a lower rate's Nyquist a resampler's wall can land.
///
/// A resampler's anti-aliasing filter has to finish before the Nyquist of the
/// rate it is leaving, so its passband ends a little short of it: ffmpeg's
/// default stops at 97%, sox's at 91-95%.
pub const RESAMPLER_MARGIN_HZ: u32 = 2000;

/// How much of a VBR file's average bitrate counts as its claim.
///
/// A CBR header is a setting; a VBR one is an average of what the content
/// needed, which runs above the quality's nominal rate on dense material and
/// below it on sparse. Discounting it keeps a V0 encode that averaged 260 kbps
/// from being held to a 320 kbps low-pass it never promised.
pub const VBR_BITRATE_CREDIT: f64 = 0.75;

/// An averaged spectrum, in fixed-width frequency bands relative to the peak.
#[derive(Debug, Clone, PartialEq)]
pub struct Bands {
    pub lower_edges_hz: Vec<f64>,
    pub db: Vec<f64>,
}

impl Bands {
    #[must_use]
    pub fn len(&self) -> usize {
        self.db.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.db.is_empty()
    }
}

/// Where the audio stops, how abruptly, and what that means.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectralAnalysis {
    pub cutoff_hz: f64,
    pub steepness_db_per_khz: f64,
    pub verdict: Verdict,
    pub detail: String,
}

/// What a file's container says its audio is, which the spectrum is judged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    Lossless,
    Lossy { bitrate_kbps: u32, is_vbr: bool },
}

/// Tuning for [`analyse`], mirroring the `verify` config section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    pub noise_floor_db: f64,
    pub steep_db_per_khz: f64,
    pub near_nyquist_margin_hz: u32,
    pub fft_size: usize,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            noise_floor_db: DEFAULT_NOISE_FLOOR_DB,
            steep_db_per_khz: DEFAULT_STEEP_DB_PER_KHZ,
            near_nyquist_margin_hz: DEFAULT_NEAR_NYQUIST_MARGIN_HZ,
            fft_size: DEFAULT_FFT_SIZE,
        }
    }
}

/// numpy's `hanning`, which is the symmetric window.
///
/// Most DSP libraries default to the periodic one, `cos(2*pi*n/M)`. Using it
/// here would shift every band slightly against the oracle.
fn hann(size: usize) -> Vec<f64> {
    if size == 1 {
        return vec![1.0];
    }
    let denominator = (size - 1) as f64;
    (0..size)
        .map(|n| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / denominator).cos())
        .collect()
}

/// Average the magnitude spectrum over every frame, normalised to its peak.
///
/// Returns `None` when the input holds no complete frame or no audible signal,
/// which is the honest answer for a file too short or too quiet to characterise.
///
/// Samples are widened to `f64` before the transform: numpy's `rfft` promotes a
/// float32 array to double, and staying in single precision drifts the spectrum.
#[must_use]
pub fn average_spectrum_db(
    chunks: &[Vec<f32>],
    sample_rate: u32,
    fft_size: usize,
) -> Option<(Vec<f64>, Vec<f64>)> {
    let window = hann(fft_size);
    let mut planner = RealFftPlanner::<f64>::new();
    let plan = planner.plan_fft_forward(fft_size);
    let mut input = plan.make_input_vec();
    let mut output = plan.make_output_vec();

    let mut total: Option<Vec<f64>> = None;
    let mut frames = 0_usize;

    for chunk in chunks {
        if chunk.len() < fft_size {
            continue;
        }
        for start in (0..=chunk.len() - fft_size).step_by(fft_size / 2) {
            for (slot, (sample, weight)) in input
                .iter_mut()
                .zip(chunk[start..start + fft_size].iter().zip(&window))
            {
                *slot = f64::from(*sample) * weight;
            }
            plan.process(&mut input, &mut output).ok()?;

            let running = total.get_or_insert_with(|| vec![0.0; output.len()]);
            for (sum, bin) in running.iter_mut().zip(output.iter()) {
                *sum += bin.norm();
            }
            frames += 1;
        }
    }

    let total = total?;
    if frames == 0 {
        return None;
    }

    let frames = frames as f64;
    let averaged: Vec<f64> = total.iter().map(|sum| sum / frames).collect();
    if averaged.iter().copied().fold(f64::MIN, f64::max) < SILENCE_MAGNITUDE {
        return None;
    }

    let frequencies: Vec<f64> = (0..averaged.len())
        .map(|bin| bin as f64 * f64::from(sample_rate) / fft_size as f64)
        .collect();
    let db: Vec<f64> = averaged
        .iter()
        .map(|magnitude| 20.0 * (magnitude + 1e-12).log10())
        .collect();
    let peak = db.iter().copied().fold(f64::MIN, f64::max);

    Some((frequencies, db.iter().map(|value| value - peak).collect()))
}

/// Collapse the spectrum into fixed-width bands, smoothing per-bin noise.
#[must_use]
pub fn band_average(frequencies: &[f64], db: &[f64], band_width_hz: usize) -> Bands {
    let highest = frequencies.last().copied().unwrap_or(0.0);
    let width = band_width_hz as f64;
    let count = if highest > 0.0 {
        (highest / width).ceil() as usize
    } else {
        0
    };
    let edges: Vec<f64> = (0..count).map(|index| index as f64 * width).collect();

    let mut sums = vec![0.0; edges.len()];
    let mut counts = vec![0_usize; edges.len()];
    for (frequency, value) in frequencies.iter().zip(db) {
        // `np.digitize(x, edges) - 1`: the number of edges at or below x, less one.
        let band = edges.partition_point(|edge| *edge <= *frequency);
        if band >= 1 && band - 1 < edges.len() {
            sums[band - 1] += value;
            counts[band - 1] += 1;
        }
    }

    let averages = sums
        .iter()
        .zip(&counts)
        .map(|(sum, count)| {
            if *count == 0 {
                -999.0
            } else {
                sum / *count as f64
            }
        })
        .collect();

    Bands {
        lower_edges_hz: edges,
        db: averages,
    }
}

/// The highest frequency still carrying audible content.
#[must_use]
pub fn find_cutoff(bands: &Bands, noise_floor_db: f64) -> f64 {
    bands
        .db
        .iter()
        .rposition(|value| *value > noise_floor_db)
        .map_or(0.0, |band| bands.lower_edges_hz[band])
}

/// Steepest local slope, in dB per kHz, across the region around the cutoff.
///
/// Scanning for the sharpest narrow slope rather than measuring one wide span is
/// what separates a brickwall from a roll-off: averaged over several kilohertz,
/// both look similar.
#[must_use]
pub fn cliff_steepness(bands: &Bands, cutoff_hz: f64, window_hz: usize) -> f64 {
    let window = window_hz as f64;
    let low = bands
        .lower_edges_hz
        .partition_point(|edge| *edge < (cutoff_hz - window * 2.0).max(0.0));
    let high = bands
        .lower_edges_hz
        .partition_point(|edge| *edge < cutoff_hz + window);
    let high = high.min(bands.len().saturating_sub(1));
    let step = (window_hz / BAND_WIDTH_HZ).max(1);

    if high <= low + step {
        return 0.0;
    }

    let per_khz = (step * BAND_WIDTH_HZ) as f64 / 1000.0;
    let steepest = bands.db[low..high - step]
        .iter()
        .zip(&bands.db[low + step..high])
        .map(|(before, after)| (before - after) / per_khz)
        .fold(f64::MIN, f64::max);

    steepest.max(0.0)
}

/// The source bitrate a cutoff at this frequency implies, if any.
#[must_use]
pub fn matched_bitrate(cutoff_hz: f64) -> Option<&'static str> {
    KNOWN_LOSSY_CUTOFFS
        .iter()
        .find(|(low, high, ..)| f64::from(*low) <= cutoff_hz && cutoff_hz <= f64::from(*high))
        .map(|(_, _, label, _)| *label)
}

/// How many of the known encoder bands a cutoff clears: 0 inside or below the
/// lowest, one more than the last band's index above it.
#[must_use]
pub fn cutoff_tier(cutoff_hz: f64) -> usize {
    KNOWN_LOSSY_CUTOFFS
        .iter()
        .filter(|(_, high, ..)| cutoff_hz > f64::from(*high))
        .count()
}

/// The band a header bitrate promises a cutoff in, or `None` below the lowest.
#[must_use]
pub fn bitrate_tier(bitrate_kbps: f64) -> Option<usize> {
    KNOWN_LOSSY_CUTOFFS
        .iter()
        .filter(|(.., nominal)| f64::from(*nominal) <= bitrate_kbps)
        .count()
        .checked_sub(1)
}

/// Judge a cutoff and its sharpness.
#[must_use]
pub fn classify(
    cutoff_hz: f64,
    steepness_db_per_khz: f64,
    sample_rate: u32,
    steep_db_per_khz: f64,
    near_nyquist_margin_hz: u32,
) -> (Verdict, String) {
    let nyquist = f64::from(sample_rate) / 2.0;

    if cutoff_hz >= nyquist - f64::from(near_nyquist_margin_hz) {
        return (
            Verdict::Ok,
            "content extends to near the Nyquist limit -- genuinely lossless".to_owned(),
        );
    }

    if cutoff_hz < f64::from(MIN_ANALYSABLE_CUTOFF_HZ) {
        return (
            Verdict::Error,
            format!(
                "content stops at {cutoff_hz:.0}Hz, below any encoder cutoff \
                 -- too band-limited to judge"
            ),
        );
    }

    let is_steep = steepness_db_per_khz >= steep_db_per_khz;
    let label = matched_bitrate(cutoff_hz);
    let measured = format!("{cutoff_hz:.0}Hz ({steepness_db_per_khz:.0}dB/kHz)");

    // A hi-res container whose content does not even fill the CD range, cut off
    // sharply, is an upsample of a lossy file rather than a hi-res master.
    if is_steep && sample_rate >= 48000 && cutoff_hz < 20500.0 {
        return (
            Verdict::Fake,
            format!(
                "{sample_rate}Hz container but content stops sharply at {measured} \
                 -- upsampled, not hi-res"
            ),
        );
    }
    if let Some(source_rate) = is_steep
        .then(|| resampled_from(cutoff_hz, sample_rate))
        .flatten()
    {
        return (
            Verdict::Upsampled,
            format!(
                "{sample_rate}Hz container but content stops sharply at {measured}, \
                 the limit of {source_rate}Hz audio -- upsampled, not hi-res"
            ),
        );
    }
    if is_steep {
        if let Some(label) = label {
            return (
                Verdict::Fake,
                format!("brickwall cutoff at {measured} matches a {label} -- transcoded"),
            );
        }
        return (
            Verdict::Suspect,
            format!(
                "steep cutoff at {measured} matches no standard bitrate, \
                 but the sharpness looks artificial -- inspect manually"
            ),
        );
    }
    (
        Verdict::Ok,
        format!("gradual roll-off from {measured} -- consistent with mastering, not a transcode"),
    )
}

/// The standard rate below `sample_rate` whose Nyquist a cutoff sits at, if any.
///
/// The band average reports the lower edge of the last band carrying
/// content, so a wall exactly at a Nyquist reads up to one band below it.
#[must_use]
pub fn resampled_from(cutoff_hz: f64, sample_rate: u32) -> Option<u32> {
    STANDARD_SAMPLE_RATES
        .iter()
        .copied()
        .filter(|rate| *rate < sample_rate)
        .find(|rate| {
            let nyquist = f64::from(*rate) / 2.0;
            nyquist - f64::from(RESAMPLER_MARGIN_HZ) <= cutoff_hz
                && cutoff_hz <= nyquist + BAND_WIDTH_HZ as f64
        })
}

/// Judge a lossy file's cutoff against the bitrate its header claims.
///
/// Only a brickwall is evidence: a gradual roll-off is the master's, and an
/// encoder at any bitrate passes it through. A wall is an upsample when it
/// sits in a lower band than the header's bitrate would put it -- a tier or
/// more, with a VBR average discounted by [`VBR_BITRATE_CREDIT`] first. A
/// header below the lowest band promises nothing to fall short of.
#[must_use]
pub fn classify_lossy(
    cutoff_hz: f64,
    steepness_db_per_khz: f64,
    sample_rate: u32,
    bitrate_kbps: u32,
    is_vbr: bool,
    steep_db_per_khz: f64,
    near_nyquist_margin_hz: u32,
) -> (Verdict, String) {
    let nyquist = f64::from(sample_rate) / 2.0;
    let vbr = if is_vbr { " VBR" } else { "" };
    let claimed = format!("{bitrate_kbps}kbps{vbr}");

    if cutoff_hz >= nyquist - f64::from(near_nyquist_margin_hz) {
        return (
            Verdict::Ok,
            "content extends to near the Nyquist limit -- nothing was cut away".to_owned(),
        );
    }
    if cutoff_hz < f64::from(MIN_ANALYSABLE_CUTOFF_HZ) {
        return (
            Verdict::Error,
            format!(
                "content stops at {cutoff_hz:.0}Hz, below any encoder cutoff \
                 -- too band-limited to judge"
            ),
        );
    }

    let measured = format!("{cutoff_hz:.0}Hz ({steepness_db_per_khz:.0}dB/kHz)");
    if steepness_db_per_khz < steep_db_per_khz {
        return (
            Verdict::Ok,
            format!(
                "gradual roll-off from {measured} -- consistent with mastering, not an upsample"
            ),
        );
    }

    let credited = if is_vbr {
        f64::from(bitrate_kbps) * VBR_BITRATE_CREDIT
    } else {
        f64::from(bitrate_kbps)
    };
    match bitrate_tier(credited) {
        Some(promised) if cutoff_tier(cutoff_hz) < promised => {
            let source = matched_bitrate(cutoff_hz).unwrap_or("source below any standard bitrate");
            (
                Verdict::Upsampled,
                format!(
                    "{claimed} header but brickwall at {measured} matches a {source} -- upsampled"
                ),
            )
        }
        _ => (
            Verdict::Ok,
            format!("cutoff at {measured} fits a {claimed} encode"),
        ),
    }
}

/// Run the whole check over decoded samples, against what the container claims.
#[must_use]
pub fn analyse(
    chunks: &[Vec<f32>],
    sample_rate: u32,
    claim: Claim,
    tuning: &Tuning,
) -> Option<SpectralAnalysis> {
    let (frequencies, db) = average_spectrum_db(chunks, sample_rate, tuning.fft_size)?;

    let bands = band_average(&frequencies, &db, BAND_WIDTH_HZ);
    let cutoff = find_cutoff(&bands, tuning.noise_floor_db);
    let steepness = cliff_steepness(&bands, cutoff, STEEPNESS_WINDOW_HZ);
    let (verdict, detail) = match claim {
        Claim::Lossless => classify(
            cutoff,
            steepness,
            sample_rate,
            tuning.steep_db_per_khz,
            tuning.near_nyquist_margin_hz,
        ),
        Claim::Lossy {
            bitrate_kbps,
            is_vbr,
        } => classify_lossy(
            cutoff,
            steepness,
            sample_rate,
            bitrate_kbps,
            is_vbr,
            tuning.steep_db_per_khz,
            tuning.near_nyquist_margin_hz,
        ),
    };

    Some(SpectralAnalysis {
        cutoff_hz: cutoff,
        steepness_db_per_khz: steepness,
        verdict,
        detail,
    })
}
