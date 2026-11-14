//! The settings dialog (playback, performance, general, about) and the
//! interface language. The engine's settings are a file the daemon reads
//! (desktop.rs); the language is the window's own, saved with its state.

use slint::{ComponentHandle, ModelRc, VecModel};
use we_linux_ui::{L, PluginEntry, SettingsValues};

use super::App;
use super::saved::Saved;
use crate::desktop::{self, Settings};
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
        let multi = desktop::monitors().len() > 1;
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
    }

    /// "OK" of the settings: what changed is written and the daemon told.
    pub(super) fn save_settings(&mut self) {
        let Some(ui) = self.ui() else { return };
        let values = ui.get_settings();
        if (values.language == 1) != lang::russian() {
            self.set_language(values.language == 1);
        }
        if values.autostart != desktop::autostart()
            && let Err(e) = desktop::set_autostart(values.autostart)
        {
            self.status(tf!("That did not work: {e}", "Не получилось: {e}"));
        }
        let multi = values.several;
        let choice = |row: Row, index: i32| {
            playback_choices(row, multi).get(index.max(0) as usize).copied().unwrap_or("run").to_owned()
        };
        let settings = Settings {
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
        if settings == Settings::load() {
            return;
        }
        let result = settings.save().and_then(|()| engine::ensure_daemon());
        self.report(result, t("Settings applied", "Настройки применены"));
        self.show_monitors();
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
