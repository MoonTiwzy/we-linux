//! A wallpaper on its way to the desktop, from the click until the renderer
//! shows it, and what the window says meanwhile, as the original says it:
//! the loader over its tile and over the picture in the panel, with the
//! stage in blue under it; the toast at the bottom (what is being done, and
//! for how long); and, when the renderer refuses it, the red alert over the
//! grid with the reason in plain words and what to do, which stays until
//! the user does something else.
//!
//! What is seen is asked on a thread of its own (`engine::seen`): the
//! renderer answers nothing while it loads, and the window must not wait on
//! it. What it means is `stage`, tested there.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slint::ComponentHandle;
use we_linux_ui::{AppWindow, Glyph, Icons};

use super::{App, slot};
use crate::lang::t;
use crate::stage::{self, Asked, Busy, Phase, Refusal, Remedy, Seen};
use crate::steam::Command;
use crate::{desktop, engine, log, tf};

/// How long a wallpaper is still looked after once it shows: the toast
/// says it is on the desktop, and a video the renderer then finds too heavy
/// starts being made lighter only once it plays.
const SETTLE: Duration = Duration::from_secs(4);

/// A wallpaper set from the window and not on the desktop yet.
#[derive(Clone)]
pub(super) struct Applying {
    pub title: String,
    /// Its project's type: what loading it takes.
    pub kind: String,
    pub output: Option<String>,
    pub dir: PathBuf,
    since: Instant,
    /// When it was first seen on the desktop.
    shown_at: Option<Instant>,
    /// What was last said of it: the log gets a change once.
    said: Option<Phase>,
}

impl Applying {
    pub(super) fn new(title: &str, dir: &Path, output: Option<String>) -> Self {
        let kind = project_kind(dir);
        Self {
            title: title.to_owned(),
            kind,
            output,
            dir: dir.to_owned(),
            since: Instant::now(),
            shown_at: None,
            said: None,
        }
    }

    fn asked(&self) -> Asked {
        Asked { wallpaper: self.dir.clone(), output: self.output.clone(), waited: self.since.elapsed() }
    }
}

