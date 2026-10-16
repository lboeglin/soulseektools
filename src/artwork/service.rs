//! Finding, verifying and embedding cover art for a library.

use std::path::{Path, PathBuf};

use crate::artwork::base::{select, ArtCandidate, ArtProvider, ArtSelection};
use crate::artwork::coverartarchive::{self, CoverArtArchiveProvider};
use crate::artwork::itunes::{self, ITunesProvider};
use crate::config::ArtworkSettings;
use crate::errors::{Error, Result};
use crate::library::tags;
use crate::models::{Artwork, LibraryEntry};
use crate::services::ServiceHttp;

const MIME_BY_MAGIC: &[(&[u8], &str)] = &[
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"\xff\xd8\xff", "image/jpeg"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct ArtworkStatus {
    pub entry: LibraryEntry,
    pub needs_artwork: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtworkResult {
    pub entry: LibraryEntry,
    pub selection: ArtSelection,
    pub artwork: Option<Artwork>,
    /// The release year this fills into a file that has none.
    pub year: Option<String>,
    pub error: String,
}

impl ArtworkResult {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.artwork.is_some() && self.error.is_empty()
    }
}

pub fn build_providers(settings: &ArtworkSettings) -> Result<Vec<Box<dyn ArtProvider>>> {
    settings
        .providers
        .iter()
        .map(|name| -> Result<Box<dyn ArtProvider>> {
            match name.as_str() {
                "itunes" => Ok(Box::new(ITunesProvider::new(
                    settings.preferred_size,
                    settings
                        .min_request_interval_seconds
                        .unwrap_or(itunes::MIN_INTERVAL_SECONDS),
                ))),
                "coverartarchive" => Ok(Box::new(CoverArtArchiveProvider::new(
                    settings.preferred_size,
                    settings
                        .min_request_interval_seconds
                        .unwrap_or(coverartarchive::MIN_INTERVAL_SECONDS),
                ))),
                other => Err(Error::Config(format!(
                    "unknown artwork provider '{other}'; choose from itunes, coverartarchive"
                ))),
            }
        })
        .collect()
}

/// A cover under `min_bytes` counts as missing: a broken download or a placeholder.
#[must_use]
pub fn audit(entries: &[LibraryEntry], settings: &ArtworkSettings) -> Vec<ArtworkStatus> {
    entries
        .iter()
        .map(|entry| {
            let (needs_artwork, reason) = if entry.has_artwork() {
                if entry.artwork_bytes < settings.min_bytes as u64 {
                    (true, format!("cover is only {} bytes", entry.artwork_bytes))
                } else {
                    (false, format!("{} bytes", entry.artwork_bytes))
                }
            } else {
                (true, "no embedded cover".to_owned())
            };
            ArtworkStatus {
                entry: entry.clone(),
                needs_artwork,
                reason,
            }
        })
        .collect()
}

pub fn find_artwork(
    entry: &LibraryEntry,
    providers: &[Box<dyn ArtProvider>],
    settings: &ArtworkSettings,
    http: &dyn ServiceHttp,
) -> ArtworkResult {
    let artist = first_non_empty(&[&entry.tags.artist, &entry.folder_artist, &entry.artist.raw]);
    let title = first_non_empty(&[&entry.tags.title, &entry.title.raw]);
    // An artist tag crediting several people ("Duca, Puyo Suzuki") finds
    // nothing where the album artist alone would.
    let album_artist = &entry.tags.album_artist;
    let mut artists = vec![artist];
    if !album_artist.is_empty()
        && !album_artist.eq_ignore_ascii_case(&artists[0])
        && !album_artist.eq_ignore_ascii_case("Various Artists")
    {
        artists.push(album_artist.clone());
    }

    let mut selection = ArtSelection::none("no providers configured");
    'search: for artist in &artists {
        for provider in providers {
            match provider.search(http, artist, &title) {
                Err(error) => {
                    selection =
                        ArtSelection::none(&format!("{} unavailable: {error}", provider.name()));
                }
                Ok(candidates) => {
                    selection = select(
                        &candidates,
                        artist,
                        settings.require_artist_match,
                        settings.artist_match_threshold,
                    );
                    if selection.is_some() {
                        break 'search;
                    }
                }
            }
        }
    }

    let Some(candidate) = selection.candidate.clone() else {
        return ArtworkResult {
            entry: entry.clone(),
            selection,
            artwork: None,
            year: None,
            error: String::new(),
        };
    };

    match download(http, &candidate.url) {
        Err(error) => ArtworkResult {
            entry: entry.clone(),
            selection,
            artwork: None,
            year: None,
            error: error.to_string(),
        },
        Ok(artwork) if artwork.data.len() < settings.min_bytes => ArtworkResult {
            entry: entry.clone(),
            selection,
            artwork: None,
            year: None,
            error: format!("downloaded image is only {} bytes", artwork.data.len()),
        },
        Ok(artwork) => ArtworkResult {
            entry: entry.clone(),
            selection,
            artwork: Some(artwork),
            year: year_to_fill(entry, &candidate, settings),
            error: String::new(),
        },
    }
}

