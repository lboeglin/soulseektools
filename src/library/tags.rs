//! Every tag read and write goes through this module, so a container format is
//! supported everywhere or nowhere.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::{ItemValue, Tag, TagItem, TagType};
use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};
use crate::models::{Artwork, ReplayGainTags, TrackTags};

/// Containers whose tags this module can read and write.
pub const TAGGABLE_EXTENSIONS: &[&str] = crate::library::scan::TAGGABLE_EXTENSIONS;

fn read_file(path: &Path) -> Result<TaggedFile> {
    if !path.is_file() {
        return Err(Error::Tag(format!("{}: no such file", path.display())));
    }
    lofty::read_from_path(path).map_err(|error| match error.kind() {
        lofty::error::ErrorKind::UnknownFormat => {
            Error::UnsupportedFormat(format!("{}: not a recognised audio file", path.display()))
        }
        _ => Error::Tag(format!("{}: {error}", path.display())),
    })
}

/// The tag block a reader should consult: the primary one, else whatever exists.
fn first_tag(file: &TaggedFile) -> Option<&Tag> {
    file.primary_tag().or_else(|| file.first_tag())
}

/// Read the metadata fields this project cares about.
pub fn read_tags(path: &Path) -> Result<TrackTags> {
    Ok(tags_of(&read_file(path)?))
}

fn tags_of(file: &TaggedFile) -> TrackTags {
    let Some(tag) = first_tag(file) else {
        return TrackTags::default();
    };

    let track_number = tag
        .get_string(&ItemKey::TrackNumber)
        .and_then(leading_number);

    TrackTags {
        artist: tag.artist().unwrap_or_default().into_owned(),
        album_artist: tag
            .get_string(&ItemKey::AlbumArtist)
            .unwrap_or_default()
            .to_owned(),
        album: tag.album().unwrap_or_default().into_owned(),
        title: tag.title().unwrap_or_default().into_owned(),
        genres: tag
            .get_strings(&ItemKey::Genre)
            .filter(|genre| !genre.is_empty())
            .map(str::to_owned)
            .collect(),
        track_number,
    }
}

/// The number a track-number field states (`3` or `3/12`), or `None`.
#[must_use]
pub fn leading_number(value: &str) -> Option<i64> {
    value
        .split('/')
        .next()
        .map(str::trim)
        .filter(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        .and_then(|digits| digits.parse().ok())
}

/// The track length in seconds, from the container header.
pub fn read_duration(path: &Path) -> Result<Option<f64>> {
    let file = read_file(path)?;
    let seconds = file.properties().duration().as_secs_f64();
    Ok((seconds != 0.0).then_some(seconds))
}

/// Codec, bitrate, sample rate and bit depth, from the container header.
pub fn read_stream_info(path: &Path) -> Result<StreamInfo> {
    Ok(stream_info_of(&read_file(path)?))
}

/// Whether an MP3 was encoded at a variable bitrate.
///
/// A VBR stream opens with a `Xing` or `VBRI` frame; `Info` (the same frame)
/// or no frame means CBR. lofty parses the frame but doesn't expose which.
#[must_use]
pub fn mp3_is_vbr(path: &Path) -> bool {
    use std::io::Read as _;

    const ID3_HEADER: usize = 10;
    const FRAME_HEADER_SPAN: usize = 64;

    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = vec![0; ID3_HEADER];
    if file.read_exact(&mut head).is_err() {
        return false;
    }
    let skipped = if head.starts_with(b"ID3") {
        let size = head[6..10]
            .iter()
            .fold(0_usize, |size, byte| size << 7 | usize::from(byte & 0x7f));
        ID3_HEADER + size
    } else {
        0
    };

    let mut rest = Vec::new();
    if file
        .take((skipped + 4096) as u64)
        .read_to_end(&mut rest)
        .is_err()
    {
        return false;
    }
    head.extend(rest);

    let Some(sync) = head.get(skipped..).and_then(|audio| {
        audio
            .windows(2)
            .position(|pair| pair[0] == 0xff && pair[1] & 0xe0 == 0xe0)
    }) else {
        return false;
    };
    let start = skipped + sync;
    let frame = &head[start..head.len().min(start + FRAME_HEADER_SPAN)];
    frame
        .windows(4)
        .any(|marker| marker == b"Xing" || marker == b"VBRI")
}

fn stream_info_of(file: &TaggedFile) -> StreamInfo {
    let properties = file.properties();
    StreamInfo {
        codec: codec_name(file.file_type()),
        bitrate: properties.audio_bitrate(),
        sample_rate: properties.sample_rate(),
        bit_depth: properties.bit_depth().map(u32::from),
        channels: properties.channels().map(u32::from),
        duration_seconds: {
            let seconds = properties.duration().as_secs_f64();
            (seconds != 0.0).then_some(seconds)
        },
    }
}

/// What a container's header says about its audio stream.
///
/// `channels` isn't stored in the index; the loudness scan needs it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamInfo {
    pub codec: String,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub duration_seconds: Option<f64>,
}

