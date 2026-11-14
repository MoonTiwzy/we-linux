//! Subscribing, unsubscribing (after the original's question) and the
//! favourites: what is asked of Steam about the wallpapers themselves. Its
//! answers come back through replies.rs.

use super::App;
use super::browse::INSTALLED;
use crate::lang::{self, t};
use crate::selection::{self, Status, Takes};
use crate::steam::Command;
use crate::{library, tf};

/// The question of dialog 14, while it is on the screen: what goes (with
/// the presets of it) and what the user asked about.
pub(super) struct Unsubscribing {
    going: Vec<u64>,
    asked: Vec<u64>,
}

impl App {
    /// The details' "Подписаться".
    pub(super) fn subscribe(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        if self.catalog.start(card.id) {
            self.status(tf!("Subscribing to \"{}\"…", "Подписка на «{}»…", card.title));
            let _ = self.steam.send(Command::Subscribe(card.id));
            self.catalog.set_when_ready(card.id);
            self.show_selection(true);
        }
    }

    /// A tile's menu, "Подписаться": every wallpaper it is for that is still to be had.
    pub(super) fn subscribe_set(&mut self, target: u64) {
        for id in self.set_for(Some(target), Takes::Only(Status::Downloadable)) {
            if self.catalog.start(id) {
                let _ = self.steam.send(Command::Subscribe(id));
            }
        }
        self.refresh_marks();
    }

    /// A double click: set what is on the disk, subscribe to what is not.
    pub(super) fn activate(&mut self, index: usize) {
        // A wallpaper that is here was set by the click that selected it.
        self.select(index);
        let Some(card) = self.selected_card() else {
            return;
        };
        if self.dir_of(&card).is_some() {
        } else if !self.catalog.is_subscribed(card.id) {
            self.subscribe();
        } else if self.catalog.is_downloading(card.id) {
            // Its download is watched: on the desktop it goes when it lands.
            self.catalog.set_when_ready(card.id);
            self.status(tf!(
                "\"{}\" is still downloading: it goes on the desktop when it is here",
                "«{}» ещё скачиваются: встанут на рабочий стол, как только загрузятся",
                card.title
            ));
        } else {
            self.status(t("Steam has not downloaded this wallpaper yet", "Steam ещё не скачал эти обои"));
        }
    }

    /// The details' heart.
    pub(super) fn toggle_favorite(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let on = !self.catalog.is_favorite(card.id);
        self.catalog.set_favorite(card.id, on);
        // The original's heart is the workshop's favourite.
        if card.id != 0 {
            let _ = self.steam.send(Command::Favorite { id: card.id, on });
        }
        self.save();
        self.refresh_marks();
    }

    /// A tile's menu, the heart: all the wallpapers it is for as the tile is not.
    pub(super) fn favorite_set(&mut self, target: u64, on: bool) {
        // The original's heart is the workshop's favourite.
        for id in self.set_for(Some(target), Takes::Any) {
            self.catalog.set_favorite(id, on);
            let _ = self.steam.send(Command::Favorite { id, on });
        }
        self.save();
        self.refresh_marks();
    }

    /// An item was unsubscribed from: off the desktop, off the disk and out
    /// of what is shown, now and not when Steam gets round to it.
    pub(super) fn forget(&mut self, id: u64) {
        self.catalog.mark_gone(id);
        let mut left_over = None;
        if let Some(dir) = self.catalog.dir_of(id) {
            // A wallpaper that is on the desktop cannot stay there without its files.
            self.desk.let_go_of(&dir);
            left_over = library::remove(&dir).err();
        }
        match left_over {
            None => self
                .status(t("Unsubscribed and removed from the disk", "Подписка снята, обои удалены с диска")),
            Some(why) => self
                .status(tf!("Unsubscribed; the files stay: {why}", "Подписка снята; файлы остались: {why}")),
        }
        self.catalog.drop_installed(id);
        self.grid.unpick(id);
        if self.browse.tab() == INSTALLED {
            // The selection stays where it is not what went (the cards keep it by id).
            self.show_installed();
        } else {
            self.grid.not_installed(id);
        }
        self.show_selection(true);
        self.show_monitors();
    }

