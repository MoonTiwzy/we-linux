//! A wallpaper as a tile of the grid, from the workshop or from the disk:
//! what every list is made of.

use std::path::PathBuf;

use super::tags::{self, RATINGS, TYPES};
use crate::library;
use crate::previews::Source;
use crate::steam;

#[derive(Clone)]
pub(super) enum Picture {
    Url(String),
    File(PathBuf),
    None,
}

impl Picture {
    /// Where its preview is decoded from; nothing when it has none.
    pub(super) fn source(&self) -> Option<Source> {
        match self {
            Picture::Url(url) => Some(Source::Url(url.clone())),
            Picture::File(path) => Some(Source::File(path.clone())),
            Picture::None => None,
        }
    }
}

/// One cell of the grid, from the workshop or from the disk.
#[derive(Clone)]
pub(super) struct Card {
    pub id: u64,
    pub title: String,
    pub kind: String,
    pub rating: String,
    pub tags: Vec<String>,
    pub size: u64,
    pub up: u32,
    pub down: u32,
    pub updated: u64,
    pub picture: Picture,
    pub dir: Option<PathBuf>,
    /// The author's Steam id (0: not known yet).
    pub owner: u64,
}

impl Card {
    /// A wallpaper on the disk.
    pub(super) fn installed(item: &library::Installed) -> Self {
        Card {
            id: item.id,
            title: item.title.clone(),
            kind: item.kind.clone(),
            rating: item.rating.clone(),
            tags: item.tags.clone(),
            size: 0,
            up: 0,
            down: 0,
            updated: item.modified,
            picture: item.preview.clone().map_or(Picture::None, Picture::File),
            dir: Some(item.dir.clone()),
            owner: 0,
        }
    }

    /// A wallpaper of the workshop; `dir` is where it is when it is installed.
    pub(super) fn of_entry(e: steam::Entry, dir: Option<PathBuf>) -> Self {
        let find = |list: &[(&'static str, &'static str)]| {
            e.tags.iter().find(|t| tags::label_of(list, t).is_some()).cloned().unwrap_or_default()
        };
        Card {
            id: e.id,
            title: e.title.clone(),
            kind: find(&TYPES).to_lowercase(),
            rating: find(&RATINGS),
            size: e.size,
            up: e.up,
            down: e.down,
            updated: u64::from(e.updated),
            picture: if e.preview_url.is_empty() {
                Picture::None
            } else {
                Picture::Url(e.preview_url.clone())
            },
            dir,
            owner: e.owner,
            tags: e.tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workshop_entry_says_its_kind_and_rating_by_its_tags() {
        let entry = steam::Entry {
            id: 7,
            title: "Rain".to_owned(),
            tags: vec!["Anime".to_owned(), "Video".to_owned(), "Mature".to_owned()],
            preview_url: String::new(),
            owner: 3,
            ..Default::default()
        };
        let card = Card::of_entry(entry, None);
        assert_eq!((card.kind.as_str(), card.rating.as_str()), ("video", "Mature"));
        assert_eq!(card.owner, 3);
        assert!(card.picture.source().is_none());
        assert!(card.dir.is_none());
    }
}