fn codec_name(file_type: lofty::file::FileType) -> String {
    use lofty::file::FileType;
    match file_type {
        FileType::Flac => "flac",
        FileType::Mpeg => "mp3",
        FileType::Mp4 => "aac",
        FileType::Vorbis => "vorbis",
        FileType::Opus => "opus",
        FileType::Wav => "wav",
        FileType::Aiff => "aiff",
        FileType::Ape => "ape",
        FileType::WavPack => "wavpack",
        FileType::Speex => "speex",
        FileType::Mpc => "musepack",
        _ => "",
    }
    .to_owned()
}

/// Everything the index stores about a file, from one parse of its header
/// rather than one per reader.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Described {
    pub tags: TrackTags,
    pub stream: StreamInfo,
    pub replay_gain: ReplayGainTags,
    pub artwork_bytes: u64,
    /// Whether this container keeps an `R128_*` field alongside the rest.
    pub r128: bool,
}

pub fn describe_file(path: &Path) -> Result<Described> {
    let file = read_file(path)?;
    let artwork_bytes = if require_artwork_container(path).is_ok() {
        artwork_of(&file).map_or(0, |artwork| artwork.data.len() as u64)
    } else {
        0
    };
    Ok(Described {
        tags: tags_of(&file),
        stream: stream_info_of(&file),
        replay_gain: replay_gain_of(&file),
        artwork_bytes,
        r128: carries_r128(&file),
    })
}

/// A metadata field this project can write, and put back. Each is a
/// `ChangeField` in the write log, so every write can be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagField {
    Artist,
    AlbumArtist,
    Album,
    Title,
    TrackNumber,
    Genres,
}

impl TagField {
    pub const ALL: [Self; 6] = [
        Self::Artist,
        Self::AlbumArtist,
        Self::Album,
        Self::Title,
        Self::TrackNumber,
        Self::Genres,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::AlbumArtist => "album_artist",
            Self::Album => "album",
            Self::Title => "title",
            Self::TrackNumber => "track_number",
            Self::Genres => "genres",
        }
    }

    /// The field's value as a dump spells it.
    #[must_use]
    pub fn value(self, tags: &TrackTags) -> String {
        match self {
            Self::Artist => tags.artist.clone(),
            Self::AlbumArtist => tags.album_artist.clone(),
            Self::Album => tags.album.clone(),
            Self::Title => tags.title.clone(),
            Self::TrackNumber => tags
                .track_number
                .map(|number| number.to_string())
                .unwrap_or_default(),
            Self::Genres => tags.genres.join(GENRE_SEPARATOR),
        }
    }

    /// Put a dumped value back. An empty one means the field carries nothing.
    pub fn set(self, tags: &mut TrackTags, value: &str) {
        let value = value.trim();
        match self {
            Self::Artist => value.clone_into(&mut tags.artist),
            Self::AlbumArtist => value.clone_into(&mut tags.album_artist),
            Self::Album => value.clone_into(&mut tags.album),
            Self::Title => value.clone_into(&mut tags.title),
            Self::TrackNumber => tags.track_number = value.parse().ok(),
            Self::Genres => {
                tags.genres = value
                    .split(GENRE_SEPARATOR.trim())
                    .map(str::trim)
                    .filter(|genre| !genre.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
        }
    }
}

impl fmt::Display for TagField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TagField {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|field| field.as_str() == text)
            .ok_or_else(|| format!("unknown tag field: {text}"))
    }
}

/// How several genres share one cell of a dump.
pub const GENRE_SEPARATOR: &str = "; ";

