//! The window: the library, the workshop, the details of a wallpaper, the
//! playlist, the monitors and the settings, laid out as the Windows client's.
//! Its look is `ui/app.slint`; its state is [`App`], made of parts that each
//! own a piece of it, with the code that works on that piece:
//!
//! - `catalog`: what is known of the wallpapers: what is on the disk, and
//!   what Steam told (subscriptions, downloads, details, authors, votes, favourites).
//! - `browse`: which list is shown: the tab, the sorting, the search, the
//!   page; the workshop's and "Рекомендации"'s questions to Steam.
//! - `filters`: the filter panel and what it asks of a list.
//! - `grid`: the tiles shown, their previews, the selection and the picked ones.
//! - `details`: the panel at the right, of the selected wallpaper.
//! - `properties`: that wallpaper's properties and presets.
//! - `playlist`: the playlist and its strip.
//! - `desk`: what is on the desktop, the monitor chooser, "Отмена".
//! - `settings`: the settings dialog and the language.
//!
//! Besides them: the flows that are about several parts at once (`picking`:
//! clicks, Ctrl, Shift, Ctrl+A; `subscriptions`; `menu`: a tile's menu;
//! `replies`: Steam's answers), the state kept between runs (`saved`), and
//! what has no window in it and is tested on its own (`card`, `listing`,
//! `search`, `tags`).
//!
//! How the parts talk: a part's fields are private to its module, and no
//! part holds another. What crosses parts is a method of `App`, next to the
//! part it is mostly about, which asks one part through its methods and
//! tells another. `App` is the one place they meet.
//!
//! How the window reaches the app: `slot` (one thread-local, and why),
//! `wiring` (every callback), `debug` (`--show`).

mod browse;
mod card;
mod catalog;
mod debug;
mod desk;
mod details;
mod filters;
mod grid;
mod listing;
mod menu;
mod picking;
mod playlist;
mod properties;
mod replies;
mod saved;
mod search;
mod settings;
mod slot;
mod subscriptions;
mod tags;
mod wiring;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use we_linux_ui::{AppWindow, Item, L, PlaylistEntry, Theme};

use browse::Browse;
use card::Card;
use catalog::Catalog;
use desk::Desk;
use details::Details;
use filters::Filters;
use grid::Grid;
use playlist::PlaylistPanel;
use properties::Properties;
use settings::Language;
use subscriptions::Unsubscribing;

use crate::lang::{self, t};
use crate::previews::Previews;
use crate::steam::{self, Command};
use crate::{desktop, engine, library, tf};

pub(super) struct App {
    ui: Weak<AppWindow>,
    /// Questions to Steam's thread; its answers come back to `steam_reply`.
    steam: Sender<Command>,
    /// Previews decoded on threads of their own; they come back to `picture_arrived`.
    previews: Previews,
    catalog: Catalog,
    browse: Browse,
    filters: Filters,
    grid: Grid,
    details: Details,
    properties: Properties,
    playlist: PlaylistPanel,
    desk: Desk,
    language: Language,
    /// The question before unsubscribing, while it is asked.
    unsubscribing: Option<Unsubscribing>,
}

impl App {
    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    /// The line at the top left: what was done last.
    fn status(&self, text: impl Into<SharedString>) {
        if let Some(ui) = self.ui() {
            ui.set_status_error(false);
        }
        if let Some(ui) = self.ui() {
            ui.set_status(text.into());
        }
    }

    /// The line at the top left, in the colour of what went wrong.
    fn warn(&self, text: impl Into<SharedString>) {
        if let Some(ui) = self.ui() {
            ui.set_status(text.into());
            ui.set_status_error(true);
        }
    }

    /// `done` when it worked, why not when it did not.
    fn report(&self, result: Result<(), String>, done: &str) {
        match result {
            Ok(()) => self.status(done),
            Err(e) => self.status(tf!("That did not work: {e}", "Не получилось: {e}")),
        }
    }

    fn selected_card(&self) -> Option<Card> {
        self.grid.selected_card()
    }

    /// Where a wallpaper is on the disk: its card knows, or the catalogue does.
    fn dir_of(&self, card: &Card) -> Option<PathBuf> {
        card.dir.clone().or_else(|| self.catalog.dir_of(card.id))
    }

    /// An item's title, wherever it is listed.
    fn title_of(&self, id: u64) -> String {
        self.grid
            .find(id)
            .map(|c| c.title.clone())
            .or_else(|| self.catalog.title_of(id))
            .unwrap_or_else(|| id.to_string())
    }
}

/// Open the window and run it until it closes.
pub fn run() -> Result<(), slint::PlatformError> {
    // A wallpaper is set and nothing shows it (the daemon was not started
    // with the session, or went away): it comes back with the window.
    if desktop::current().is_some() {
        let _ = engine::ensure_daemon();
    }

    let ui = AppWindow::new()?;
    // The window's Wayland app id: what compositor rules and the menu entry
    // match. Only after the window is made (there is no platform before that)
    // and before it shows.
    slint::set_xdg_app_id("we-linux")?;
    // The language first: every label made below is in it. Until the first
    // run asks, it is the system's.
    let language = saved::language();
    let russian = language.as_deref().map_or_else(lang::system_is_russian, |l| l == "ru");
    lang::set_russian(russian);
    ui.global::<L>().set_russian(russian);
    ui.global::<Theme>().set_font(desktop::ui_font().into());
    ui.set_caption(format!("WE Linux {}", env!("CARGO_PKG_VERSION")).into());
    ui.set_about(settings::about().into());
    if language.is_none() {
        ui.set_dialog(9);
    }

    let model = Rc::new(VecModel::<Item>::default());
    let playlist_model = Rc::new(VecModel::<PlaylistEntry>::default());
    ui.set_items(ModelRc::from(model.clone()));
    ui.set_playlist(ModelRc::from(playlist_model.clone()));

    slot::install(App {
        ui: ui.as_weak(),
        steam: steam::spawn(|reply| slot::on_ui(move |app| app.steam_reply(reply))),
        previews: Previews::new(|picture| slot::on_ui(move |app| app.picture_arrived(picture))),
        catalog: Catalog::from_disk(),
        browse: Browse::default(),
        filters: Filters::default(),
        grid: Grid::new(model),
        details: Details::default(),
        properties: Properties::default(),
        playlist: PlaylistPanel::load(playlist_model),
        desk: Desk::now(),
        language: Language::picked(language.clone()),
        unsubscribing: None,
    });

    let start_monitors = Timer::default();
    slot::with(|app| {
        app.restore();
        app.show_groups();
        app.show_monitors();
        app.show_settings();
        app.show_playlist();
        app.show_tab();
        // "Choose monitor" first, where there is more than one to choose from
        // (over the window once it is on the screen: it is blurred behind).
        if app.desk.monitors_on_start() && language.is_some() && desktop::monitors().len() > 1 {
            start_monitors.start(TimerMode::SingleShot, Duration::from_millis(300), || {
                slot::with(|app| app.blur_backdrop());
            });
        }
        if library::assets().is_none() {
            app.status(t(
                "Wallpaper Engine is not installed in Steam: scenes will not start without its assets",
                "Wallpaper Engine не установлен в Steam: без его ассетов сцены не запустятся",
            ));
        }
    });

    let _shown = debug::apply(&ui, &start_monitors);
    wiring::wire(&ui);
    let _follow = desk::follow();

    let result = ui.run();
    slot::with(|app| {
        let _ = app.steam.send(Command::Quit);
    });
    result
}
