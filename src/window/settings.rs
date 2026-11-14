//! The settings dialog (playback, performance, general, about) and the
//! interface language. The engine's settings are a file the daemon reads
//! (desktop.rs); the language is the window's own, saved with its state.

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use we_linux_ui::{DeviceEntry, L, PluginEntry, SettingsValues};

use super::App;
use super::saved::Saved;
use crate::desktop::{self, Device, Settings};
use crate::lang::{self, t};
use crate::units::megabytes;
use crate::{engine, tf};

/// The interface language: "en" or "ru", once picked (nothing until the
/// first run asked).
#[derive(Default)]
pub(super) struct Language(Option<String>);

impl Language {
    pub(super) fn picked(language: Option<String>) -> Self {
        Self(language)
    }

    pub(super) fn save_into(&self, saved: &mut Saved) {
        saved.language = self.0.clone();
    }
}

/// A row of the settings' "Воспроизведение".
#[derive(Clone, Copy)]
enum Row {
    Focus,
    Maximized,
    Fullscreen,
    Audio,
    Sleep,
}

/// The choices of a row of "Воспроизведение", as the original lists them:
/// pausing per monitor or all of them where there are several.
fn playback_choices(row: Row, several: bool) -> Vec<&'static str> {
    let (mute, per_monitor, stop) = match row {
        Row::Focus => (true, several, false),
        Row::Maximized | Row::Fullscreen => (true, several, true),
        Row::Audio => (true, false, false),
        Row::Sleep => (false, false, true),
    };
    let mut out = vec!["run"];
    if mute {
        out.push("mute");
    }
    out.push("pause");
    if per_monitor {
        out.push("pauseall");
    }
    if stop {
        out.push("stop");
    }
    out
}

const FILLS: [&str; 3] = ["cover", "contain", "stretch"];

/// What the devices' editor asks of the renderer while it is open: a picture
/// to show at once, a look at what a device shows.
enum Live {
    Picture(String, Vec<(&'static str, serde_json::Value)>),
    Preview(String),
}

/// The thread that asks: the window never waits for the renderer. Of what
/// piled up while it was busy, the newest picture of a device and one look count.
fn live() -> &'static std::sync::mpsc::Sender<Live> {
    static LIVE: std::sync::OnceLock<std::sync::mpsc::Sender<Live>> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Live>();
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut pictures: Vec<(String, Vec<(&'static str, serde_json::Value)>)> = Vec::new();
                let mut preview = None;
                for job in std::iter::once(first).chain(rx.try_iter()) {
                    match job {
                        Live::Picture(name, picture) => {
                            pictures.retain(|(other, _)| *other != name);
                            pictures.push((name, picture));
                        }
                        Live::Preview(name) => preview = Some(name),
                    }
                }
                for (name, picture) in pictures {
                    for (key, value) in picture {
                        let _ = engine::call(serde_json::json!({
                            "cmd": "set", "path": format!("outputs/{name}/{key}"), "value": value
                        }));
                    }
                }
                if let Some(name) = preview {
                    let file = engine::runtime_dir().join("device-preview.png");
                    let shot =
                        engine::call(serde_json::json!({ "cmd": "snapshot", "file": file, "output": name }))
                            .ok()
                            .and_then(|_| image::open(&file).ok())
                            .map(|picture| picture.to_rgba8());
                    super::slot::on_ui(move |app| app.device_preview_arrived(&name, shot));
                }
            }
        });
        tx
    })
}

thread_local! {
    /// Looks at the device in the editor a few times a second while the settings are open.
    static PREVIEWS: slint::Timer = slint::Timer::default();
}

/// A device as the settings' editor holds it.
fn device_entry(device: &Device) -> DeviceEntry {
    DeviceEntry {
        name: device.name.as_str().into(),
        enabled: device.enabled,
        width: device.width as i32,
        height: device.height as i32,
        fps: device.fps as i32,
        command: device.command.as_str().into(),
        fill: FILLS.iter().position(|fill| *fill == device.fill).unwrap_or(0) as i32,
        zoom: device.zoom,
        shift_x: device.shift_x,
        shift_y: device.shift_y,
    }
}

