//! Shared test fixtures.
//!
//! Audio is built here, in Rust, wherever it can be: a signal, a WAV header or
//! a FLAC stream are all things a test can write, and generating them keeps the
//! suite runnable on a machine with nothing installed. What cannot be built
//! here is committed under `tests/data/audio` instead -- there is no pure-Rust
//! encoder for MP3, AAC, Vorbis or Opus, and the transcode detector needs audio
//! a real lossy encoder produced, because the brickwall it recognises is LAME's
//! and an authentic one cannot be synthesised. `scripts/make-fixtures.sh`
//! regenerates those files; nothing in the suite runs it.
//!
//! Ported from `py/tests/conftest.py`.
#![allow(
    dead_code,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    // `copy_tone` asks whether a caller left the defaults alone, which is an
    // exact question about two literals rather than a measurement.
    clippy::float_cmp
)]

use std::path::{Path, PathBuf};

use flacenc::component::BitRepr;
use flacenc::error::Verify;
use flacenc::source::MemSource;
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::ItemKey;
use lofty::tag::Tag;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Every generated fixture is stereo, because a bitrate is shared across the
/// pair: the same nominal 128 kbps applied to one channel barely low-passes at
/// all, and the transcode check would have nothing to find.
const CHANNELS: usize = 2;

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

/// A committed fixture, by name. Read it; never write to it.
#[must_use]
pub fn committed(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/audio")
        .join(name)
}

/// What a generated file should hold.
///
/// `modulation_hz` adds a slow amplitude sweep, giving the tone the loudness
/// dynamics a fingerprint needs and a steady sine lacks. `noise` writes
/// broadband noise instead, which is what the spectral check needs: a pure tone
/// has no high-frequency content to judge a cutoff by.
pub struct Encoded<'a> {
    pub seconds: f64,
    pub frequency: u32,
    pub sample_rate: u32,
    pub modulation_hz: f64,
    pub noise: bool,
    /// A scale applied to the whole signal, for a quieter copy of one master.
    pub gain: f32,
    pub metadata: &'a [(&'a str, &'a str)],
}

impl Default for Encoded<'_> {
    fn default() -> Self {
        Self {
            // The length of the committed tone, so a container this crate
            // cannot encode holds the same audio as one it can.
            seconds: 2.0,
            frequency: 440,
            sample_rate: 44100,
            modulation_hz: 0.0,
            noise: false,
            gain: 1.0,
            metadata: &[],
        }
    }
}

/// Write a file at `path`, holding what `options` describes and tagged with
/// its `metadata`.
///
/// FLAC and WAV are generated. Every other container is a copy of a committed
/// fixture, because there is no pure-Rust encoder for MP3, AAC, Vorbis or
/// Opus -- so for those the audio is whatever `scripts/make-fixtures.sh`
/// produced, and asking for anything else is a panic rather than a quiet
/// substitution.
pub fn encode(path: &Path, options: &Encoded<'_>) -> PathBuf {
    create_parent(path);
    match extension(path).as_deref() {
        Some("flac") => write_flac(path, &samples(options), options.sample_rate),
        Some("wav") => write_wav(path, &samples(options), options.sample_rate),
        Some(container) => copy_tone(path, container, options),
        None => panic!("{} names no container", path.display()),
    }
    write_metadata(path, options.metadata);
    path.to_path_buf()
}

/// The committed tone that stands in for an encoder.
fn copy_tone(destination: &Path, container: &str, options: &Encoded<'_>) {
    let tone = Encoded::default();
    assert!(
        options.seconds == tone.seconds
            && options.frequency == tone.frequency
            && options.sample_rate == tone.sample_rate
            && options.modulation_hz == tone.modulation_hz
            && options.noise == tone.noise
            && options.gain == tone.gain,
        "only the committed tone exists as {container}; generate a .flac or commit another fixture"
    );
    std::fs::copy(committed(&format!("tone.{container}")), destination)
        .expect("a committed fixture to copy");
}

