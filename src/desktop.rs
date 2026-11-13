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
/// or hyprpaper (whichever runs) shows it.
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
        return if engine::show_image(path) { Ok(()) } else { Err("no awww, swww or hyprpaper to show a picture".into()) };
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
                "the monitors are not known yet: they are once a wallpaper is on the desktop (or on Hyprland)",
                "мониторы пока не известны: они появятся, когда на столе будут обои (или в Hyprland)",
            )
            .to_owned()
        } else {
            crate::tf!("there is no monitor {output}: there are {names}", "монитора {output} нет: есть {names}")
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

/// Forget the per-monitor wallpapers.
pub fn clear_outputs() -> Result<(), String> {
    let mut desktop = Desktop::load();
    if !desktop.outputs.is_empty() {
        desktop.outputs.clear();
        desktop.save()?;
    }
    Ok(())
}

/// No live wallpaper: the engine stops, what is under it shows.
pub fn off() -> Result<(), String> {
    let mut desktop = Desktop::load();
    desktop.wallpaper = None;
    desktop.outputs.clear();
    desktop.save()
}

/// The live wallpaper of every monitor, if any.
pub fn current() -> Option<PathBuf> {
    Desktop::load().wallpaper
}

/// Per-monitor wallpapers: output name and path.
pub fn outputs() -> Vec<(String, PathBuf)> {
    Desktop::load().outputs.into_iter().collect()
}

pub struct Monitor {
    pub name: String,
    pub detail: String,
}

pub fn monitors() -> Vec<Monitor> {
    let hyprland = Command::new("hyprctl").args(["monitors", "-j"]).output().ok().and_then(|out| serde_json::from_slice::<serde_json::Value>(&out.stdout).ok());
    if let Some(list) = hyprland.as_ref().and_then(|json| json.as_array()) {
        return list
            .iter()
            .map(|m| Monitor {
                name: m["name"].as_str().unwrap_or("").to_owned(),
                detail: format!(
                    "{}x{} @ {:.0} Hz",
                    m["width"].as_i64().unwrap_or(0),
                    m["height"].as_i64().unwrap_or(0),
                    m["refreshRate"].as_f64().unwrap_or(0.0)
                ),
            })
            .collect();
    }
    // Not Hyprland: the renderer knows the monitors it draws on.
    engine::outputs().into_iter().map(|(name, width, height)| Monitor { name, detail: format!("{width}x{height}") }).collect()
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
    /// Hyprland: freeze the wallpaper of a monitor that shows a fullscreen
    /// window; it goes on from the same frame after.
    pub pause_fullscreen: bool,
    /// Hyprland: stop the engine while every monitor shows a fullscreen
    /// window (the video memory is freed, the scene starts over after).
    pub stop_fullscreen: bool,
    /// Give awww, swww or hyprpaper a still of the wallpaper.
    pub still: bool,
    /// A shell command run when the wallpaper changes, with WE_LINUX_WALLPAPER,
    /// WE_LINUX_STILL and WE_LINUX_TITLE in its environment.
    pub hook: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { fps: 60, video_gpu: false, audio: false, volume: 50, pause_fullscreen: true, stop_fullscreen: false, still: true, hook: String::new() }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
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
                    .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(str::to_owned))
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

/// Ask for a file with the desktop's own dialog (zenity or kdialog).
pub fn pick_file() -> Result<Option<PathBuf>, String> {
    let attempts: [(&str, &[&str]); 2] = [
        ("zenity", &["--file-selection", "--title=Open a wallpaper"]),
        ("kdialog", &["--getopenfilename", "."]),
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
