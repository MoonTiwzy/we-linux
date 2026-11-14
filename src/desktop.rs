//! The desktop side: what is on it (desktop.json), the engine's settings
//! (settings.json), the monitors and the playlist. The window and the command
//! line write the files; `we-linux daemon` (engine.rs) makes them so.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::engine;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
}

fn quiet(mut command: Command) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Waited for off this thread: a child nobody waits for stays in the
    // process table until we exit.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Written whole: the daemon never reads half a file.
    let part = path.with_extension("part");
    let json = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&part, json).and_then(|()| std::fs::rename(&part, path)).map_err(|e| e.to_string())
}

pub fn desktop_file() -> PathBuf {
    config_dir().join("we-linux/desktop.json")
}

/// What is on the desktop.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Desktop {
    /// The wallpaper of every monitor: a folder with a project.json.
    pub wallpaper: Option<PathBuf>,
    /// Monitors with a wallpaper of their own, by name.
    pub outputs: BTreeMap<String, PathBuf>,
    /// Monitors the wallpaper of every monitor is not cloned on: each shows its
    /// own, or nothing. A device starts out here.
    pub apart: Vec<String>,
    /// The picture set last through us (a static wallpaper is not ours to keep).
    pub image: Option<PathBuf>,
}

impl Desktop {
    pub fn load() -> Self {
        std::fs::read(desktop_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        write_json(&desktop_file(), self)
    }
}

const IMAGES: [&str; 7] = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "avif"];
const VIDEOS: [&str; 6] = ["mp4", "webm", "mkv", "mov", "avi", "m4v"];

/// A workshop id, or the path of a wallpaper folder, a video or a picture.
pub fn resolve(target: &str) -> Result<PathBuf, String> {
    if let Ok(id) = target.parse::<u64>() {
        return crate::library::installed()
            .into_iter()
            .find(|i| i.id == id)
            .map(|i| i.dir)
            .ok_or_else(|| format!("workshop item {id} is not on the disk: subscribe to it first"));
    }
    let path = PathBuf::from(target).canonicalize().map_err(|e| format!("{target}: {e}"))?;
    if path.file_name().is_some_and(|n| n == "project.json") {
        return Ok(path.parent().map(PathBuf::from).unwrap_or(path));
    }
    Ok(path)
}

/// Set a wallpaper folder, a video or a picture as the wallpaper of every
/// monitor. A picture is not ours to play: the engine stops and awww, swww
/// or the desktop's own wallpaper program shows it.
pub fn apply(path: &Path) -> Result<(), String> {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let mut desktop = Desktop::load();
    if path.is_dir() {
        if !path.join("project.json").is_file() {
            return Err(format!("{} has no project.json", path.display()));
        }
        desktop.wallpaper = Some(path.to_owned());
    } else if VIDEOS.contains(&extension.as_str()) {
        desktop.wallpaper = Some(engine::video_item(path)?);
    } else if IMAGES.contains(&extension.as_str()) {
        desktop.wallpaper = None;
        desktop.outputs.clear();
        desktop.image = Some(path.to_owned());
        engine::forget_images();
        desktop.save()?;
        engine::announce(path.to_owned());
        return if engine::show_image(path) {
            Ok(())
        } else {
            Err("no awww, swww or wallpaper program of the desktop's own to show a picture".into())
        };
    } else {
        return Err(format!("{}: not a wallpaper folder, a video or a picture", path.display()));
    }
    desktop.save()?;
    engine::ensure_daemon()
}

