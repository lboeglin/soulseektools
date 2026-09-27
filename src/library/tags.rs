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

fn first_tag(file: &TaggedFile) -> Option<&Tag> {
    file.primary_tag().or_else(|| file.first_tag())
}

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

pub fn read_duration(path: &Path) -> Result<Option<f64>> {
    let file = read_file(path)?;
    let seconds = file.properties().duration().as_secs_f64();
    Ok((seconds != 0.0).then_some(seconds))
}

pub fn read_stream_info(path: &Path) -> Result<StreamInfo> {
    Ok(stream_info_of(&read_file(path)?))
}

/// VBR opens with a `Xing` or `VBRI` frame; lofty parses it but doesn't say which.
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

/// Everything the index stores, from one header parse rather than one per reader.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Described {
    pub tags: TrackTags,
    pub stream: StreamInfo,
    pub replay_gain: ReplayGainTags,
    pub artwork_bytes: u64,
    pub r128: bool,
    pub isrc: Option<String>,
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
        isrc: isrc_of(&file),
    })
}

pub fn read_isrc(path: &Path) -> Result<Option<String>> {
    Ok(isrc_of(&read_file(path)?))
}

fn isrc_of(file: &TaggedFile) -> Option<String> {
    first_tag(file)?
        .get_strings(&ItemKey::Isrc)
        .find_map(crate::text::normalize::isrc)
}

/// Each is a `ChangeField` in the write log, so every write can be undone.
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

    /// An empty value means the field carries nothing.
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

pub const GENRE_SEPARATOR: &str = "; ";

