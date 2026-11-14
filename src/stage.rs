//! How far a wallpaper set a moment ago has come, from what the daemon and
//! the renderer say at one moment ([`Seen`]); and what a refusal of the
//! renderer means to the user, in plain words and with what to do about it
//! ([`Refusal`]). Nothing here reads a file or a socket: `engine::seen`
//! gathers what is seen, the window and `we-linux status` say it.

use std::path::PathBuf;
use std::time::Duration;

use crate::lang::t;
use crate::tf;

/// What the daemon is waiting on the renderer for.
#[derive(Clone, Debug, PartialEq)]
pub enum Busy {
    /// Starting it, until it answers.
    Starting,
    /// A `show` it has not answered yet: loading that wallpaper on that monitor.
    Showing { output: String, wallpaper: PathBuf },
}

/// What the daemon and the renderer say, gathered at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    /// The daemon runs.
    pub daemon: bool,
    /// What the daemon is waiting on the renderer for, and for how long now.
    pub busy: Option<(Busy, Duration)>,
    /// What each monitor shows, as the renderer says. None when it was not
    /// asked (it is loading and answers nothing until it is done) or is not there.
    pub shown: Option<Vec<(String, PathBuf)>>,
    /// What the daemon could not show since the desktop last changed, and why.
    pub errors: Vec<(PathBuf, String)>,
    /// How many videos the renderer is re-encoding into cheaper copies.
    pub optimising: usize,
}

/// A wallpaper that was set: where (a monitor, or all of them), and how long ago.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub wallpaper: PathBuf,
    pub output: Option<String>,
    pub waited: Duration,
}

/// How long a wallpaper may be on its way with no word from the renderer
/// before the window says something is stuck: the daemon itself gives a
/// `show` thirty seconds, and starts a renderer that exits three times.
pub const STUCK: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// No daemon yet, or it is starting the renderer.
    Starting,
    /// The renderer is loading it.
    Loading,
    /// The renderer is loading another wallpaper, set before: this one is next.
    Waiting(PathBuf),
    /// On the desktop; so many videos are being re-encoded meanwhile.
    Optimising(usize),
    Shown,
    /// The renderer (or the daemon) did not show it, for this reason.
    Failed(String),
    /// Long on its way with nothing loading it: the daemon is not getting to it.
    Stuck,
}

impl Phase {
    /// Still on its way: the window keeps saying so.
    pub fn pending(&self) -> bool {
        matches!(self, Phase::Starting | Phase::Loading | Phase::Waiting(_))
    }
}

/// Whether the monitors the wallpaper was set on show it: the one asked for,
/// or every monitor the renderer draws on.
fn on_target(asked: &Asked, shown: &[(String, PathBuf)]) -> bool {
    match &asked.output {
        Some(output) => shown.iter().any(|(name, path)| name == output && *path == asked.wallpaper),
        None => !shown.is_empty() && shown.iter().all(|(_, path)| *path == asked.wallpaper),
    }
}

/// How far `asked` has come, given what is `seen`.
pub fn phase(asked: &Asked, seen: &Seen) -> Phase {
    if let Some((_, why)) = seen.errors.iter().find(|(failed, _)| *failed == asked.wallpaper) {
        return Phase::Failed(why.clone());
    }
    if let Some(shown) = &seen.shown
        && on_target(asked, shown)
    {
        return if seen.optimising > 0 { Phase::Optimising(seen.optimising) } else { Phase::Shown };
    }
    match &seen.busy {
        Some((Busy::Showing { wallpaper, .. }, _)) if *wallpaper == asked.wallpaper => Phase::Loading,
        // Another one, set before: the daemon asks for this one once the
        // renderer is done with that (a click in between is not lost: the
        // last one set is what the daemon reads next).
        Some((Busy::Showing { wallpaper, .. }, _)) => Phase::Waiting(wallpaper.clone()),
        Some((Busy::Starting, _)) => Phase::Starting,
        None if !seen.daemon || seen.shown.is_none() => {
            if asked.waited > STUCK {
                Phase::Stuck
            } else {
                Phase::Starting
            }
        }
        // The renderer is there and idle: the daemon sees the change within
        // a moment and asks for it.
        None if asked.waited > STUCK => Phase::Stuck,
        None => Phase::Loading,
    }
}

