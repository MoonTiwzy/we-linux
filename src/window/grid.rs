//! The grid: the tiles of the list shown, their previews (the ones in view
//! play when animated), the selection and what is picked besides it.
//!
//! The selection is an index into the tiles and lasts while the list does;
//! what is picked is kept by id, since the tiles are made anew whenever the
//! list is sorted or filtered. What a click does to them is `selection.rs`;
//! picking.rs puts it to the grid.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};
use we_linux_ui::Item;

use super::card::Card;
use super::tags::has_tag;
use super::{App, slot};
use crate::previews::{Decoded, Job, Previews};
use crate::selection;

/// A grid cell that plays: its frames with their delays, the one shown, and
/// when the next is due.
struct Cell {
    frames: Vec<(Image, u32)>,
    frame: usize,
    due: Instant,
}

pub(super) struct Grid {
    cards: Vec<Card>,
    /// The installed tab: every card the filters leave, sorted, before it is
    /// cut into pages (what Shift and Ctrl+A go through, as the original's do).
    listed: Vec<Card>,
    model: Rc<VecModel<Item>>,
    selected: Option<usize>,
    /// Picked besides the selection with Ctrl or Shift, by id.
    picked: Vec<u64>,
    /// Of the cells' previews asked for; older ones are dropped.
    picture_generation: u64,
    /// Grid cells whose animated preview plays, by id; the rows in view; the
    /// ids whose animation was asked for.
    cells: HashMap<u64, Cell>,
    cell_timer: Timer,
    visible: (usize, usize),
    animations_asked: HashSet<u64>,
    /// The grid's tiles: 0 small, 1 medium, 2 large (the grid's menu, "Вид").
    icon_size: i32,
    /// The tile whose menu is open.
    context: Option<usize>,
}

/// A decoded picture as the window shows it.
pub(super) fn image_of(rgba: &[u8], width: u32, height: u32) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, width, height))
}

impl Grid {
    pub(super) fn new(model: Rc<VecModel<Item>>) -> Self {
        Self {
            cards: Vec::new(),
            listed: Vec::new(),
            model,
            selected: None,
            picked: Vec::new(),
            picture_generation: 0,
            cells: HashMap::new(),
            cell_timer: Timer::default(),
            visible: (0, 40),
            animations_asked: HashSet::new(),
            icon_size: 1,
            context: None,
        }
    }

    // ------------------------------------------------------------- the tiles

    pub(super) fn cards(&self) -> &[Card] {
        &self.cards
    }

    pub(super) fn card(&self, index: usize) -> Option<&Card> {
        self.cards.get(index)
    }

    /// A card of the page or of the whole installed list, by id.
    pub(super) fn find(&self, id: u64) -> Option<&Card> {
        self.cards.iter().chain(&self.listed).find(|c| c.id == id)
    }

    /// Every card the filters leave on the installed tab (all its pages).
    pub(super) fn set_listed(&mut self, cards: Vec<Card>) {
        self.listed = cards;
    }

    /// Another list: what was picked in this one is not.
    pub(super) fn forget_list(&mut self) {
        self.picked.clear();
        self.listed.clear();
    }

    /// What the original's `We()` is: every item shown, in order, all pages of
    /// the installed tab.
    pub(super) fn shown_ids(&self, installed_tab: bool) -> Vec<u64> {
        let list = if installed_tab { &self.listed } else { &self.cards };
        list.iter().map(|c| c.id).collect()
    }

    /// An item's files are gone: its tiles are no longer installed.
    pub(super) fn not_installed(&mut self, id: u64) {
        for card in &mut self.cards {
            if card.id == id {
                card.dir = None;
            }
        }
    }

    /// Items have come on the disk: the tiles that were not there learn where they are.
    pub(super) fn found_on_disk(&mut self, dir_of: impl Fn(u64) -> Option<std::path::PathBuf>) {
        for card in &mut self.cards {
            if card.dir.is_none() {
                card.dir = dir_of(card.id);
            }
        }
    }

    pub(super) fn icon_size(&self) -> i32 {
        self.icon_size
    }

    pub(super) fn set_icon_size(&mut self, size: i32) {
        self.icon_size = size;
    }

    pub(super) fn context_card(&self) -> Option<Card> {
        self.context.and_then(|i| self.cards.get(i)).cloned()
    }

    pub(super) fn set_context(&mut self, index: usize) {
        self.context = Some(index);
    }

