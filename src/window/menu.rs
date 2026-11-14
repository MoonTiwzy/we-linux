//! A tile's menu, as the original builds it for the item under the pointer
//! (which stays unselected), and what its lines do.

use slint::{ComponentHandle, ModelRc, VecModel};
use we_linux_ui::{Glyph, Icons, MenuEntry};

use super::App;
use super::browse::INSTALLED;
use crate::lang::t;
use crate::steam::Command;
use crate::tf;

impl App {
    /// The menu of tile `index`: playlist, subscription, favourite, the
    /// workshop, the folder, the grid's view.
    pub(super) fn context_menu(&mut self, index: usize) {
        let Some(ui) = self.ui() else { return };
        let Some(card) = self.grid.card(index).cloned() else {
            return;
        };
        self.grid.set_context(index);
        let icons = ui.global::<Icons>();
        let line = |label: &str, icon: Glyph, id: &str, enabled: bool| MenuEntry {
            label: label.into(),
            icon,
            enabled,
            checked: false,
            divider: false,
            sub: -1,
            id: id.into(),
        };
        let divider = || MenuEntry { divider: true, enabled: true, sub: -1, ..Default::default() };
        let submenu = |label: &str, icon: Glyph, sub: i32| MenuEntry { sub, ..line(label, icon, "", true) };
        let dir = self.dir_of(&card);
        let steam_ok = self.catalog.steam_ok();
        let downloadable = dir.is_none() && !self.catalog.is_subscribed(card.id);
        let mut entries = Vec::new();
        if !downloadable {
            if dir.as_ref().is_some_and(|d| self.playlist.contains(d)) {
                entries.push(line(
                    t("Remove from playlist", "Удалить из плейлиста"),
                    icons.get_minus(),
                    "playlist-remove",
                    true,
                ));
            } else {
                entries.push(line(
                    t("Add to playlist", "Добавить в плейлист"),
                    icons.get_plus(),
                    "playlist-add",
                    dir.is_some(),
                ));
            }
        }
        if card.id != 0 {
            if downloadable {
                entries.push(line(
                    t("Subscribe", "Подписаться"),
                    icons.get_download(),
                    "subscribe",
                    steam_ok,
                ));
            } else {
                entries.push(line(
                    t("Unsubscribe", "Отписаться"),
                    icons.get_xmark(),
                    "unsubscribe",
                    steam_ok,
                ));
            }
            if self.catalog.is_favorite(card.id) {
                entries.push(line(
                    t("Remove from Favorites", "Удалить из избранного"),
                    icons.get_heart_regular(),
                    "unfavorite",
                    true,
                ));
            } else {
                entries.push(line(
                    t("Add to Favorites", "Добавить в избранное"),
                    icons.get_heart(),
                    "favorite",
                    true,
                ));
            }
            entries.push(divider());
            entries.push(line(
                t("Open in Workshop", "Открыть в Мастерской"),
                icons.get_steam(),
                "workshop",
                true,
            ));
            entries.push(submenu(t("Related Wallpapers", "Связанные обои"), icons.get_image(), 0));
            entries.push(submenu(
                t("Report & Block", "Пожаловаться и заблокировать"),
                icons.get_triangle_exclamation(),
                1,
            ));
        }
        if dir.is_some() {
            entries.push(divider());
            // A key that sets the wallpaper from anywhere is the compositor's to bind.
            entries.push(line(
                t("Assign Hotkey", "Назначить горячую клавишу"),
                icons.get_keyboard(),
                "hotkey",
                false,
            ));
            entries.push(line(
                t("Open in Explorer", "Открыть в Проводнике"),
                icons.get_folder_open(),
                "folder",
                true,
            ));
            entries.push(submenu(t("Play in Window", "Воспроизвести в окне"), icons.get_window_restore(), 2));
        }
        entries.push(divider());
        entries.push(submenu(t("View", "Вид"), icons.get_eye(), 3));

        let owner = self.catalog.owner_of(&card);
        let author = self.catalog.persona(owner).map_or("?".to_owned(), |(name, _)| name.clone());
        if owner != 0 && self.catalog.persona(owner).is_none() {
            let _ = self.steam.send(Command::Persona(owner));
        }
        let related = vec![line(
            &tf!("Browse all by {author} (?)", "Просмотреть всё от {author} (?)"),
            icons.get_user(),
            "author",
            owner != 0 && steam_ok,
        )];
        let report = vec![
            line(t("Report", "Сообщить"), icons.get_triangle_exclamation(), "report", true),
            line(&tf!("Block {author}", "Заблок. {author}"), icons.get_user_slash(), "block", false),
        ];
        // Playing a wallpaper in a window of its own is not something the renderer does.
        let window = vec![
            line(t("Full HD Preview", "Предпросмотр, Full HD"), icons.get_computer(), "", false),
            line(t("Phone Preview", "Предпросмотр, телефон"), icons.get_mobile_screen_button(), "", false),
            divider(),
            line(
                t("Maximized Preview", "Предпросмотр, на весь экран"),
                icons.get_window_maximize(),
                "",
                false,
            ),
        ];
        let checked = |entry: MenuEntry, on: bool| MenuEntry { checked: on, ..entry };
        let icon_size = self.grid.icon_size();
        let mut view = vec![
            checked(
                line(t("Small Icons", "Маленькие значки"), Glyph::default(), "icons-0", true),
                icon_size == 0,
            ),
            checked(
                line(t("Medium Icons", "Средние значки"), Glyph::default(), "icons-1", true),
                icon_size == 1,
            ),
            checked(
                line(t("Large Icons", "Большие значки"), Glyph::default(), "icons-2", true),
                icon_size == 2,
            ),
        ];
        if self.browse.tab() == INSTALLED {
            view.push(divider());
            for n in [50, 100, 200] {
                view.push(checked(
                    line(
                        &tf!("{n} per page", "{n} на страницу"),
                        Glyph::default(),
                        &format!("per-page-{n}"),
                        true,
                    ),
                    self.browse.per_page() == n,
                ));
            }
        }
        let model = |list: Vec<MenuEntry>| ModelRc::new(VecModel::from(list));
        ui.set_context_entries(model(entries));
        ui.set_context_submenus(ModelRc::new(VecModel::from(vec![
            model(related),
            model(report),
            model(window),
            model(view),
        ])));
    }