/// Broadband noise: content all the way to Nyquist, like a real recording.
pub fn noise_source(destination: &Path, seconds: u32, sample_rate: u32) -> PathBuf {
    encode(
        destination,
        &Encoded {
            seconds: f64::from(seconds),
            sample_rate,
            noise: true,
            ..Encoded::default()
        },
    )
}

/// A file of digital silence.
pub fn silence(destination: &Path, seconds: u32, sample_rate: u32) -> PathBuf {
    encode(
        destination,
        &Encoded {
            seconds: f64::from(seconds),
            sample_rate,
            gain: 0.0,
            ..Encoded::default()
        },
    )
}

/// A committed MP3 re-encoded as FLAC -- a fake lossless file.
///
/// The MP3 is committed rather than encoded here because the point of it is
/// that LAME made it: the brickwall `audio/spectral.rs` recognises is an
/// artefact of a real encoder, and nothing synthesised carries one.
pub fn transcode_to_flac(scratch: &Scratch, name: &str, bitrate: &str) -> PathBuf {
    let source = committed(&format!("noise-{bitrate}.mp3"));
    let mut audio = stk::audio::decode::Audio::open(&source).expect("a decodable fixture");
    let sample_rate = audio.stream().sample_rate;

    let mut samples: Vec<f32> = Vec::new();
    audio
        .frames(&mut |frames: &[f32]| {
            samples.extend_from_slice(frames);
            Ok(())
        })
        .expect("a decoded fixture");

    let faked = scratch.join(&format!("{name}.flac"));
    write_flac(&faked, &samples, sample_rate);
    faked
}

/// Interleaved stereo samples, in `[-1, 1]`.
fn samples(options: &Encoded<'_>) -> Vec<f32> {
    let rate = f64::from(options.sample_rate);
    let frames = (options.seconds * rate) as usize;
    // Seeded, so a fixture is the same file on every run and on every machine.
    let mut rng = StdRng::seed_from_u64(20_260_907);

    let mut samples = Vec::with_capacity(frames * CHANNELS);
    for frame in 0..frames {
        let at = frame as f64 / rate;
        let value = if options.noise {
            rng.random_range(-0.5..0.5)
        } else {
            let envelope = if options.modulation_hz == 0.0 {
                1.0
            } else {
                0.55 + 0.45 * (std::f64::consts::TAU * options.modulation_hz * at).sin()
            };
            0.5 * (std::f64::consts::TAU * f64::from(options.frequency) * at).sin() * envelope
        };
        // The two channels carry the same signal: nothing here tests a stereo
        // image, and identical channels survive a downmix unchanged.
        let sample = value as f32 * options.gain;
        samples.extend([sample; CHANNELS]);
    }
    samples
}

/// The block size every generated FLAC frame carries.
///
/// The count of samples is padded up to a multiple of it, so that every frame
/// is the same length and STREAMINFO can state one block size rather than a
/// range. `flacenc` otherwise reports the short final frame as the stream's
/// minimum, and a reader that takes `min != max` to mean variable-blocksize
/// framing -- symphonia does -- then rejects every frame header in the file and
/// runs off the end looking for one it likes. The tail this adds is under a
/// tenth of a second of silence.
const BLOCK_SIZE: usize = 4096;

fn write_flac(path: &Path, samples: &[f32], sample_rate: u32) {
    let mut pcm: Vec<i32> = samples
        .iter()
        .map(|sample| i32::from(to_i16(*sample)))
        .collect();
    let frames = pcm.len() / CHANNELS;
    let whole = frames.div_ceil(BLOCK_SIZE) * BLOCK_SIZE;
    pcm.resize(whole * CHANNELS, 0);

    let source = MemSource::from_samples(&pcm, CHANNELS, 16, sample_rate as usize);
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .expect("a valid encoder configuration");
    let stream = flacenc::encode_with_fixed_block_size(&config, source, BLOCK_SIZE)
        .expect("an encoded FLAC stream");

    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).expect("a serialised FLAC stream");
    std::fs::write(path, with_padding(sink.as_slice())).expect("a written FLAC file");
}

