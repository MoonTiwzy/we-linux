//! The panel at the right: the selected wallpaper, its picture (played when
//! it is animated), what the workshop says of it, and its buttons. Its
//! properties and presets are properties.rs.

use std::time::Duration;

use slint::{Image, ModelRc, SharedString, Timer, TimerMode, VecModel};

use super::card::Card;
use super::grid::image_of;
use super::listing::votes;
use super::tags::{TYPES, label_of, tag_label};
use super::{App, slot};
use crate::desktop;
use crate::lang::t;
use crate::previews::{Decoded, Job};
use crate::steam::Command;
use crate::units::megabytes;

unsafe extern "C" {
    /// glibc: give the freed heap back to the system.
    fn malloc_trim(pad: usize) -> i32;
}

/// The selected item's animated preview: frames with their delays, the one
/// showing, and the timer that turns them.
#[derive(Default)]
pub(super) struct Details {
    frames: Vec<(Image, u32)>,
    frame: usize,
    frame_timer: Timer,
}

/// The tags that say what an item is rather than what it shows: the panel
/// has them elsewhere, or not at all.
const NOT_SHOWN: [&str; 11] = [
    "Wallpaper",
    "Preset",
    "Web",
    "Video",
    "Scene",
    "Application",
    "Approved",
    "Multi-monitor optimized",
    "Workshop",
    "Official",
    "Local",
];

/// The tags shown as pills, in the workshop's order: genres, the box's tags
/// and the ratings translated.
fn pills(tags: &[String]) -> Vec<String> {
    tags.iter()
        .filter(|t| !NOT_SHOWN.contains(&t.as_str()))
        .map(|t| tag_label(t).map_or_else(|| t.clone(), str::to_owned))
        .collect()
}

/// Pills in rows of what fits `room` across: a letter is about seven wide, a
/// pill has twelve of padding and four between it and the next.
fn pill_rows(pills: Vec<String>, room: usize) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut used = 0usize;
    for pill in pills {
        let width = pill.chars().count() * 7 + 16;
        if rows.is_empty() || used + width > room {
            rows.push(Vec::new());
            used = 0;
        }
        used += width;
        if let Some(row) = rows.last_mut() {
            row.push(pill);
        }
    }
    rows
}

impl Details {
    /// No picture plays.
    fn stop(&mut self) {
        self.frame_timer.stop();
        self.frames.clear();
    }

    /// The next frame, and how long it shows.
    fn turn(&mut self) -> Option<(Image, u32)> {
        if self.frames.is_empty() {
            return None;
        }
        self.frame = (self.frame + 1) % self.frames.len();
        Some(self.frames[self.frame].clone())
    }

    fn arm(&mut self, delay: u32) {
        self.frame_timer.start(TimerMode::SingleShot, Duration::from_millis(u64::from(delay)), || {
            slot::try_with(|app| app.next_frame());
        });
    }
}

impl App {
    /// Fill the side panel from the selected card. `same` keeps its picture
    /// and its properties (only the marks changed).
    pub(super) fn show_selection(&mut self, same: bool) {
        let Some(ui) = self.ui() else { return };
        ui.set_selected(self.grid.selected().map_or(-1, |i| i as i32));
        let Some(card) = self.grid.selected_card() else {
            ui.set_has_selection(false);
            ui.set_sel_installed(false);
            self.details.stop();
            return;
        };
        let dir = self.dir_of(&card);
        self.show_selected_state(dir.as_deref());
        ui.set_has_selection(true);
        ui.set_sel_title(card.title.as_str().into());
        ui.set_sel_subscribed(self.catalog.is_subscribed(card.id));
        ui.set_sel_installed(dir.is_some());
        ui.set_sel_current(dir.is_some() && dir.as_ref() == self.desk.current());
        ui.set_sel_favorite(self.catalog.is_favorite(card.id));
        ui.set_sel_busy(self.catalog.is_busy(card.id));
        ui.set_sel_progress(self.catalog.progress(card.id));
        // The workshop's word on it: from the page it came with, or (installed)
        // asked for by id.
        let info = self.catalog.info(card.id);
        let (up, down) = info.map_or((card.up, card.down), |e| (e.up, e.down));
        let size = info.map_or(card.size, |e| e.size);
        let mut tags: Vec<String> = info.map_or_else(|| card.tags.clone(), |e| e.tags.clone());
        if info.is_none() && !card.rating.is_empty() && !tags.contains(&card.rating) {
            tags.push(card.rating.clone());
        }
        ui.set_sel_stars(votes(up, down).round() as i32);
        ui.set_sel_vote(self.catalog.vote(card.id));
        let owner = self.catalog.owner_of(&card);
        match self.catalog.persona(owner) {
            Some((name, avatar)) => {
                ui.set_sel_author(name.as_str().into());
                ui.set_sel_avatar(avatar.clone().unwrap_or_default());
            }
            None => {
                ui.set_sel_author("".into());
                ui.set_sel_avatar(Image::default());
                if owner != 0 && self.catalog.first_ask_for_persona(owner) {
                    let _ = self.steam.send(Command::Persona(owner));
                }
            }
        }
        if card.id != 0 && !same && !self.catalog.knows_vote(card.id) {
            let _ = self.steam.send(Command::Vote(card.id));
        }

        // The kind and the size on one line, the tags as pills below: as the
        // original has them.
        let mut facts = Vec::new();
        if let Some(kind) = label_of(&TYPES, &card.kind) {
            facts.push(kind.to_owned());
        }
        if size > 0 {
            facts.push(megabytes(size));
        }
        ui.set_sel_facts(facts.join("  ").into());
        let room = if ui.get_mini() { 280 } else { 300 };
        let rows: Vec<ModelRc<SharedString>> = pill_rows(pills(&tags), room)
            .into_iter()
            .map(|row| {
                ModelRc::new(VecModel::from(row.into_iter().map(SharedString::from).collect::<Vec<_>>()))
            })
            .collect();
        ui.set_sel_tags(ModelRc::new(VecModel::from(rows)));
        self.mark_playlist();
        ui.set_sel_workshop(card.id != 0);

        if same {
            return;
        }
        self.load_properties(&card, dir.as_deref());
        self.details.stop();
        ui.set_sel_loaded(false);
        let Some(source) = card.picture.source() else {
            return;
        };
        self.previews.request(Job {
            id: card.id,
            source,
            size: 400,
            generation: 0,
            large: true,
            animate: true,
        });
    }

