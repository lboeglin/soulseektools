//! One place where container formats are handled.
//!
//! The scripts this replaces each grew their own dispatch, and the copies
//! drifted: some handled Ogg and some did not, some treated a missing ID3 header
//! as an error and some as an empty tag. Everything that touches metadata goes
//! through this module, so a format is supported everywhere or nowhere.
//!
//! The Python reads through mutagen's "easy" key mapping, which presents
//! artist/album/title/genre identically across ID3, Vorbis comments and MP4
//! atoms. `lofty`'s `Tag` accessors are the same idea, so the container
//! dispatch mutagen needed for artwork is not repeated here.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::{Tag, TagType};

use crate::errors::{Error, Result};
use crate::models::{Artwork, TrackTags};

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
    let file = read_file(path)?;
    let Some(tag) = first_tag(&file) else {
        return Ok(TrackTags::default());
    };

    // A track number is written as "3" or as "3/12"; the Python keeps the
    // leading digits and ignores anything that is not a plain number.
    let track_number = tag
        .get_string(&ItemKey::TrackNumber)
        .and_then(|value| value.split('/').next())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
        .and_then(|value| value.parse().ok());

    Ok(TrackTags {
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
    })
}

/// The track length in seconds, straight from the container header.
///
/// The header already carries this, so nothing here needs to spawn ffprobe once
/// per file the way the original duplicate scan did.
pub fn read_duration(path: &Path) -> Result<Option<f64>> {
    let file = read_file(path)?;
    let seconds = file.properties().duration().as_secs_f64();
    Ok((seconds != 0.0).then_some(seconds))
}

/// The stream properties `stk upgrade` needs, from the same header read.
///
/// New in schema v3 (#2). `read_duration` already opens the file and parses its
/// header; codec, bitrate, sample rate and bit depth come off the same parse, so
/// storing them costs no extra I/O and makes "every file under 320 kbps"
/// expressible for the first time.
pub fn read_stream_info(path: &Path) -> Result<StreamInfo> {
    let file = read_file(path)?;
    let properties = file.properties();
    Ok(StreamInfo {
        codec: codec_name(file.file_type()),
        bitrate: properties.audio_bitrate(),
        sample_rate: properties.sample_rate(),
        bit_depth: properties.bit_depth().map(u32::from),
        duration_seconds: {
            let seconds = properties.duration().as_secs_f64();
            (seconds != 0.0).then_some(seconds)
        },
    })
}

/// What a container's header says about its audio stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamInfo {
    pub codec: String,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
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

/// A metadata field this project can write, and put back.
///
/// The set is closed on purpose: these are the fields an edited `tags dump`
/// carries, and each one is a `ChangeField` in the write log, so an undo has
/// somewhere to put every value this module can change.
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
/// Total rather than a patch, so it mirrors [`read_tags`] and round-trips with
/// it. A caller changing one field reads the file, edits what it means to
/// change and writes the result back, which is one save per file however many
/// fields moved.
///
/// An empty value removes the key rather than writing an empty one, so a file
/// whose title was cleared reads back the way an untagged file does. Embedded
/// pictures are left alone: only these six keys are touched.
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

/// Containers that can carry an embedded cover.
///
/// `lofty` would return no picture for a WAV rather than refusing, but the
/// Python dispatches on the suffix and has no branch for one. Callers rely on
/// the difference: `artwork_size` turns the refusal into a zero, while
/// `art fetch` must not believe it embedded a cover it could not.
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
    let file = read_file(path)?;
    let Some(tag) = first_tag(&file) else {
        return Ok(None);
    };

    // A front cover wins; failing that, whatever image is there. An untagged
    // file has none, which is not an error.
    let picture = tag
        .pictures()
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first());

    Ok(picture.map(|picture| Artwork {
        data: picture.data().to_vec(),
        mime: picture
            .mime_type()
            .map_or_else(|| "image/jpeg".to_owned(), ToString::to_string),
    }))
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
/// embed comes to (#2).
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
/// ID3v2 keeps only the last of several `Genre` items, because a TCON frame is
/// one frame: several values live inside it separated by NUL, which is what
/// ID3v2.4 specifies and what lofty splits back out on the way in. Vorbis
/// comments and MP4 atoms genuinely repeat the field, so joining there would
/// write one genre with a NUL in its name.
fn genre_items(tag_type: TagType, values: &[&String]) -> Vec<String> {
    if values.is_empty() {
        return Vec::new();
    }
    if tag_type == TagType::Id3v2 {
        return vec![values
            .iter()
            .map(|genre| genre.as_str())
            .collect::<Vec<_>>()
            .join(" ")];
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