/// What a wallpaper's kind takes to load, said while it loads.
pub fn loading_words(kind: &str) -> &'static str {
    match kind {
        "video" => t("Opening the video", "Открывается видео"),
        "web" => t("Opening the web page", "Открывается веб-страница"),
        "preset" => t("Loading the preset's wallpaper", "Загружаются обои пресета"),
        _ => t("Loading the scene", "Загружается сцена"),
    }
}

/// What the window says of a phase still on its way, under the wallpaper's
/// title: what is being done, and how long it has been.
pub fn doing(phase: &Phase, kind: &str, waited: Duration, behind: &str) -> String {
    let seconds = waited.as_secs();
    let what = match phase {
        Phase::Starting => t("Starting the renderer", "Запускается рендерер").to_owned(),
        Phase::Loading => loading_words(kind).to_owned(),
        // What it waits for is the other one's time, not its own.
        Phase::Waiting(_) => return tf!("Next, after \"{behind}\"", "Следующие, после «{behind}»"),
        Phase::Optimising(1) => {
            t("Playing; making the video lighter", "Играют, видео облегчается").to_owned()
        }
        Phase::Optimising(n) => tf!("Playing; making {n} videos lighter", "Играют, облегчается видео: {n}"),
        Phase::Shown => return t("On the desktop", "На рабочем столе").to_owned(),
        Phase::Stuck => return t("No word from the renderer", "Рендерер молчит").to_owned(),
        Phase::Failed(_) => return t("Not shown", "Не показаны").to_owned(),
    };
    // A second or two is not worth a number; the seconds after are what tells
    // a slow wallpaper from a stuck one.
    if seconds >= 3 { tf!("{what} · {seconds} s", "{what} · {seconds} с") } else { format!("{what}…") }
}

/// Why the renderer did not show a wallpaper, as far as the user can do
/// something about it.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// A web wallpaper, and the web plugin cannot draw it: not installed
    /// (`listed`: its description is there but not its program), or turned off.
    NoWebPlugin {
        listed: bool,
        off: bool,
    },
    /// A Windows program as a wallpaper.
    Application,
    /// A preset of a wallpaper that is not installed (its workshop id).
    PresetBase(u64),
    VideoMemory,
    /// Wallpaper Engine itself is not installed: no stock assets.
    NoAssets,
    /// The renderer is not installed.
    NoRenderer,
    /// It hung loading this one and was killed.
    Hung,
    /// It keeps exiting or hanging right after it starts.
    Crashes,
    /// Anything else: the renderer's own words.
    Other(String),
}

/// Read the renderer's (or the daemon's) reason. `web_off`: the web plugin
/// is installed but turned off, which the reason itself does not tell.
pub fn refusal(why: &str, web_off: bool) -> Refusal {
    let lower = why.to_lowercase();
    if lower.contains("web wallpapers are not drawn") {
        return Refusal::NoWebPlugin { listed: false, off: web_off };
    }
    if lower.contains("wepaper-web") && lower.contains("could not be started") {
        return Refusal::NoWebPlugin { listed: true, off: false };
    }
    if lower.contains("application wallpapers are not drawn") {
        return Refusal::Application;
    }
    // "the preset is for wallpaper 1234, which is not installed"
    if let Some(rest) = lower.split("the preset is for wallpaper ").nth(1)
        && lower.contains("not installed")
        && let Ok(id) = rest.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("").parse::<u64>()
    {
        return Refusal::PresetBase(id);
    }
    if lower.contains("video memory") || lower.contains("out of memory") || lower.contains("out_of_memory") {
        return Refusal::VideoMemory;
    }
    if lower.contains("assets were not found") || lower.contains("no assets") {
        return Refusal::NoAssets;
    }
    if lower.contains("renderer) is not installed") {
        return Refusal::NoRenderer;
    }
    if lower.contains("hung loading it") {
        return Refusal::Hung;
    }
    if lower.contains("keeps exiting") || lower.contains("keeps hanging") {
        return Refusal::Crashes;
    }
    Refusal::Other(why.trim().trim_end_matches('.').to_owned())
}

