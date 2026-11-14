//! What is on the desktop: setting the selected wallpaper (on every monitor
//! or on the one chosen), following it until the renderer shows it, the
//! monitor chooser and its profiles, a file opened from the disk, and
//! "Отмена", which puts back what was there when the window opened.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel,
};
use we_linux_ui::MonitorEntry;

use super::saved::Saved;
use super::{App, slot};
use crate::lang::t;
use crate::{desktop, engine, tf};

/// A wallpaper was set and is not on the desktop yet: its title, where it
/// goes (a monitor, or all), its folder and when it was set.
type Applying = (String, Option<String>, PathBuf, Instant);

pub(super) struct Desk {
    /// What every monitor shows, and what it showed when the window opened ("Отмена").
    current: Option<PathBuf>,
    opened_with: desktop::Desktop,
    /// The monitor wallpapers are set on: None is all of them.
    monitor: Option<String>,
    applying: Option<Applying>,
    /// "Choose monitor" comes down when the window opens.
    monitors_on_start: bool,
}

/// The window behind a panel over it, blurred as the original's backdrop is:
/// drawn small, softened, and scaled back up.
fn blurred(shot: &SharedPixelBuffer<Rgba8Pixel>) -> Image {
    let (width, height) = (shot.width(), shot.height());
    let Some(full) = image::RgbaImage::from_raw(width, height, shot.as_bytes().to_vec()) else {
        return Image::default();
    };
    let small = image::imageops::thumbnail(&full, (width / 8).max(1), (height / 8).max(1));
    let soft = image::imageops::blur(&small, 1.5);
    // Scaled back up here, smoothly: the window's renderer would repeat each
    // pixel of the small picture, and the blur would be a mosaic.
    let soft = image::imageops::resize(&soft, width, height, image::imageops::FilterType::Triangle);
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        soft.as_raw(),
        soft.width(),
        soft.height(),
    ))
}

impl Desk {
    /// The desktop as it is when the window opens.
    pub(super) fn now() -> Self {
        Self {
            current: desktop::current(),
            opened_with: desktop::Desktop::load(),
            monitor: None,
            applying: None,
            monitors_on_start: true,
        }
    }

    /// What every monitor shows.
    pub(super) fn current(&self) -> Option<&PathBuf> {
        self.current.as_ref()
    }

    /// What the monitor wallpapers are being set on shows now.
    pub(super) fn on_target(&self) -> Option<PathBuf> {
        match &self.monitor {
            Some(name) => desktop::outputs()
                .into_iter()
                .find(|(output, _)| output == name)
                .map(|(_, path)| path)
                .or_else(|| self.current.clone()),
            None => self.current.clone(),
        }
    }

    /// A wallpaper's files are going: it comes off every monitor it is on.
    pub(super) fn let_go_of(&mut self, dir: &Path) {
        if desktop::current().as_deref() == Some(dir) {
            let _ = desktop::off();
            self.current = None;
        }
        for (output, shown) in desktop::outputs() {
            if shown == dir {
                let _ = desktop::remove_from(&output);
            }
        }
    }

    pub(super) fn monitors_on_start(&self) -> bool {
        self.monitors_on_start
    }

    pub(super) fn save_into(&self, saved: &mut Saved) {
        saved.monitors_on_start = Some(self.monitors_on_start);
    }

    pub(super) fn restore(&mut self, saved: &Saved) {
        self.monitors_on_start = saved.monitors_on_start.unwrap_or(true);
    }
}

/// The wallpaper can change from outside (the command line, the playlist):
/// the window follows it while this timer lives.
pub(super) fn follow() -> Timer {
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(700), || {
        slot::try_with(|app| app.follow_desktop());
    });
    timer
}

impl App {
    /// "Применить", and a click on a wallpaper that is here: on the desktop
    /// (or the monitor chosen) it goes.
    pub(super) fn apply(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let Some(dir) = self.dir_of(&card) else {
            self.status(t("This wallpaper is not downloaded yet", "Эти обои ещё не скачаны"));
            return;
        };
        let desk = &mut self.desk;
        // "On the desktop" is said when the engine shows it (show_applying).
        let result = match &desk.monitor {
            Some(output) => desktop::apply_to(output, &dir),
            None => desktop::clear_outputs()
                .and_then(|()| desktop::apply(&dir))
                .map(|()| desk.current = Some(dir.clone())),
        };
        match result {
            Ok(()) => {
                desk.applying = Some((card.title.clone(), desk.monitor.clone(), dir, Instant::now()));
                self.show_applying();
            }
            Err(e) => self.status(tf!("That did not work: {e}", "Не получилось: {e}")),
        }
        self.refresh_marks();
        self.show_monitors();
    }