/// Total rather than a patch: an empty value removes the key. Pictures are left alone.
pub fn write_tags(path: &Path, tags: &TrackTags) -> Result<()> {
    if let Some(mut file) = NativeFile::open(path)? {
        file.set(Field::Artist, &[&tags.artist]);
        file.set(Field::AlbumArtist, &[&tags.album_artist]);
        file.set(Field::Album, &[&tags.album]);
        file.set(Field::Title, &[&tags.title]);
        file.set_track(tags.track_number);
        let genres: Vec<&str> = tags.genres.iter().map(String::as_str).collect();
        file.set(Field::Genre, &genres);
        return file.save(path);
    }

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

pub fn write_genres(path: &Path, genres: &[String]) -> Result<()> {
    if let Some(mut file) = NativeFile::open(path)? {
        let values: Vec<&str> = genres.iter().map(String::as_str).collect();
        file.set(Field::Genre, &values);
        return file.save(path);
    }

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

/// `key` is the tag format's own spelling (`COMM`, `©too`), so it can be put back by name.
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

/// Binary items are skipped.
pub fn read_extras(path: &Path) -> Result<Vec<Extra>> {
    if let Some(comments) = NativeFile::open(path)?
        .as_mut()
        .and_then(NativeFile::comments)
    {
        return Ok(comments
            .items()
            .filter(|(key, _)| !vorbis_kept(key))
            .map(|(key, value)| Extra {
                key: key.to_owned(),
                value: value.to_owned(),
                locator: false,
            })
            .collect());
    }
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

/// Total rather than a patch, so an undo puts back exactly what was logged.
pub fn write_extras(path: &Path, extras: &[Extra]) -> Result<()> {
    let mut native = NativeFile::open(path)?;
    if let Some(comments) = native.as_mut().and_then(NativeFile::comments) {
        let current = comments.items().filter(|(key, _)| !vorbis_kept(key));
        if current.eq(extras
            .iter()
            .map(|extra| (extra.key.as_str(), extra.value.as_str())))
        {
            return Ok(());
        }
        let kept: Vec<(String, String)> = comments
            .take_items()
            .filter(|(key, _)| vorbis_kept(key))
            .collect();
        for (key, value) in kept {
            comments.push(key, value);
        }
        for extra in extras {
            comments.push(extra.key.clone(), extra.value.clone());
        }
        return native.as_ref().map_or(Ok(()), |file| file.save(path));
    }

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

const REPLAY_GAIN_KEYS: [ItemKey; 4] = [
    ItemKey::ReplayGainTrackGain,
    ItemKey::ReplayGainTrackPeak,
    ItemKey::ReplayGainAlbumGain,
    ItemKey::ReplayGainAlbumPeak,
];

const R128_TRACK_GAIN: &str = "R128_TRACK_GAIN";
const R128_ALBUM_GAIN: &str = "R128_ALBUM_GAIN";

fn unknown_key(key: &str) -> ItemKey {
    ItemKey::Unknown(key.to_owned())
}

/// Opus does, against -23 LUFS. Decided by container, since Opus is often named `.ogg`.
fn carries_r128(file: &TaggedFile) -> bool {
    file.file_type() == lofty::file::FileType::Opus
}

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

/// Total rather than a patch, so an undo never restores a gain without its peak.
pub fn write_replay_gain(path: &Path, gain: &ReplayGainTags) -> Result<()> {
    if let Some(mut file) = NativeFile::open(path)? {
        for (key, value) in [
            ("REPLAYGAIN_TRACK_GAIN", &gain.track_gain),
            ("REPLAYGAIN_TRACK_PEAK", &gain.track_peak),
            ("REPLAYGAIN_ALBUM_GAIN", &gain.album_gain),
            ("REPLAYGAIN_ALBUM_PEAK", &gain.album_peak),
            (R128_TRACK_GAIN, &gain.r128_track_gain),
            (R128_ALBUM_GAIN, &gain.r128_album_gain),
        ] {
            file.set_gain(key, value.as_deref());
        }
        return file.save(path);
    }

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

pub fn read_artwork(path: &Path) -> Result<Option<Artwork>> {
    require_artwork_container(path)?;
    Ok(artwork_of(&read_file(path)?))
}

fn artwork_of(file: &TaggedFile) -> Option<Artwork> {
    let tag = first_tag(file)?;

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

pub fn write_artwork(path: &Path, artwork: &Artwork) -> Result<()> {
    require_artwork_container(path)?;
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

    if let Some(mut file) = NativeFile::open(path)? {
        file.replace_pictures(Some(picture))?;
        return file.save(path);
    }

    let mut file = read_file(path)?;
    let tag_type = existing_or_default_tag_type(&file);
    let tag = tag_for_writing(&mut file, tag_type);
    while !tag.pictures().is_empty() {
        tag.remove_picture(0);
    }
    tag.push_picture(picture);

    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
}

pub fn clear_artwork(path: &Path) -> Result<()> {
    require_artwork_container(path)?;
    if let Some(mut file) = NativeFile::open(path)? {
        file.replace_pictures(None)?;
        return file.save(path);
    }

    let mut file = read_file(path)?;
    let tag_type = existing_or_default_tag_type(&file);
    let tag = tag_for_writing(&mut file, tag_type);
    while !tag.pictures().is_empty() {
        tag.remove_picture(0);
    }

    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
}

/// ID3v2 has one TCON frame, so several genres are NUL-joined inside it.
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

/// A file edited through its own tag format, one field at a time. lofty's
/// generic `Tag` rewrites the whole block on save, folding aliases together
/// (ORGANIZATION into LABEL), dropping multi-valued frames down to one value,
/// losing lyrics languages and ratings, and adding an ENCODER field.
enum NativeFile {
    Flac(lofty::flac::FlacFile),
    Ogg(lofty::ogg::VorbisFile),
    Opus(lofty::ogg::OpusFile),
    Mpeg(lofty::mpeg::MpegFile),
    Mp4(lofty::mp4::Mp4File),
}

/// A field stk writes, spelled per format.
#[derive(Clone, Copy)]
enum Field {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Genre,
}

impl Field {
    fn vorbis(self) -> &'static str {
        match self {
            Self::Artist => "ARTIST",
            Self::AlbumArtist => "ALBUMARTIST",
            Self::Album => "ALBUM",
            Self::Title => "TITLE",
            Self::Genre => "GENRE",
        }
    }

    fn id3(self) -> &'static str {
        match self {
            Self::Artist => "TPE1",
            Self::AlbumArtist => "TPE2",
            Self::Album => "TALB",
            Self::Title => "TIT2",
            Self::Genre => "TCON",
        }
    }

    fn mp4(self) -> [u8; 4] {
        match self {
            Self::Artist => *b"\xa9ART",
            Self::AlbumArtist => *b"aART",
            Self::Album => *b"\xa9alb",
            Self::Title => *b"\xa9nam",
            Self::Genre => *b"\xa9gen",
        }
    }
}

impl NativeFile {
    /// `None` for a container stk writes through the generic tag, such as WAV.
    fn open(path: &Path) -> Result<Option<Self>> {
        use lofty::file::FileType;

        let failed = |error: &dyn fmt::Display| Error::Tag(format!("{}: {error}", path.display()));
        let kind = lofty::probe::Probe::open(path)
            .map_err(|error| failed(&error))?
            .guess_file_type()
            .map_err(|error| failed(&error))?
            .file_type();

        let mut file = std::fs::File::open(path).map_err(|error| failed(&error))?;
        let parse = lofty::config::ParseOptions::new();
        let opened = match kind {
            Some(FileType::Flac) => {
                lofty::flac::FlacFile::read_from(&mut file, parse).map(Self::Flac)
            }
            Some(FileType::Vorbis) => {
                lofty::ogg::VorbisFile::read_from(&mut file, parse).map(Self::Ogg)
            }
            Some(FileType::Opus) => {
                lofty::ogg::OpusFile::read_from(&mut file, parse).map(Self::Opus)
            }
            Some(FileType::Mpeg) => {
                lofty::mpeg::MpegFile::read_from(&mut file, parse).map(|mut mpeg| {
                    if let Some(tag) = mpeg.id3v2_mut() {
                        mark_unknown_languages(tag);
                    }
                    Self::Mpeg(mpeg)
                })
            }
            Some(FileType::Mp4) => {
                lofty::mp4::Mp4File::read_from(&mut file, parse).map(|mut mp4| {
                    if let Some(ilst) = mp4.ilst_mut() {
                        keep_integer_widths(ilst);
                    }
                    Self::Mp4(mp4)
                })
            }
            _ => return Ok(None),
        };
        let mut opened = opened.map_err(|error| failed(&error))?;
        if let (Some(stored), Some(comments)) = (stored_comments(path, kind), opened.comments()) {
            comments.set_vendor(stored.vendor);
            comments.take_items().for_each(drop);
            for (key, value) in stored.items {
                comments.push(key, value);
            }
        }
        Ok(Some(opened))
    }

    /// The Vorbis comments of a FLAC, Ogg or Opus file, created when missing.
    fn comments(&mut self) -> Option<&mut lofty::ogg::VorbisComments> {
        match self {
            Self::Flac(flac) => {
                if flac.vorbis_comments().is_none() {
                    flac.set_vorbis_comments(lofty::ogg::VorbisComments::default());
                }
                flac.vorbis_comments_mut()
            }
            Self::Ogg(ogg) => Some(ogg.vorbis_comments_mut()),
            Self::Opus(opus) => Some(opus.vorbis_comments_mut()),
            Self::Mpeg(_) | Self::Mp4(_) => None,
        }
    }

    fn id3(&mut self) -> Option<&mut lofty::id3::v2::Id3v2Tag> {
        let Self::Mpeg(mpeg) = self else {
            return None;
        };
        if mpeg.id3v2().is_none() {
            mpeg.set_id3v2(lofty::id3::v2::Id3v2Tag::default());
        }
        mpeg.id3v2_mut()
    }

    fn ilst(&mut self) -> Option<&mut lofty::mp4::Ilst> {
        let Self::Mp4(mp4) = self else {
            return None;
        };
        if mp4.ilst().is_none() {
            mp4.set_ilst(lofty::mp4::Ilst::default());
        }
        mp4.ilst_mut()
    }

    fn set(&mut self, field: Field, values: &[&str]) {
        let values: Vec<&str> = values
            .iter()
            .copied()
            .filter(|value| !value.is_empty())
            .collect();
        if let Some(comments) = self.comments() {
            set_comment(comments, field.vorbis(), &values);
        } else if let Some(tag) = self.id3() {
            // One frame, values NUL-separated, as ID3v2.4 specifies.
            set_id3_text(tag, field.id3(), &values.join("\0"));
        } else if let Some(ilst) = self.ilst() {
            set_atom(ilst, lofty::mp4::AtomIdent::Fourcc(field.mp4()), &values);
        }
    }

    fn set_track(&mut self, number: Option<i64>) {
        let number = number.and_then(|number| u32::try_from(number).ok());
        if let Some(comments) = self.comments() {
            // `3/12` and `03` both say track 3; leave them as written.
            let current = comments
                .get("TRACKNUMBER")
                .and_then(|text| text.split('/').next()?.trim().parse::<u32>().ok());
            if current != number {
                let text = number.map(|number| number.to_string()).unwrap_or_default();
                set_comment(comments, "TRACKNUMBER", &[&text]);
            }
            return;
        }
        let tag: &mut dyn Accessor = if let Some(tag) = self.id3() {
            tag
        } else if let Some(ilst) = self.ilst() {
            ilst
        } else {
            return;
        };
        if tag.track() == number {
            return;
        }
        match number {
            Some(number) => tag.set_track(number),
            None => tag.remove_track(),
        }
    }

    fn set_gain(&mut self, key: &str, value: Option<&str>) {
        let value = value.filter(|value| !value.is_empty());
        if let Some(comments) = self.comments() {
            set_comment(comments, key, &[value.unwrap_or_default()]);
        } else if let Some(tag) = self.id3() {
            set_user_text(tag, key, value);
        } else if let Some(ilst) = self.ilst() {
            use lofty::mp4::AtomIdent;

            // Taggers disagree on case; keep the file's spelling, drop duplicates.
            let names: Vec<String> = (&*ilst)
                .into_iter()
                .filter_map(|atom| match atom.ident() {
                    AtomIdent::Freeform { mean, name }
                        if mean == "com.apple.iTunes" && name.eq_ignore_ascii_case(key) =>
                    {
                        Some(name.to_string())
                    }
                    _ => None,
                })
                .collect();
            let freeform = |name: String| AtomIdent::Freeform {
                mean: "com.apple.iTunes".into(),
                name: name.into(),
            };
            for duplicate in names.iter().skip(1) {
                ilst.remove(&freeform(duplicate.clone())).for_each(drop);
            }
            let name = names.into_iter().next().unwrap_or_else(|| key.to_owned());
            set_atom(ilst, freeform(name), value.as_slice());
        }
    }

    fn replace_pictures(&mut self, picture: Option<Picture>) -> Result<()> {
        use lofty::ogg::OggPictureStorage;

        let failed = |error: lofty::error::LoftyError| Error::Tag(error.to_string());
        match self {
            Self::Flac(flac) => {
                flac.remove_pictures();
                if let Some(picture) = picture {
                    flac.insert_picture(picture, None).map_err(failed)?;
                }
            }
            Self::Ogg(_) | Self::Opus(_) => {
                let comments = self.comments().expect("an Ogg file has comments");
                comments.remove_pictures();
                if let Some(picture) = picture {
                    comments.insert_picture(picture, None).map_err(failed)?;
                }
            }
            Self::Mpeg(_) => {
                let tag = self.id3().expect("an MP3 has an ID3v2 tag");
                let apic = lofty::id3::v2::FrameId::Valid("APIC".into());
                tag.remove(&apic).for_each(drop);
                if let Some(picture) = picture {
                    tag.insert_picture(picture);
                }
            }
            Self::Mp4(_) => {
                let ilst = self.ilst().expect("an MP4 has an ilst");
                ilst.remove_pictures();
                if let Some(picture) = picture {
                    ilst.insert_picture(picture);
                }
            }
        }
        Ok(())
    }

    fn save(&self, path: &Path) -> Result<()> {
        let options = WriteOptions::default();
        match self {
            Self::Flac(file) => file.save_to_path(path, options),
            Self::Ogg(file) => file.save_to_path(path, options),
            Self::Opus(file) => file.save_to_path(path, options),
            Self::Mpeg(file) => file.save_to_path(path, options),
            Self::Mp4(file) => file.save_to_path(path, options),
        }
        .map_err(|error| Error::Tag(format!("{}: {error}", path.display())))
    }
}

/// lofty reads a comment or lyrics frame whose language isn't three letters,
/// some taggers write zeros, but refuses to save it. `XXX` is ID3's unknown.
fn mark_unknown_languages(tag: &mut lofty::id3::v2::Id3v2Tag) {
    use lofty::id3::v2::Frame;

    let broken = |frame: &Frame<'_>| {
        let language = match frame {
            Frame::Comment(comment) => comment.language,
            Frame::UnsynchronizedText(lyrics) => lyrics.language,
            _ => return false,
        };
        !language.iter().all(u8::is_ascii_alphabetic)
    };
    let fixed: Vec<Frame<'static>> = (&*tag)
        .into_iter()
        .filter(|frame| broken(frame))
        .cloned()
        .map(|mut frame| {
            match &mut frame {
                Frame::Comment(comment) => comment.language = *b"XXX",
                Frame::UnsynchronizedText(lyrics) => lyrics.language = *b"XXX",
                _ => {}
            }
            frame
        })
        .collect();
    if fixed.is_empty() {
        return;
    }
    tag.retain(|frame| !broken(frame));
    for frame in fixed {
        tag.insert(frame);
    }
}

/// Taggers disagree on the case of TXXX descriptions; keep the file's
/// spelling and drop duplicates.
fn set_user_text(tag: &mut lofty::id3::v2::Id3v2Tag, key: &str, value: Option<&str>) {
    use lofty::id3::v2::{ExtendedTextFrame, Frame};

    let is_key = |frame: &Frame<'_>| matches!(frame, Frame::UserText(text) if text.description.eq_ignore_ascii_case(key));
    let current: Vec<&ExtendedTextFrame<'_>> = (&*tag)
        .into_iter()
        .filter_map(|frame| match frame {
            Frame::UserText(text) if is_key(frame) => Some(text),
            _ => None,
        })
        .collect();
    if current.iter().map(|text| text.content.as_str()).eq(value) {
        return;
    }
    let description = current
        .first()
        .map_or(key, |text| text.description.as_str())
        .to_owned();
    tag.retain(|frame| !is_key(frame));
    if let Some(value) = value {
        tag.insert(Frame::UserText(ExtendedTextFrame::new(
            lofty::TextEncoding::UTF8,
            description,
            value.to_owned(),
        )));
    }
}

fn set_id3_text(tag: &mut lofty::id3::v2::Id3v2Tag, id: &'static str, value: &str) {
    use lofty::id3::v2::{Frame, FrameId, TextInformationFrame};

    let frame_id = FrameId::Valid(id.into());
    if tag.get_text(&frame_id).unwrap_or_default() == value {
        return;
    }
    tag.remove(&frame_id).for_each(drop);
    if !value.is_empty() {
        tag.insert(Frame::Text(TextInformationFrame::new(
            frame_id,
            lofty::TextEncoding::UTF8,
            value.to_owned(),
        )));
    }
}

/// lofty writes every signed integer atom as four bytes, but players read
/// these as one byte (two for `tmpo`) and would see 0.
fn keep_integer_widths(ilst: &mut lofty::mp4::Ilst) {
    use lofty::mp4::{Atom, AtomData, AtomIdent, DataType};

    const WIDTHS: [([u8; 4], usize); 7] = [
        (*b"rtng", 1),
        (*b"stik", 1),
        (*b"pgap", 1),
        (*b"hdvd", 1),
        (*b"akID", 1),
        (*b"shwm", 1),
        (*b"tmpo", 2),
    ];
    for (fourcc, width) in WIDTHS {
        let ident = AtomIdent::Fourcc(fourcc);
        let Some(&AtomData::SignedInteger(value)) =
            ilst.get(&ident).and_then(|atom| atom.data().next())
        else {
            continue;
        };
        let data = value.to_be_bytes()[4 - width..].to_vec();
        ilst.replace_atom(Atom::new(
            ident,
            AtomData::Unknown {
                code: DataType::BeSignedInteger,
                data,
            },
        ));
    }
}

fn set_atom(ilst: &mut lofty::mp4::Ilst, ident: lofty::mp4::AtomIdent<'static>, values: &[&str]) {
    use lofty::mp4::{Atom, AtomData};

    let current: Vec<&str> = ilst
        .get(&ident)
        .map(|atom| {
            atom.data()
                .filter_map(|data| match data {
                    AtomData::UTF8(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if current == values {
        return;
    }
    ilst.remove(&ident).for_each(drop);
    let data = values
        .iter()
        .map(|value| AtomData::UTF8((*value).to_owned()))
        .collect();
    if let Some(atom) = Atom::from_collection(ident, data) {
        ilst.insert(atom);
    }
}

struct StoredComments {
    vendor: String,
    items: Vec<(String, String)>,
}

/// The comments exactly as the file stores them. lofty's reader splits
/// `TRACKNUMBER=3/12`, drops zero padding and re-cases the key, which a save
/// would then write back. `None` when the block can't be read.
fn stored_comments(path: &Path, kind: Option<lofty::file::FileType>) -> Option<StoredComments> {
    use lofty::file::FileType;
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let body = match kind? {
        FileType::Flac => {
            let mut head = [0_u8; 10];
            file.read_exact(&mut head).ok()?;
            if head.starts_with(b"ID3") {
                let size = head[6..10]
                    .iter()
                    .fold(0_u64, |size, byte| (size << 7) | u64::from(byte & 0x7f));
                let footer = if head[5] & 0x10 == 0 { 0 } else { 10 };
                file.seek(SeekFrom::Start(10 + size + footer)).ok()?;
            } else {
                file.seek(SeekFrom::Start(0)).ok()?;
            }
            let mut marker = [0_u8; 4];
            file.read_exact(&mut marker).ok()?;
            if &marker != b"fLaC" {
                return None;
            }
            loop {
                let mut header = [0_u8; 4];
                file.read_exact(&mut header).ok()?;
                let length = u32::from_be_bytes([0, header[1], header[2], header[3]]);
                if header[0] & 0x7f == 4 {
                    let mut body = vec![0_u8; usize::try_from(length).ok()?];
                    file.read_exact(&mut body).ok()?;
                    break body;
                }
                if header[0] & 0x80 != 0 {
                    return None;
                }
                file.seek(SeekFrom::Current(i64::from(length))).ok()?;
            }
        }
        FileType::Vorbis | FileType::Opus => {
            let packets = ogg_pager::Packets::read_count(&mut file, 2).ok()?;
            let packet = packets.get(1)?;
            let magic: &[u8] = if kind == Some(FileType::Vorbis) {
                b"\x03vorbis"
            } else {
                b"OpusTags"
            };
            packet.strip_prefix(magic)?.to_vec()
        }
        _ => return None,
    };
    parse_comments(&body)
}

fn parse_comments(mut data: &[u8]) -> Option<StoredComments> {
    fn field<'a>(data: &mut &'a [u8]) -> Option<&'a [u8]> {
        let (length, rest) = data.split_first_chunk::<4>()?;
        let length = usize::try_from(u32::from_le_bytes(*length)).ok()?;
        if rest.len() < length {
            return None;
        }
        let (field, rest) = rest.split_at(length);
        *data = rest;
        Some(field)
    }

    let vendor = String::from_utf8_lossy(field(&mut data)?).into_owned();
    let (count, rest) = data.split_first_chunk::<4>()?;
    data = rest;
    let mut items = Vec::new();
    for _ in 0..u32::from_le_bytes(*count) {
        let text = std::str::from_utf8(field(&mut data)?).ok();
        // lofty keeps pictures apart from the other comments, and drops what
        // isn't UTF-8.
        let Some((key, value)) = text.and_then(|text| text.split_once('=')) else {
            continue;
        };
        if !key.eq_ignore_ascii_case("METADATA_BLOCK_PICTURE") {
            items.push((key.to_owned(), value.to_owned()));
        }
    }
    Some(StoredComments { vendor, items })
}

/// Replace every value under `key` where the first one stood, keeping every
/// other comment in its place. Does nothing when the values already match.
fn set_comment(comments: &mut lofty::ogg::VorbisComments, key: &str, values: &[&str]) {
    let wanted: Vec<&str> = values
        .iter()
        .copied()
        .filter(|value| !value.is_empty())
        .collect();
    if comments.get_all(key).eq(wanted.iter().copied()) {
        return;
    }
    let mut placed = false;
    for (existing, value) in comments.take_items().collect::<Vec<_>>() {
        if !existing.eq_ignore_ascii_case(key) {
            comments.push(existing, value);
        } else if !placed {
            placed = true;
            for value in &wanted {
                comments.push(key.to_owned(), (*value).to_owned());
            }
        }
    }
    if !placed {
        for value in wanted {
            comments.push(key.to_owned(), value.to_owned());
        }
    }
}

/// Vorbis keys `read_extras` leaves out, as `is_kept` does for other formats.
const VORBIS_KEPT: &[&str] = &[
    "ARTIST",
    "ALBUMARTIST",
    "ALBUM",
    "TITLE",
    "TRACKNUMBER",
    "TRACKTOTAL",
    "TOTALTRACKS",
    "DISCNUMBER",
    "DISCTOTAL",
    "TOTALDISCS",
    "GENRE",
    "ENCODER",
    "ENCODING",
    "ENCODERSETTINGS",
    "REPLAYGAIN_TRACK_GAIN",
    "REPLAYGAIN_TRACK_PEAK",
    "REPLAYGAIN_ALBUM_GAIN",
    "REPLAYGAIN_ALBUM_PEAK",
    R128_TRACK_GAIN,
    R128_ALBUM_GAIN,
];

fn vorbis_kept(key: &str) -> bool {
    VORBIS_KEPT
        .iter()
        .any(|kept| kept.eq_ignore_ascii_case(key))
}