    // ------------------------------------------------------------- the selection

    pub(super) fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(super) fn selected_card(&self) -> Option<Card> {
        self.selected.and_then(|i| self.cards.get(i)).cloned()
    }

    pub(super) fn selected_id(&self) -> Option<u64> {
        self.selected.and_then(|i| self.cards.get(i)).map(|c| c.id)
    }

    /// Select a tile (or none), and nothing else: showing it is the caller's.
    pub(super) fn select(&mut self, index: Option<usize>) {
        self.selected = index;
    }

    pub(super) fn has_picks(&self) -> bool {
        !self.picked.is_empty()
    }

    pub(super) fn picked(&self) -> &[u64] {
        &self.picked
    }

    pub(super) fn clear_picks(&mut self) {
        self.picked.clear();
    }

    pub(super) fn unpick(&mut self, id: u64) {
        self.picked.retain(|p| *p != id);
    }

    pub(super) fn add_picks(&mut self, ids: &[u64]) {
        selection::add_all(&mut self.picked, ids);
    }

    /// Ctrl and a click on `id`; returns whether the selection was let go of.
    pub(super) fn toggle_pick(&mut self, id: u64) -> bool {
        let current = self.selected_id();
        let after = selection::toggle(&mut self.picked, current, id);
        self.let_go_unless(after, current)
    }

    /// Shift and a click on `id`: the run up to it, through `shown`.
    pub(super) fn extend_picks(&mut self, id: u64, shown: &[u64]) {
        let current = self.selected_id();
        selection::extend(&mut self.picked, current, id, shown);
    }

    /// A rectangle let go of over `ids`; returns whether the selection was let go of.
    pub(super) fn band_picks(&mut self, ids: &[u64], ctrl: bool, shift: bool) -> bool {
        let current = self.selected_id();
        let after = selection::band(&mut self.picked, current, ids, ctrl, shift);
        self.let_go_unless(after, current)
    }

    fn let_go_unless(&mut self, after: Option<u64>, current: Option<u64>) -> bool {
        if after != current {
            self.selected = None;
        }
        after != current
    }

    // ------------------------------------------------------------- previews

    /// The cells in view: their animated previews are asked for, the ones
    /// out of view stop (their frames go).
    fn animate_visible(&mut self, previews: &Previews) {
        let (first, last) = self.visible;
        let shown: HashSet<u64> =
            self.cards.iter().skip(first).take(last.saturating_sub(first)).map(|c| c.id).collect();
        self.cells.retain(|id, _| shown.contains(id));
        // A cell out of view asks again when it comes back.
        self.animations_asked.retain(|id| shown.contains(id));
        let generation = self.picture_generation;
        for card in self.cards.iter().skip(first).take(last.saturating_sub(first)) {
            if !self.animations_asked.insert(card.id) {
                continue;
            }
            let Some(source) = card.picture.source() else {
                continue;
            };
            previews.request(Job { id: card.id, source, size: 240, generation, large: false, animate: true });
        }
    }

    /// One timer turns the frames of every cell that plays, each at its own pace.
    fn arm_cells(&mut self) {
        if self.cells.is_empty() || self.cell_timer.running() {
            return;
        }
        self.cell_timer.start(TimerMode::Repeated, Duration::from_millis(15), || {
            slot::try_with(|app| app.grid.turn_cells());
        });
    }

    fn turn_cells(&mut self) {
        if self.cells.is_empty() {
            self.cell_timer.stop();
            return;
        }
        let now = Instant::now();
        let mut turned = Vec::new();
        for (id, cell) in &mut self.cells {
            if cell.due > now {
                continue;
            }
            cell.frame = (cell.frame + 1) % cell.frames.len();
            let (image, delay) = cell.frames[cell.frame].clone();
            cell.due = now + Duration::from_millis(u64::from(delay));
            turned.push((*id, image));
        }
        for (id, image) in turned {
            if let Some(row) = self.cards.iter().position(|c| c.id == id)
                && let Some(mut item) = self.model.row_data(row)
            {
                item.image = image;
                self.model.set_row_data(row, item);
            }
        }
    }

