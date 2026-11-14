//! What the window knows of the wallpapers besides the list it shows: what
//! is on the disk, and what Steam told of them (subscriptions and downloads,
//! the workshop's details, the authors, the user's votes and favourites).
//! It is filled by Steam's answers (replies.rs) and read by every part that
//! shows a wallpaper.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use slint::Image;

use super::card::Card;
use super::listing;
use crate::library::{self, Installed};
use crate::selection;
use crate::steam::{self, ItemState, Progress};

pub(super) struct Catalog {
    installed: Vec<Installed>,
    /// Unsubscribed from in this run: not shown as installed even while
    /// Steam still lists it.
    gone: HashSet<u64>,
    /// Items' states by id, as Steam last told.
    subscribed: HashMap<u64, ItemState>,
    progress: HashMap<u64, f32>,
    /// Asked of Steam (subscribing, unsubscribing) and not answered yet.
    busy: HashSet<u64>,
    /// The wallpaper subscribed to last: it is set once Steam has it, if it
    /// is still the selected one.
    set_when_ready: Option<u64>,
    favorites: HashSet<u64>,
    /// The workshop's details of installed items (votes, size, author), by id,
    /// and the ids asked for already.
    workshop_info: HashMap<u64, steam::Entry>,
    asked_details: HashSet<u64>,
    /// Authors: name and avatar, by Steam id; and the ones asked for.
    personas: HashMap<u64, (String, Option<Image>)>,
    asked_personas: HashSet<u64>,
    /// The user's votes (1 up, -1 down), by item, as Steam told.
    votes: HashMap<u64, i32>,
    /// What installed folders take on the disk, counted when sorted by it.
    sizes: HashMap<u64, u64>,
    /// Steam is there to be asked.
    steam_ok: bool,
    /// Steam said it was not there, and has not come back since.
    steam_missing: bool,
}

