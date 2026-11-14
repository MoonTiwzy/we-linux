//! Steam's answers, as they come back from its thread: each goes into the
//! catalogue, and what shows it is brought up to date.

use super::App;
use super::browse::INSTALLED;
use super::grid::image_of;
use crate::lang::t;
use crate::steam::Reply;
use crate::tf;
use crate::units::megabytes;

impl App {
    pub(super) fn steam_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Session(result) => {
                self.catalog.set_steam_ok(result.is_ok());
                if let Some(ui) = self.ui() {
                    ui.set_steam_ok(self.catalog.steam_ok());
                    if let Err(e) = result
                        && self.browse.online()
                        && self.grid.cards().is_empty()
                    {
                        ui.set_loading(false);
                        ui.set_error(
                            tf!(
                                "{e}. Start Steam: the list will load by itself.",
                                "{e}. Запусти Steam: список загрузится сам."
                            )
                            .into(),
                        );
                    }
                }
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
                self.catalog.set_vote(id, vote);
                self.show_selection(true);
            }
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
            Reply::Subscribed { id, ok } => {
                self.catalog.subscribed(id, ok);
                if ok {
                    self.status(t(
                        "Subscribed, Steam is downloading the wallpaper",
                        "Подписка оформлена, Steam загружает обои",
                    ));
                } else {
                    self.status(t("Steam did not take the subscription", "Steam не принял подписку"));
                }
                self.refresh_marks();
            }
            Reply::Unsubscribed { id, ok } => {
                self.catalog.unsubscribed(id, ok);
                if ok {
                    self.forget(id);
                } else {
                    self.status(t("Steam did not remove the subscription", "Steam не снял подписку"));
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
            Reply::Progress { id, state, done, total } => {
                let was_installed = self.catalog.dir_of(id).is_some();
                if self.catalog.downloading(id, state, done, total) {
                    self.status(t("The wallpaper is downloaded", "Обои загружены"));
                    // It is among the installed ones from now on.
                    if self.browse.tab() == INSTALLED && !was_installed {
                        self.show_installed();
                    }
                } else if total > 0 {
                    self.status(tf!(
                        "Steam is downloading: {} of {}",
                        "Steam загружает обои: {} из {}",
                        megabytes(done),
                        megabytes(total)
                    ));
                }
                self.refresh_marks();
            }
        }
    }
}
