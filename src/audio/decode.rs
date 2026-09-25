//! Reading audio out of a file, at its own rate and channel layout.
// Casts between frame counts and offsets are bounded by a track length.
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

pub struct Audio {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    stream: StreamInfo,
    read: bool,
    /// ORed samples, left-aligned in 32 bits, for [`Audio::bits_used`]; kept only above 16 bits.
    sample_bits: Option<i64>,
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
            sample_bits: bit_depth.filter(|depth| *depth > 16).map(|_| 0),
        })
    }

    #[must_use]
    pub fn stream(&self) -> &StreamInfo {
        &self.stream
    }

    /// A 16-bit master padded to 24 leaves the low 8 bits zero. `None` for 16 bits or fewer.
    #[must_use]
    pub fn bits_used(&self) -> Option<u32> {
        let bits = self.sample_bits.filter(|bits| *bits != 0)?;
        let depth = self.stream.bit_depth?;
        Some((32 - bits.trailing_zeros()).min(depth))
    }

    /// Streamed: a whole album decoded at once would run to gigabytes.
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

    /// Summed and scaled by `1/sqrt(count)` as `ffmpeg -ac 1` does; the thresholds were measured so.
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
            if let Some(bits) = self.sample_bits.as_mut() {
                *bits = buffer[dropped * channels..]
                    .iter()
                    .fold(*bits, |bits, sample| {
                        bits | (f64::from(*sample) * 2_147_483_648.0) as i64
                    });
            }
            samples.extend(
                buffer[dropped * channels..]
                    .chunks_exact(channels)
                    .take(wanted - samples.len())
                    .map(|frame| frame.iter().sum::<f32>() * gain),
            );
        }
        Ok(samples)
    }

    /// Returns the frames the next decode has to drop to land exactly there.
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

        // An accurate seek may land early; skip the difference.
        Ok(usize::try_from(seeked.required_ts.get() - seeked.actual_ts.get()).unwrap_or(0))
    }

    /// A packet that won't decode is skipped, not fatal.
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

/// Stored verdicts were made at millisecond resolution; finer shifts borderline files.
fn to_milliseconds(seconds: f64) -> f64 {
    format!("{seconds:.3}").parse().unwrap_or(seconds)
}

/// ffprobe's spelling, as `verify` reports carry it.
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

/// 32-bit FLAC is refused: symphonia decorrelates it through a too-narrow `i32` and returns noise.
fn unsupported(path: &Path) -> Error {
    Error::UnsupportedFormat(format!("{}: no decoder for this container", path.display()))
}

/// symphonia reads ALAC's magic cookie but doesn't copy the depth onto the codec parameters.
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
