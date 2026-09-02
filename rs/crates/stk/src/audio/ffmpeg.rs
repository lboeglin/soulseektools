//! Thin wrappers around ffmpeg and ffprobe.
//!
//! Every subprocess this module's package spawns lives here, so the rest of the
//! audio code works on sample slices and can be tested without either binary
//! installed.
// Window lengths and probe offsets are cast between `usize` and `f64`, all
// bounded by a track length rather than an address space.
#![allow(clippy::cast_precision_loss)]

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::errors::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(300);

/// The audio stream properties the checks depend on.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub codec: String,
    pub channels: u32,
    pub bit_depth: Option<u32>,
    pub duration_seconds: Option<f64>,
}

impl StreamInfo {
    #[must_use]
    pub fn nyquist_hz(&self) -> f64 {
        f64::from(self.sample_rate) / 2.0
    }
}

#[must_use]
pub fn ffmpeg_path() -> Option<PathBuf> {
    which::which("ffmpeg").ok()
}

#[must_use]
pub fn ffprobe_path() -> Option<PathBuf> {
    which::which("ffprobe").ok()
}

/// The path to `tool`, or an error explaining what to install.
pub fn require(tool: &str) -> Result<PathBuf> {
    which::which(tool).map_err(|_| {
        Error::ExternalTool(format!(
            "{tool} is not on PATH; install ffmpeg to use this command"
        ))
    })
}

fn run(program: &Path, arguments: &[String]) -> Result<Vec<u8>> {
    run_captured(program, arguments).map(|(stdout, _)| stdout)
}

/// Run a tool and keep both streams.
///
/// ffmpeg's filters report on stderr even when the run succeeds -- `ebur128`
/// prints its whole summary there -- so a caller after a measurement rather
/// than a decode needs the half [`run`] throws away.
fn run_captured(program: &Path, arguments: &[String]) -> Result<(Vec<u8>, Vec<u8>)> {
    let name = program.file_stem().map_or_else(
        || program.display().to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    );

    let mut child = Command::new(program)
        .args(arguments)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| Error::ExternalTool(format!("{name} could not be started: {error}")))?;

    // Both pipes are drained on their own threads while the child runs. A decode
    // writes megabytes -- twenty seconds of mono f32 is 3.5 MB -- so waiting for
    // the child before reading deadlocks it against a full pipe buffer, and every
    // decode then takes the timeout instead of a second.
    let mut stdout = child.stdout.take().expect("a piped stdout");
    let mut stderr = child.stderr.take().expect("a piped stderr");
    let reading_stdout = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let reading_stderr = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });

    // `Command` has no timeout of its own; the Python's 300s guard exists so one
    // wedged decode cannot stall a library-wide run.
    let deadline = std::time::Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                return Err(Error::ExternalTool(format!("{name} timed out")));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(Error::ExternalTool(format!("{name} failed: {error}"))),
        }
    };

    let stdout = reading_stdout.join().unwrap_or_default();
    let stderr = reading_stderr.join().unwrap_or_default();

    if status.success() {
        return Ok((stdout, stderr));
    }

    let stderr = String::from_utf8_lossy(&stderr);
    let detail = stderr.trim().lines().next_back().map(str::to_owned);
    Err(Error::ExternalTool(
        detail.unwrap_or_else(|| format!("{name} failed")),
    ))
}

#[derive(Deserialize)]
struct Probed {
    #[serde(default)]
    streams: Vec<ProbedStream>,
    #[serde(default)]
    format: ProbedFormat,
}

#[derive(Deserialize)]
struct ProbedStream {
    #[serde(default)]
    codec_type: String,
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    sample_rate: Option<String>,
    #[serde(default)]
    channels: Option<u32>,
    #[serde(default)]
    bits_per_raw_sample: Option<String>,
    #[serde(default)]
    bits_per_sample: Option<serde_json::Value>,
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Deserialize, Default)]
struct ProbedFormat {
    #[serde(default)]
    duration: Option<String>,
}

/// Read the audio stream's properties.
pub fn probe(path: &Path) -> Result<StreamInfo> {
    let arguments = [
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_streams",
        "-show_format",
    ]
    .iter()
    .map(|argument| (*argument).to_owned())
    .chain([path.display().to_string()])
    .collect::<Vec<_>>();

    let stdout = run(&require("ffprobe")?, &arguments)?;
    let data: Probed = serde_json::from_slice(&stdout).map_err(|error| {
        Error::ExternalTool(format!("ffprobe returned no usable JSON: {error}"))
    })?;

    let stream = data
        .streams
        .iter()
        .find(|stream| stream.codec_type == "audio")
        .ok_or_else(|| Error::ExternalTool(format!("{}: no audio stream", path.display())))?;

    // ffprobe reports `bits_per_raw_sample` as a string and `bits_per_sample` as
    // a number; the Python takes the first that is present and non-zero.
    let raw_depth = stream
        .bits_per_raw_sample
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
        .or_else(|| {
            stream.bits_per_sample.as_ref().and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                    .and_then(|depth| u32::try_from(depth).ok())
            })
        })
        .filter(|depth| *depth != 0);

    let duration = data
        .format
        .duration
        .as_deref()
        .or(stream.duration.as_deref())
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|seconds| *seconds != 0.0);

    Ok(StreamInfo {
        sample_rate: stream
            .sample_rate
            .as_deref()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| Error::ExternalTool(format!("{}: no sample rate", path.display())))?,
        codec: stream.codec_name.clone(),
        channels: stream.channels.unwrap_or(1),
        bit_depth: raw_depth,
        duration_seconds: duration,
    })
}

