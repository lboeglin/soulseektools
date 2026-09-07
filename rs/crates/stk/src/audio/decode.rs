//! Reading audio out of a file.
//!
//! One open file rather than a function per job: every caller wants the stream
//! properties and the samples from the same parse, and the check that samples a
//! few windows of a long track wants several reads from one open. What this
//! replaces spawned ffprobe once and ffmpeg once per window -- five processes
//! for a single `verify` -- which is what made it the most expensive command in
//! the tool.
//!
//! Nothing here resamples. `verify` analyses at the file's own rate, a loudness
//! scan has to measure the channels as they were mastered, and chromaprint does
//! its own resampling, so a resampler would have no caller.
// Frame counts and window offsets are cast between `usize`, `u64` and `f64`
// throughout. Every one of them is bounded by a track length rather than an
// address space.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::path::Path;

use symphonia::core::audio::conv::ConvertibleSample;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3,
    CODEC_ID_PCM_F32BE, CODEC_ID_PCM_F32LE, CODEC_ID_PCM_S16BE, CODEC_ID_PCM_S16LE,
    CODEC_ID_PCM_S24BE, CODEC_ID_PCM_S24LE, CODEC_ID_PCM_S32BE, CODEC_ID_PCM_S32LE,
    CODEC_ID_PCM_S8, CODEC_ID_PCM_U8, CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{
    AudioCodecId, AudioCodecParameters, AudioDecoder, AudioDecoderOptions,
};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as Symphonia;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;
use symphonia_common::apple::audio::alac::MagicCookie;

use crate::errors::{Error, Result};

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

/// An open audio file, decoded on demand.
pub struct Audio {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    stream: StreamInfo,
    /// Whether anything has been read yet, which is what decides if a window
    /// starting at zero has to seek at all.
    read: bool,
}

impl Audio {
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .map_err(|error| Error::Audio(format!("{}: {error}", path.display())))?;
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(std::ffi::OsStr::to_str) {
            hint.with_extension(extension);
        }

