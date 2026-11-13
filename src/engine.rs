//! `we-linux daemon`: what keeps the wallpaper on the desktop.
//!
//! It runs penguin-engine's `penguind` for the wallpaper in `desktop.json`
//! (one process for every monitor; a process a monitor when one of them has a
//! wallpaper of its own), starts it again when the wallpaper or the settings
//! change, and on Hyprland freezes it on a monitor that shows a fullscreen
//! window (or, when asked, stops it while every monitor does, so a game gets
//! the video memory). It also gives the desktop a
//! still of the wallpaper (awww, swww or hyprpaper, when one runs), runs the
//! user's hook, keeps the engine's layer over the static wallpaper's, and
//! steps through the playlist.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use crate::desktop::{self, Desktop, Settings};

unsafe extern "C" {
    fn prctl(option: i32, ...) -> i32;
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
}
const PR_SET_PDEATHSIG: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

/// The daemon was asked to go (SIGTERM, SIGINT).
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_stop(_: i32) {
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The software video path's mpv options: a quick scaler, three decoder
/// threads (each holds frames) and no read-ahead.
const MPV_OPTS: &str = "sws-fast=yes,sws-scaler=fast-bilinear,sws-allow-zimg=no,vd-lavc-threads=3,cache=no,\
                        demuxer-max-bytes=4MiB,demuxer-max-back-bytes=0,demuxer-readahead-secs=0";

/// Where the daemon's lock, the engines' sockets and their logs are
/// (WE_LINUX_RUNTIME_DIR names another place: a second daemon for a test).
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WE_LINUX_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    // Without a runtime directory: in the user's own cache, not in /tmp,
    // where anyone could have made the folder first.
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| cache_dir().join("run"), |dir| PathBuf::from(dir).join("we-linux"))
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"))
        .join("we-linux")
}

