//! The playlist under the grid: the wallpapers it goes through, its strip of
//! thumbnails, and the saved playlists. The daemon plays it from its file
//! (desktop.rs); here it is only made and started.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{Image, Model, ModelRc, SharedString, VecModel};
use we_linux_ui::PlaylistEntry;

use super::App;
use crate::desktop::Playlist;
use crate::lang::t;
use crate::previews::{Job, Source};
use crate::{engine, selection, tf};

pub(super) struct PlaylistPanel {
    playlist: Playlist,
    model: Rc<VecModel<PlaylistEntry>>,
}

impl PlaylistPanel {
    /// The playlist as its file has it.
    pub(super) fn load(model: Rc<VecModel<PlaylistEntry>>) -> Self {
        Self { playlist: Playlist::load(), model }
    }

    pub(super) fn contains(&self, dir: &Path) -> bool {
        self.playlist.items.iter().any(|p| p == dir)
    }

    /// The workshop id of an entry, by its folder's name.
    fn id_at(&self, row: usize) -> Option<u64> {
        self.playlist
            .items
            .get(row)
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse().ok())
    }
}

impl App {
    pub(super) fn show_playlist(&mut self) {
        let Some(ui) = self.ui() else { return };
        let panel = &self.playlist;
        let entries: Vec<PlaylistEntry> = panel
            .playlist
            .items
            .iter()
            .map(|path| PlaylistEntry {
                title: self.catalog.at(path).map_or("", |i| i.title.as_str()).into(),
                image: Image::default(),
                loaded: false,
                selected: false,
            })
            .collect();
        panel.model.set_vec(entries);
        self.mark_playlist();
        let panel = &self.playlist;
        for (row, path) in panel.playlist.items.iter().enumerate() {
            if let (Some(id), Some(preview)) =
                (panel.id_at(row), self.catalog.at(path).and_then(|i| i.preview.clone()))
            {
                self.previews.request(Job {
                    id,
                    source: Source::File(preview),
                    size: 96,
                    generation: 0,
                    large: false,
                    animate: false,
                });
            }
        }
        ui.set_playlist_running(panel.playlist.running);
        ui.set_playlist_minutes(panel.playlist.minutes as i32);
        ui.set_playlist_order(i32::from(panel.playlist.random));
        ui.set_playlist_saved(ModelRc::new(VecModel::from(
            Playlist::saved().into_iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
    }

    /// A thumbnail of the strip, decoded.
    pub(super) fn playlist_thumbnail(&mut self, id: u64, image: Image) {
        let panel = &self.playlist;
        for row in 0..panel.model.row_count() {
            if panel.id_at(row) == Some(id)
                && let Some(mut entry) = panel.model.row_data(row)
            {
                entry.image = image.clone();
                entry.loaded = true;
                panel.model.set_row_data(row, entry);
            }
        }
    }

    /// "Добавить обои": the selected wallpaper, and the picked ones with it.
    pub(super) fn playlist_add(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let Some(dir) = self.dir_of(&card) else {
            return;
        };
        // With others picked they go in with it, as from a tile's menu.
        if self.grid.has_picks() {
            self.playlist_put(None, true);
            return;
        }
        if self.playlist.playlist.items.contains(&dir) {
            self.status(t("This wallpaper is in the playlist already", "Эти обои уже в плейлисте"));
            return;
        }
        self.playlist.playlist.items.push(dir);
        self.playlist.playlist.save();
        self.show_playlist();
        self.status(tf!("\"{}\" added to the playlist", "«{}» в плейлисте", card.title));
    }

    /// Into the playlist, or out of it: the wallpapers an action on `target` is for.
    pub(super) fn playlist_put(&mut self, target: Option<u64>, add: bool) {
        let dirs: Vec<PathBuf> = self
            .set_for(target, selection::Takes::Subscribed)
            .into_iter()
            .filter_map(|id| self.grid.find(id).cloned())
            .filter_map(|card| self.dir_of(&card))
            .collect();
        let playlist = &mut self.playlist.playlist;
        let before = playlist.items.len();
        if add {
            for dir in dirs {
                if !playlist.items.contains(&dir) {
                    playlist.items.push(dir);
                }
            }
        } else {
            playlist.items.retain(|p| !dirs.contains(p));
            if playlist.items.is_empty() {
                playlist.running = false;
            }
        }
        if playlist.items.len() != before {
            playlist.save();
            self.show_playlist();
        }
    }

    /// The selected wallpaper in the playlist: its entry framed, and "Добавить
    /// обои" turned into "Удалить обои".
    pub(super) fn mark_playlist(&mut self) {
        let dir = self.selected_card().and_then(|card| self.dir_of(&card));
        let panel = &self.playlist;
        for (row, path) in panel.playlist.items.iter().enumerate() {
            if let Some(mut entry) = panel.model.row_data(row) {
                let selected = dir.as_ref() == Some(path);
                if entry.selected != selected {
                    entry.selected = selected;
                    panel.model.set_row_data(row, entry);
                }
            }
        }
        if let Some(ui) = self.ui() {
            ui.set_sel_in_playlist(dir.is_some_and(|d| panel.playlist.items.contains(&d)));
        }
    }

    /// The cross of an entry of the strip.
    pub(super) fn playlist_remove(&mut self, row: usize) {
        let playlist = &mut self.playlist.playlist;
        if row < playlist.items.len() {
            playlist.items.remove(row);
            if playlist.items.is_empty() {
                playlist.running = false;
            }
            playlist.save();
            self.show_playlist();
        }
    }

    /// The playlist setup's start and stop.
    pub(super) fn playlist_run(&mut self, running: bool) {
        let playlist = &mut self.playlist.playlist;
        if let Some(ui) = self.ui.upgrade() {
            playlist.minutes = ui.get_playlist_minutes().max(1) as u32;
            playlist.random = ui.get_playlist_order() == 1;
        }
        playlist.running = running && !playlist.items.is_empty();
        playlist.save();
        if playlist.running {
            self.report(engine::ensure_daemon(), t("Playlist started", "Плейлист запущен"));
        } else {
            self.status(t("Playlist stopped", "Плейлист остановлен"));
        }
        self.show_playlist();
    }

    /// "Сохранить": under the first number no saved playlist has (one may
    /// have been removed).
    pub(super) fn playlist_save(&mut self) {
        let saved = Playlist::saved();
        let name = (saved.len() + 1..)
            .map(|n| tf!("Playlist {}", "Плейлист {}", n))
            .find(|name| !saved.contains(name))
            .unwrap_or_default();
        self.playlist.playlist.save_as(&name);
        self.show_playlist();
        self.status(tf!("Saved as \"{name}\"", "Сохранён как «{name}»"));
    }

    /// A saved playlist instead of this one; whether it runs stays as it was.
    pub(super) fn playlist_load(&mut self, row: i32) {
        if let Some(loaded) =
            Playlist::saved().get(row.max(0) as usize).and_then(|name| Playlist::load_saved(name))
        {
            let playlist = &mut self.playlist.playlist;
            let running = playlist.running;
            *playlist = Playlist { running, ..loaded };
            playlist.save();
            self.show_playlist();
            self.status(t("Playlist loaded", "Плейлист загружен"));
        }
    }

    pub(super) fn playlist_clear(&mut self) {
        let playlist = &mut self.playlist.playlist;
        playlist.items.clear();
        playlist.running = false;
        playlist.save();
        self.show_playlist();
    }

    /// "Удалить обои": the picked ones out of the playlist, else the selected one.
    pub(super) fn playlist_toggle_selected(&mut self) {
        if self.grid.has_picks() {
            self.playlist_put(None, false);
        } else if let Some(row) = self
            .selected_card()
            .and_then(|card| self.dir_of(&card))
            .and_then(|dir| self.playlist.playlist.items.iter().position(|p| *p == dir))
        {
            self.playlist_remove(row);
        }
    }

    /// An entry of the strip clicked.
    pub(super) fn playlist_select(&mut self, row: i32) {
        // As in the original: the entry's wallpaper is selected in the grid, where it is.
        if let Some(path) = self.playlist.playlist.items.get(row.max(0) as usize).cloned()
            && let Some(index) = self.grid.cards().iter().position(|c| self.dir_of(c).as_ref() == Some(&path))
        {
            self.select(index);
        }
    }
}
