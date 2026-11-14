//! Selecting a tile and picking several, as the original's grid does: what
//! a click, Ctrl, Shift, a rectangle, Ctrl+A and the small window's arrows do
//! (the rules themselves are `selection.rs`), and which wallpapers an action
//! is for.

use super::App;
use super::browse::{INSTALLED, RECOMMENDED};
use crate::selection::{self, Status, Takes};

impl App {
    /// Select a tile and show it. As in Wallpaper Engine, a wallpaper that is
    /// here goes on the desktop at once ("Cancel" puts back what was there).
    pub(super) fn select(&mut self, index: usize) {
        if index < self.grid.cards().len() {
            // Another selection empties what was picked (the same one again does not).
            if self.grid.selected() != Some(index) && self.grid.has_picks() {
                self.grid.clear_picks();
                self.refresh_marks();
            }
            self.grid.select(Some(index));
            self.show_selection(false);
            self.show_presets();
            let here = self.selected_card().and_then(|card| self.dir_of(&card));
            if here.is_some() && here.as_ref() != self.desk.on_target().as_ref() {
                self.apply();
            }
        }
    }

    /// The selection was let go of: the panel empties.
    fn selection_gone(&mut self) {
        self.show_selection(false);
        self.show_presets();
    }

    /// The ids `set_for` goes through: every tile shown, all pages of the installed tab.
    fn shown_ids(&self) -> Vec<u64> {
        self.grid.shown_ids(self.browse.tab() == INSTALLED)
    }

    /// What an item is to an action: on the disk, to be had, or on its way.
    fn status_of(&self, id: u64) -> Status {
        if self.grid.find(id).is_some_and(|c| self.dir_of(c).is_some()) {
            Status::Installed
        } else if self.catalog.is_subscribed(id) {
            Status::Pending
        } else {
            Status::Downloadable
        }
    }

    /// What an action started on `target` (a tile's menu) or on nothing (a
    /// key, a button of the details) is for: the original's `z`.
    pub(super) fn set_for(&self, target: Option<u64>, takes: Takes) -> Vec<u64> {
        let current = self.grid.selected_id();
        selection::set_of(
            self.grid.picked(),
            current,
            target,
            &self.shown_ids(),
            |id| self.status_of(id),
            takes,
        )
    }

    /// A left click on a tile: with Ctrl it is picked or put back, with Shift
    /// the run up to it is picked, otherwise it is selected (and set).
    pub(super) fn click(&mut self, index: usize, ctrl: bool, shift: bool) {
        let Some(id) = self.grid.card(index).map(|c| c.id) else {
            return;
        };
        if ctrl {
            if self.grid.toggle_pick(id) {
                self.selection_gone();
            }
        } else if shift {
            let shown = self.shown_ids();
            self.grid.extend_picks(id, &shown);
        } else {
            self.select(index);
            return;
        }
        self.refresh_marks();
    }

    /// A rectangle let go of over the grid (the original's
    /// `callbackWallpaperSelectionRectangle`): the tiles it touches, laid out
    /// as `grid` says.
    pub(super) fn band(
        &mut self,
        grid: &selection::Grid,
        rectangle: (f32, f32, f32, f32),
        ctrl: bool,
        shift: bool,
    ) {
        let tiles = selection::in_band(grid, self.grid.cards().len(), rectangle);
        let ids: Vec<u64> = tiles.iter().filter_map(|i| self.grid.card(*i)).map(|c| c.id).collect();
        if self.grid.band_picks(&ids, ctrl, shift) {
            self.selection_gone();
        }
        self.refresh_marks();
    }

    /// Ctrl+A: every wallpaper of the installed list (all pages), the page of
    /// the workshop, the row of "Рекомендации" the selection is in.
    pub(super) fn select_all(&mut self) {
        let ids: Vec<u64> = if self.browse.tab() == RECOMMENDED {
            let Some(at) = self.grid.selected() else { return };
            self.browse.explore_row(at)
        } else {
            self.shown_ids()
        };
        self.grid.add_picks(&ids);
        self.refresh_marks();
    }

    /// The small window's arrows: the wallpaper before or after in the list.
    pub(super) fn navigate(&mut self, step: i32) {
        let next = self.grid.selected().map_or(0, |i| i as i64 + i64::from(step));
        if next >= 0 && (next as usize) < self.grid.cards().len() {
            self.select(next as usize);
        }
    }

    /// Delete: unsubscribe from the selection and what is picked, after asking.
    pub(super) fn delete_pressed(&mut self) {
        let set = self.set_for(None, Takes::Subscribed);
        self.ask_unsubscribe(set);
    }
}