/// A program of penguin-engine (penguind, penguinctl, scene-viewer): in
/// $WE_LINUX_ENGINE_DIR, next to this binary, or on the PATH.
pub fn bin(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("WE_LINUX_ENGINE_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.to_owned());
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// The control sockets of the running engines.
pub fn sockets() -> Vec<PathBuf> {
    std::fs::read_dir(runtime_dir())
        .map(|dirs| dirs.flatten().map(|d| d.path().join("control.sock")).filter(|s| s.exists()).collect())
        .unwrap_or_default()
}

/// The control sockets of the engines that show `wallpaper`.
pub fn sockets_showing(wallpaper: &Path) -> Vec<PathBuf> {
    specs(&Desktop::load(), &Settings::load())
        .into_iter()
        .flatten()
        .filter(|spec| spec.wallpaper == wallpaper)
        .map(|spec| runtime_dir().join(spec.key).join("control.sock"))
        .filter(|socket| socket.exists())
        .collect()
}

pub fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

fn run(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

fn hypr_json(what: &str) -> Option<serde_json::Value> {
    let out = Command::new("hyprctl").args([what, "-j"]).stderr(Stdio::null()).output().ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

/// Hyprland's monitors, each with whether it shows a fullscreen window
/// (not just a maximized one): its desktop is not there to look at.
fn covered_monitors() -> Vec<(String, bool)> {
    let (Some(monitors), Some(clients)) = (hypr_json("monitors"), hypr_json("clients")) else { return Vec::new() };
    let fullscreen: Vec<i64> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["fullscreen"].as_i64().unwrap_or(0) >= 2)
        .filter_map(|c| c["workspace"]["id"].as_i64())
        .collect();
    monitors
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let covered = [m["activeWorkspace"]["id"].as_i64(), m["specialWorkspace"]["id"].as_i64()]
                .into_iter()
                .flatten()
                .any(|id| id != 0 && fullscreen.contains(&id));
            (m["name"].as_str().unwrap_or("").to_owned(), covered)
        })
        .collect()
}

/// The size of the largest monitor (Hyprland), else 4K: videos in scenes are
/// scaled down to its width and stills are rendered at it.
fn largest_monitor() -> (u32, u32) {
    hypr_json("monitors")
        .and_then(|m| {
            m.as_array()?
                .iter()
                .filter_map(|m| Some((m["width"].as_u64()? as u32, m["height"].as_u64()? as u32)))
                .max_by_key(|(w, h)| w * h)
        })
        .unwrap_or((3840, 2160))
}

/// Hyprland stacks the layers of one level by their `order`, lower on top:
/// -1 keeps the engine over a static wallpaper's layer (awww, swww,
/// hyprpaper: 0) whichever of them started last. The layer fades in and out.
fn hypr_layer_rule() {
    if !hyprland() {
        return;
    }
    // The Lua config (0.55+), the block syntax (0.53+), the old one.
    let lua = r#"hl.layer_rule({ match = { namespace = "^(penguin-engine)$" }, order = -1, animation = "fade" })"#;
    let _ = run("hyprctl", &["eval", lua])
        || run("hyprctl", &["keyword", "layerrule", "match:namespace ^(penguin-engine)$, order -1, animation fade"])
        || (run("hyprctl", &["keyword", "layerrule", "order -1, penguin-engine"])
            && run("hyprctl", &["keyword", "layerrule", "animation fade, penguin-engine"]));
}

// ------------------------------------------------------------------ stills

fn project(dir: &Path) -> serde_json::Value {
    std::fs::read(dir.join("project.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn fps(settings: &Settings) -> i32 {
    settings.fps.clamp(10, 240)
}

fn engine_env(command: &mut Command, settings: &Settings) {
    let (width, _) = largest_monitor();
    command
        // Effect masks are halved while that loses nothing; the art is left alone.
        .env("PENGUIN_TEX_MAX", "16384")
        .env("PENGUIN_VIDEO_MAX", width.to_string())
        // Video frames past the engine's frame rate would be decoded for nothing.
        .env("PENGUIN_VIDEO_FPS", fps(settings).to_string())
        // A layer's own buffers at the size it reaches the monitor at: a 4K
        // scene on a 1440p monitor takes a third less video memory.
        .env("PENGUIN_TARGET_SCALE", "1")
        .env("PENGUIN_SHARE_VIDEO", "1")
        .env("MALLOC_ARENA_MAX", "2");
    if !settings.video_gpu {
        command.env("WPE_VIDEO", "cpu").env("WPE_MPV_OPTS", MPV_OPTS);
    }
    if settings.mirror {
        command.env("PENGUIN_MIRROR", "1");
    }
}

/// A still of a wallpaper, for the static wallpaper under the engine and for
/// the hook: a frame the engine renders of a scene, the first frame of a
/// video, else the wallpaper's own preview. Cached.
pub fn still(wallpaper: &Path) -> Option<PathBuf> {
    let json = project(wallpaper);
    let kind = json["type"].as_str().unwrap_or("scene").to_lowercase();
    let dir = cache_dir().join("stills");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{:016x}", fnv(&wallpaper.to_string_lossy()));
    let fresh = |out: &Path| {
        let made = std::fs::metadata(out).and_then(|m| m.modified()).ok();
        let source = std::fs::metadata(wallpaper.join("project.json")).and_then(|m| m.modified()).ok();
        made.is_some() && made >= source
    };
    if kind == "scene"
        && let (Some(viewer), Some(assets)) = (bin("scene-viewer"), crate::library::assets())
    {
        let (w, h) = largest_monitor();
        let out = dir.join(format!("{name}-{w}x{h}.png"));
        if fresh(&out) {
            return Some(out);
        }
        let part = dir.join(format!("{name}.{}.part.png", std::process::id()));
        let mut command = Command::new("timeout");
        command.arg("30").arg(viewer).arg(wallpaper).arg("--assets").arg(assets);
        command.args(["--size", &format!("{w}x{h}"), "--time", "2", "--frames", "4", "--screenshot"]).arg(&part);
        engine_env(&mut command, &Settings::load());
        let done = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if done.is_ok_and(|s| s.success()) && std::fs::rename(&part, &out).is_ok() {
            return Some(out);
        }
        let _ = std::fs::remove_file(&part);
    }
    if kind == "video"
        && let Some(file) = json["file"].as_str()
    {
        let out = dir.join(format!("{name}.jpg"));
        if fresh(&out) {
            return Some(out);
        }
        let part = dir.join(format!("{name}.{}.part.jpg", std::process::id()));
        let video = wallpaper.join(file);
        let done = run(
            "ffmpeg",
            &["-v", "error", "-y", "-i", &video.to_string_lossy(), "-frames:v", "1", "-q:v", "2", &part.to_string_lossy()],
        );
        if done && std::fs::rename(&part, &out).is_ok() {
            return Some(out);
        }
        let _ = std::fs::remove_file(&part);
    }
    // The preview: a GIF's first frame, as a picture any wallpaper setter takes.
    let preview = wallpaper.join(json["preview"].as_str()?);
    let out = dir.join(format!("{name}-preview.png"));
    if !fresh(&out) {
        image::open(&preview).ok()?.save(&out).ok()?;
    }
    Some(out)
}

/// Hand a picture to the static wallpaper setter that runs: awww, swww or
/// hyprpaper. False when there is none.
pub fn show_image(image: &Path) -> bool {
    let path = image.to_string_lossy();
    run("awww", &["img", &path])
        || run("swww", &["img", &path])
        || (run("hyprctl", &["hyprpaper", "preload", &path]) && run("hyprctl", &["hyprpaper", "wallpaper", &format!(",{path}")]))
}

/// Where the pictures the static setter showed before our still are kept.
fn before_file() -> PathBuf {
    desktop::config_dir().join("we-linux/before.json")
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What awww, swww or hyprpaper shows now: monitor and picture.
fn shown_images() -> Vec<(String, PathBuf)> {
    // ": DP-1: 2560x1440, scale: 1, currently displaying: image: /a/b.png"
    let daemon = |text: String| -> Vec<(String, PathBuf)> {
        text.lines()
            .filter_map(|line| {
                let (head, image) = line.split_once("currently displaying: image: ")?;
                let head = head.split_once(", scale").map_or(head, |(h, _)| h);
                let monitor = head.rsplit_once(": ")?.0.rsplit(':').next()?.trim();
                (!monitor.is_empty()).then(|| (monitor.to_owned(), PathBuf::from(image.trim())))
            })
            .collect()
    };
    if let Some(text) = output("awww", &["query"]).or_else(|| output("swww", &["query"])) {
        return daemon(text);
    }
    // "DP-1 = /a/b.png"
    output("hyprctl", &["hyprpaper", "listactive"])
        .map(|text| {
            text.lines()
                .filter_map(|line| line.split_once(" = "))
                .filter(|(_, image)| image.starts_with('/'))
                .map(|(monitor, image)| (monitor.trim().to_owned(), PathBuf::from(image.trim())))
                .collect()
        })
        .unwrap_or_default()
}

/// Keep what the static setter shows, once, before a still goes over it.
fn remember_images() {
    if before_file().exists() {
        return;
    }
    let stills = cache_dir().join("stills");
    let images: Vec<(String, PathBuf)> =
        shown_images().into_iter().filter(|(_, image)| !image.starts_with(&stills) && image.is_file()).collect();
    if !images.is_empty()
        && let Ok(text) = serde_json::to_string(&images)
    {
        let _ = std::fs::write(before_file(), text);
    }
}

/// The user's own picture: nothing of before is to come back over it.
pub fn forget_images() {
    let _ = std::fs::remove_file(before_file());
}

/// Put back the pictures that were there before the live wallpaper.
pub fn restore_images() {
    let Ok(text) = std::fs::read_to_string(before_file()) else { return };
    forget_images();
    let images: Vec<(String, PathBuf)> = serde_json::from_str(&text).unwrap_or_default();
    for (monitor, image) in images.iter().filter(|(_, image)| image.is_file()) {
        let path = image.to_string_lossy();
        let _ = run("awww", &["img", "-o", monitor, &path])
            || run("swww", &["img", "-o", monitor, &path])
            || (run("hyprctl", &["hyprpaper", "preload", &path])
                && run("hyprctl", &["hyprpaper", "wallpaper", &format!("{monitor},{path}")]));
    }
}

/// The user's hook: a shell command told what is on the desktop now.
fn hook(settings: &Settings, wallpaper: &Path, still: Option<&Path>) {
    if settings.hook.trim().is_empty() {
        return;
    }
    let title = project(wallpaper)["title"].as_str().unwrap_or("").to_owned();
    let _ = Command::new("sh")
        .arg("-c")
        .arg(&settings.hook)
        .env("WE_LINUX_WALLPAPER", wallpaper)
        .env("WE_LINUX_STILL", still.unwrap_or(Path::new("")))
        .env("WE_LINUX_TITLE", title)
        .stdin(Stdio::null())
        // Waited for (this runs on a thread of its own): no leftover process.
        .status();
}

/// A wallpaper is on the desktop: the still under it, then the hook.
pub fn announce(wallpaper: PathBuf) {
    std::thread::spawn(move || {
        let settings = Settings::load();
        let still = if wallpaper.is_dir() { still(&wallpaper) } else { Some(wallpaper.clone()) };
        if settings.still
            && wallpaper.is_dir()
            && let Some(still) = &still
        {
            remember_images();
            show_image(still);
        }
        hook(&settings, &wallpaper, still.as_deref());
    });
}

/// A video file as a wallpaper the engine plays: a folder with a
/// project.json of the "video" type next to a link to the file.
pub fn video_item(video: &Path) -> Result<PathBuf, String> {
    let video = video.canonicalize().map_err(|e| e.to_string())?;
    let dir = cache_dir().join("local").join(format!("{:016x}", fnv(&video.to_string_lossy())));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let extension = video.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let link = dir.join(format!("video.{extension}"));
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&video, &link).map_err(|e| e.to_string())?;
    let title = video.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
    let json = serde_json::json!({ "type": "video", "file": format!("video.{extension}"), "title": title });
    // Written when it changes only: a still is as new as its project.json,
    // and the same video set again has the same still.
    let (project, json) = (dir.join("project.json"), json.to_string());
    if std::fs::read_to_string(&project).ok().as_deref() != Some(json.as_str()) {
        std::fs::write(&project, json).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

// ------------------------------------------------------------------ the daemon

fn lock_file() -> Option<std::fs::File> {
    std::fs::create_dir_all(runtime_dir()).ok()?;
    std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(runtime_dir().join("daemon.lock")).ok()
}

/// Start `we-linux daemon` on its own unless one runs already.
pub fn ensure_daemon() -> Result<(), String> {
    let Some(lock) = lock_file() else { return Err("no runtime directory".into()) };
    if lock.try_lock().is_err() {
        return Ok(());
    }
    drop(lock);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .arg("daemon")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// One engine process: what it shows and where.
struct Spec {
    key: String,
    wallpaper: PathBuf,
    output: Option<String>,
    audio: bool,
}

impl Spec {
    /// What a running engine was started with; a change starts it again.
    fn signature(&self, settings: &Settings) -> String {
        format!("{}|{:?}|{}", self.wallpaper.display(), self.output, settings.engine_signature())
    }
}

/// The engines there should be. None: a monitor has a wallpaper of its own
/// and Hyprland does not say which monitors there are right now (it is busy,
/// or restarting): what runs is left as it is until it does.
fn specs(desktop: &Desktop, settings: &Settings) -> Option<Vec<Spec>> {
    let everywhere = || -> Vec<Spec> {
        desktop
            .wallpaper
            .iter()
            .filter(|p| p.is_dir())
            .map(|p| Spec { key: "all".into(), wallpaper: p.clone(), output: None, audio: settings.audio })
            .collect()
    };
    let own = |name: &str| desktop.outputs.get(name).filter(|p| p.is_dir());
    if settings.mirror || !desktop.outputs.values().any(|p| p.is_dir()) {
        return Some(everywhere());
    }
    // No monitor that has a wallpaper of its own is there (it was unplugged,
    // or the monitors are not known: not Hyprland): the wallpaper of all.
    let monitors = desktop::monitors();
    if monitors.is_empty() && hyprland() {
        return None;
    }
    if !monitors.iter().any(|m| own(&m.name).is_some()) {
        return Some(everywhere());
    }
    let mut sounding = settings.audio;
    let specs = monitors
        .into_iter()
        .filter_map(|m| {
            let wallpaper = own(&m.name).or(desktop.wallpaper.as_ref())?.clone();
            // Sound plays from one engine only: two would double it.
            let audio = std::mem::take(&mut sounding);
            Some(Spec { key: m.name.clone(), wallpaper, output: Some(m.name), audio })
        })
        .collect();
    Some(specs)
}

fn spawn(spec: &Spec, settings: &Settings) -> Result<Child, String> {
    let penguind = bin("penguind").ok_or("penguind (penguin-engine) is not installed")?;
    let assets = crate::library::assets().ok_or("Wallpaper Engine is not installed in Steam: no assets")?;
    let dir = runtime_dir().join(&spec.key);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let socket = dir.join("control.sock");
    let _ = std::fs::remove_file(&socket);
    // A new file, not the old one emptied: what reads it sees a new engine.
    let _ = std::fs::remove_file(dir.join("engine.log"));
    let log = std::fs::File::create(dir.join("engine.log")).map_err(|e| e.to_string())?;
    let mut command = Command::new(penguind);
    command.arg("--socket").arg(&socket).arg("--assets").arg(assets);
    command.args(["--fps", &fps(settings).to_string()]);
    if spec.audio {
        command.args(["--audio", "--volume", &settings.volume.clamp(0, 100).to_string()]);
    }
    let id = spec.wallpaper.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u64>().ok());
    for (name, value) in id.map(crate::props::overrides).unwrap_or_default() {
        command.arg("--property").arg(format!("{name}={value}"));
    }
    if let Some(output) = &spec.output {
        command.args(["--output", output]);
    }
    command.arg(&spec.wallpaper);
    engine_env(&mut command, settings);
    // The engine keeps what it showed last there: ours is in desktop.json.
    command.env("XDG_STATE_HOME", &dir);
    command.stdin(Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    // The engine goes when the daemon does, however that ends.
    unsafe {
        command.pre_exec(|| {
            prctl(PR_SET_PDEATHSIG, SIGTERM);
            Ok(())
        });
    }
    command.spawn().map_err(|e| e.to_string())
}

fn stop(child: &mut Child) {
    let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
    for _ in 0..30 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Why the engine of `key` will never show its wallpaper, read off its log:
/// loading runs on a thread of its own, and when that dies the process
/// stays, showing nothing.
fn engine_failure(key: &str) -> Option<String> {
    let log = std::fs::read_to_string(runtime_dir().join(key).join("engine.log")).ok()?;
    if log.contains(": playing \"") {
        return None;
    }
    if log.contains("Out of Memory") {
        return Some("not enough video memory for this wallpaper".to_owned());
    }
    log.contains("panicked at").then(|| "the engine crashed loading this wallpaper (see `we-linux doctor`)".to_owned())
}

/// A running engine.
struct Engine {
    /// [`Spec::signature`] it was started with.
    signature: String,
    wallpaper: PathBuf,
    child: Child,
    since: Instant,
}

/// An engine that goes sooner than this after its start did not get to work.
const SHORT_LIFE: Duration = Duration::from_secs(30);
/// So many short lives in a row and the engine is not started again.
const SHORT_LIVES: u32 = 3;

fn errors_file() -> PathBuf {
    runtime_dir().join("error")
}

/// What the daemon could not show and why: a wallpaper and a reason a line.
pub fn errors() -> Vec<(PathBuf, String)> {
    std::fs::read_to_string(errors_file())
        .map(|text| {
            text.lines()
                .filter_map(|line| line.split_once('\t'))
                .map(|(wallpaper, why)| (PathBuf::from(wallpaper), why.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Hyprland's event socket: what may change whether a fullscreen window
/// covers the desktop, or which monitors there are.
fn watch_hyprland(wake: Sender<()>) {
    let (Some(runtime), Some(signature)) = (std::env::var_os("XDG_RUNTIME_DIR"), std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE"))
    else {
        return;
    };
    let socket = PathBuf::from(runtime).join("hypr").join(signature).join(".socket2.sock");
    std::thread::spawn(move || {
        loop {
            if let Ok(stream) = UnixStream::connect(&socket) {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let relevant = ["fullscreen", "workspace", "focusedmon", "activespecial", "openwindow", "closewindow", "movewindow", "monitoradded", "monitorremoved"];
                    if relevant.iter().any(|e| line.starts_with(e)) && wake.send(()).is_err() {
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// desktop.json and settings.json: the window and the command line write them.
fn watch_files(wake: Sender<()>) {
    std::thread::spawn(move || {
        let stamp = || {
            [desktop::desktop_file(), desktop::settings_file()].map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        };
        let mut seen = stamp();
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let now = stamp();
            if now != seen {
                seen = now;
                if wake.send(()).is_err() {
                    return;
                }
            }
        }
    });
}

pub fn run_daemon() {
    let Some(lock) = lock_file() else { return };
    if lock.try_lock().is_err() {
        eprintln!("we-linux: the daemon is running already");
        return;
    }
    let (wake, woken) = channel();
    // Until something changes; an engine that died is looked at again in a
    // few seconds.
    let rest = || {
        let _ = woken.recv_timeout(Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(120));
        while woken.try_recv().is_ok() {}
    };
    watch_hyprland(wake.clone());
    watch_files(wake);
    std::thread::spawn(desktop::run_playlist);
    hypr_layer_rule();

    let mut running: HashMap<String, Engine> = HashMap::new();
    let mut announced: Option<PathBuf> = None;
    let mut failed = String::new();
    // What an engine could not show (by Spec::signature), with its wallpaper
    // and why: not tried again until the user sets something.
    let mut broken: HashMap<String, (PathBuf, String)> = HashMap::new();
    // Short lives in a row, by Spec::signature.
    let mut short_lives: HashMap<String, u32> = HashMap::new();
    let mut desktop_stamp = None;
    // What the errors file holds (None: not known, it is written anew). The
    // file is written when this changes only: its time says how new it is.
    let mut reported: Option<String> = None;
    // What an earlier daemon left: logs and sockets of engines long gone.
    if let Ok(dirs) = std::fs::read_dir(runtime_dir()) {
        for dir in dirs.flatten() {
            let _ = std::fs::remove_file(dir.path().join("engine.log"));
            let _ = std::fs::remove_file(dir.path().join("control.sock"));
        }
    }
    // The sound the running engines have: (on, volume).
    let mut sound: Option<(bool, i32)> = None;
    // Monitors whose wallpaper is frozen under a fullscreen window.
    let mut frozen: HashMap<String, bool> = HashMap::new();
    unsafe {
        signal(SIGTERM, on_stop);
        signal(SIGINT, on_stop);
    }
    loop {
        // Told to go: the engines stop and the desktop gets its picture back.
        if STOP.load(std::sync::atomic::Ordering::Relaxed) {
            for (_, mut engine) in running.drain() {
                stop(&mut engine.child);
            }
            restore_images();
            std::process::exit(0);
        }
        let desktop = Desktop::load();
        let settings = Settings::load();
        // Nothing live any more: what was there before comes back.
        if desktop.wallpaper.is_none() && desktop.outputs.is_empty() {
            restore_images();
        }
        if desktop.wallpaper != announced {
            announced = desktop.wallpaper.clone();
            if let Some(wallpaper) = &announced {
                announce(wallpaper.clone());
            }
        }
        // The user set something: what failed before gets another try.
        let stamp = std::fs::metadata(desktop::desktop_file()).and_then(|m| m.modified()).ok();
        if stamp != desktop_stamp {
            desktop_stamp = stamp;
            broken.clear();
            short_lives.clear();
            reported = None;
        }
        let covered = if hyprland() { covered_monitors() } else { Vec::new() };
        // Every monitor is fullscreen and the user wants the memory back.
        let hidden = settings.stop_fullscreen && !covered.is_empty() && covered.iter().all(|(_, c)| *c);
        let want = if hidden {
            Vec::new()
        } else if let Some(want) = specs(&desktop, &settings) {
            want
        } else {
            rest();
            continue;
        };
        // What is not asked for any more (other settings, a monitor gone) is
        // not an error to keep telling of, and gets a new try when asked again.
        if !hidden {
            broken.retain(|signature, _| want.iter().any(|s| &s.signature(&settings) == signature));
        }
        let mut dead: Vec<(String, String)> = Vec::new();
        for (key, engine) in &mut running {
            // An engine whose loader died shows nothing and never will.
            if let Some(why) = engine_failure(key) {
                dead.push((key.clone(), why));
                continue;
            }
            // An engine that went on its own is started again (the compositor
            // may have come back), but not one that goes every time.
            let wanted = want.iter().any(|s| &s.key == key && s.signature(&settings) == engine.signature);
            if wanted && let Ok(Some(status)) = engine.child.try_wait() {
                let lives = short_lives.entry(engine.signature.clone()).or_insert(0);
                *lives = if engine.since.elapsed() < SHORT_LIFE { *lives + 1 } else { 0 };
                if *lives >= SHORT_LIVES {
                    dead.push((key.clone(), format!("the engine keeps exiting ({status}): see `we-linux doctor`")));
                }
            }
        }
        for (key, why) in dead {
            eprintln!("we-linux: {why}");
            // Its log stays: `we-linux doctor` shows it.
            if let Some(mut engine) = running.remove(&key) {
                stop(&mut engine.child);
                broken.insert(engine.signature, (engine.wallpaper, why));
            }
            let _ = std::fs::remove_file(runtime_dir().join(&key).join("control.sock"));
            // No scene to stand in for: the user's own picture is better
            // than a still of what did not load.
            restore_images();
        }
        running.retain(|key, engine| {
            let keep = want.iter().any(|s| &s.key == key && s.signature(&settings) == engine.signature)
                && matches!(engine.child.try_wait(), Ok(None));
            if !keep {
                stop(&mut engine.child);
                let _ = std::fs::remove_file(runtime_dir().join(key).join("control.sock"));
                let _ = std::fs::remove_file(runtime_dir().join(key).join("engine.log"));
            }
            keep
        });
        // What did not start this round, with its wallpaper.
        let mut unstarted: Vec<(PathBuf, String)> = Vec::new();
        for spec in &want {
            if running.contains_key(&spec.key) || broken.contains_key(&spec.signature(&settings)) {
                continue;
            }
            match spawn(spec, &settings) {
                Ok(child) => {
                    failed.clear();
                    // A new engine plays on every monitor.
                    frozen.clear();
                    // It was started with the sound as the settings have it.
                    sound = Some((settings.audio, settings.volume));
                    let engine = Engine { signature: spec.signature(&settings), wallpaper: spec.wallpaper.clone(), child, since: Instant::now() };
                    running.insert(spec.key.clone(), engine);
                }
                Err(e) => {
                    if e != failed {
                        eprintln!("we-linux: {e}");
                        failed.clone_from(&e);
                    }
                    unstarted.push((spec.wallpaper.clone(), e));
                }
            }
        }
        // Every failure with its wallpaper: who asks about another wallpaper
        // (another monitor's) is not told this one's.
        let errors: Vec<String> =
            broken.values().chain(&unstarted).map(|(wallpaper, why)| format!("{}\t{why}", wallpaper.display())).collect();
        let report = errors.join("\n");
        if reported.as_ref() != Some(&report) {
            if report.is_empty() {
                let _ = std::fs::remove_file(errors_file());
            } else {
                let _ = std::fs::write(errors_file(), &report);
            }
            reported = Some(report);
        }
        // Sound and volume changed: the engines are told, nothing restarts.
        if sound.is_some_and(|s| s != (settings.audio, settings.volume))
            && let Some(penguinctl) = bin("penguinctl")
        {
            let tell = |key: &str, args: &[&str]| {
                Command::new(&penguinctl)
                    .arg("--socket")
                    .arg(runtime_dir().join(key).join("control.sock"))
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
            };
            let volume = settings.volume.clamp(0, 100).to_string();
            // Sound plays from one engine only: the first.
            let told = want.iter().enumerate().all(|(i, spec)| {
                let on = settings.audio && i == 0;
                tell(&spec.key, &["volume", &volume]) && tell(&spec.key, &["audio", if on { "on" } else { "off" }])
            });
            if told {
                sound = Some((settings.audio, settings.volume));
            }
        }
        // A monitor under a fullscreen window keeps its frame: the scene
        // stops there (no work for nothing) and goes on from that frame after.
        if let Some(penguinctl) = bin("penguinctl") {
            for (monitor, covered) in &covered {
                let freeze = settings.pause_fullscreen && *covered;
                if frozen.get(monitor).copied().unwrap_or(false) == freeze {
                    continue;
                }
                let key = if running.contains_key(monitor.as_str()) { monitor.as_str() } else { "all" };
                let socket = runtime_dir().join(key).join("control.sock");
                let told = socket.exists()
                    && Command::new(&penguinctl)
                        .arg("--socket")
                        .arg(&socket)
                        .args([if freeze { "pause" } else { "resume" }, "--output", monitor])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .is_ok_and(|s| s.success());
                // An engine still loading is told again on the next round.
                if told {
                    frozen.insert(monitor.clone(), freeze);
                }
            }
        }
        rest();
    }
}

/// How far a wallpaper set a moment ago has come.
pub enum Stage {
    /// The engine is starting or loading the scene.
    Loading,
    /// ffmpeg scales a scene's video down to the monitor (once a video).
    Preparing,
    /// The engine shows it.
    Shown,
    Failed(String),
}

/// The stage of `wallpaper`: read off the engines' logs, where an engine
/// says what it loaded (its folder) and that it plays it.
pub fn stage(wallpaper: &Path) -> Stage {
    let loaded = format!("({})", wallpaper.display());
    let shown = std::fs::read_dir(runtime_dir())
        .map(|dirs| dirs.flatten().map(|d| d.path().join("engine.log")).collect::<Vec<_>>())
        .unwrap_or_default()
        .iter()
        .filter_map(|log| std::fs::read_to_string(log).ok())
        .any(|text| text.contains(&loaded) && text.contains(": playing \""));
    if shown {
        return Stage::Shown;
    }
    // An error older than desktop.json is of what was set before: the daemon
    // has not looked at the new one yet.
    let modified = |path: PathBuf| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if modified(errors_file()) >= modified(desktop::desktop_file())
        && let Some((_, why)) = errors().into_iter().find(|(failed, _)| failed == wallpaper)
    {
        return Stage::Failed(why);
    }
    // The engine's ffmpeg writes into its video cache.
    let preparing = std::fs::read_dir("/proc").is_ok_and(|procs| {
        procs.flatten().any(|p| {
            std::fs::read(p.path().join("cmdline")).is_ok_and(|c| {
                let c = String::from_utf8_lossy(&c);
                let program = c.split('\0').next().unwrap_or("");
                Path::new(program).file_name().is_some_and(|n| n == "ffmpeg") && c.contains("penguin-engine/video")
            })
        })
    });
    if preparing { Stage::Preparing } else { Stage::Loading }
}

/// `we-linux doctor`: what a bug report needs, in one paste.
pub fn doctor() -> String {
    let found = |path: Option<PathBuf>| path.map_or_else(|| "NOT FOUND".to_owned(), |p| p.display().to_string());
    let output = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_owned())
            .filter(|line| !line.is_empty())
    };
    let env = |name: &str| std::env::var(name).unwrap_or_else(|_| "(unset)".into());
    let desktop = Desktop::load();
    let libraries = crate::library::libraries();
    let mut out = Vec::new();
    out.push(format!("we-linux {}", env!("CARGO_PKG_VERSION")));
    out.push(format!("session: {} / {}, WAYLAND_DISPLAY={}", env("XDG_CURRENT_DESKTOP"), env("XDG_SESSION_TYPE"), env("WAYLAND_DISPLAY")));
    if hyprland() {
        out.push(format!("hyprland: {}", output("hyprctl", &["version"]).unwrap_or_else(|| "hyprctl does not answer".into())));
        let monitors: Vec<String> = desktop::monitors().into_iter().map(|m| format!("{} {}", m.name, m.detail)).collect();
        out.push(format!("monitors: {}", monitors.join(", ")));
    }
    if let Some(memory) = output("nvidia-smi", &["--query-gpu=name,memory.used,memory.total", "--format=csv,noheader"]) {
        out.push(format!("gpu: {memory}"));
    }
    out.push(String::new());
    for name in ["penguind", "penguinctl", "scene-viewer"] {
        out.push(format!("{name}: {}", found(bin(name))));
    }
    out.push(format!("ffmpeg: {}", output("ffmpeg", &["-version"]).unwrap_or_else(|| "NOT FOUND (videos inside scenes are not scaled down, no stills of videos)".into())));
    out.push(format!("steam libraries: {}", if libraries.is_empty() { "NONE FOUND".to_owned() } else { libraries.iter().map(|l| l.display().to_string()).collect::<Vec<_>>().join(", ") }));
    out.push(format!("wallpaper engine assets: {}", found(crate::library::assets())));
    out.push(format!("wallpapers on disk: {}", crate::library::installed().len()));
    out.push(String::new());
    let lock = lock_file();
    let daemon = lock.is_some_and(|l| l.try_lock().is_err());
    out.push(format!("daemon: {}", if daemon { "running" } else { "NOT RUNNING (start it: we-linux daemon)" }));
    out.push(format!("wallpaper: {}", desktop.wallpaper.as_ref().map_or_else(|| "none".to_owned(), |p| p.display().to_string())));
    if let Some(wallpaper) = &desktop.wallpaper {
        let json = project(wallpaper);
        out.push(format!("  type: {}, title: {}", json["type"].as_str().unwrap_or("scene"), json["title"].as_str().unwrap_or("")));
    }
    for (name, path) in &desktop.outputs {
        out.push(format!("  {name}: {}", path.display()));
    }
    out.push(format!("engines running: {}", sockets().len()));
    for (wallpaper, why) in errors() {
        out.push(format!("last error: {why} ({})", wallpaper.display()));
    }
    out.push(format!("settings: {}", std::fs::read_to_string(desktop::settings_file()).map_or_else(|_| "(defaults)".to_owned(), |s| s.split_whitespace().collect::<Vec<_>>().join(" "))));
    if let Ok(dirs) = std::fs::read_dir(runtime_dir()) {
        for dir in dirs.flatten() {
            let Ok(log) = std::fs::read_to_string(dir.path().join("engine.log")) else { continue };
            out.push(String::new());
            out.push(format!("--- engine log ({}), last lines ---", dir.file_name().to_string_lossy()));
            let lines: Vec<&str> = log.lines().collect();
            out.extend(lines[lines.len().saturating_sub(40)..].iter().map(|l| strip_colours(l)));
        }
    }
    out.join("\n")
}

/// A log line without its terminal colour codes.
fn strip_colours(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