/// What the alert offers to do about a refusal.
#[derive(Clone, Debug, PartialEq)]
pub enum Remedy {
    /// Subscribe to this workshop item.
    Subscribe(u64),
    /// Open a link (a page, or Steam's).
    Open(&'static str),
    /// Open the settings at this tab.
    Settings(i32),
}

/// Where the install of the web plugin is told.
pub const INSTALL_PAGE: &str = "https://github.com/MoonTiwzy/we-linux#install";
/// Steam installs Wallpaper Engine: its assets are what scenes are made of.
pub const INSTALL_WE: &str = "steam://install/431960";
/// The settings' tabs the alerts open.
pub const SETTINGS_PERFORMANCE: i32 = 0;
pub const SETTINGS_PLUGINS: i32 = 2;

/// What the window says of a refusal: a headline, what it means and what to
/// do, and the button for it (its label and what it does). `base` is the
/// title of a preset's wallpaper when the workshop told it.
pub struct Words {
    pub headline: String,
    pub body: String,
    pub remedy: Option<(String, Remedy)>,
}

pub fn words(refusal: &Refusal, base: Option<&str>) -> Words {
    let (headline, body, remedy): (String, String, Option<(String, Remedy)>) = match refusal {
        Refusal::NoWebPlugin { off: true, .. } => (
            t("Web wallpapers are turned off", "Веб-обои выключены").into(),
            t(
                "The renderer's web plugin is installed but turned off. Turn \"Web wallpapers\" on in Settings, Plugins, and set this one again.",
                "Плагин веб-обоев рендерера установлен, но выключен. Включи «Web wallpapers» в настройках, раздел «Плагины», и поставь эти обои снова.",
            )
            .into(),
            Some((t("Plugins", "Плагины").into(), Remedy::Settings(SETTINGS_PLUGINS))),
        ),
        Refusal::NoWebPlugin { listed, .. } => (
            t("Web wallpapers need the wepaper-web plugin", "Веб-обоям нужен плагин wepaper-web").into(),
            if *listed {
                t(
                    "The plugin is listed, but its program wepaper-web is not on the PATH. Install it again: ./install.sh --engine in the WE Linux folder.",
                    "Плагин числится, но его программы wepaper-web нет в PATH. Поставь его заново: ./install.sh --engine в папке WE Linux.",
                )
            } else {
                t(
                    "It is not installed, so web pages cannot be drawn. ./install.sh builds it when GTK 4, WebKitGTK 6 and gtk4-layer-shell are installed (./install.sh --engine to add it later); on NixOS, update the we-linux flake: its package carries the plugin.",
                    "Он не установлен, и веб-страницы рисовать нечем. ./install.sh собирает его, когда установлены GTK 4, WebKitGTK 6 и gtk4-layer-shell (добавить потом: ./install.sh --engine); на NixOS обнови флейк we-linux: в его пакете плагин есть.",
                )
            }
            .into(),
            Some((t("How to install", "Как установить").into(), Remedy::Open(INSTALL_PAGE))),
        ),
        Refusal::Application => (
            t("Application wallpapers do not run here", "Обои-приложения здесь не запускаются").into(),
            t(
                "This wallpaper is a Windows program. WE Linux plays scenes, videos and web pages.",
                "Эти обои - программа для Windows. WE Linux играет сцены, видео и веб-страницы.",
            )
            .into(),
            None,
        ),
        Refusal::PresetBase(id) => {
            let base = base.map_or_else(|| id.to_string(), |title| format!("\"{title}\""));
            (
                t("This preset needs its wallpaper", "Этому пресету нужны его обои").into(),
                tf!(
                    "It is a preset of wallpaper {base}, which is not installed. Subscribe to it: once it is downloaded, the preset goes on the desktop by itself.",
                    "Это пресет обоев {base}, а они не установлены. Подпишись на них: когда они скачаются, пресет встанет на рабочий стол сам.",
                ),
                Some((t("Subscribe to it", "Подписаться на них").into(), Remedy::Subscribe(*id))),
            )
        }
        Refusal::VideoMemory => (
            t("Not enough video memory", "Не хватает видеопамяти").into(),
            t(
                "The wallpaper does not fit the graphics card next to what else is open. Close something heavy (a game, a browser with video) and set it again, or set the texture resolution to High performance in Settings.",
                "Обои не помещаются в видеокарту рядом с тем, что уже открыто. Закрой что-то тяжёлое (игру, браузер с видео) и поставь снова, или выбери в настройках разрешение текстуры «Высокая производительность».",
            )
            .into(),
            Some((t("Settings", "Настройки").into(), Remedy::Settings(SETTINGS_PERFORMANCE))),
        ),
        Refusal::NoAssets => (
            t("Wallpaper Engine's files are missing", "Нет файлов Wallpaper Engine").into(),
            t(
                "Scenes are made of the assets of Wallpaper Engine itself. Install it in Steam: it is never started, only its files are read.",
                "Сцены собираются из ассетов самого Wallpaper Engine. Установи его в Steam: запускать его не нужно, читаются только его файлы.",
            )
            .into(),
            Some((t("Install in Steam", "Установить в Steam").into(), Remedy::Open(INSTALL_WE))),
        ),
        Refusal::NoRenderer => (
            t("The renderer is not installed", "Рендерер не установлен").into(),
            t(
                "wepaper, which draws the wallpapers, is not on the PATH. Run ./install.sh again, or install WE Linux's package.",
                "wepaper, который рисует обои, не найден в PATH. Запусти ./install.sh ещё раз или поставь пакет WE Linux.",
            )
            .into(),
            Some((t("How to install", "Как установить").into(), Remedy::Open(INSTALL_PAGE))),
        ),
        Refusal::Hung => (
            t("The renderer hung on this wallpaper", "Рендерер завис на этих обоях").into(),
            t(
                "It did not finish loading it in time and was stopped. `we-linux logs` in a terminal shows where it stopped.",
                "Он не загрузил их вовремя и был остановлен. `we-linux logs` в терминале покажет, на чём он встал.",
            )
            .into(),
            None,
        ),
        Refusal::Crashes => (
            t("The renderer keeps stopping", "Рендерер падает").into(),
            t(
                "It exited several times in a row right after it started, and is not started again until you set something. `we-linux logs` in a terminal shows why.",
                "Он несколько раз подряд закрылся сразу после запуска и не запускается, пока ты что-нибудь не поставишь. `we-linux logs` в терминале покажет почему.",
            )
            .into(),
            None,
        ),
        Refusal::Other(why) => (
            t("The renderer did not show this wallpaper", "Рендерер не показал эти обои").into(),
            tf!(
                "It said: {why}. More in `we-linux logs` in a terminal.",
                "Он ответил: {why}. Подробнее: `we-linux logs` в терминале.",
            ),
            None,
        ),
    };
    Words { headline, body, remedy }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(format!("/w/{name}"))
    }