    /// A cell's preview: shown, and played when it is animated and in view.
    fn picture_arrived(&mut self, picture: Decoded, image: Image) {
        if picture.generation != self.picture_generation {
            return;
        }
        let Some(row) = self.cards.iter().position(|c| c.id == picture.id) else {
            return;
        };
        // An animated preview of a cell in view plays, as the original's do.
        if !picture.frames.is_empty() {
            let (first, last) = self.visible;
            if row < first || row >= last {
                return;
            }
            let (width, height) = (picture.width, picture.height);
            let frames: Vec<(Image, u32)> = std::iter::once((image.clone(), picture.delay))
                .chain(
                    picture.frames.into_iter().map(|(rgba, delay)| (image_of(&rgba, width, height), delay)),
                )
                .collect();
            let due = Instant::now() + Duration::from_millis(u64::from(picture.delay));
            self.cells.insert(picture.id, Cell { frames, frame: 0, due });
            self.arm_cells();
        }
        if let Some(mut item) = self.model.row_data(row) {
            item.image = image;
            item.loaded = true;
            self.model.set_row_data(row, item);
        }
    }
}

impl App {
    /// A tile as the grid shows it: the card and the marks the catalogue
    /// and the picking give it.
    fn item_of(&self, card: &Card) -> Item {
        let (state, state_text) = self.tile_state(self.dir_of(card).as_deref());
        Item {
            title: card.title.as_str().into(),
            image: Image::default(),
            loaded: false,
            kind: card.kind.as_str().into(),
            approved: has_tag(&card.tags, "Approved"),
            subscribed: self.browse.online() && self.catalog.is_subscribed(card.id),
            favorite: self.catalog.is_favorite(card.id),
            progress: self.catalog.progress(card.id),
            picked: self.grid.picked.contains(&card.id),
            state,
            state_text: state_text.into(),
        }
    }

    /// A new list in the grid. The selection stays with its card when the
    /// card is still in it.
    pub(super) fn set_cards(&mut self, cards: Vec<Card>) {
        let keep = self.grid.selected_id();
        self.grid.cards = cards;
        self.grid.picture_generation = self.previews.next_generation();
        let items: Vec<Item> = self.grid.cards.iter().map(|c| self.item_of(c)).collect();
        self.grid.model.set_vec(items);
        for card in &self.grid.cards {
            let Some(source) = card.picture.source() else {
                continue;
            };
            self.previews.request(Job {
                id: card.id,
                source,
                size: 268,
                generation: self.grid.picture_generation,
                large: false,
                animate: false,
            });
        }
        self.grid.cells.clear();
        self.grid.animations_asked.clear();
        self.grid.cell_timer.stop();
        self.grid.animate_visible(&self.previews);
        let selected = keep.and_then(|id| self.grid.cards.iter().position(|c| c.id == id));
        let same = selected.is_some();
        self.grid.selected = selected;
        self.show_selection(same);
    }

    /// A preview decoded: the side panel's, the playlist's or a cell's.
    pub(super) fn picture_arrived(&mut self, picture: Decoded) {
        let image = image_of(&picture.rgba, picture.width, picture.height);
        if picture.large {
            self.large_preview_arrived(picture, image);
        } else if picture.generation == 0 {
            // The playlist strip's thumbnails are asked for with generation 0.
            self.playlist_thumbnail(picture.id, image);
        } else {
            self.grid.picture_arrived(picture, image);
        }
    }

    /// The rows of the grid in view changed (it was scrolled or resized).
    pub(super) fn visible_changed(&mut self, first: usize, last: usize) {
        self.grid.visible = (first, last.min(self.grid.cards.len()));
        self.grid.animate_visible(&self.previews);
    }

    /// Bring the marks of every cell (subscribed, on the desktop, progress) up to date.
    pub(super) fn refresh_marks(&mut self) {
        for (row, card) in self.grid.cards.iter().enumerate() {
            let Some(mut item) = self.grid.model.row_data(row) else {
                continue;
            };
            let fresh = self.item_of(card);
            if item.subscribed != fresh.subscribed
                || item.progress != fresh.progress
                || item.favorite != fresh.favorite
                || item.picked != fresh.picked
                || item.state != fresh.state
                || item.state_text != fresh.state_text
            {
                item.subscribed = fresh.subscribed;
                item.progress = fresh.progress;
                item.favorite = fresh.favorite;
                item.picked = fresh.picked;
                item.state = fresh.state;
                item.state_text = fresh.state_text;
                self.grid.model.set_row_data(row, item);
            }
        }
        self.show_selection(true);
    }
}