    /// The selected wallpaper's picture; an animated one plays, as in the
    /// Windows client.
    pub(super) fn large_preview_arrived(&mut self, picture: Decoded, image: Image) {
        let (Some(ui), Some(card)) = (self.ui(), self.grid.selected_card()) else {
            return;
        };
        if card.id != picture.id {
            return;
        }
        ui.set_sel_image(image.clone());
        ui.set_sel_loaded(true);
        let details = &mut self.details;
        details.frames = std::iter::once((image, picture.delay))
            .chain(
                picture
                    .frames
                    .into_iter()
                    .map(|(rgba, delay)| (image_of(&rgba, picture.width, picture.height), delay)),
            )
            .collect();
        details.frame = 0;
        // The frames of the preview shown before go back to the system.
        unsafe { malloc_trim(0) };
        if details.frames.len() > 1 {
            // The first frame is on the screen: for its own time.
            details.arm(picture.delay);
        } else {
            details.frames.clear();
        }
    }

    /// Show the next frame of the selected item's preview and arm the timer for the one after.
    fn next_frame(&mut self) {
        let Some((image, delay)) = self.details.turn() else {
            return;
        };
        if let Some(ui) = self.ui() {
            ui.set_sel_image(image);
        }
        self.details.arm(delay);
    }

    /// The thumbs under the rating.
    pub(super) fn vote(&mut self, up: bool) {
        if let Some(card) = self.selected_card()
            && card.id != 0
        {
            let _ = self.steam.send(Command::SetVote { id: card.id, up });
        }
    }

    /// The author's other wallpapers, in the workshop.
    pub(super) fn browse_selected_author(&mut self) {
        if let Some(card) = self.selected_card() {
            let owner = self.catalog.owner_of(&card);
            self.browse_author(owner);
        }
    }

    /// The author's page in Steam.
    pub(super) fn open_author(&mut self) {
        if let Some(card) = self.selected_card() {
            let owner = self.catalog.owner_of(&card);
            let result = desktop::open_steam(&format!("steam://url/SteamIDPage/{owner}"));
            self.report(result, t("The author's page is open in Steam", "Страница автора открыта в Steam"));
        }
    }

    pub(super) fn open_steam(&mut self) {
        if let Some(card) = self.selected_card() {
            self.open_workshop_page(&card);
        }
    }

    /// The wallpaper's page in Steam.
    pub(super) fn open_workshop_page(&mut self, card: &Card) {
        let result = desktop::open_steam(&format!("steam://url/CommunityFilePage/{}", card.id));
        self.report(result, t("The page is open in Steam", "Страница открыта в Steam"));
    }

    pub(super) fn copy_link(&mut self) {
        if let Some(card) = self.selected_card() {
            let link = format!("https://steamcommunity.com/sharedfiles/filedetails/?id={}", card.id);
            self.report(desktop::copy(&link), t("The link is on the clipboard", "Ссылка в буфере обмена"));
        }
    }

    /// Reports are Steam's: its page of the wallpaper.
    pub(super) fn report_selected(&mut self) {
        if let Some(card) = self.selected_card() {
            let result = desktop::open_steam(&format!("steam://url/CommunityFilePage/{}", card.id));
            self.report(
                result,
                t(
                    "Reports are filed on the wallpaper's page in Steam: it is open",
                    "Жалобы подаются на странице обоев в Steam: она открыта",
                ),
            );
        }
    }

    pub(super) fn open_folder(&mut self) {
        if let Some(dir) = self.selected_card().and_then(|card| self.dir_of(&card)) {
            self.open_wallpaper_folder(&dir);
        }
    }

    pub(super) fn open_wallpaper_folder(&mut self, dir: &std::path::Path) {
        let result = desktop::open(&dir.to_string_lossy());
        self.report(result, t("The wallpaper folder is open", "Папка обоев открыта"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn what_says_what_an_item_is_is_not_a_pill() {
        let shown = pills(&strings(&["Scene", "1920 x 1080", "Approved", "HDR", "Workshop"]));
        assert_eq!(shown, ["1920 x 1080", "HDR"]);
    }

    #[test]
    fn pills_go_on_the_next_row_when_they_do_not_fit() {
        // "HDR" is 3 * 7 + 16 = 37 wide, "Landscape" 79.
        assert_eq!(pill_rows(strings(&["HDR", "HDR"]), 80), [strings(&["HDR", "HDR"])]);
        assert_eq!(
            pill_rows(strings(&["HDR", "HDR", "HDR"]), 80),
            [strings(&["HDR", "HDR"]), strings(&["HDR"])]
        );
        // One too wide for a row still gets one of its own.
        assert_eq!(
            pill_rows(strings(&["Landscape", "HDR"]), 50),
            [strings(&["Landscape"]), strings(&["HDR"])]
        );
        assert!(pill_rows(Vec::new(), 300).is_empty());
    }
}