/// What a folder takes on the disk, its files counted.
fn folder_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => folder_size(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

impl Catalog {
    /// What is on the disk now; Steam has told nothing yet.
    pub(super) fn from_disk() -> Self {
        Self {
            installed: library::installed(),
            gone: HashSet::new(),
            subscribed: HashMap::new(),
            progress: HashMap::new(),
            busy: HashSet::new(),
            set_when_ready: None,
            favorites: HashSet::new(),
            workshop_info: HashMap::new(),
            asked_details: HashSet::new(),
            personas: HashMap::new(),
            asked_personas: HashSet::new(),
            votes: HashMap::new(),
            sizes: HashMap::new(),
            steam_ok: false,
            steam_missing: false,
        }
    }

    // ------------------------------------------------------------- the disk

    pub(super) fn installed(&self) -> &[Installed] {
        &self.installed
    }

    /// Where an installed item is.
    pub(super) fn dir_of(&self, id: u64) -> Option<PathBuf> {
        self.installed.iter().find(|i| i.id == id).map(|i| i.dir.clone())
    }

    /// The installed item in a folder.
    pub(super) fn at(&self, dir: &Path) -> Option<&Installed> {
        self.installed.iter().find(|i| i.dir == dir)
    }

    pub(super) fn title_of(&self, id: u64) -> Option<String> {
        self.installed.iter().find(|i| i.id == id).map(|i| i.title.clone())
    }

    /// What is on the disk, without what was unsubscribed from here: Steam
    /// may still think of such an item as installed for a while.
    fn on_disk(&self) -> Vec<Installed> {
        library::installed().into_iter().filter(|item| !self.gone.contains(&item.id)).collect()
    }

    /// Look at the disk again.
    pub(super) fn reread(&mut self) {
        self.installed = self.on_disk();
    }

    /// The genres the installed wallpapers carry most.
    pub(super) fn taste(&self) -> Vec<String> {
        listing::taste(&self.installed)
    }

    /// `ids` and the installed presets of them.
    pub(super) fn with_presets(&self, ids: &[u64]) -> Vec<u64> {
        let installed: Vec<(u64, Option<u64>)> =
            self.installed.iter().map(|i| (i.id, i.dependency)).collect();
        let mut going = ids.to_vec();
        going.extend(selection::presets_of(ids, &installed));
        going
    }

    /// An item was unsubscribed from: it is gone from now on, whatever Steam
    /// still lists.
    pub(super) fn mark_gone(&mut self, id: u64) {
        self.gone.insert(id);
    }

    /// Its files are off the disk: out of the installed and the favourites.
    pub(super) fn drop_installed(&mut self, id: u64) {
        self.installed.retain(|i| i.id != id);
        self.favorites.remove(&id);
    }

    /// The folders' sizes of `cards`, counted once each.
    pub(super) fn count_sizes(&mut self, cards: &[Card]) {
        for card in cards {
            if let Some(dir) = &card.dir {
                self.sizes.entry(card.id).or_insert_with(|| folder_size(dir));
            }
        }
    }

    // ------------------------------------------------------------- Steam's word

    pub(super) fn steam_ok(&self) -> bool {
        self.steam_ok
    }

    /// Whether Steam was found missing already (not only not found yet).
    pub(super) fn steam_missing(&self) -> bool {
        self.steam_missing
    }

    /// Whether Steam is there now. Returns whether it had been missing:
    /// true when it comes back, not when it is there from the start.
    pub(super) fn set_steam_ok(&mut self, ok: bool) -> bool {
        let was_missing = self.steam_missing;
        self.steam_ok = ok;
        self.steam_missing = !ok;
        was_missing && ok
    }

    /// Its state, none when Steam said nothing of it.
    pub(super) fn state(&self, id: u64) -> ItemState {
        self.subscribed.get(&id).copied().unwrap_or_default()
    }

    pub(super) fn is_subscribed(&self, id: u64) -> bool {
        self.state(id).subscribed()
    }

    /// How far its download is, -1 when none is watched.
    pub(super) fn progress(&self, id: u64) -> f32 {
        self.progress.get(&id).copied().unwrap_or(-1.0)
    }

    pub(super) fn is_downloading(&self, id: u64) -> bool {
        self.progress.contains_key(&id)
    }

    pub(super) fn is_busy(&self, id: u64) -> bool {
        self.busy.contains(&id)
    }

    /// Steam is asked about it; false when it already was and has not answered.
    pub(super) fn start(&mut self, id: u64) -> bool {
        self.busy.insert(id)
    }

    /// The wallpaper to set once Steam has it on the disk.
    pub(super) fn set_when_ready(&mut self, id: u64) {
        self.set_when_ready = Some(id);
    }

    /// Not to be set by itself whenever it gets to the disk.
    pub(super) fn not_when_ready(&mut self, id: u64) {
        if self.set_when_ready == Some(id) {
            self.set_when_ready = None;
        }
    }

    /// The wallpaper waited for, once it is on the disk (and no longer waited for).
    pub(super) fn ready(&mut self) -> Option<u64> {
        let id = self.set_when_ready.filter(|id| self.installed.iter().any(|i| i.id == *id))?;
        self.set_when_ready = None;
        Some(id)
    }

    /// Steam's answer to a subscription.
    pub(super) fn subscribed(&mut self, id: u64, ok: bool) {
        self.busy.remove(&id);
        if ok {
            let state = self.subscribed.entry(id).or_default();
            *state = state.with(ItemState::SUBSCRIBED);
            self.progress.insert(id, 0.0);
        } else {
            self.progress.remove(&id);
            self.not_when_ready(id);
        }
    }

    /// Steam's answer to an unsubscription.
    pub(super) fn unsubscribed(&mut self, id: u64, ok: bool) {
        self.busy.remove(&id);
        self.progress.remove(&id);
        self.not_when_ready(id);
        if ok {
            self.subscribed.remove(&id);
        }
    }

    /// Every subscription, as Steam lists them; the disk read again. Returns
    /// whether what is installed changed.
    pub(super) fn subscriptions(&mut self, list: Vec<(u64, ItemState)>) -> bool {
        self.subscribed = list.into_iter().collect();
        let on_disk = self.on_disk();
        let ids = |list: &[Installed]| list.iter().map(|i| i.id).collect::<HashSet<u64>>();
        let changed = ids(&on_disk) != ids(&self.installed);
        self.installed = on_disk;
        changed
    }

    /// A download moved on: its state now, and how much of it is here.
    /// Returns whether it is finished.
    pub(super) fn downloading(&mut self, id: u64, Progress { state, done, total }: Progress) -> bool {
        self.subscribed.insert(id, state);
        let finished = state.finished();
        if finished {
            self.progress.remove(&id);
            self.gone.remove(&id);
        } else if total > 0 {
            self.progress.insert(id, done as f32 / total as f32);
        }
        finished
    }

    // ------------------------------------------------------------- favourites

    pub(super) fn is_favorite(&self, id: u64) -> bool {
        self.favorites.contains(&id)
    }

    pub(super) fn favorites(&self) -> &HashSet<u64> {
        &self.favorites
    }

    pub(super) fn set_favorite(&mut self, id: u64, on: bool) {
        if on {
            self.favorites.insert(id);
        } else {
            self.favorites.remove(&id);
        }
    }

    /// The favourites in order, as they are saved.
    pub(super) fn favorites_sorted(&self) -> Vec<u64> {
        let mut favorites: Vec<u64> = self.favorites.iter().copied().collect();
        favorites.sort_unstable();
        favorites
    }

    pub(super) fn set_favorites(&mut self, favorites: Vec<u64>) {
        self.favorites = favorites.into_iter().collect();
    }

    // ------------------------------------------------------------- the workshop's details

    pub(super) fn info(&self, id: u64) -> Option<&steam::Entry> {
        self.workshop_info.get(&id)
    }

    /// The tags the workshop gives an item (none when it was not asked).
    pub(super) fn workshop_tags(&self, id: u64) -> &[String] {
        self.workshop_info.get(&id).map(|e| e.tags.as_slice()).unwrap_or(&[])
    }

    /// Its author: the workshop's word, else the card's.
    pub(super) fn owner_of(&self, card: &Card) -> u64 {
        self.workshop_info.get(&card.id).map_or(card.owner, |e| e.owner)
    }

    pub(super) fn add_details(&mut self, entries: Vec<steam::Entry>) {
        for entry in entries {
            self.workshop_info.insert(entry.id, entry);
        }
    }

    /// The installed items the workshop was not asked about yet: asked about now.
    pub(super) fn ask_details(&mut self) -> Vec<u64> {
        let unknown: Vec<u64> = self
            .installed
            .iter()
            .map(|i| i.id)
            .filter(|id| !self.workshop_info.contains_key(id) && !self.asked_details.contains(id))
            .collect();
        self.asked_details.extend(unknown.iter().copied());
        unknown
    }

    // ------------------------------------------------------------- authors and votes

    pub(super) fn persona(&self, id: u64) -> Option<&(String, Option<Image>)> {
        self.personas.get(&id)
    }

    /// An author's name, "?" until Steam says.
    pub(super) fn persona_name(&self, id: u64) -> &str {
        self.personas.get(&id).map_or("?", |(name, _)| name.as_str())
    }

    /// Whether to ask Steam about an author: only the first time.
    pub(super) fn first_ask_for_persona(&mut self, id: u64) -> bool {
        self.asked_personas.insert(id)
    }

    pub(super) fn add_persona(&mut self, id: u64, name: String, avatar: Option<Image>) {
        self.personas.insert(id, (name, avatar));
    }

    /// The user's vote, 0 when none (or not known).
    pub(super) fn vote(&self, id: u64) -> i32 {
        self.votes.get(&id).copied().unwrap_or(0)
    }

    pub(super) fn knows_vote(&self, id: u64) -> bool {
        self.votes.contains_key(&id)
    }

    pub(super) fn set_vote(&mut self, id: u64, vote: i32) {
        self.votes.insert(id, vote);
    }

    /// What the installed tab's orders go by, of a card.
    pub(super) fn facts(&self, card: &Card) -> listing::Facts {
        let info = self.workshop_info.get(&card.id);
        listing::Facts {
            rating: info.map_or(0.0, |e| listing::votes(e.up, e.down)),
            favorite: self.favorites.contains(&card.id),
            size: self.sizes.get(&card.id).copied().unwrap_or(0),
            updated: info.map_or(card.updated, |e| u64::from(e.updated)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_counts_as_back_only_after_it_was_missing() {
        let mut catalog = Catalog::from_disk();
        // There from the start: nothing to ask again.
        assert!(!catalog.set_steam_ok(true));
        assert!(!catalog.set_steam_ok(false));
        assert!(!catalog.steam_ok());
        assert!(catalog.set_steam_ok(true));
        assert!(!catalog.set_steam_ok(true));
    }

    #[test]
    fn a_refused_subscription_is_no_longer_busy_or_downloading() {
        let mut catalog = Catalog::from_disk();
        assert!(catalog.start(5));
        assert!(!catalog.start(5));
        catalog.subscribed(5, false);
        assert!(!catalog.is_busy(5) && !catalog.is_subscribed(5) && !catalog.is_downloading(5));
        assert!(catalog.start(6));
        catalog.subscribed(6, true);
        assert!(catalog.is_subscribed(6) && catalog.is_downloading(6));
        let done = ItemState::SUBSCRIBED.with(ItemState::INSTALLED);
        assert!(catalog.downloading(6, Progress { state: done, done: 9, total: 9 }));
        assert!(!catalog.is_downloading(6));
    }
}