/// A wallpaper on one monitor only, next to the wallpaper of the rest.
pub fn apply_to(output: &str, path: &Path) -> Result<(), String> {
    // A name that is no monitor's would keep the wallpaper off every desktop.
    let monitors = monitors();
    if !monitors.iter().any(|m| m.name == output) {
        let names = monitors.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ");
        return Err(if names.is_empty() {
            crate::lang::t(
                "the monitors are not known yet: the compositor does not list them, and they are known once a wallpaper is on the desktop",
                "мониторы пока не известны: композитор их не перечисляет, а известны они станут, когда на столе будут обои",
            )
            .to_owned()
        } else {
            crate::tf!(
                "there is no monitor {output}: there are {names}",
                "монитора {output} нет: есть {names}"
            )
        });
    }
    // The engine plays a folder or a video; a picture is the static setter's.
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let wallpaper = if path.is_dir() {
        if !path.join("project.json").is_file() {
            return Err(format!("{} has no project.json", path.display()));
        }
        path.to_owned()
    } else if VIDEOS.contains(&extension.as_str()) {
        engine::video_item(path)?
    } else {
        return Err(crate::tf!(
            "{}: one monitor takes a wallpaper folder or a video, not a picture",
            "{}: одному монитору можно поставить папку обоев или видео, не картинку",
            path.display()
        ));
    };
    let mut desktop = Desktop::load();
    desktop.outputs.insert(output.to_owned(), wallpaper);
    desktop.save()?;
    engine::ensure_daemon()
}

/// Forget the per-monitor wallpapers. A device keeps its own: a wallpaper set
/// for every monitor is set for the screens, and what was picked for a panel
/// of another shape stays on it.
pub fn clear_outputs() -> Result<(), String> {
    let mut desktop = Desktop::load();
    let devices: Vec<String> = devices().into_iter().map(|device| device.name).collect();
    let before = desktop.outputs.len();
    desktop.outputs.retain(|name, _| devices.contains(name));
    if desktop.outputs.len() != before {
        desktop.save()?;
    }
    Ok(())
}

/// No live wallpaper: the engine stops, what is under it shows. Back within
/// about three seconds whatever the daemon and the renderer are doing.
pub fn off() -> Result<(), String> {
    take_off()?;
    engine::see_off();
    Ok(())
}

/// What `off` writes, for the window: it sees the renderer off on a thread
/// of its own ([`engine::see_off`]) instead of waiting for it.
pub fn take_off() -> Result<(), String> {
    let mut desktop = Desktop::load();
    desktop.wallpaper = None;
    desktop.outputs.clear();
    desktop.save()
}

fn profiles_dir() -> PathBuf {
    config_dir().join("we-linux/profiles")
}

/// The saved monitor profiles: what each monitor showed, by name.
pub fn profiles() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(profiles_dir())
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Keep what every monitor shows now under a name.
pub fn save_profile(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') {
        return Err(crate::lang::t(
            "a profile needs a name without \"/\"",
            "у профиля должно быть имя без «/»",
        )
        .into());
    }
    write_json(&profiles_dir().join(format!("{name}.json")), &Desktop::load())
}