/// Only from a match whose artist was verified, so a wrong cover can't bring a wrong year.
fn year_to_fill(
    entry: &LibraryEntry,
    candidate: &ArtCandidate,
    settings: &ArtworkSettings,
) -> Option<String> {
    (settings.fill_year && settings.require_artist_match && entry.tags.year.is_empty())
        .then_some(candidate.year)
        .flatten()
        .map(|year| format!("{year:04}"))
}

/// The type comes from the bytes, not the headers.
pub fn download(http: &dyn ServiceHttp, url: &str) -> Result<Artwork> {
    let data = http.get_bytes(url)?;
    Ok(Artwork {
        mime: detect_mime(&data).to_owned(),
        data,
    })
}

/// A cover chosen by hand, from a file or an `http(s)` URL, refused unless it
/// decodes as JPEG or PNG and reaches `min_bytes`. The warning is for an image
/// under `preferred_size`, which is embedded anyway.
pub fn load_image(
    source: &str,
    http: &dyn ServiceHttp,
    settings: &ArtworkSettings,
) -> Result<(Artwork, Option<String>)> {
    let refused = |reason: String| Error::Config(format!("{source}: {reason}"));
    let data = if source.starts_with("http://") || source.starts_with("https://") {
        http.get_bytes(source)?
    } else {
        std::fs::read(source).map_err(|error| refused(error.to_string()))?
    };

    let format = image::guess_format(&data)
        .ok()
        .filter(|format| matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png))
        .ok_or_else(|| refused("not a JPEG or PNG image".to_owned()))?;
    let decoded = image::load_from_memory_with_format(&data, format)
        .map_err(|error| refused(format!("not a readable image: {error}")))?;
    if data.len() < settings.min_bytes {
        return Err(refused(format!(
            "only {} bytes, under artwork.min_bytes ({})",
            data.len(),
            settings.min_bytes
        )));
    }

    let (width, height) = (decoded.width(), decoded.height());
    let warning = (width.min(height) < settings.preferred_size).then(|| {
        format!(
            "{width}x{height} is under artwork.preferred_size ({})",
            settings.preferred_size
        )
    });
    Ok((
        Artwork {
            mime: detect_mime(&data).to_owned(),
            data,
        },
        warning,
    ))
}

/// A file is its own target; a folder gives the files directly inside it that
/// can carry a cover, since an artist folder may mix albums below.
pub fn cover_targets(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let listing = std::fs::read_dir(path)
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))?;
    let mut files: Vec<PathBuf> = listing
        .filter_map(|item| Some(item.ok()?.path()))
        .filter(|file| file.is_file() && tags::carries_artwork(file))
        .collect();
    files.sort();
    Ok(files)
}

#[must_use]
pub fn detect_mime(data: &[u8]) -> &'static str {
    MIME_BY_MAGIC
        .iter()
        .find(|(magic, _)| data.starts_with(magic))
        .map_or("image/jpeg", |(_, mime)| *mime)
}

pub fn embed(result: &ArtworkResult) -> Result<()> {
    let Some(artwork) = &result.artwork else {
        return Err(Error::Tag(format!(
            "{}: nothing to embed",
            result.entry.path.display()
        )));
    };
    tags::write_artwork(&result.entry.path, artwork)
        .map_err(|error| Error::Tag(format!("{}: {error}", result.entry.path.display())))
}

fn first_non_empty(values: &[&String]) -> String {
    values
        .iter()
        .find(|value| !value.is_empty())
        .map_or_else(String::new, |value| (*value).clone())
}
