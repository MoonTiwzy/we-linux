//! Steam's answers, as they come back from its thread: each goes into the
//! catalogue, and what shows it is brought up to date.

use super::App;
use super::browse::INSTALLED;
use super::grid::image_of;
use crate::lang::t;
use crate::steam::Reply;
use crate::units::megabytes;
use crate::{log, tf};

/// What a list of the workshop says in place of its wallpapers while there
/// is no Steam: why, and that nothing needs doing here once it is fixed.
pub(super) fn offline(why: &str) -> String {
    tf!(
        "{why}. The list loads by itself once Steam is there.",
        "{why}. Список загрузится сам, когда Steam будет на месте."
    )
}

impl App {
    pub(super) fn steam_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Session(Ok(())) => {
                let was_missing = self.catalog.set_steam_ok(true);
                if was_missing {
                    log::info("Steam is back: the workshop works again");
                }
                if let Some(ui) = self.ui() {
                    ui.set_steam_ok(true);
                }
                // Steam came back: what could not be listed without it is
                // asked for again.
                if was_missing {
                    self.status(t(
                        "Steam is back: the workshop works again",
                        "Steam снова на месте: мастерская работает",
                    ));
                    if self.browse.online() {
                        self.reload();
                    }
                }
            }
            Reply::Session(Err(why)) => {
                // Said in the log once, when it changes: the thread says it again
                // every time it tries.
                if !self.catalog.steam_missing() {
                    log::warn(format!("Steam: {why}"));
                }
                self.catalog.set_steam_ok(false);
                if let Some(ui) = self.ui() {
                    ui.set_steam_ok(false);
                    if self.browse.online() && self.grid.cards().is_empty() {
                        ui.set_loading(false);
                        ui.set_error(offline(&why).into());
                    }
                }
                self.alarm(tf!(
                    "{why}. The workshop comes back by itself with Steam; installed wallpapers work as usual.",
                    "{why}. Мастерская вернётся сама вместе со Steam; установленные обои работают как обычно."
                ));
            }
            Reply::Page { generation, result } => self.page_arrived(generation, result),
            Reply::Details(entries) => {
                let installed_tab = self.browse.tab() == INSTALLED;
                let local_sort = self.browse.local_sort();
                self.catalog.add_details(entries);
                // The marks (approved) and the panel come from them now; the
                // orders by rating and by the last update, the list itself.
                if installed_tab && local_sort != 1 && local_sort != 5 {
                    self.show_selection(true);
                } else if installed_tab {
                    self.show_installed();
                }
            }
            Reply::Vote { id, vote } => {
                self.catalog.set_vote(id, vote.sign());
                self.show_selection(true);
            }
            Reply::Voted { id, result } => match result {
                Ok(vote) => {
                    self.catalog.set_vote(id, vote.sign());
                    self.show_selection(true);
                }
                Err(why) => {
                    self.warn(tf!("Steam did not take the vote: {why}", "Steam не принял оценку: {why}"))
                }
            },
            Reply::Favorite { id, on, result: Err(why) } => {
                // The heart was turned at once; Steam would not, so it goes back.
                self.catalog.set_favorite(id, !on);
                self.save();
                self.refresh_marks();
                self.show_selection(true);
                self.warn(tf!(
                    "Steam did not change the favourites: {why}",
                    "Steam не изменил избранное: {why}"
                ));
            }
            Reply::Favorite { result: Ok(()), .. } => {}
            Reply::Persona { id, persona } => {
                let avatar = persona.avatar.map(|(w, h, rgba)| image_of(&rgba, w, h));
                self.catalog.add_persona(id, persona.name, avatar);
                self.show_selection(true);
                if self.browse.author() == Some(id)
                    && let Some(ui) = self.ui()
                {
                    let name = self.catalog.persona_name(id);
                    ui.set_notice(tf!("Browsing wallpapers by {name}", "Смотрим все обои от {name}").into());
                }
            }
            Reply::Subscribed { id, result } => {
                self.catalog.subscribed(id, result.is_ok());
                match result {
                    Ok(()) => self.status(t(
                        "Subscribed, Steam is downloading the wallpaper",
                        "Подписка оформлена, Steam загружает обои",
                    )),
                    Err(why) => self.warn(tf!(
                        "Steam did not take the subscription: {why}",
                        "Steam не принял подписку: {why}"
                    )),
                }
                self.refresh_marks();
                self.show_selection(true);
            }
            Reply::Unsubscribed { id, result } => {
                self.catalog.unsubscribed(id, result.is_ok());
                match result {
                    Ok(()) => self.forget(id),
                    Err(why) => self.warn(tf!(
                        "Steam did not remove the subscription: {why}",
                        "Steam не снял подписку: {why}"
                    )),
                }
                self.refresh_marks();
            }
            Reply::Subscriptions(list) => {
                let changed = self.catalog.subscriptions(list);
                if self.browse.tab() == INSTALLED && changed {
                    self.show_installed();
                } else {
                    let catalog = &self.catalog;
                    self.grid.found_on_disk(|id| catalog.dir_of(id));
                    self.refresh_marks();
                }
                // What was just subscribed to is on the disk: on the desktop
                // it goes, when it is still what the user looks at.
                if let Some(id) = self.catalog.ready()
                    && self.grid.selected_id() == Some(id)
                {
                    self.show_selection(true);
                    self.apply();
                }
            }
            Reply::Progress { id, progress } => {
                let was_installed = self.catalog.dir_of(id).is_some();
                if self.catalog.downloading(id, progress) {
                    self.status(t("The wallpaper is downloaded", "Обои загружены"));
                    // It is among the installed ones from now on.
                    if self.browse.tab() == INSTALLED && !was_installed {
                        self.show_installed();
                    }
                } else if progress.total > 0 {
                    self.status(tf!(
                        "Steam is downloading: {} of {}",
                        "Steam загружает обои: {} из {}",
                        megabytes(progress.done),
                        megabytes(progress.total)
                    ));
                }
                self.refresh_marks();
            }
        }
    }
}