/// Put a saved profile on the desktop.
pub fn load_profile(name: &str) -> Result<(), String> {
    let profile: Desktop = std::fs::read(profiles_dir().join(format!("{name}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| crate::tf!("the profile {name} is not readable", "профиль {name} не читается"))?;
    // The picture is not ours to show: only the live wallpapers come back.
    let desktop = Desktop { image: Desktop::load().image, ..profile };
    desktop.save()?;
    if desktop.wallpaper.is_some() || !desktop.outputs.is_empty() { engine::ensure_daemon() } else { Ok(()) }
}

/// The live wallpaper of every monitor, if any.
pub fn current() -> Option<PathBuf> {
    Desktop::load().wallpaper
}

/// Per-monitor wallpapers: output name and path.
pub fn outputs() -> Vec<(String, PathBuf)> {
    Desktop::load().outputs.into_iter().collect()
}

/// A device: something that shows a wallpaper and is not a monitor (the panel
/// on a cooler, a keyboard's screen, a stream). The renderer draws its wallpaper
/// at the device's size and writes the frames to `command`'s standard input as
/// raw RGB (the renderer's docs/devices.md). To the rest of the program it is one
/// more monitor: it has a name, and a wallpaper of its own or the one of all.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Device {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Frames a second it is sent at most.
    pub fps: u32,
    /// The program the frames go to, run through `sh -c`.
    pub command: String,
    pub enabled: bool,
    /// How a wallpaper of another shape fills it: "cover" (cropped), "contain"
    /// (the whole picture, with bars) or "stretch".
    pub fill: String,
    /// Which part of the wallpaper it shows, on top of the fill. Enlarged about
    /// the device's middle, in percent: 100 as the fill has it, 200 twice as
    /// large, 50 half as large (more of the wallpaper, its background around it).
    pub zoom: f32,
    /// Moved right and up, in percent of the device's width and height. It is
    /// the wallpaper that moves: what the fill cropped away comes into view.
    pub shift_x: f32,
    pub shift_y: f32,
}

/// How far a device's picture can be enlarged, shrunk and moved, in percent
/// (the renderer's own limits).
pub const ZOOM_RANGE: (f32, f32) = (5.0, 2000.0);
pub const SHIFT_RANGE: (f32, f32) = (-2000.0, 2000.0);

impl Default for Device {
    fn default() -> Self {
        Self {
            name: String::new(),
            width: 480,
            height: 480,
            fps: 30,
            command: String::new(),
            enabled: true,
            fill: "cover".into(),
            zoom: 100.0,
            shift_x: 0.0,
            shift_y: 0.0,
        }
    }
}

impl Device {
    /// Why it cannot be a device, if it cannot: the renderer would refuse it.
    pub fn problem(&self) -> Option<String> {
        let named = !self.name.is_empty()
            && self.name.len() <= 64
            && self.name != "*"
            && self.name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '));
        if !named {
            return Some(crate::tf!(
                "a device's name is letters, digits, spaces, - _ and . (\"{}\" is not)",
                "имя устройства: буквы, цифры, пробелы, - _ и . («{}» не подходит)",
                self.name
            ));
        }
        if !(1..=8192).contains(&self.width) || !(1..=8192).contains(&self.height) {
            return Some(crate::tf!(
                "{}: the size is from 1 to 8192 pixels a side",
                "{}: размер от 1 до 8192 пикселей по стороне",
                self.name
            ));
        }
        if self.command.trim().is_empty() {
            return Some(crate::tf!(
                "{}: a device needs the command its frames go to",
                "{}: устройству нужна команда, которой уходят кадры",
                self.name
            ));
        }
        None
    }

    /// As the renderer takes it (`devices/<name>`).
    pub fn spec(&self) -> serde_json::Value {
        serde_json::json!({
            "size": [self.width, self.height],
            "fps": self.fps.clamp(1, 240),
            "exec": self.command,
            "enabled": self.enabled,
        })
    }

    /// Its picture, as the renderer takes it: the output's fill, and the zoom
    /// and shifts of its look.
    pub fn picture(&self) -> [(&'static str, serde_json::Value); 4] {
        let fill =
            if ["cover", "contain", "stretch"].contains(&self.fill.as_str()) { &self.fill } else { "cover" };
        [
            ("fill", serde_json::json!(fill)),
            ("look/zoom", part(self.zoom, ZOOM_RANGE)),
            ("look/shift_x", part(self.shift_x, SHIFT_RANGE)),
            ("look/shift_y", part(self.shift_y, SHIFT_RANGE)),
        ]
    }
}

/// A percentage as the renderer takes it: a part of one, within `range`, to a
/// ten-thousandth (what is told is compared with what was told before).
fn part(percent: f32, range: (f32, f32)) -> serde_json::Value {
    let percent = if percent.is_finite() { percent.clamp(range.0, range.1) } else { 100.0 };
    serde_json::json!((f64::from(percent) * 100.0).round() / 10000.0)
}

/// The devices that are set up and can be: the ones the renderer is told.
pub fn devices() -> Vec<Device> {
    let mut seen = std::collections::BTreeSet::new();
    Settings::load()
        .devices
        .into_iter()
        .filter(|device| device.problem().is_none() && seen.insert(device.name.clone()))
        .collect()
}

pub struct Monitor {
    pub name: String,
    /// Its size in pixels, as "2560x1440".
    pub detail: String,
    /// Where it stands on the desk and how large it is there.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The screens: the monitors of the compositor.
pub fn screens() -> Vec<Monitor> {
    let devices: Vec<String> = devices().into_iter().map(|device| device.name).collect();
    if let Some(monitors) = crate::compositor::current().monitors().filter(|m| !m.is_empty()) {
        return monitors
            .into_iter()
            .map(|m| Monitor {
                detail: format!("{}x{}", m.pixels.0, m.pixels.1),
                name: m.name,
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            })
            .collect();
    }
    // The compositor does not say: the renderer knows the monitors it draws
    // on, not where they stand; they are put side by side. (It lists the
    // devices as outputs too: those are not screens.)
    let mut x = 0.0;
    engine::outputs()
        .into_iter()
        .filter(|(name, _, _)| !devices.contains(name))
        .map(|(name, width, height)| {
            let monitor = Monitor {
                name,
                detail: format!("{width}x{height}"),
                x,
                y: 0.0,
                width: width as f32,
                height: height as f32,
            };
            x += width as f32;
            monitor
        })
        .collect()
}

/// Everything a wallpaper can be put on: the screens, then the devices that
/// are on. A device stands to the right of the screens, drawn large enough to
/// be picked whatever its own size is.
pub fn monitors() -> Vec<Monitor> {
    let mut monitors = screens();
    let top = monitors.iter().map(|m| m.y).fold(f32::MAX, f32::min);
    let tallest = monitors.iter().map(|m| m.height).fold(0.0, f32::max);
    let (top, side) = if monitors.is_empty() { (0.0, 480.0) } else { (top, tallest * 0.6) };
    let mut x = monitors.iter().map(|m| m.x + m.width).fold(0.0, f32::max);
    for device in devices().into_iter().filter(|device| device.enabled) {
        if monitors.iter().any(|m| m.name == device.name) {
            continue;
        }
        let larger = device.width.max(device.height) as f32;
        let (width, height) = (side * device.width as f32 / larger, side * device.height as f32 / larger);
        x += side * 0.12;
        monitors.push(Monitor {
            detail: format!("{}x{}", device.width, device.height),
            name: device.name,
            x,
            y: top,
            width,
            height,
        });
        x += width;
    }
    monitors
}

/// A device got another name: its own wallpaper, and whether it takes the
/// wallpaper of every monitor, go on under the new one.
pub fn rename_output(from: &str, to: &str) -> Result<(), String> {
    let mut desktop = Desktop::load();
    let before = desktop.clone();
    if let Some(wallpaper) = desktop.outputs.remove(from) {
        desktop.outputs.insert(to.to_owned(), wallpaper);
    }
    for name in desktop.apart.iter_mut().filter(|name| *name == from) {
        *name = to.to_owned();
    }
    if desktop == before { Ok(()) } else { desktop.save() }
}

/// Whether the wallpaper of every monitor is cloned on this one.
pub fn cloned(output: &str) -> bool {
    !Desktop::load().apart.iter().any(|name| name == output)
}

/// Have the wallpaper of every monitor cloned on a monitor, or keep the monitor
/// apart: it then shows its own wallpaper, or nothing.
pub fn set_cloned(output: &str, cloned: bool) -> Result<(), String> {
    let mut desktop = Desktop::load();
    let before = desktop.clone();
    desktop.apart.retain(|name| name != output);
    if !cloned {
        desktop.apart.push(output.to_owned());
    }
    if desktop == before { Ok(()) } else { desktop.save() }
}

/// A device that went: its own wallpaper and its place among the monitors go too.
pub fn forget_output(output: &str) -> Result<(), String> {
    let mut desktop = Desktop::load();
    let before = desktop.clone();
    desktop.outputs.remove(output);
    desktop.apart.retain(|name| name != output);
    if desktop == before { Ok(()) } else { desktop.save() }
}

/// Take a monitor's own wallpaper off: it shows what every monitor does.
pub fn remove_from(output: &str) -> Result<(), String> {
    let mut desktop = Desktop::load();
    desktop.outputs.remove(output);
    desktop.save()
}

/// The family the system has for Arial, which is what the window is set in.
pub fn ui_font() -> String {
    Command::new("fc-match")
        .args(["-f", "%{family}", "Arial"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|families| families.split(',').next().map(|family| family.trim().to_owned()))
        .filter(|family| !family.is_empty())
        .unwrap_or_else(|| "Liberation Sans".to_owned())
}

/// Put a text on the clipboard (wl-copy does it).
pub fn copy(text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "wl-copy (wl-clipboard) is not installed".to_owned())?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    child.wait().map_err(|e| e.to_string()).and_then(|status| {
        if status.success() { Ok(()) } else { Err("wl-copy did not take the text".to_owned()) }
    })
}

const UNIT: &str = "we-linux.service";

/// Whether systemd starts the daemon with the session.
pub fn autostart() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-enabled", UNIT])
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "enabled")
}

/// Have systemd start the daemon with the graphical session, or no longer.
/// A session that does not start graphical-session.target (a compositor
/// started by hand, without a session manager) starts no such service: then
/// the answer is the line of the compositor's config that does it.
pub fn set_autostart(on: bool) -> Result<Option<String>, String> {
    let run = |args: &[&str]| {
        let status = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("systemctl: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("systemctl --user {} did not work", args.join(" ")))
        }
    };
    if !on {
        return run(&["disable", UNIT]).map(|()| None);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let unit = format!(
        "[Unit]\nDescription=WE Linux: live wallpapers\nPartOf=graphical-session.target\nAfter=graphical-session.target\n\n\
         [Service]\nExecStart={} daemon\nRestart=on-failure\n\n[Install]\nWantedBy=graphical-session.target\n",
        exe.display()
    );
    let file = config_dir().join("systemd/user").join(UNIT);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, unit).map_err(|e| e.to_string())?;
    run(&["daemon-reload"])?;
    run(&["enable", UNIT])?;
    Ok(if crate::compositor::session_target() {
        None
    } else {
        Some(crate::compositor::current().autostart_line().unwrap_or_else(|| "we-linux daemon".to_owned()))
    })
}

pub fn settings_file() -> PathBuf {
    config_dir().join("we-linux/settings.json")
}

/// The engine's settings.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub fps: i32,
    /// Videos inside scenes on the GPU instead of the CPU.
    pub video_gpu: bool,
    pub audio: bool,
    pub volume: i32,
    /// What a wallpaper does while another application is focused on its
    /// monitor, maximised over it, fullscreen on it, while another application
    /// plays sound, and while its monitor sleeps (as the compositor tells; the
    /// sound as the sound server does):
    /// "run", "mute", "pause" (that monitor), "pauseall" (every monitor) or
    /// "stop" (the renderer goes once every monitor stops, and the video
    /// memory with it). The original's "Воспроизведение".
    pub playback_focus: String,
    pub playback_maximized: String,
    pub playback_fullscreen: String,
    pub playback_audio: String,
    pub playback_sleep: String,
    /// Textures no larger than 2048 pixels a side.
    pub texture_half: bool,
    /// Give awww, swww or the desktop's own wallpaper program a still of the wallpaper.
    pub still: bool,
    /// A shell command run when the wallpaper changes, with WE_LINUX_WALLPAPER,
    /// WE_LINUX_STILL and WE_LINUX_TITLE in its environment.
    pub hook: String,
    /// What shows a wallpaper and is not a monitor.
    pub devices: Vec<Device>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fps: 60,
            video_gpu: false,
            audio: false,
            volume: 50,
            playback_focus: "run".into(),
            playback_maximized: "run".into(),
            playback_fullscreen: "pause".into(),
            playback_audio: "run".into(),
            playback_sleep: "run".into(),
            texture_half: false,
            still: true,
            hook: String::new(),
            devices: Vec::new(),
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let Some(mut json) = std::fs::read(settings_file())
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            return Self::default();
        };
        // Written before "Воспроизведение" had the original's choices: what
        // was paused or stopped under a fullscreen or a maximised window still is.
        if json.get("playback_fullscreen").is_none()
            && let Some(map) = json.as_object_mut()
        {
            let flag = |key: &str| map.get(key).and_then(serde_json::Value::as_bool);
            let fullscreen = match (flag("stop_fullscreen"), flag("pause_fullscreen")) {
                (Some(true), _) => "stop",
                (_, Some(false)) => "run",
                _ => "pause",
            };
            let maximized = match map.get("maximized").and_then(serde_json::Value::as_i64) {
                Some(1) => "pause",
                Some(2) => "stop",
                _ => "run",
            };
            map.insert("playback_fullscreen".into(), fullscreen.into());
            map.insert("playback_maximized".into(), maximized.into());
        }
        serde_json::from_value(json).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        write_json(&settings_file(), self)
    }
}