    fn asked(name: &str, output: Option<&str>, waited: u64) -> Asked {
        Asked {
            wallpaper: path(name),
            output: output.map(str::to_owned),
            waited: Duration::from_secs(waited),
        }
    }

    fn showing(pairs: &[(&str, &str)]) -> Option<Vec<(String, PathBuf)>> {
        Some(pairs.iter().map(|(output, name)| ((*output).to_owned(), path(name))).collect())
    }

    fn seen() -> Seen {
        Seen { daemon: true, shown: showing(&[("DP-1", "old")]), ..Seen::default() }
    }

    #[test]
    fn no_daemon_or_no_renderer_yet_is_starting() {
        let rain = asked("rain", None, 1);
        assert_eq!(phase(&rain, &Seen::default()), Phase::Starting);
        let starting = Seen { daemon: true, busy: Some((Busy::Starting, Duration::ZERO)), ..Seen::default() };
        assert_eq!(phase(&rain, &starting), Phase::Starting);
        // The daemon runs, the renderer does not answer and nothing is loading.
        assert_eq!(phase(&rain, &Seen { daemon: true, ..Seen::default() }), Phase::Starting);
    }

    #[test]
    fn a_show_in_flight_is_loading_that_one_and_waiting_for_others() {
        let busy = |name: &str| Seen {
            daemon: true,
            busy: Some((
                Busy::Showing { output: "DP-1".into(), wallpaper: path(name) },
                Duration::from_secs(4),
            )),
            shown: None,
            ..Seen::default()
        };
        assert_eq!(phase(&asked("rain", None, 4), &busy("rain")), Phase::Loading);
        // Clicked while "city" loads: it comes next, and the window says behind what.
        assert_eq!(phase(&asked("rain", None, 1), &busy("city")), Phase::Waiting(path("city")));
    }