/// Replace every metadata field this project writes.
///
/// Total rather than a patch, so it round-trips with [`read_tags`]. An empty
/// value removes the key. Embedded pictures are left alone.
pub fn write_tags(path: &Path, tags: &TrackTags) -> Result<()> {
    let mut file = read_file(path)?;
    let tag_type = existing_or_default_tag_type(&file);
    let tag = tag_for_writing(&mut file, tag_type);

    for (key, value) in [
        (ItemKey::TrackArtist, tags.artist.as_str()),
        (ItemKey::AlbumArtist, tags.album_artist.as_str()),
        (ItemKey::AlbumTitle, tags.album.as_str()),
        (ItemKey::TrackTitle, tags.title.as_str()),
    ] {
        tag.remove_key(&key);
        if !value.is_empty() {
            tag.push(lofty::tag::TagItem::new(
                key,
                lofty::tag::ItemValue::Text(value.to_owned()),
            ));
        }
    }

    // Only the number: a total is a separate `TrackTotal` item, left alone.
    tag.remove_key(&ItemKey::TrackNumber);
    if let Some(number) = tags.track_number {
        tag.push(lofty::tag::TagItem::new(
            ItemKey::TrackNumber,
            lofty::tag::ItemValue::Text(number.to_string()),
        ));
    }

    tag.remove_key(&ItemKey::Genre);
    let genres: Vec<&String> = tags
        .genres
        .iter()
        .filter(|genre| !genre.is_empty())
        .collect();
    for value in genre_items(tag_type, &genres) {
        tag.push(lofty::tag::TagItem::new(
            ItemKey::Genre,
            lofty::tag::ItemValue::Text(value),
        ));
    }

    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
}

/// Replace the genre tag, creating a tag block if the file has none.
pub fn write_genres(path: &Path, genres: &[String]) -> Result<()> {
    let values: Vec<&String> = genres.iter().filter(|genre| !genre.is_empty()).collect();
    let mut file = read_file(path)?;
    let tag_type = existing_or_default_tag_type(&file);
    let tag = tag_for_writing(&mut file, tag_type);
    tag.remove_key(&ItemKey::Genre);
    for value in genre_items(tag_type, &values) {
        tag.push(lofty::tag::TagItem::new(
            ItemKey::Genre,
            lofty::tag::ItemValue::Text(value),
        ));
    }

    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
}

/// A tag item outside the fields this project manages: a comment, an encoder
/// name, a URL, lyrics -- whatever else a peer's tagger left in the block.
///
/// `key` is spelled the way the file's tag format spells it (`COMM`, `ENCODER`,
/// `©too`), so it can be put back by name. `locator` marks an ID3v2/APE URL item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extra {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub locator: bool,
}

/// Keys that are never extras: the managed fields, plus the encoder fields,
/// which on Vorbis are the vendor string and can't be stripped.
fn is_kept(key: &ItemKey) -> bool {
    matches!(
        key,
        ItemKey::EncoderSoftware
            | ItemKey::EncoderSettings
            | ItemKey::TrackArtist
            | ItemKey::AlbumArtist
            | ItemKey::AlbumTitle
            | ItemKey::TrackTitle
            | ItemKey::TrackNumber
            | ItemKey::TrackTotal
            | ItemKey::DiscNumber
            | ItemKey::DiscTotal
            | ItemKey::Genre
    ) || REPLAY_GAIN_KEYS.contains(key)
        || *key == unknown_key(R128_TRACK_GAIN)
        || *key == unknown_key(R128_ALBUM_GAIN)
}

/// Every text item a file carries that this project does not manage.
///
/// Binary items are skipped. Embedded pictures are never touched.
pub fn read_extras(path: &Path) -> Result<Vec<Extra>> {
    Ok(extras_of(&read_file(path)?))
}

fn extras_of(file: &TaggedFile) -> Vec<Extra> {
    let Some(tag) = first_tag(file) else {
        return Vec::new();
    };
    let tag_type = tag.tag_type();
    tag.items()
        .filter(|item| !is_kept(item.key()))
        .filter_map(|item| {
            let (value, locator) = match item.value() {
                ItemValue::Text(text) => (text, false),
                ItemValue::Locator(url) => (url, true),
                ItemValue::Binary(_) => return None,
            };
            Some(Extra {
                key: spell_key(item.key(), tag_type),
                value: value.clone(),
                locator,
            })
        })
        .collect()
}