/// The device the editor's row describes.
fn device_of(entry: &DeviceEntry) -> Device {
    Device {
        name: entry.name.trim().to_owned(),
        width: entry.width.max(0) as u32,
        height: entry.height.max(0) as u32,
        fps: entry.fps.clamp(1, 240) as u32,
        command: entry.command.trim().to_owned(),
        enabled: entry.enabled,
        fill: FILLS[entry.fill.clamp(0, 2) as usize].to_owned(),
        zoom: entry.zoom.clamp(desktop::ZOOM_RANGE.0, desktop::ZOOM_RANGE.1),
        shift_x: entry.shift_x.clamp(desktop::SHIFT_RANGE.0, desktop::SHIFT_RANGE.1),
        shift_y: entry.shift_y.clamp(desktop::SHIFT_RANGE.0, desktop::SHIFT_RANGE.1),
    }
}

/// A name no device in `taken` has: `wanted`, or `wanted 2`, `wanted 3`...
fn free_name(wanted: &str, taken: &[String]) -> String {
    if !taken.iter().any(|name| name == wanted) {
        return wanted.to_owned();
    }
    (2..).map(|n| format!("{wanted} {n}")).find(|name| !taken.contains(name)).unwrap_or_default()
}

/// What the settings' "About" tab says.
pub(super) fn about() -> String {
    let renderer = engine::bin("wepaper")
        .map_or_else(|| t("not found", "не найден").to_owned(), |path| path.display().to_string());
    [
        format!("WE Linux {}", env!("CARGO_PKG_VERSION")),
        t(
            "Wallpaper Engine's library, workshop and live wallpapers, native on Linux.",
            "Библиотека, мастерская и живые обои Wallpaper Engine, нативно на Linux.",
        )
        .to_owned(),
        String::new(),
        format!("{}: {renderer}", t("Renderer", "Рендерер")),
        "https://github.com/MoonTiwzy/we-linux".to_owned(),
        "https://github.com/MoonTiwzy/wepaper".to_owned(),
        String::new(),
        t(
            "WE Linux is not made by the Wallpaper Engine team. Wallpapers and the assets they are drawn with come from your own copy of Wallpaper Engine in Steam.",
            "WE Linux сделан не командой Wallpaper Engine. Обои и ассеты, которыми они рисуются, берутся из твоей копии Wallpaper Engine в Steam.",
        )
        .to_owned(),
        String::new(),
        t("License: MIT.", "Лицензия: MIT.").to_owned(),
        t(
            "Icons: Font Awesome Free 6.7.2 by Fonticons, Inc. (CC BY 4.0), https://fontawesome.com",
            "Иконки: Font Awesome Free 6.7.2, Fonticons, Inc. (CC BY 4.0), https://fontawesome.com",
        )
        .to_owned(),
    ]
    .join("\n")
}

impl App {
    /// Switch the interface to a language and make every label again.
    pub(super) fn set_language(&mut self, russian: bool) {
        self.language = Language(Some(if russian { "ru" } else { "en" }.to_owned()));
        lang::set_russian(russian);
        if let Some(ui) = self.ui() {
            ui.global::<L>().set_russian(russian);
            let mut values = ui.get_settings();
            values.language = i32::from(russian);
            ui.set_settings(values);
            ui.set_about(about().into());
        }
        self.save();
        self.show_groups();
        self.show_monitors();
        self.show_playlist();
        self.show_tab();
    }

    /// The first run's question, answered.
    pub(super) fn language_picked(&mut self, index: i32) {
        self.set_language(index == 1);
        if let Some(ui) = self.ui() {
            ui.set_dialog(0);
        }
    }

    /// The settings as they are in force, in the dialog's boxes.
    pub(super) fn show_settings(&self) {
        let Some(ui) = self.ui() else { return };
        let settings = Settings::load();
        ui.set_cache_size(megabytes(engine::cache_size()).into());
        let multi = desktop::screens().len() > 1;
        let tells = crate::compositor::current().tells();
        let index = |row: Row, value: &str| {
            playback_choices(row, multi).iter().position(|c| *c == value).unwrap_or(0) as i32
        };
        ui.set_settings(SettingsValues {
            focus: index(Row::Focus, &settings.playback_focus),
            maximized: index(Row::Maximized, &settings.playback_maximized),
            fullscreen: index(Row::Fullscreen, &settings.playback_fullscreen),
            playing: index(Row::Audio, &settings.playback_audio),
            sleep: index(Row::Sleep, &settings.playback_sleep),
            several: multi,
            fps: settings.fps,
            texture: i32::from(settings.texture_half),
            autostart: desktop::autostart(),
            language: i32::from(lang::russian()),
            still: settings.still,
            audio: settings.audio,
            volume: settings.volume,
            video_gpu: settings.video_gpu,
            tells_focus: tells.focus,
            tells_maximized: tells.maximized,
            tells_fullscreen: tells.fullscreen,
            tells_sleep: tells.sleep,
        });
        let plugins: Vec<PluginEntry> = engine::plugins()
            .into_iter()
            .map(|plugin| PluginEntry {
                name: plugin.name.into(),
                path: plugin.path.to_string_lossy().as_ref().into(),
                kind: plugin.kind.into(),
                loaded: plugin.enabled,
            })
            .collect();
        ui.set_plugins(ModelRc::new(VecModel::from(plugins)));
        self.watch_devices();
        let devices: Vec<DeviceEntry> = settings.devices.iter().map(device_entry).collect();
        ui.set_device(ui.get_device().clamp(0, (devices.len() as i32 - 1).max(0)));
        ui.set_devices(ModelRc::new(VecModel::from(devices)));
    }

