//! Shared test fixtures.
//!
//! Audio fixtures are generated with ffmpeg rather than committed as binaries,
//! so the suites exercise real containers written by an independent encoder
//! instead of files this crate produced itself. Ported from
//! `py/tests/conftest.py`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A temporary directory that removes itself.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    #[must_use]
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "stk-test-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self { path }
    }

    #[must_use]
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Whether ffmpeg is on PATH.
///
/// Rust has no runtime skip, so tests needing it return early and say so rather
/// than failing on a machine without it. CI installs ffmpeg, so the coverage is
/// not quietly lost.
#[must_use]
pub fn ffmpeg() -> Option<PathBuf> {
    which::which("ffmpeg").ok()
}

/// Run ffmpeg, failing loudly with its stderr.
pub fn run_ffmpeg(arguments: &[&str]) {
    let ffmpeg = ffmpeg().expect("ffmpeg on PATH");
    let output = Command::new(ffmpeg)
        .args(["-v", "error", "-y"])
        .args(arguments)
        .output()
        .expect("ffmpeg runs");
    assert!(
        output.status.success(),
        "ffmpeg {arguments:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Broadband noise: content all the way to Nyquist, like a real recording.
///
/// Stereo, because a bitrate is shared across the pair: the same nominal
/// 128 kbps applied to one channel barely low-passes at all.
pub fn noise_source(destination: &Path, seconds: u32, sample_rate: u32) -> PathBuf {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).expect("a parent directory");
    }
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("anoisesrc=d={seconds}:c=white:r={sample_rate}"),
        "-ac",
        "2",
        &destination.display().to_string(),
    ]);
    destination.to_path_buf()
}

/// Encode to MP3 and back to FLAC -- a fake lossless file.
pub fn transcode_to_flac(source: &Path, scratch: &Scratch, name: &str, bitrate: &str) -> PathBuf {
    let lossy = scratch.join(&format!("{name}.mp3"));
    run_ffmpeg(&[
        "-i",
        &source.display().to_string(),
        "-c:a",
        "libmp3lame",
        "-b:a",
        bitrate,
        "-ac",
        "2",
        &lossy.display().to_string(),
    ]);

    let faked = scratch.join(&format!("{name}.flac"));
    run_ffmpeg(&[
        "-i",
        &lossy.display().to_string(),
        "-c:a",
        "flac",
        &faked.display().to_string(),
    ]);
    faked
}

/// A file of digital silence.
pub fn silence(destination: &Path, seconds: u32, sample_rate: u32) -> PathBuf {
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("anullsrc=r={sample_rate}:d={seconds}"),
        &destination.display().to_string(),
    ]);
    destination.to_path_buf()
}

/// Write a short test tone to `path`, tagged with `metadata`.
///
/// `modulation_hz` adds a slow amplitude sweep, giving the tone the loudness
/// dynamics that fingerprinting needs and a steady sine lacks. `noise` writes
/// broadband noise instead, which is what the spectral check needs: a pure tone
/// has no high-frequency content to judge a cutoff by.
pub struct Encoded<'a> {
    pub seconds: f64,
    pub frequency: u32,
    pub sample_rate: u32,
    pub modulation_hz: f64,
    pub noise: bool,
    pub metadata: &'a [(&'a str, &'a str)],
}

impl Default for Encoded<'_> {
    fn default() -> Self {
        Self {
            seconds: 1.0,
            frequency: 440,
            sample_rate: 44100,
            modulation_hz: 0.0,
            noise: false,
            metadata: &[],
        }
    }
}

pub fn encode(path: &Path, options: &Encoded<'_>) -> PathBuf {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("a parent directory");
    }

    let Encoded {
        seconds,
        frequency,
        sample_rate,
        modulation_hz,
        noise,
        metadata,
    } = *options;

    let source = if noise {
        format!("anoisesrc=d={seconds}:c=white:r={sample_rate}")
    } else if modulation_hz != 0.0 {
        format!(
            "aevalsrc=0.5*sin(2*PI*{frequency}*t)*\
             (0.55+0.45*sin(2*PI*{modulation_hz}*t)):d={seconds}:s={sample_rate}"
        )
    } else {
        format!("sine=frequency={frequency}:duration={seconds}:sample_rate={sample_rate}")
    };

    let codec: &[&str] = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("flac") => &["-c:a", "flac"],
        Some("mp3") => &["-c:a", "libmp3lame", "-b:a", "192k"],
        Some("m4a") => &["-c:a", "aac", "-b:a", "192k"],
        Some("ogg") => &["-c:a", "libvorbis", "-q:a", "4"],
        Some("opus") => &["-c:a", "libopus", "-b:a", "128k"],
        Some("wav") => &["-c:a", "pcm_s16le"],
        other => panic!("no encoder for {other:?}"),
    };

    let mut arguments: Vec<String> = ["-f", "lavfi", "-i"]
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect();
    arguments.push(source);
    arguments.extend(["-ac".to_owned(), "2".to_owned()]);
    arguments.extend(codec.iter().map(|argument| (*argument).to_owned()));
    for (key, value) in metadata {
        arguments.extend(["-metadata".to_owned(), format!("{key}={value}")]);
    }
    arguments.push(path.display().to_string());

    let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
    run_ffmpeg(&borrowed);
    path.to_path_buf()
}