/// The playlist: wallpapers set one after another by the daemon.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Playlist {
    pub items: Vec<PathBuf>,
    pub minutes: u32,
    pub random: bool,
    pub running: bool,
    /// The item set last.
    pub index: usize,
}

fn playlist_file() -> PathBuf {
    config_dir().join("we-linux/playlist.json")
}

fn saved_dir() -> PathBuf {
    config_dir().join("we-linux/playlists")
}

impl Playlist {
    pub fn load() -> Self {
        let mut playlist: Playlist = std::fs::read(playlist_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if playlist.minutes == 0 {
            playlist.minutes = 30;
        }
        playlist
    }

    pub fn save(&self) {
        // Written whole: the daemon reading half a file would take the
        // playlist for stopped and go on to the next wallpaper.
        let _ = write_json(&playlist_file(), self);
    }

    /// Names of the saved playlists, newest first.
    pub fn saved() -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(saved_dir())
            .map(|d| {
                d.flatten()
                    .filter_map(|e| {
                        e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names.reverse();
        names
    }

    pub fn save_as(&self, name: &str) {
        let _ = std::fs::create_dir_all(saved_dir());
        let copy = Playlist { running: false, index: 0, ..self.clone() };
        if let Ok(json) = serde_json::to_vec_pretty(&copy) {
            let _ = std::fs::write(saved_dir().join(format!("{name}.json")), json);
        }
    }

    pub fn load_saved(name: &str) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(saved_dir().join(format!("{name}.json"))).ok()?).ok()
    }
}

/// The daemon's playlist half: while the playlist runs, set its wallpapers
/// in turn. The window changes the file; it is looked at every few seconds.
pub fn run_playlist() {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64 | 1);
    let modified = || std::fs::metadata(playlist_file()).and_then(|m| m.modified()).ok();
    // What was set last and where in the playlist it was. Kept here: the
    // window writes the file with the index it read when it opened.
    let mut current: Option<PathBuf> = None;
    let mut index = 0;
    loop {
        let mut playlist = Playlist::load();
        playlist.items.retain(|p| p.exists());
        if !playlist.running || playlist.items.is_empty() {
            current = None;
            std::thread::sleep(std::time::Duration::from_secs(3));
            continue;
        }
        let count = playlist.items.len();
        index = match &current {
            // The playlist was started: go on from where it stopped.
            None => playlist.index,
            Some(shown) => match playlist.items.iter().position(|p| p == shown) {
                Some(at) if playlist.random && count > 1 => {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    at + 1 + (seed as usize) % (count - 1)
                }
                Some(at) => at + 1,
                // What was shown left the playlist: what took its place.
                None => index.min(count - 1),
            },
        } % count;
        let item = playlist.items[index].clone();
        let _ = clear_outputs().and_then(|()| apply(&item));
        playlist.index = index;
        playlist.save();
        let mut written = modified();
        let mut minutes = playlist.minutes;
        let since = std::time::Instant::now();
        current = Some(item.clone());
        while since.elapsed() < std::time::Duration::from_secs(u64::from(minutes.max(1)) * 60) {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let stamp = modified();
            if stamp == written {
                continue;
            }
            // The window changed the playlist. What is shown stays for its
            // time (a wallpaper added or another removed does not start it
            // again) unless it is not in the playlist any more.
            written = stamp;
            let changed = Playlist::load();
            if !changed.running || !changed.items.contains(&item) {
                break;
            }
            minutes = changed.minutes;
        }
    }
}

/// Ask for a file with the desktop's own dialog (zenity or kdialog): a
/// picture, or what plays (a video, a wallpaper's project.json).
pub fn pick_file(picture: bool) -> Result<Option<PathBuf>, String> {
    let patterns: Vec<String> = if picture {
        IMAGES.iter().map(|e| format!("*.{e}")).collect()
    } else {
        VIDEOS.iter().map(|e| format!("*.{e}")).chain(["project.json".to_owned()]).collect()
    };
    let zenity_filter = format!("--file-filter={}", patterns.join(" "));
    let kdialog_filter = patterns.join(" ");
    let attempts: [(&str, Vec<&str>); 2] = [
        ("zenity", vec!["--file-selection", "--title=Open a wallpaper", &zenity_filter]),
        ("kdialog", vec!["--getopenfilename", ".", &kdialog_filter]),
    ];
    for (program, args) in attempts {
        match Command::new(program).args(args).stderr(Stdio::null()).output() {
            Ok(out) => {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                return Ok(Some(PathBuf::from(path)).filter(|p| !p.as_os_str().is_empty() && p.exists()));
            }
            Err(_) => continue,
        }
    }
    Err("zenity or kdialog is needed for the file dialog".into())
}

pub fn open(target: &str) -> Result<(), String> {
    let mut command = Command::new("xdg-open");
    command.arg(target);
    quiet(command)
}

pub fn open_steam(target: &str) -> Result<(), String> {
    let mut command = Command::new("steam");
    command.arg(target);
    quiet(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Device {
        Device { name: "panel".into(), command: "send-frames".into(), ..Device::default() }
    }

    #[test]
    fn a_device_is_told_to_the_renderer_as_it_takes_one() {
        let device = Device { fps: 500, ..panel() };
        assert_eq!(
            device.spec(),
            serde_json::json!({ "size": [480, 480], "fps": 240, "exec": "send-frames", "enabled": true })
        );
        let picture =
            Device { fill: "contain".into(), zoom: 150.0, shift_x: -20.5, shift_y: 5000.0, ..panel() }
                .picture();
        assert_eq!(picture[0], ("fill", serde_json::json!("contain")));
        assert_eq!(picture[1], ("look/zoom", serde_json::json!(1.5)));
        assert_eq!(picture[2], ("look/shift_x", serde_json::json!(-0.205)));
        assert_eq!(picture[3], ("look/shift_y", serde_json::json!(20.0)), "a shift has the renderer's limit");
        let small = Device { zoom: 1.0, ..panel() }.picture();
        assert_eq!(small[1].1, serde_json::json!(0.05), "and so has a zoom");
        assert_eq!(Device { fill: "sideways".into(), ..panel() }.picture()[0].1, "cover");
    }

    #[test]
    fn what_cannot_be_a_device_says_why() {
        assert!(panel().problem().is_none());
        for broken in [
            Device { name: String::new(), ..panel() },
            Device { name: "a/b".into(), ..panel() },
            Device { name: "*".into(), ..panel() },
            Device { width: 0, ..panel() },
            Device { height: 9000, ..panel() },
            Device { command: "  ".into(), ..panel() },
        ] {
            assert!(broken.problem().is_some(), "{broken:?}");
        }
    }

    #[test]
    fn settings_written_before_devices_still_load() {
        let old: Settings = serde_json::from_str(r#"{"fps": 30, "hook": "true"}"#).unwrap();
        assert!(old.devices.is_empty() && old.fps == 30);
        let with: Settings =
            serde_json::from_str(r#"{"devices": [{"name": "p", "command": "cat", "width": 64}]}"#).unwrap();
        assert_eq!(
            with.devices,
            [Device { name: "p".into(), command: "cat".into(), width: 64, ..Device::default() }],
            "what a device does not say is as a device comes"
        );
    }
}
