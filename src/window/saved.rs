//! The window's state between runs: `state.json`. Each part writes its own
//! fields into it and reads them back (`save_into`, `restore`); this is the
//! file's shape, and the one place that knows every part that keeps some.

use std::collections::HashMap;
use std::path::PathBuf;

use super::App;
use crate::desktop;

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub(super) struct Saved {
    pub tab: Option<i32>,
    pub workshop_sort: Option<usize>,
    pub local_sort: Option<usize>,
    pub filters_open: Option<bool>,
    /// Checked tags by group title.
    pub filters: HashMap<String, Vec<String>>,
    /// Every tag a group had when it was saved: a tag added since keeps its default.
    #[serde(default)]
    pub filter_tags: HashMap<String, Vec<String>>,
    /// Titles of the groups the filter settings hide.
    pub hidden_groups: Vec<String>,
    pub favorites: Vec<u64>,
    /// "en" or "ru"; nothing until the first run asked.
    pub language: Option<String>,
    /// "Choose monitor" comes down when the window opens.
    pub monitors_on_start: Option<bool>,
    /// The installed wallpapers are sorted the other way round.
    pub sort_descending: Option<bool>,
    /// The grid's menu, "Вид": the tiles' size and the installed a page.
    pub icon_size: Option<i32>,
    pub per_page: Option<usize>,
}

fn config_file() -> PathBuf {
    desktop::config_dir().join("we-linux/state.json")
}

fn read() -> Option<Saved> {
    let bytes = std::fs::read(config_file()).ok()?;
    serde_json::from_slice::<Saved>(&bytes).ok()
}

/// The language saved, read before anything else: every label is made in it.
pub(super) fn language() -> Option<String> {
    read().and_then(|saved| saved.language)
}

impl App {
    pub(super) fn save(&self) {
        let mut saved = Saved {
            filters_open: self.ui().map(|ui| ui.get_filters_open()),
            favorites: self.catalog.favorites_sorted(),
            icon_size: Some(self.grid.icon_size()),
            ..Saved::default()
        };
        self.browse.save_into(&mut saved);
        self.filters.save_into(&mut saved);
        self.desk.save_into(&mut saved);
        self.language.save_into(&mut saved);
        let path = config_file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&saved) {
            let _ = std::fs::write(path, json);
        }
    }

    pub(super) fn restore(&mut self) {
        let Some(saved) = read() else {
            return;
        };
        self.browse.restore(&saved);
        self.catalog.set_favorites(saved.favorites.clone());
        self.filters.restore(&saved);
        self.desk.restore(&saved);
        self.grid.set_icon_size(saved.icon_size.unwrap_or(1).clamp(0, 2));
        if let Some(ui) = self.ui() {
            ui.set_monitors_on_start(self.desk.monitors_on_start());
            ui.set_sort_descending(self.browse.sort_descending());
            if let Some(open) = saved.filters_open {
                ui.set_filters_open(open);
            }
        }
    }
}