    /// The name the renderer knows the device of that row by: the one it was
    /// saved with. A row not saved yet is no device of the renderer's.
    fn told_device(index: i32) -> Option<String> {
        usize::try_from(index).ok().and_then(|i| Settings::load().devices.into_iter().nth(i)).map(|d| d.name)
    }

    /// The editor moved a device's picture: the device shows it at once.
    pub(super) fn device_picture(&mut self, index: i32) {
        let Some(ui) = self.ui() else { return };
        let (Some(row), Some(name)) = (
            usize::try_from(index).ok().and_then(|i| ui.get_devices().row_data(i)),
            Self::told_device(index),
        ) else {
            return;
        };
        let _ = live().send(Live::Picture(name, device_of(&row).picture().into()));
    }

    /// "Отмена": the devices show their saved pictures again.
    pub(super) fn settings_cancelled(&mut self) {
        for device in desktop::devices() {
            let _ = live().send(Live::Picture(device.name.clone(), device.picture().into()));
        }
    }

    /// A few times a second while the settings are open on the devices: ask what
    /// the device in the editor shows.
    fn watch_devices(&self) {
        PREVIEWS.with(|timer| {
            if timer.running() {
                return;
            }
            timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(300), || {
                super::slot::try_with(|app| {
                    let Some(ui) = app.ui() else { return };
                    if !ui.get_settings_open() {
                        PREVIEWS.with(slint::Timer::stop);
                        ui.set_device_previewed(false);
                        return;
                    }
                    if ui.get_settings_tab() != 3 {
                        return;
                    }
                    match Self::told_device(ui.get_device()) {
                        Some(name) => {
                            let _ = live().send(Live::Preview(name));
                        }
                        None => ui.set_device_previewed(false),
                    }
                });
            });
        });
    }

    /// What a device shows came back (nothing: it shows nothing, or is not there).
    fn device_preview_arrived(&mut self, name: &str, shot: Option<image::RgbaImage>) {
        let Some(ui) = self.ui() else { return };
        // Another device was picked meanwhile: this is not its picture.
        if Self::told_device(ui.get_device()).as_deref() != Some(name) {
            return;
        }
        match shot {
            Some(shot) => {
                let (width, height) = shot.dimensions();
                ui.set_device_preview(slint::Image::from_rgba8(
                    slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        shot.as_raw(),
                        width,
                        height,
                    ),
                ));
                ui.set_device_previewed(true);
            }
            None => ui.set_device_previewed(false),
        }
    }

    /// The devices as the editor has them now.
    fn edited_devices(&self) -> Vec<Device> {
        self.ui()
            .map(|ui| ui.get_devices().iter().map(|entry| device_of(&entry)).collect())
            .unwrap_or_default()
    }

    /// "ДОБАВИТЬ": one more row in the editor, picked, for the user to fill in.
    pub(super) fn device_add(&mut self) {
        let Some(ui) = self.ui() else { return };
        let mut devices = self.edited_devices();
        let taken: Vec<String> = devices.iter().map(|device| device.name.clone()).collect();
        devices.push(Device { name: free_name("device", &taken), ..Device::default() });
        ui.set_devices(ModelRc::new(VecModel::from(devices.iter().map(device_entry).collect::<Vec<_>>())));
        ui.set_device(devices.len() as i32 - 1);
    }

    /// "УДАЛИТЬ": the row goes; the device goes with "OK".
    pub(super) fn device_remove(&mut self, index: i32) {
        let Some(ui) = self.ui() else { return };
        let mut devices = self.edited_devices();
        if let Ok(index) = usize::try_from(index)
            && index < devices.len()
        {
            devices.remove(index);
        }
        ui.set_devices(ModelRc::new(VecModel::from(devices.iter().map(device_entry).collect::<Vec<_>>())));
        ui.set_device((ui.get_device()).clamp(0, (devices.len() as i32 - 1).max(0)));
    }

    /// "OK" of the settings: what changed is written and the daemon told.
    pub(super) fn save_settings(&mut self) {
        let Some(ui) = self.ui() else { return };
        let values = ui.get_settings();
        if (values.language == 1) != lang::russian() {
            self.set_language(values.language == 1);
        }
        // What the user has to do for the autostart, said last: nothing says it over.
        let mut autostart = None;
        if values.autostart != desktop::autostart() {
            autostart = match desktop::set_autostart(values.autostart) {
                Err(e) => Some(tf!("That did not work: {e}", "Не получилось: {e}")),
                // The session starts no systemd services: the compositor's config does it.
                Ok(Some(line)) => Some(tf!(
                    "This session does not start services: add to your compositor's config: {line}",
                    "Эта сессия не запускает службы: добавьте в конфиг композитора: {line}"
                )),
                Ok(None) => None,
            };
        }
        let multi = values.several;
        let choice = |row: Row, index: i32| {
            playback_choices(row, multi).get(index.max(0) as usize).copied().unwrap_or("run").to_owned()
        };
        // The devices as edited. One that cannot be a device is kept as it was
        // typed (it is the user's to finish) and said to be unfinished.
        let devices = self.edited_devices();
        let before = Settings::load();
        let mut unfinished = None;
        let mut names = std::collections::BTreeSet::new();
        for device in &devices {
            if let Some(problem) = device.problem() {
                unfinished = Some(problem);
            } else if !names.insert(device.name.clone()) {
                unfinished = Some(tf!(
                    "two devices are called {}: the second is not used",
                    "два устройства называются {}: второе не используется",
                    device.name
                ));
            }
        }
        // A device that was only given another name keeps its own wallpaper;
        // one that went takes it along.
        let renamed = before.devices.len() == devices.len();
        for (index, old) in before.devices.iter().enumerate() {
            if devices.iter().any(|device| device.name == old.name) {
                continue;
            }
            let _ = match devices.get(index).filter(|new| renamed && new.problem().is_none()) {
                Some(new) => desktop::rename_output(&old.name, &new.name),
                None => desktop::forget_output(&old.name),
            };
        }
        // A new device is not one of the screens: the wallpaper of every monitor
        // is not cloned on it until the monitor chooser is told to.
        if !renamed {
            for new in devices.iter().filter(|new| !before.devices.iter().any(|old| old.name == new.name)) {
                let _ = desktop::set_cloned(&new.name, false);
            }
        }
        let settings = Settings {
            devices,
            fps: values.fps,
            video_gpu: values.video_gpu,
            audio: values.audio,
            volume: values.volume,
            playback_focus: choice(Row::Focus, values.focus),
            playback_maximized: choice(Row::Maximized, values.maximized),
            playback_fullscreen: choice(Row::Fullscreen, values.fullscreen),
            playback_audio: choice(Row::Audio, values.playing),
            playback_sleep: choice(Row::Sleep, values.sleep),
            texture_half: values.texture == 1,
            still: values.still,
            ..Settings::load()
        };
        if settings != Settings::load() {
            let result = settings.save().and_then(|()| engine::ensure_daemon());
            self.report(result, t("Settings applied", "Настройки применены"));
            self.show_monitors();
        }
        if let Some(note) = autostart.or(unfinished) {
            self.status(note);
        }
    }

    /// "Очистить кэш".
    pub(super) fn clear_cache(&mut self) {
        let freed = engine::clear_cache();
        self.status(tf!("Cache cleared: {} freed", "Кэш очищен: освобождено {}", megabytes(freed)));
        self.show_settings();
    }

    /// "Сбросить": every setting as it comes.
    pub(super) fn reset_settings(&mut self) {
        let fresh = Settings { hook: Settings::load().hook, ..Settings::default() };
        let result = fresh.save().and_then(|()| engine::ensure_daemon());
        self.report(result, t("Settings reset", "Настройки сброшены"));
        self.show_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_rows_have_the_original_choices() {
        assert_eq!(playback_choices(Row::Fullscreen, true), ["run", "mute", "pause", "pauseall", "stop"]);
        assert_eq!(playback_choices(Row::Maximized, false), ["run", "mute", "pause", "stop"]);
        assert_eq!(playback_choices(Row::Focus, true), ["run", "mute", "pause", "pauseall"]);
        assert_eq!(playback_choices(Row::Audio, true), ["run", "mute", "pause"]);
        assert_eq!(playback_choices(Row::Sleep, true), ["run", "pause", "stop"]);
    }
}