/// Decode a window of the file to mono `f32` samples.
pub fn decode(
    path: &Path,
    sample_rate: u32,
    start_seconds: f64,
    duration_seconds: Option<f64>,
) -> Result<Vec<f32>> {
    let mut arguments = vec!["-v".to_owned(), "quiet".to_owned()];
    if start_seconds != 0.0 {
        arguments.extend(["-ss".to_owned(), format!("{start_seconds:.3}")]);
    }
    if let Some(seconds) = duration_seconds.filter(|seconds| *seconds != 0.0) {
        arguments.extend(["-t".to_owned(), format!("{seconds:.3}")]);
    }
    arguments.extend([
        "-i".to_owned(),
        path.display().to_string(),
        "-f".to_owned(),
        "f32le".to_owned(),
        "-ac".to_owned(),
        "1".to_owned(),
        "-ar".to_owned(),
        sample_rate.to_string(),
        "-".to_owned(),
    ]);

    let stdout = run(&require("ffmpeg")?, &arguments)?;
    Ok(stdout
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect())
}

/// Decode several windows spread across the track rather than the whole file.
///
/// A handful of samples is enough to characterise the spectrum, and decoding a
/// few minutes of every file in a library is not.
pub fn decode_probes(
    path: &Path,
    sample_rate: u32,
    total_seconds: Option<f64>,
    window_seconds: f64,
    probes: usize,
) -> Result<Vec<Vec<f32>>> {
    let mut chunks = Vec::new();
    for (start, length) in probe_windows(total_seconds, window_seconds, probes) {
        let chunk = decode(path, sample_rate, start, Some(length))?;
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
    }
    Ok(chunks)
}

fn probe_windows(
    total_seconds: Option<f64>,
    window_seconds: f64,
    probes: usize,
) -> Vec<(f64, f64)> {
    let total = total_seconds.filter(|seconds| *seconds != 0.0);
    let Some(total) = total.filter(|seconds| *seconds > window_seconds * probes as f64) else {
        let length = total.map_or(window_seconds, |seconds| (seconds - 0.5).max(1.0));
        return vec![(0.0, length)];
    };

    // The first and last tenth are skipped: intros and fade-outs are quiet and
    // would drag the averaged spectrum toward the noise floor.
    let margin = total * 0.1;
    let span = total - 2.0 * margin;
    let step = span / (probes.saturating_sub(1).max(1)) as f64;
    (0..probes)
        .map(|index| (margin + step * index as f64, window_seconds))
        .collect()
}

/// Scan a file for EBU R 128 loudness, returning the filter's summary.
///
/// The whole file is decoded rather than probed: a gain figure describes the
/// track, and the windows `decode_probes` takes for the spectral check would
/// answer a different question. `peak=true` costs an oversampling pass and
/// catches inter-sample peaks a sample scan misses, which is what a player
/// applying positive gain clips on.
pub fn ebur128(path: &Path, true_peak: bool) -> Result<String> {
    let peak = if true_peak { "true" } else { "sample" };
    let arguments = [
        "-hide_banner".to_owned(),
        "-nostats".to_owned(),
        "-i".to_owned(),
        path.display().to_string(),
        "-filter_complex".to_owned(),
        format!("ebur128=peak={peak}"),
        "-f".to_owned(),
        "null".to_owned(),
        "-".to_owned(),
    ];

    // The summary is logged at info level, alongside a running line per frame;
    // there is no level that keeps one and drops the other.
    let (_, stderr) = run_captured(&require("ffmpeg")?, &arguments)?;
    Ok(String::from_utf8_lossy(&stderr).into_owned())
}

/// Write a spectrogram image, for eyeballing a flagged file.
pub fn render_spectrogram(path: &Path, destination: &Path) -> Result<PathBuf> {
    let arguments = [
        "-v".to_owned(),
        "quiet".to_owned(),
        "-y".to_owned(),
        "-i".to_owned(),
        path.display().to_string(),
        "-lavfi".to_owned(),
        "showspectrumpic=s=1024x512:legend=1".to_owned(),
        destination.display().to_string(),
    ];
    run(&require("ffmpeg")?, &arguments)?;
    Ok(destination.to_path_buf())
}
