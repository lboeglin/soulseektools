//! Transcodes and upsamples, told by the low-pass edge: a lossy encoder's is a brickwall.
// Casts between sample counts, band indices and floats are bounded by an FFT
// size or a track length, so they cannot overflow.
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

/// Below this averaged magnitude there is no signal. Without it, a silent
/// spectrum normalised to its peak reads 0 dB everywhere and passes as lossless.
const SILENCE_MAGNITUDE: f64 = 1e-9;

/// Below this there is no cutoff to judge: every lossy encoder's low-pass sits
/// well above it.
pub const MIN_ANALYSABLE_CUTOFF_HZ: u32 = 8000;

/// Measured from LAME stereo transcodes; contiguous so an off-nominal one isn't only SUSPECT.
pub const KNOWN_LOSSY_CUTOFFS: &[(u32, u32, &str, u32)] = &[
    (15000, 16800, "~128kbps source", 128),
    (16800, 19000, "~160-192kbps source", 160),
    (19000, 20600, "~256-320kbps source", 256),
];

pub const STANDARD_SAMPLE_RATES: &[u32] = &[44100, 48000, 88200, 96000, 176_400];

/// The highest rate that is not a hi-res claim.
pub const HI_RES_ABOVE_HZ: u32 = 48000;

/// A steep resampler (soxr) stops 91-97% of the way to the Nyquist it protects.
pub const RESAMPLER_MARGIN_HZ: u32 = 2000;

/// How far above a lower rate's Nyquist a soft resampler's tail can run.
/// ffmpeg's default is only 15 dB down at 22.05 kHz and drops to silence about
/// 3 kHz later; [`RESAMPLER_FLOOR_DB`] finds that drop.
pub const RESAMPLER_OVERSHOOT_HZ: u32 = 3500;

/// The floor below which a resampler's stopband counts as silence. Resampled
/// files measured -92 to -164 dB past their edge; genuine hi-res recordings
/// carry converter noise to Nyquist and never get this quiet.
pub const RESAMPLER_FLOOR_DB: f64 = -85.0;

/// How much of a VBR file's average bitrate counts as its claim, so a V0 encode
/// averaging 260 kbps isn't held to a 320 kbps low-pass.
pub const VBR_BITRATE_CREDIT: f64 = 0.75;

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

#[derive(Debug, Clone, PartialEq)]
pub struct SpectralAnalysis {
    pub cutoff_hz: f64,
    pub steepness_db_per_khz: f64,
    pub verdict: Verdict,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    Lossless,
    Lossy { bitrate_kbps: u32, is_vbr: bool },
}

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

/// The symmetric Hann window, which the thresholds were measured with.
fn hann(size: usize) -> Vec<f64> {
    if size == 1 {
        return vec![1.0];
    }
    let denominator = (size - 1) as f64;
    (0..size)
        .map(|n| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / denominator).cos())
        .collect()
}

/// `None` without a complete frame or audible signal. In `f64`: single precision drifts.
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

#[must_use]
pub fn find_cutoff(bands: &Bands, noise_floor_db: f64) -> f64 {
    bands
        .db
        .iter()
        .rposition(|value| *value > noise_floor_db)
        .map_or(0.0, |band| bands.lower_edges_hz[band])
}

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

#[derive(Debug, Clone, PartialEq)]
pub struct Resampled {
    /// Every rate whose resampling could have left this edge, lowest first.
    /// An edge near 23 kHz fits both 44.1 and 48 kHz.
    pub from_hz: Vec<u32>,
    pub edge_hz: f64,
    pub steepness_db_per_khz: f64,
}

/// Only above [`HI_RES_ABOVE_HZ`]; a steep edge just below or past a lower rate's Nyquist.
#[must_use]
pub fn resampled_from(bands: &Bands, sample_rate: u32, tuning: &Tuning) -> Option<Resampled> {
    if sample_rate <= HI_RES_ABOVE_HZ {
        return None;
    }
    let nyquist = f64::from(sample_rate) / 2.0;
    [tuning.noise_floor_db, RESAMPLER_FLOOR_DB]
        .into_iter()
        .find_map(|floor| {
            let edge = find_cutoff(bands, floor);
            if edge >= nyquist - f64::from(tuning.near_nyquist_margin_hz) {
                return None;
            }
            let steepness = cliff_steepness(bands, edge, STEEPNESS_WINDOW_HZ);
            if steepness < tuning.steep_db_per_khz {
                return None;
            }
            let from_hz: Vec<u32> = STANDARD_SAMPLE_RATES
                .iter()
                .copied()
                .filter(|rate| *rate < sample_rate)
                .filter(|rate| {
                    let old_nyquist = f64::from(*rate) / 2.0;
                    old_nyquist - f64::from(RESAMPLER_MARGIN_HZ) <= edge
                        && edge <= old_nyquist + f64::from(RESAMPLER_OVERSHOOT_HZ)
                })
                .collect();
            (!from_hz.is_empty()).then_some(Resampled {
                from_hz,
                edge_hz: edge,
                steepness_db_per_khz: steepness,
            })
        })
}

/// Only a brickwall counts; a wall a band below the header's bitrate is an upsample.
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
        Claim::Lossless => {
            let judged = classify(
                cutoff,
                steepness,
                sample_rate,
                tuning.steep_db_per_khz,
                tuning.near_nyquist_margin_hz,
            );
            match (judged.0, resampled_from(&bands, sample_rate, tuning)) {
                (Verdict::Ok | Verdict::Suspect, Some(resampled)) => {
                    let rates: Vec<String> = resampled
                        .from_hz
                        .iter()
                        .map(|rate| format!("{rate}Hz"))
                        .collect();
                    (
                        Verdict::Upsampled,
                        format!(
                            "{sample_rate}Hz container but content ends sharply at {:.0}Hz \
                             ({:.0}dB/kHz), the limit of {} audio -- upsampled, not hi-res",
                            resampled.edge_hz,
                            resampled.steepness_db_per_khz,
                            rates.join(" or ")
                        ),
                    )
                }
                _ => judged,
            }
        }
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