    /// A line of a tile's menu was picked. With wallpapers picked besides, the
    /// playlist, the subscription and the favourites are for all of them (the
    /// original's `z`); the rest is about the tile.
    pub(super) fn context_activated(&mut self, id: &str) {
        let Some(card) = self.grid.context_card() else {
            return;
        };
        let dir = self.dir_of(&card);
        match id {
            "playlist-add" | "playlist-remove" => self.playlist_put(Some(card.id), id == "playlist-add"),
            "subscribe" => self.subscribe_set(card.id),
            "unsubscribe" => {
                let set = self.set_for(Some(card.id), crate::selection::Takes::Subscribed);
                self.ask_unsubscribe(set);
            }
            "favorite" | "unfavorite" => self.favorite_set(card.id, id == "favorite"),
            "workshop" | "report" => self.open_workshop_page(&card),
            "author" => {
                let owner = self.catalog.owner_of(&card);
                self.browse_author(owner);
            }
            "folder" => {
                if let Some(dir) = dir {
                    self.open_wallpaper_folder(&dir);
                }
            }
            _ => {
                if let Some(size) = id.strip_prefix("icons-").and_then(|n| n.parse::<i32>().ok()) {
                    self.grid.set_icon_size(size);
                    if let Some(ui) = self.ui() {
                        ui.set_icon_size(size);
                    }
                    self.save();
                } else if let Some(n) = id.strip_prefix("per-page-").and_then(|n| n.parse::<usize>().ok()) {
                    self.browse.set_per_page(n);
                    self.save();
                    self.reload();
                }
            }
        }
    }
}