    /// Say how far the wallpaper set last has come, until it is on the desktop.
    fn show_applying(&mut self) {
        let Some((title, monitor, dir, since)) = self.desk.applying.clone() else {
            return;
        };
        let waited = since.elapsed().as_secs();
        match engine::stage(&dir) {
            engine::Stage::Shown => {
                self.desk.applying = None;
                match monitor {
                    Some(output) => self
                        .status(tf!("\"{title}\" is on monitor {output}", "«{title}» на мониторе {output}")),
                    None => self.status(tf!("\"{title}\" is on the desktop", "«{title}» на рабочем столе")),
                }
            }
            engine::Stage::Failed(error) => {
                self.desk.applying = None;
                let error = if error.contains("video memory") {
                    t("not enough video memory for this wallpaper", "этим обоям не хватает видеопамяти")
                        .to_owned()
                } else {
                    error
                };
                self.status(tf!("\"{title}\" did not start: {error}", "«{title}» не запустились: {error}"));
            }
            _ if waited > 180 => {
                self.desk.applying = None;
                self.status(tf!(
                    "\"{title}\" is taking too long: see `we-linux doctor`",
                    "«{title}» грузятся слишком долго: смотри `we-linux doctor`"
                ));
            }
            engine::Stage::Loading => self.status(tf!("\"{title}\": loading…", "«{title}»: загрузка…")),
        }
    }

    /// The timer of `follow`: what was set is said once it shows, and a
    /// wallpaper changed from outside is marked.
    fn follow_desktop(&mut self) {
        self.show_applying();
        let current = desktop::current();
        if current != self.desk.current {
            self.desk.current = current;
            self.refresh_marks();
            self.show_monitors();
        }
    }

    /// "Открыть обои": a wallpaper folder, a video or a picture from the disk.
    pub(super) fn open_file(&mut self, picture: bool) {
        match desktop::pick_file(picture) {
            Ok(Some(path)) => {
                // A project.json stands for the wallpaper folder it is in.
                let target = if path.file_name().is_some_and(|n| n == "project.json") {
                    path.parent().map(PathBuf::from).unwrap_or(path)
                } else {
                    path
                };
                let result = desktop::apply(&target);
                if result.is_ok() {
                    self.desk.current = desktop::current();
                }
                self.report(result, &tf!("{} is on the desktop", "{} на рабочем столе", target.display()));
                self.refresh_marks();
                self.show_monitors();
            }
            Ok(None) => {}
            Err(e) => self.status(tf!("That did not work: {e}", "Не получилось: {e}")),
        }
    }

    /// "Редактор обоев": there is none here.
    pub(super) fn open_editor(&mut self) {
        self.status(t(
            "The editor is a part of Wallpaper Engine for Windows; wallpapers made in it play here",
            "Редактор есть только в Wallpaper Engine для Windows; сделанные в нём обои здесь играют",
        ));
    }

    /// "Отмена": put back what the desktop showed when the window opened.
    pub(super) fn revert(&mut self) {
        let before = self.desk.opened_with.clone();
        if desktop::Desktop::load() == before {
            return;
        }
        let _ = if before.wallpaper.is_some() || !before.outputs.is_empty() {
            // The live wallpapers as they were, every monitor's own too.
            before.save().and_then(|()| engine::ensure_daemon())
        } else {
            // Nothing was live: the engine stops, and the picture set before
            // (if one was) shows again.
            match &before.image {
                Some(picture) => desktop::apply(picture),
                None => desktop::take_off().map(|()| drop(std::thread::spawn(engine::see_off))),
            }
        };
        self.desk.current = desktop::current();
    }

    // ------------------------------------------------------------- the monitor chooser