    #[test]
    fn shown_where_it_was_set() {
        let mut seen = seen();
        seen.shown = showing(&[("DP-1", "rain"), ("HDMI-A-1", "rain")]);
        assert_eq!(phase(&asked("rain", None, 2), &seen), Phase::Shown);
        assert_eq!(phase(&asked("rain", Some("HDMI-A-1"), 2), &seen), Phase::Shown);
        // On every monitor: one still showing the old one is not done.
        seen.shown = showing(&[("DP-1", "rain"), ("HDMI-A-1", "old")]);
        assert_eq!(phase(&asked("rain", None, 2), &seen), Phase::Loading);
        assert_eq!(phase(&asked("rain", Some("DP-1"), 2), &seen), Phase::Shown);
        assert_eq!(phase(&asked("rain", Some("HDMI-A-1"), 2), &seen), Phase::Loading);
    }

    #[test]
    fn shown_while_videos_are_reencoded_says_so() {
        let mut seen = seen();
        seen.shown = showing(&[("DP-1", "clip")]);
        seen.optimising = 1;
        assert_eq!(phase(&asked("clip", None, 5), &seen), Phase::Optimising(1));
        assert!(!Phase::Optimising(1).pending());
    }

    #[test]
    fn a_refusal_wins_over_everything() {
        let mut seen = seen();
        seen.errors = vec![(path("page"), "web wallpapers are not drawn by the engine".into())];
        seen.busy = Some((Busy::Showing { output: "DP-1".into(), wallpaper: path("page") }, Duration::ZERO));
        assert_eq!(
            phase(&asked("page", None, 1), &seen),
            Phase::Failed("web wallpapers are not drawn by the engine".into())
        );
        // Another wallpaper's failure is not this one's.
        assert_eq!(phase(&asked("rain", None, 1), &seen), Phase::Waiting(path("page")));
    }

    #[test]
    fn long_with_nothing_loading_is_stuck() {
        assert_eq!(phase(&asked("rain", None, 30), &seen()), Phase::Loading);
        assert_eq!(phase(&asked("rain", None, 121), &seen()), Phase::Stuck);
        assert_eq!(phase(&asked("rain", None, 121), &Seen::default()), Phase::Stuck);
        // A long load is not stuck while the renderer is on it.
        let loading = Seen {
            daemon: true,
            busy: Some((
                Busy::Showing { output: "DP-1".into(), wallpaper: path("rain") },
                Duration::from_secs(25),
            )),
            ..Seen::default()
        };
        assert_eq!(phase(&asked("rain", None, 200), &loading), Phase::Loading);
    }