/// A FLAC stream with room after its headers for a tag to be written into.
///
/// `flacenc` emits a stream whose only metadata block is STREAMINFO, marked as
/// the last one, and `lofty` then reports a successful write and changes
/// nothing -- so a fixture would carry neither the tags a test asked for nor
/// the cover another embedded. Reference encoders reserve 8 kB of padding for
/// exactly this; a kilobyte is enough here and is what makes these files
/// ordinary.
fn with_padding(stream: &[u8]) -> Vec<u8> {
    const PADDING: usize = 1024;
    assert_eq!(
        &stream[..4],
        b"fLaC",
        "a FLAC stream starts with its marker"
    );

    // Walk the metadata blocks to find where the audio frames begin, clearing
    // the last-block flag from whichever block claims it.
    let mut headers = Vec::new();
    let mut at = 4;
    loop {
        let last = stream[at] & 0x80 != 0;
        let length = usize::from(stream[at + 1]) << 16
            | usize::from(stream[at + 2]) << 8
            | usize::from(stream[at + 3]);
        headers.push(at);
        at += 4 + length;
        if last {
            break;
        }
    }

    let mut out = stream[..at].to_vec();
    out[*headers.last().expect("at least a STREAMINFO block")] &= 0x7f;
    out.extend([0x80 | 1, 0, (PADDING >> 8) as u8, 0]);
    out.extend(std::iter::repeat_n(0, PADDING));
    out.extend_from_slice(&stream[at..]);
    out
}

fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) {
    let spec = hound::WavSpec {
        channels: CHANNELS as u16,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("a WAV writer");
    for sample in samples {
        writer
            .write_sample(to_i16(*sample))
            .expect("a written sample");
    }
    writer.finalize().expect("a finished WAV file");
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

/// Write the fields a fixture is asked to carry.
///
/// Through `lofty` directly rather than through `stk`: several suites check
/// what `library/tags.rs` reads back, and a fixture tagged by the code under
/// test would agree with itself whatever it did. The containers whose tags are
/// read across formats carry tags ffmpeg wrote instead -- see `tagged.*` under
/// `tests/data/audio`.
fn write_metadata(path: &Path, metadata: &[(&str, &str)]) {
    if metadata.is_empty() {
        return;
    }

    let mut file = lofty::read_from_path(path).expect("a readable fixture");
    let kind = file.primary_tag_type();
    if file.tag(kind).is_none() {
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.tag_mut(kind).expect("a tag to write into");
    for (key, value) in metadata {
        tag.insert_text(item_key(key), (*value).to_owned());
    }
    file.save_to_path(path, WriteOptions::default())
        .expect("written metadata");
}

/// The four-letter names a command line used, as the keys `lofty` writes.
///
/// Anything else is passed through as it was written, which is what a fixture
/// carrying a `REPLAYGAIN_*` field this project does not manage needs.
fn item_key(key: &str) -> ItemKey {
    match key {
        "artist" => ItemKey::TrackArtist,
        "albumartist" => ItemKey::AlbumArtist,
        "album" => ItemKey::AlbumTitle,
        "title" => ItemKey::TrackTitle,
        "genre" => ItemKey::Genre,
        "track" => ItemKey::TrackNumber,
        "REPLAYGAIN_TRACK_GAIN" => ItemKey::ReplayGainTrackGain,
        "REPLAYGAIN_TRACK_PEAK" => ItemKey::ReplayGainTrackPeak,
        "REPLAYGAIN_ALBUM_GAIN" => ItemKey::ReplayGainAlbumGain,
        "REPLAYGAIN_ALBUM_PEAK" => ItemKey::ReplayGainAlbumPeak,
        other => ItemKey::Unknown(other.to_owned()),
    }
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
}

fn create_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("a parent directory");
    }
}