fn spell_key(key: &ItemKey, tag_type: TagType) -> String {
    match key {
        ItemKey::Unknown(name) => name.clone(),
        mapped => mapped
            .map_key(tag_type, true)
            .map_or_else(|| format!("{mapped:?}"), str::to_owned),
    }
}

/// Replace every unmanaged text item with `extras`, so an empty list strips a
/// peer's comment, encoder and URL fields and leaves nothing else changed.
///
/// Total rather than a patch, so an undo puts back exactly what was logged.
pub fn write_extras(path: &Path, extras: &[Extra]) -> Result<()> {
    let mut file = read_file(path)?;
    let tag_type = existing_or_default_tag_type(&file);
    let tag = tag_for_writing(&mut file, tag_type);
    tag.retain(|item| is_kept(item.key()));
    for extra in extras {
        let value = if extra.locator {
            ItemValue::Locator(extra.value.clone())
        } else {
            ItemValue::Text(extra.value.clone())
        };
        tag.push_unchecked(TagItem::new(ItemKey::from_key(tag_type, &extra.key), value));
    }

    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
}

/// The keys the four ReplayGain figures live under. lofty maps each for every
/// tag format.
const REPLAY_GAIN_KEYS: [ItemKey; 4] = [
    ItemKey::ReplayGainTrackGain,
    ItemKey::ReplayGainTrackPeak,
    ItemKey::ReplayGainAlbumGain,
    ItemKey::ReplayGainAlbumPeak,
];

/// The Opus spelling, which is a key `lofty` has no variant for.
const R128_TRACK_GAIN: &str = "R128_TRACK_GAIN";
const R128_ALBUM_GAIN: &str = "R128_ALBUM_GAIN";

fn unknown_key(key: &str) -> ItemKey {
    ItemKey::Unknown(key.to_owned())
}

/// Whether a container keeps its gain as an `R128_*` header field.
///
/// Opus does, relative to a fixed -23 LUFS; both spellings are written.
/// Decided on the container, not the suffix, because Opus is often named `.ogg`.
fn carries_r128(file: &TaggedFile) -> bool {
    file.file_type() == lofty::file::FileType::Opus
}

/// The loudness tags a file already carries, if any.
pub fn read_replay_gain(path: &Path) -> Result<ReplayGainTags> {
    Ok(replay_gain_of(&read_file(path)?))
}

fn replay_gain_of(file: &TaggedFile) -> ReplayGainTags {
    let Some(tag) = first_tag(file) else {
        return ReplayGainTags::default();
    };
    let read = |key: &ItemKey| tag.get_string(key).map(str::to_owned);
    ReplayGainTags {
        track_gain: read(&ItemKey::ReplayGainTrackGain),
        track_peak: read(&ItemKey::ReplayGainTrackPeak),
        album_gain: read(&ItemKey::ReplayGainAlbumGain),
        album_peak: read(&ItemKey::ReplayGainAlbumPeak),
        r128_track_gain: read(&unknown_key(R128_TRACK_GAIN)),
        r128_album_gain: read(&unknown_key(R128_ALBUM_GAIN)),
    }
}

/// Replace every loudness field, removing the ones the caller left absent.
///
/// Total rather than a patch, so an undo never restores a gain without its
/// peak. Nothing else in the tag block is touched.
pub fn write_replay_gain(path: &Path, gain: &ReplayGainTags) -> Result<()> {
    let mut file = read_file(path)?;
    let tag_type = existing_or_default_tag_type(&file);
    let tag = tag_for_writing(&mut file, tag_type);

    for (key, value) in REPLAY_GAIN_KEYS.into_iter().zip([
        gain.track_gain.as_deref(),
        gain.track_peak.as_deref(),
        gain.album_gain.as_deref(),
        gain.album_peak.as_deref(),
    ]) {
        tag.remove_key(&key);
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            tag.push(lofty::tag::TagItem::new(
                key,
                lofty::tag::ItemValue::Text(value.to_owned()),
            ));
        }
    }

    for (key, value) in [
        (R128_TRACK_GAIN, gain.r128_track_gain.as_deref()),
        (R128_ALBUM_GAIN, gain.r128_album_gain.as_deref()),
    ] {
        tag.remove_key(&unknown_key(key));
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            // `push` refuses keys outside lofty's table; Opus takes any name.
            tag.push_unchecked(lofty::tag::TagItem::new(
                unknown_key(key),
                lofty::tag::ItemValue::Text(value.to_owned()),
            ));
        }
    }

    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
}