    #[test]
    fn the_renderers_reasons_are_read() {
        assert_eq!(
            refusal("web wallpapers are not drawn by the engine", false),
            Refusal::NoWebPlugin { listed: false, off: false }
        );
        assert_eq!(
            refusal("web wallpapers are not drawn by the engine", true),
            Refusal::NoWebPlugin { listed: false, off: true }
        );
        assert_eq!(
            refusal("wepaper-web could not be started: No such file or directory (os error 2)", false),
            Refusal::NoWebPlugin { listed: true, off: false }
        );
        assert_eq!(
            refusal("application wallpapers are not drawn by the engine", false),
            Refusal::Application
        );
        assert_eq!(
            refusal("the preset is for wallpaper 1838024372, which is not installed", false),
            Refusal::PresetBase(1838024372)
        );
        assert_eq!(refusal("not enough video memory", false), Refusal::VideoMemory);
        assert_eq!(refusal("GL_OUT_OF_MEMORY creating a texture", false), Refusal::VideoMemory);
        assert_eq!(
            refusal(
                "Wallpaper Engine's assets were not found: install it in Steam, or name the folder with --assets",
                false
            ),
            Refusal::NoAssets
        );
        // The daemon's own words.
        assert_eq!(
            refusal("Wallpaper Engine is not installed in Steam: no assets", false),
            Refusal::NoAssets
        );
        assert_eq!(refusal("wepaper (the renderer) is not installed", false), Refusal::NoRenderer);
        assert_eq!(refusal("the renderer hung loading it and was killed", false), Refusal::Hung);
        assert_eq!(
            refusal("the renderer keeps exiting (signal: 11 (SIGSEGV)): see `we-linux doctor`", false),
            Refusal::Crashes
        );
        assert_eq!(refusal("the renderer keeps hanging: see `we-linux doctor`", false), Refusal::Crashes);
        assert_eq!(
            refusal("scene.json: no such file.", false),
            Refusal::Other("scene.json: no such file".into())
        );
    }

    #[test]
    fn each_refusal_says_what_to_do() {
        // The tests run in English: nothing sets the language.
        let web = words(&Refusal::NoWebPlugin { listed: false, off: false }, None);
        assert_eq!(web.headline, "Web wallpapers need the wepaper-web plugin");
        assert!(web.body.contains("./install.sh --engine") && web.body.contains("NixOS"), "{}", web.body);
        assert_eq!(web.remedy.map(|(_, r)| r), Some(Remedy::Open(INSTALL_PAGE)));

        let off = words(&Refusal::NoWebPlugin { listed: false, off: true }, None);
        assert_eq!(off.remedy.map(|(_, r)| r), Some(Remedy::Settings(SETTINGS_PLUGINS)));

        let preset = words(&Refusal::PresetBase(42), Some("Rain"));
        assert!(preset.body.contains("\"Rain\""), "{}", preset.body);
        assert_eq!(preset.remedy.map(|(_, r)| r), Some(Remedy::Subscribe(42)));
        // Without a title from the workshop, its id.
        assert!(words(&Refusal::PresetBase(42), None).body.contains(" 42,"));

        let memory = words(&Refusal::VideoMemory, None);
        assert_eq!(memory.headline, "Not enough video memory");
        assert_eq!(memory.remedy.map(|(_, r)| r), Some(Remedy::Settings(SETTINGS_PERFORMANCE)));

        assert_eq!(words(&Refusal::NoAssets, None).remedy.map(|(_, r)| r), Some(Remedy::Open(INSTALL_WE)));
        assert!(words(&Refusal::Application, None).remedy.is_none());
        let other = words(&Refusal::Other("scene.json: no such file".into()), None);
        assert!(other.body.contains("scene.json: no such file") && other.body.contains("we-linux logs"));
    }

    #[test]
    fn what_is_being_done_and_for_how_long() {
        assert_eq!(doing(&Phase::Loading, "scene", Duration::from_secs(1), ""), "Loading the scene…");
        assert_eq!(doing(&Phase::Loading, "video", Duration::from_secs(7), ""), "Opening the video · 7 s");
        assert_eq!(
            doing(&Phase::Waiting(path("city")), "scene", Duration::from_secs(9), "City"),
            "Next, after \"City\""
        );
        assert_eq!(
            doing(&Phase::Optimising(1), "video", Duration::from_secs(4), ""),
            "Playing; making the video lighter · 4 s"
        );
    }
}
