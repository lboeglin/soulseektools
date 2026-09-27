//! Finding, verifying and embedding cover art for a library.

use crate::artwork::base::{select, ArtProvider, ArtSelection};
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
            error: String::new(),
        };
    };

    match download(http, &candidate.url) {
        Err(error) => ArtworkResult {
            entry: entry.clone(),
            selection,
            artwork: None,
            error: error.to_string(),
        },
        Ok(artwork) if artwork.data.len() < settings.min_bytes => ArtworkResult {
            entry: entry.clone(),
            selection,
            artwork: None,
            error: format!("downloaded image is only {} bytes", artwork.data.len()),
        },
        Ok(artwork) => ArtworkResult {
            entry: entry.clone(),
            selection,
            artwork: Some(artwork),
            error: String::new(),
        },
    }
}

/// The type comes from the bytes, not the headers.
pub fn download(http: &dyn ServiceHttp, url: &str) -> Result<Artwork> {
    let data = http.get_bytes(url)?;
    Ok(Artwork {
        mime: detect_mime(&data).to_owned(),
        data,
    })
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