/// Containers that can carry an embedded cover. lofty would silently return
/// nothing for a WAV; refusing instead stops `art fetch` believing it embedded.
const ARTWORK_EXTENSIONS: &[&str] = &[".flac", ".mp3", ".m4a", ".mp4", ".ogg", ".opus"];

fn require_artwork_container(path: &Path) -> Result<()> {
    let suffix = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_lowercase()))
        .unwrap_or_default();

    if ARTWORK_EXTENSIONS.contains(&suffix.as_str()) {
        return Ok(());
    }
    Err(Error::UnsupportedFormat(format!(
        "{}: artwork is not supported for '{suffix}'",
        path.display()
    )))
}

/// The embedded front cover, or `None` when the file has none.
pub fn read_artwork(path: &Path) -> Result<Option<Artwork>> {
    require_artwork_container(path)?;
    Ok(artwork_of(&read_file(path)?))
}

fn artwork_of(file: &TaggedFile) -> Option<Artwork> {
    let tag = first_tag(file)?;

    // A front cover wins; failing that, whatever image is there.
    let picture = tag
        .pictures()
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())?;

    Some(Artwork {
        data: picture.data().to_vec(),
        mime: picture
            .mime_type()
            .map_or_else(|| "image/jpeg".to_owned(), ToString::to_string),
    })
}

/// Size in bytes of the embedded cover, or `0` when there is none.
#[must_use]
pub fn artwork_size(path: &Path) -> u64 {
    read_artwork(path)
        .ok()
        .flatten()
        .map_or(0, |artwork| artwork.data.len() as u64)
}

/// Replace the embedded cover, removing any images already present.
pub fn write_artwork(path: &Path, artwork: &Artwork) -> Result<()> {
    require_artwork_container(path)?;
    let mut file = read_file(path)?;
    let tag_type = existing_or_default_tag_type(&file);
    let mime = match artwork.mime.as_str() {
        "image/png" => MimeType::Png,
        _ => MimeType::Jpeg,
    };
    let picture = Picture::new_unchecked(
        PictureType::CoverFront,
        Some(mime),
        Some("Cover".to_owned()),
        artwork.data.clone(),
    );

    let tag = tag_for_writing(&mut file, tag_type);
    while !tag.pictures().is_empty() {
        tag.remove_picture(0);
    }
    tag.push_picture(picture);

    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
}

/// Remove every embedded image, which is what undoing a first-time cover
/// embed comes to.
pub fn clear_artwork(path: &Path) -> Result<()> {
    require_artwork_container(path)?;
    let mut file = read_file(path)?;
    let tag_type = existing_or_default_tag_type(&file);
    let tag = tag_for_writing(&mut file, tag_type);
    while !tag.pictures().is_empty() {
        tag.remove_picture(0);
    }

    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
}

/// How a multi-valued genre has to be handed to a tag block.
///
/// ID3v2 has one TCON frame, so several genres are NUL-joined inside it.
/// Vorbis and MP4 repeat the field instead.
fn genre_items(tag_type: TagType, values: &[&String]) -> Vec<String> {
    if values.is_empty() {
        return Vec::new();
    }
    if tag_type == TagType::Id3v2 {
        return vec![values
            .iter()
            .map(|genre| genre.as_str())
            .collect::<Vec<_>>()
            .join("\0")];
    }
    values.iter().map(|genre| (*genre).clone()).collect()
}

/// The tag block to write into, created when the file carries none.
fn tag_for_writing(file: &mut TaggedFile, tag_type: TagType) -> &mut Tag {
    if file.tag(tag_type).is_none() {
        file.insert_tag(Tag::new(tag_type));
    }
    file.tag_mut(tag_type)
        .unwrap_or_else(|| unreachable!("the tag was just inserted"))
}

fn existing_or_default_tag_type(file: &TaggedFile) -> TagType {
    file.primary_tag()
        .or_else(|| file.first_tag())
        .map_or_else(|| file.primary_tag_type(), Tag::tag_type)
}