        let source = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                source,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|error| failure(path, &error))?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| Error::Audio(format!("{}: no audio stream", path.display())))?;
        let track_id = track.id;
        let frames = track.num_frames;
        let parameters = track
            .codec_params
            .as_ref()
            .and_then(CodecParameters::audio)
            .ok_or_else(|| unsupported(path))?
            .clone();

        let sample_rate = parameters
            .sample_rate
            .ok_or_else(|| Error::Audio(format!("{}: no sample rate", path.display())))?;
        if parameters.codec == CODEC_ID_FLAC
            && parameters.bits_per_sample.is_some_and(|depth| depth > 24)
        {
            return Err(Error::UnsupportedFormat(format!(
                "{}: 32-bit FLAC, which the decoder gets wrong",
                path.display()
            )));
        }
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&parameters, &AudioDecoderOptions::default())
            .map_err(|_| unsupported(path))?;

        let bit_depth = parameters
            .bits_per_sample
            .or_else(|| alac_bit_depth(&parameters))
            .filter(|depth| *depth != 0);

        Ok(Self {
            format,
            decoder,
            track_id,
            stream: StreamInfo {
                sample_rate,
                codec: codec_name(parameters.codec).to_owned(),
                channels: parameters
                    .channels
                    .map_or(1, |channels| channels.count() as u32),
                bit_depth,
                duration_seconds: frames
                    .map(|frames| frames as f64 / f64::from(sample_rate))
                    .filter(|seconds| *seconds != 0.0),
            },
            read: false,
        })
    }

    #[must_use]
    pub fn stream(&self) -> &StreamInfo {
        &self.stream
    }

    /// Hand every frame to `on_frames`, interleaved, at the file's own rate and
    /// channel layout.
    ///
    /// Streamed rather than collected: a four-minute stereo track is 85 MB of
    /// `f32`, and a whole record is scanned at once across the thread pool, so
    /// returning a `Vec` would peak in the gigabytes on an ordinary album.
    ///
    /// The sample type is the caller's, because it decides what a figure means:
    /// a loudness scan works in `f32`, and chromaprint is defined over the
    /// `i16` samples a 16-bit rip already holds, where a detour through `f32`
    /// would round.
    pub fn frames<S: ConvertibleSample>(
        &mut self,
        on_frames: &mut dyn FnMut(&[S]) -> Result<()>,
    ) -> Result<()> {
        let mut buffer = Vec::new();
        while self.next_frames(&mut buffer)?.is_some() {
            if !buffer.is_empty() {
                on_frames(&buffer)?;
            }
        }
        Ok(())
    }

    /// Mono samples from one window of the track, at the file's own rate.
    ///
    /// The channels are summed and divided by the square root of their count,
    /// which is `ffmpeg -ac 1`: swresample's stereo downmix is `1/sqrt(2)` per
    /// channel, preserving power rather than amplitude. Averaging instead is
    /// 3 dB quieter, and while a constant gain cannot move a spectrum
    /// normalised to its own peak, `spectral.rs` floors magnitudes at an
    /// absolute `1e-12` before taking their logarithm -- so above a brickwall,
    /// where the content really is nothing, the two disagree by a tenth of a
    /// dB/kHz. `oracle/verify.csv` was captured through ffmpeg.
    ///
    /// That it is a sum and not one channel picked matters for its own reason:
    /// out-of-phase high frequencies cancel in a real downmix, and the spectrum
    /// being judged has to be the one a listener would hear.
    pub fn window(&mut self, start_seconds: f64, seconds: f64) -> Result<Vec<f32>> {
        let wanted = (seconds * f64::from(self.stream.sample_rate))
            .round()
            .max(0.0) as usize;
        let channels = (self.stream.channels as usize).max(1);
        let gain = 1.0 / (channels as f32).sqrt();

        let mut skip = self.seek(start_seconds)?;
        let mut samples = Vec::with_capacity(wanted);
        let mut buffer: Vec<f32> = Vec::new();
        while samples.len() < wanted {
            if self.next_frames(&mut buffer)?.is_none() {
                break;
            }
            let dropped = skip.min(buffer.len() / channels);
            skip -= dropped;
            samples.extend(
                buffer[dropped * channels..]
                    .chunks_exact(channels)
                    .take(wanted - samples.len())
                    .map(|frame| frame.iter().sum::<f32>() * gain),
            );
        }
        Ok(samples)
    }

    /// Position the reader at `start_seconds`, returning the frames the next
    /// decode has to drop to land exactly there.
    fn seek(&mut self, start_seconds: f64) -> Result<usize> {
        if start_seconds <= 0.0 && !self.read {
            return Ok(0);
        }

        let seconds = start_seconds.max(0.0);
        let time =
            Time::try_new(seconds as i64, (seconds.fract() * 1e9) as u32).unwrap_or(Time::ZERO);
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|error| Error::Audio(error.to_string()))?;
        self.decoder.reset();
        self.read = true;

        // An accurate seek lands at or before the frame asked for and reports
        // both, so the difference is what would otherwise be heard as a window
        // starting early.
        Ok(usize::try_from(seeked.required_ts.get() - seeked.actual_ts.get()).unwrap_or(0))
    }

    /// Decode the next packet into `buffer`, or report the end of the stream.
    ///
    /// A packet that will not decode is skipped rather than fatal, as it is for
    /// ffmpeg: one corrupt frame in the middle of a track is not a reason to
    /// have no verdict for it.
    fn next_frames<S: ConvertibleSample>(&mut self, buffer: &mut Vec<S>) -> Result<Option<()>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(None),
                Err(Symphonia::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(None)
                }
                Err(error) => return Err(Error::Audio(error.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }

            self.read = true;
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    decoded.copy_to_vec_interleaved(buffer);
                    return Ok(Some(()));
                }
                Err(Symphonia::DecodeError(_)) => {}
                Err(Symphonia::ResetRequired) => self.decoder.reset(),
                Err(error) => return Err(Error::Audio(error.to_string())),
            }
        }
    }
}