    /// The details' button: the selected wallpaper alone, after asking.
    pub(super) fn unsubscribe(&mut self) {
        if let Some(card) = self.selected_card() {
            self.ask_unsubscribe(vec![card.id]);
        }
    }

    /// Unsubscribe from `ids` and the presets that depend on them, after
    /// asking (the original's `Ye`; every caller of it here asks first).
    pub(super) fn ask_unsubscribe(&mut self, ids: Vec<u64>) {
        let ids: Vec<u64> = ids.into_iter().filter(|id| *id != 0).collect();
        if ids.is_empty() {
            return;
        }
        if !self.catalog.steam_ok() {
            self.status(t(
                "Steam connection could not be initialized: start Steam and try again",
                "Соединение Steam не может быть инициализировано: запусти Steam и попробуй ещё раз",
            ));
            return;
        }
        self.ask(ids);
    }

    /// Put the question of dialog 14: what goes and the presets that go with it.
    pub(super) fn ask(&mut self, ids: Vec<u64>) {
        let Some(ui) = self.ui() else { return };
        let going = self.catalog.with_presets(&ids);
        let titles: Vec<String> = ids.iter().map(|id| self.title_of(*id)).collect();
        let presets: Vec<String> = going[ids.len()..].iter().map(|id| self.title_of(*id)).collect();
        let question = selection::question(&titles, &presets, lang::russian());
        ui.set_unsubscribe_question(
            slint::StyledText::from_markdown(&question)
                .unwrap_or_else(|_| slint::StyledText::from_plain_text(&question.replace("**", ""))),
        );
        self.unsubscribing = Some(Unsubscribing { going, asked: ids });
        ui.set_dialog(14);
    }

    /// The answer to dialog 14.
    pub(super) fn unsubscribe_answered(&mut self, ok: bool) {
        if let Some(ui) = self.ui() {
            ui.set_dialog(0);
        }
        if let Some(Unsubscribing { going, asked }) = self.unsubscribing.take()
            && ok
        {
            self.unsubscribe_now(going, &asked);
        }
    }

    /// Steam is told to drop `going`; `asked` is what the user chose of it.
    /// Each item leaves the disk and the lists when Steam says it is done
    /// (`forget`); the selection goes now if it was among them.
    fn unsubscribe_now(&mut self, going: Vec<u64>, asked: &[u64]) {
        let mut sent = Vec::new();
        for &id in &going {
            if self.catalog.start(id) {
                let _ = self.steam.send(Command::Unsubscribe(id));
                self.catalog.not_when_ready(id);
                sent.push(id);
            }
        }
        match sent.as_slice() {
            [] => return,
            [one] => {
                let title = self.title_of(*one);
                self.status(tf!("Unsubscribing from \"{}\"…", "Отписка от «{}»…", title));
            }
            many => {
                self.status(tf!("Unsubscribing from {} wallpapers…", "Отписка от обоев: {}…", many.len()))
            }
        }
        // As the original: on the installed tab the selection is let go of, and
        // the small window moves on to the next wallpaper instead.
        if let Some(current) = self.grid.selected_id()
            && asked.contains(&current)
            && self.browse.tab() == INSTALLED
        {
            let page: Vec<u64> = self.grid.cards().iter().map(|c| c.id).collect();
            let mini = self.ui().is_some_and(|ui| ui.get_mini());
            let next = selection::after_removal(&page, current, &going)
                .filter(|_| mini)
                .and_then(|id| page.iter().position(|p| *p == id));
            match next {
                Some(index) => self.select(index),
                None => {
                    self.grid.select(None);
                    self.show_selection(false);
                    self.show_presets();
                }
            }
        }
        self.refresh_marks();
    }
}