    pub(super) fn show_monitors(&self) {
        let Some(ui) = self.ui() else { return };
        let overrides = desktop::outputs();
        let title = |path: &PathBuf| {
            self.catalog.at(path).map_or_else(
                || path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                |i| i.title.clone(),
            )
        };
        let monitors = desktop::monitors();
        let left = monitors.iter().map(|m| m.x).fold(f32::MAX, f32::min);
        let top = monitors.iter().map(|m| m.y).fold(f32::MAX, f32::min);
        let rows: Vec<MonitorEntry> = monitors
            .iter()
            .map(|m| MonitorEntry {
                name: m.name.as_str().into(),
                detail: m.detail.as_str().into(),
                wallpaper: overrides
                    .iter()
                    .find(|(name, _)| name == &m.name)
                    .map(|(_, path)| path)
                    .or(self.desk.current.as_ref())
                    .map(title)
                    .unwrap_or_default()
                    .into(),
                x: m.x - left,
                y: m.y - top,
                w: m.width,
                h: m.height,
                image: Image::default(),
                has_image: false,
            })
            .collect();
        ui.set_desk_width(monitors.iter().map(|m| m.x - left + m.width).fold(1.0, f32::max));
        ui.set_desk_height(monitors.iter().map(|m| m.y - top + m.height).fold(1.0, f32::max));
        let index = self.desk.monitor.as_ref().and_then(|name| monitors.iter().position(|m| &m.name == name));
        ui.set_monitor_index(index.map_or(-1, |i| i as i32));
        // As the original: the row is there when there is a monitor to choose
        // (or one so wide it is likely several), the number when there are more.
        let lone_wide = monitors.len() == 1
            && monitors
                .iter()
                .all(|m| m.width / m.height.max(1.0) > 3.5 || m.height / m.width.max(1.0) > 3.5);
        ui.set_monitor_row(monitors.len() > 1 || lone_wide);
        ui.set_monitor_title(
            if monitors.len() > 1 {
                tf!("Display {}", "Монитор {}", index.unwrap_or(0) + 1)
            } else {
                t("Display", "Монитор").to_owned()
            }
            .into(),
        );
        ui.set_monitors(ModelRc::new(VecModel::from(rows)));
    }

    /// Take the wallpaper off a monitor, or off all of them.
    pub(super) fn clear_monitor(&mut self, index: i32) {
        let name =
            usize::try_from(index).ok().and_then(|i| desktop::monitors().into_iter().nth(i)).map(|m| m.name);
        let result = match &name {
            // Its own wallpaper goes; without one, there is only everybody's to take off.
            Some(name) if desktop::outputs().iter().any(|(output, _)| output == name) => {
                desktop::remove_from(name)
            }
            _ => desktop::take_off().map(|()| drop(std::thread::spawn(engine::see_off))),
        };
        self.desk.current = desktop::current();
        self.report(result, t("The wallpaper is off", "Обои убраны"));
        self.refresh_marks();
        self.show_monitors();
    }

    /// The monitor chooser comes down over the window, the window blurred behind it.
    pub(super) fn open_monitors(&mut self) {
        let Some(ui) = self.ui() else { return };
        if ui.get_monitors_open() {
            ui.set_monitors_open(false);
            return;
        }
        self.blur_backdrop();
    }

    /// Bring the monitor chooser down over a blurred copy of the window.
    pub(super) fn blur_backdrop(&mut self) {
        let Some(ui) = self.ui() else { return };
        self.show_monitors();
        if let Ok(shot) = ui.window().take_snapshot() {
            ui.set_backdrop(blurred(&shot));
        }
        ui.set_monitors_open(true);
    }

    pub(super) fn pick_monitor(&mut self, index: i32) {
        self.desk.monitor =
            usize::try_from(index).ok().and_then(|i| desktop::monitors().into_iter().nth(i)).map(|m| m.name);
        self.show_monitors();
    }

    /// The chooser's box "show it when the window opens".
    pub(super) fn monitors_on_start_changed(&mut self, on: bool) {
        self.desk.monitors_on_start = on;
        self.save();
    }

    /// The chooser's profiles: the dialog that loads one (false) or saves the desk as one.
    pub(super) fn monitor_profiles(&mut self, save: bool) {
        if let Some(ui) = self.ui() {
            let names: Vec<SharedString> = desktop::profiles().into_iter().map(SharedString::from).collect();
            ui.set_profiles(ModelRc::new(VecModel::from(names)));
            ui.set_preset_name("".into());
            ui.set_dialog(if save { 11 } else { 10 });
        }
    }

    pub(super) fn profile_save(&mut self, name: &str) {
        let result = desktop::save_profile(name.trim());
        self.report(result, t("The profile is saved", "Профиль сохранён"));
    }

    pub(super) fn profile_load(&mut self, row: i32) {
        if let Some(name) = desktop::profiles().get(row.max(0) as usize) {
            let result = desktop::load_profile(name);
            self.desk.current = desktop::current();
            self.report(result, t("The profile is on the desktop", "Профиль на рабочем столе"));
            self.refresh_marks();
            self.show_monitors();
        }
    }
}