/// Windows spread across a track, rather than the whole of it.
///
/// A handful of samples is enough to characterise a spectrum, and decoding a
/// few minutes of every file in a library is not.
#[must_use]
pub fn probe_windows(
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
        .map(|index| {
            (
                to_milliseconds(margin + step * index as f64),
                window_seconds,
            )
        })
        .collect()
}

/// Round an offset to the millisecond the report was always built from.
///
/// The offsets used to reach a decoder as `-ss {:.3}` on a command line, so
/// that is the resolution every stored verdict and `oracle/verify.csv` were
/// produced at. A third of a millisecond is fourteen samples, which is nothing
/// to listen to and enough to move the averaged spectrum of a file whose
/// content trails off gently across the noise floor: two of the reference
/// library's 258 files change band without it.
fn to_milliseconds(seconds: f64) -> f64 {
    format!("{seconds:.3}").parse().unwrap_or(seconds)
}

/// ffprobe's spelling of a codec, which is what a report carries.
///
/// Deliberately not [`crate::library::tags`]'s mapping: that one answers what
/// the index should call a container, where this one has to keep saying `alac`
/// where ffprobe said `alac`, since an old report is diffed against a new one.
fn codec_name(codec: AudioCodecId) -> &'static str {
    match codec {
        CODEC_ID_FLAC => "flac",
        CODEC_ID_ALAC => "alac",
        CODEC_ID_AAC => "aac",
        CODEC_ID_MP1 => "mp1",
        CODEC_ID_MP2 => "mp2",
        CODEC_ID_MP3 => "mp3",
        CODEC_ID_VORBIS => "vorbis",
        CODEC_ID_PCM_S8 => "pcm_s8",
        CODEC_ID_PCM_U8 => "pcm_u8",
        CODEC_ID_PCM_S16LE => "pcm_s16le",
        CODEC_ID_PCM_S16BE => "pcm_s16be",
        CODEC_ID_PCM_S24LE => "pcm_s24le",
        CODEC_ID_PCM_S24BE => "pcm_s24be",
        CODEC_ID_PCM_S32LE => "pcm_s32le",
        CODEC_ID_PCM_S32BE => "pcm_s32be",
        CODEC_ID_PCM_F32LE => "pcm_f32le",
        CODEC_ID_PCM_F32BE => "pcm_f32be",
        _ => "",
    }
}

/// What a container this crate has no decoder for looks like to a caller.
///
/// `.ape`, `.wv` and `.opus` reach this: they can be read as tags and cannot be
/// read as audio, and saying so is the honest answer for a `verify` or a
/// loudness scan pointed at one. So does 32-bit FLAC, which symphonia accepts
/// and then decorrelates through an `i32` too narrow to hold `2*mid + side`:
/// it panics on a debug build and returns noise on a release one, so it is
/// refused rather than believed.
fn unsupported(path: &Path) -> Error {
    Error::UnsupportedFormat(format!("{}: no decoder for this container", path.display()))
}

/// The depth an ALAC stream states in its own header.
///
/// ALAC keeps it in the magic cookie rather than in the container, symphonia's
/// decoder reads that cookie and does not copy the depth back onto the codec
/// parameters, and `oracle/verify.csv` has a row carrying the number ffprobe
/// read out of the same bytes.
fn alac_bit_depth(parameters: &AudioCodecParameters) -> Option<u32> {
    if parameters.codec != CODEC_ID_ALAC {
        return None;
    }
    let cookie = MagicCookie::read(parameters.extra_data.as_ref()?).ok()?;
    Some(u32::from(cookie.bit_depth))
}

fn failure(path: &Path, error: &Symphonia) -> Error {
    match error {
        Symphonia::Unsupported(_) => unsupported(path),
        other => Error::Audio(format!("{}: {other}", path.display())),
    }
}