/// A wallpaper's type as its project.json says it.
fn project_kind(dir: &Path) -> String {
    std::fs::read(dir.join("project.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|json| json["type"].as_str().map(str::to_lowercase))
        .unwrap_or_else(|| "scene".to_owned())
}

/// A refusal on the screen.
struct Alert {
    dir: PathBuf,
    title: String,
    output: Option<String>,
    refusal: Refusal,
}

#[derive(Default)]
pub(super) struct Progress {
    applying: Option<Applying>,
    alert: Option<Alert>,
    /// What was seen last.
    seen: Seen,
    /// A look at the daemon and the renderer is under way.
    probing: Arc<AtomicBool>,
    /// A preset whose wallpaper is being downloaded: set again once it is here.
    after_base: Option<(u64, Applying)>,
    /// `--show` put the window in a state of its own: nothing is asked.
    fixed: bool,
}

impl Progress {
    /// What the applying wallpaper's phase is, by what was seen last.
    fn phase(&self) -> Option<Phase> {
        self.applying.as_ref().map(|a| stage::phase(&a.asked(), &self.seen))
    }

    /// A tile's state (as `Item.state` takes it) and its word, for the wallpaper in `dir`.
    fn tile(&self, dir: Option<&Path>) -> (i32, String) {
        let Some(dir) = dir else { return (0, String::new()) };
        if self.alert.as_ref().is_some_and(|a| a.dir == dir) {
            return (3, t("Not shown", "Не показаны").to_owned());
        }
        if self.applying.as_ref().is_some_and(|a| a.dir == dir)
            && let Some(phase) = self.phase()
        {
            return match phase {
                Phase::Starting => (1, t("Starting", "Запуск").to_owned()),
                Phase::Loading => (1, t("Loading", "Загрузка").to_owned()),
                Phase::Waiting(_) => (2, t("Next", "Следующие").to_owned()),
                Phase::Stuck => (2, t("No answer", "Нет ответа").to_owned()),
                _ => (0, String::new()),
            };
        }
        // What the renderer is busy with, set before the one now asked for.
        match &self.seen.busy {
            Some((Busy::Showing { wallpaper, .. }, _)) if wallpaper == dir => {
                (1, t("Loading", "Загрузка").to_owned())
            }
            _ => (0, String::new()),
        }
    }

    /// Whether there is anything to keep looking for.
    fn watching(&self) -> bool {
        !self.fixed && self.applying.is_some()
    }
}

impl App {
    /// The state of the tile of the wallpaper in `dir`.
    pub(super) fn tile_state(&self, dir: Option<&Path>) -> (i32, String) {
        self.progress.tile(dir)
    }

    /// `wallpaper` was written to the desktop: follow it until it shows. A
    /// refusal on the screen goes: the user did something else.
    pub(super) fn follow_applying(&mut self, applying: Applying) {
        log::info(format!(
            "set \"{}\" ({}) on {}",
            applying.title,
            applying.dir.display(),
            applying.output.as_deref().map_or_else(|| "every monitor".to_owned(), |o| format!("monitor {o}"))
        ));
        if let Some(waiting) = &self.progress.applying
            && waiting.dir != applying.dir
            && waiting.said.as_ref().is_some_and(Phase::pending)
        {
            log::info(format!("\"{}\" was not shown yet: the one set after it goes instead", waiting.title));
        }
        self.progress.alert = None;
        self.progress.applying = Some(applying);
        self.show_alert();
        self.show_progress();
        self.probe();
    }

    /// The timer of `desk::follow`: another look, while there is something to say.
    pub(super) fn progress_tick(&mut self) {
        // The preset's wallpaper is here: the preset goes on the desktop now.
        if let Some((base, _)) = &self.progress.after_base
            && self.catalog.dir_of(*base).is_some()
            && let Some((_, applying)) = self.progress.after_base.take()
        {
            self.status(tf!("\"{}\": its wallpaper is here", "«{}»: их обои на месте", applying.title));
            self.set_again(applying);
        }
        if self.progress.watching() {
            self.probe();
        }
    }

    /// Ask what the daemon and the renderer say, on a thread: the renderer
    /// answers nothing while it loads.
    fn probe(&mut self) {
        if self.progress.fixed || self.progress.probing.swap(true, Ordering::SeqCst) {
            return;
        }
        let probing = self.progress.probing.clone();
        std::thread::spawn(move || {
            let seen = engine::seen();
            probing.store(false, Ordering::SeqCst);
            slot::on_ui(move |app| app.seen_arrived(seen));
        });
    }

    /// What was seen: the wallpaper on its way has come so far.
    fn seen_arrived(&mut self, seen: Seen) {
        if self.progress.fixed {
            return;
        }
        self.progress.seen = seen;
        let Some(phase) = self.progress.phase() else {
            self.show_progress();
            return;
        };
        let Some(applying) = self.progress.applying.as_mut() else { return };
        let changed = applying.said.as_ref() != Some(&phase);
        let was_optimising = matches!(applying.said, Some(Phase::Optimising(_)));
        applying.said = Some(phase.clone());
        let first_shown = matches!(phase, Phase::Shown | Phase::Optimising(_)) && applying.shown_at.is_none();
        // On the desktop, or done making its video lighter: the toast says
        // so for a moment from now.
        if first_shown || (phase == Phase::Shown && was_optimising) {
            applying.shown_at = Some(Instant::now());
        }
        let applying = applying.clone();
        match &phase {
            Phase::Shown | Phase::Optimising(_) => {
                if first_shown {
                    let place = match &applying.output {
                        Some(output) => {
                            tf!("\"{}\" is on monitor {output}", "«{}» на мониторе {output}", applying.title)
                        }
                        None => tf!("\"{}\" is on the desktop", "«{}» на рабочем столе", applying.title),
                    };
                    self.status(place);
                }
                if changed && let Phase::Optimising(n) = phase {
                    self.status(tf!(
                        "\"{}\" plays; {n} video(s) are being re-encoded for this monitor, it gets lighter after",
                        "«{}» играют; видео пережимается под монитор ({n}), потом станет легче",
                        applying.title
                    ));
                }
                // Done with it once it settled on the desktop.
                if phase == Phase::Shown && applying.shown_at.is_some_and(|at| at.elapsed() >= SETTLE) {
                    self.progress.applying = None;
                }
            }
            Phase::Failed(why) => {
                self.progress.applying = None;
                let web_off = engine::plugins()
                    .iter()
                    .any(|p| p.kind == "renderer" && p.name.to_lowercase().contains("web") && !p.enabled);
                let refusal = stage::refusal(why, web_off);
                let words = stage::words(&refusal, self.base_title(&refusal).as_deref());
                log::warn(format!(
                    "\"{}\" is not shown: {why} (the window says: {})",
                    applying.title, words.headline
                ));
                self.alarm(tf!("\"{}\": {}", "«{}»: {}", applying.title, words.headline));
                self.progress.alert = Some(Alert {
                    dir: applying.dir.clone(),
                    title: applying.title.clone(),
                    output: applying.output.clone(),
                    refusal,
                });
                self.show_alert();
            }
            Phase::Stuck if changed => {
                log::warn(format!(
                    "\"{}\": no word from the renderer in {} s",
                    applying.title,
                    stage::STUCK.as_secs()
                ));
                self.alarm(tf!(
                    "\"{}\": no word from the renderer; `we-linux logs` says what it is doing",
                    "«{}»: рендерер молчит; что он делает, видно в `we-linux logs`",
                    applying.title
                ));
            }
            _ => {}
        }
        self.show_progress();
    }

    /// The title of a preset's wallpaper, when it is listed anywhere.
    fn base_title(&self, refusal: &Refusal) -> Option<String> {
        let Refusal::PresetBase(id) = refusal else { return None };
        let title = self.title_of(*id);
        (title != id.to_string()).then_some(title)
    }

    /// The toast, the panel's picture and the tiles, as things are now.
    pub(super) fn show_progress(&mut self) {
        let Some(ui) = self.ui() else { return };
        let phase = self.progress.phase();
        match (&self.progress.applying, &phase) {
            (Some(applying), Some(Phase::Shown)) => {
                ui.set_toast_kind(1);
                ui.set_toast_title(applying.title.as_str().into());
                ui.set_toast_info(t("On the desktop", "На рабочем столе").into());
                ui.set_toast_shown(true);
            }
            (Some(applying), Some(phase)) if !matches!(phase, Phase::Failed(_)) => {
                let behind = match phase {
                    Phase::Waiting(before) => {
                        self.catalog.at(before).map(|i| i.title.clone()).unwrap_or_else(|| {
                            before.file_name().unwrap_or_default().to_string_lossy().into_owned()
                        })
                    }
                    _ => String::new(),
                };
                // A video made lighter is timed from when it showed: that is
                // what the seconds are of.
                let from = match phase {
                    Phase::Optimising(_) => applying.shown_at.unwrap_or(applying.since),
                    _ => applying.since,
                };
                let mut info = stage::doing(phase, &applying.kind, from.elapsed(), &behind);
                if *phase == Phase::Stuck {
                    info = tf!("{info}: see `we-linux logs`", "{info}: смотри `we-linux logs`");
                }
                ui.set_toast_kind(match phase {
                    Phase::Waiting(_) => 2,
                    Phase::Stuck => 3,
                    _ => 0,
                });
                ui.set_toast_title(applying.title.as_str().into());
                ui.set_toast_info(info.into());
                ui.set_toast_shown(true);
            }
            _ => ui.set_toast_shown(false),
        }
        self.refresh_marks();
    }

    /// The panel's picture of the selected wallpaper, with its state over it.
    pub(super) fn show_selected_state(&self, dir: Option<&Path>) {
        let Some(ui) = self.ui() else { return };
        let (state, word) = self.progress.tile(dir);
        // The panel has room for what is being done, not only a word.
        let text = match (&self.progress.applying, self.progress.phase()) {
            (Some(applying), Some(phase)) if dir == Some(applying.dir.as_path()) && phase.pending() => {
                let behind = if let Phase::Waiting(before) = &phase {
                    self.catalog.at(before).map(|i| i.title.clone()).unwrap_or_default()
                } else {
                    String::new()
                };
                stage::doing(&phase, &applying.kind, applying.since.elapsed(), &behind)
            }
            _ => word,
        };
        ui.set_sel_state(state);
        ui.set_sel_state_text(text.into());
    }

    /// The refusal over the grid, or none.
    fn show_alert(&mut self) {
        let Some(ui) = self.ui() else { return };
        let Some(alert) = &self.progress.alert else {
            ui.set_alert_shown(false);
            return;
        };
        let words = stage::words(&alert.refusal, self.base_title(&alert.refusal).as_deref());
        ui.set_alert_headline(tf!("\"{}\": {}", "«{}»: {}", alert.title, words.headline).into());
        ui.set_alert_body(words.body.into());
        let (label, icon) = match &words.remedy {
            Some((label, remedy)) => (label.clone(), remedy_icon(&ui, remedy)),
            None => (String::new(), Glyph::default()),
        };
        ui.set_alert_action(label.into());
        ui.set_alert_icon(icon);
        ui.set_alert_shown(true);
    }

    /// The alert's button.
    pub(super) fn alert_act(&mut self) {
        let Some(alert) = &self.progress.alert else { return };
        let words = stage::words(&alert.refusal, None);
        let Some((_, remedy)) = words.remedy else { return };
        match remedy {
            Remedy::Subscribe(id) => {
                let applying = Applying::new(&alert.title, &alert.dir, alert.output.clone());
                if self.catalog.start(id) {
                    let _ = self.steam.send(Command::Subscribe(id));
                }
                self.progress.after_base = Some((id, applying));
                self.status(tf!(
                    "Subscribing to the preset's wallpaper ({id}): the preset goes on the desktop once it is here",
                    "Подписка на обои пресета ({id}): пресет встанет на рабочий стол, когда они скачаются"
                ));
                self.progress.alert = None;
                self.show_alert();
                self.refresh_marks();
            }
            Remedy::Open(target) => {
                let opened = if target.starts_with("steam://") {
                    desktop::open_steam(target)
                } else {
                    desktop::open(target)
                };
                self.report(opened, t("Opened", "Открыто"));
            }
            Remedy::Settings(tab) => {
                self.show_settings();
                if let Some(ui) = self.ui() {
                    ui.set_settings_tab(tab);
                    ui.set_settings_open(true);
                }
            }
        }
    }

    /// The alert's cross.
    pub(super) fn alert_closed(&mut self) {
        self.progress.alert = None;
        self.show_alert();
        self.refresh_marks();
    }

    /// Set a wallpaper again where it was set before (a preset whose wallpaper came).
    fn set_again(&mut self, applying: Applying) {
        let result = match &applying.output {
            Some(output) => desktop::apply_to(output, &applying.dir),
            None => desktop::clear_outputs().and_then(|()| desktop::apply(&applying.dir)),
        };
        match result {
            Ok(()) => self.follow_applying(Applying::new(&applying.title, &applying.dir, applying.output)),
            Err(e) => self.warn(tf!("That did not work: {e}", "Не получилось: {e}")),
        }
    }

    /// `--show`: the selected wallpaper in a phase of its own, as if it were
    /// seen so (`kind` of the refusal for "failed").
    pub(super) fn show_fixed(&mut self, phase: &str, refusal: &str) {
        let Some(card) = self.selected_card() else { return };
        let Some(dir) = self.dir_of(&card) else { return };
        let mut applying = Applying::new(&card.title, &dir, None);
        let other = PathBuf::from("/nowhere/before");
        let busy = |wallpaper: &Path| {
            Some((Busy::Showing { output: "DP-1".into(), wallpaper: wallpaper.to_owned() }, Duration::ZERO))
        };
        let mut seen = Seen { daemon: true, ..Seen::default() };
        applying.since = Instant::now() - Duration::from_secs(7);
        match phase {
            "starting" => seen.busy = Some((Busy::Starting, Duration::ZERO)),
            "waiting" => seen.busy = busy(&other),
            "optimising" => {
                seen.shown = Some(vec![("DP-1".into(), dir.clone())]);
                seen.optimising = 1;
            }
            "stuck" => {
                seen.shown = Some(Vec::new());
                applying.since = Instant::now() - stage::STUCK - Duration::from_secs(5);
            }
            "done" => seen.shown = Some(vec![("DP-1".into(), dir.clone())]),
            "failed" => {
                let why = match refusal {
                    "preset" => "the preset is for wallpaper 1838024372, which is not installed",
                    "memory" => "not enough video memory",
                    "assets" => {
                        "Wallpaper Engine's assets were not found: install it in Steam, or name the folder with --assets"
                    }
                    "app" => "application wallpapers are not drawn by the engine",
                    "hung" => "the renderer hung loading it and was killed",
                    "other" => "scene.json: unexpected end of file",
                    _ => "web wallpapers are not drawn by the engine",
                };
                seen.errors = vec![(dir.clone(), why.to_owned())];
            }
            _ => seen.busy = busy(&dir),
        }
        self.progress.fixed = false;
        self.progress.alert = None;
        self.progress.applying = Some(applying);
        self.seen_arrived(seen);
        self.progress.fixed = true;
        self.show_selection(true);
    }
}

/// The icon of an alert's button.
fn remedy_icon(ui: &AppWindow, remedy: &Remedy) -> Glyph {
    let icons = ui.global::<Icons>();
    match remedy {
        Remedy::Subscribe(_) => icons.get_download(),
        Remedy::Open(target) if target.starts_with("steam://") => icons.get_steam(),
        Remedy::Open(_) => icons.get_arrow_up_right_from_square(),
        Remedy::Settings(_) => icons.get_gear(),
    }
}
