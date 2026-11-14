//! `we-linux daemon`: what keeps the wallpaper on the desktop.
//!
//! It runs one `wepaper daemon` (the renderer) and tells it over its control
//! socket what desktop.json and settings.json say: which wallpaper on which
//! monitor, with which properties, how loud, how fast. Nothing restarts when a
//! setting changes. On Hyprland it freezes the wallpaper of a monitor that
//! shows a fullscreen window (or, when asked, stops the renderer while every
//! monitor does, so a game gets the video memory). It also gives the desktop
//! a still of the wallpaper (awww, swww or hyprpaper, when one runs), runs the
//! user's hook, keeps the renderer's layer over the static wallpaper's, and
//! steps through the playlist.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::desktop::{self, Desktop, Settings};

unsafe extern "C" {
    fn prctl(option: i32, ...) -> i32;
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
    fn kill(pid: i32, signal: i32) -> i32;
    fn getuid() -> u32;
    fn getsockopt(fd: i32, level: i32, name: i32, value: *mut std::ffi::c_void, length: *mut u32) -> i32;
}
const PR_SET_PDEATHSIG: i32 = 1;
const SIGINT: i32 = 2;
const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;
const SOL_SOCKET: i32 = 1;
/// Linux's number on x86 and ARM, which is where the renderer runs.
const SO_PEERCRED: i32 = 17;

/// The daemon was asked to go (SIGTERM, SIGINT).
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_stop(_: i32) {
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The renderer's binary.
const ENGINE: &str = "wepaper";
/// The layer-shell namespace of its surfaces: what compositor rules match.
const NAMESPACE: &str = "wepaper";

/// Where the daemon's lock, the engines' sockets and their logs are
/// (WE_LINUX_RUNTIME_DIR names another place: a second daemon for a test).
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WE_LINUX_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    // Without a runtime directory: in the user's own cache, not in /tmp,
    // where anyone could have made the folder first.
    std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(|| cache_dir().join("run"), |dir| PathBuf::from(dir).join("we-linux"))
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"))
        .join("we-linux")
}

/// A program of the renderer (`wepaper`): in $WE_LINUX_ENGINE_DIR, next to
/// this binary, or on the PATH.
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

/// The renderer's control socket.
pub fn socket() -> PathBuf {
    runtime_dir().join("wepaper.sock")
}

/// A plugin of the renderer, as its folder describes it.
pub struct Plugin {
    pub name: String,
    pub kind: String,
    pub path: PathBuf,
    pub enabled: bool,
}

/// The renderer's plugins: every folder with a `plugin.json` in the places the
/// renderer looks, a later place winning over an earlier one.
pub fn plugins() -> Vec<Plugin> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let var = |name: &str, fallback: PathBuf| {
        std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or(fallback)
    };
    let mut places: Vec<PathBuf> = Vec::new();
    if let Some(prefix) = bin(ENGINE).as_deref().and_then(Path::parent).and_then(Path::parent) {
        places.push(prefix.join("share/wepaper/plugins"));
    }
    places.extend(["/usr/share", "/usr/local/share"].map(|dir| PathBuf::from(dir).join("wepaper/plugins")));
    places.push(var("XDG_DATA_HOME", home.join(".local/share")).join("wepaper/plugins"));
    places.push(var("XDG_CONFIG_HOME", home.join(".config")).join("wepaper/plugins"));
    let mut found: Vec<(String, Plugin)> = Vec::new();
    for place in places {
        for entry in std::fs::read_dir(place).into_iter().flatten().flatten() {
            let path = entry.path();
            let Some(manifest) = std::fs::read(path.join("plugin.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            else {
                continue;
            };
            let folder = entry.file_name().to_string_lossy().into_owned();
            let id = manifest["id"].as_str().map_or(folder.clone(), str::to_owned);
            let plugin = Plugin {
                name: manifest["name"].as_str().map_or(folder, str::to_owned),
                kind: manifest["kind"].as_str().unwrap_or("").to_owned(),
                path,
                enabled: manifest["enabled"].as_bool().unwrap_or(true),
            };
            found.retain(|(known, _)| *known != id);
            found.push((id, plugin));
        }
    }
    found.into_iter().map(|(_, plugin)| plugin).collect()
}

/// How a request to the renderer went wrong.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// Nothing listens on its socket.
    Absent,
    /// It took the request and gave no answer in time: it hangs.
    Silent,
    /// It said no (its own words), or went without a word.
    Said(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            Failure::Absent => "the renderer is not running",
            Failure::Silent => "the renderer does not answer",
            Failure::Said(words) => words,
        })
    }
}

/// How long the renderer may take over a request. It answers between two
/// frames; only loading a wallpaper (`show`) and drawing one to a file
/// (`snapshot`) are seconds of work.
pub fn patience(request: &Value) -> Duration {
    match request["cmd"].as_str() {
        Some("show" | "snapshot") => Duration::from_secs(30),
        _ => Duration::from_secs(5),
    }
}

thread_local! {
    /// Whether a request of this thread found the renderer hanging. The
    /// daemon's loop looks at it: a renderer that does not answer it is killed.
    static SILENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// One request to the renderer listening on `socket`, waiting for its answer
/// no longer than `patience`.
pub fn ask(socket: &Path, request: &Value, patience: Duration) -> Result<Value, Failure> {
    let stream = UnixStream::connect(socket).map_err(|_| Failure::Absent)?;
    let _ = stream.set_read_timeout(Some(patience));
    let _ = stream.set_write_timeout(Some(patience));
    let timed_out = |e: &std::io::Error| {
        matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
    };
    let silent = || {
        SILENT.set(true);
        Failure::Silent
    };
    let mut line = request.to_string();
    line.push('\n');
    if let Err(e) = (&stream).write_all(line.as_bytes()) {
        return Err(if timed_out(&e) { silent() } else { Failure::Said(e.to_string()) });
    }
    let mut answer = String::new();
    if let Err(e) = BufReader::new(&stream).read_line(&mut answer) {
        return Err(if timed_out(&e) { silent() } else { Failure::Said(e.to_string()) });
    }
    let answer: Value =
        serde_json::from_str(&answer).map_err(|_| Failure::Said("the renderer gave no answer".to_owned()))?;
    if answer["ok"].as_bool() == Some(true) {
        Ok(answer["value"].clone())
    } else {
        Err(Failure::Said(answer["error"].as_str().unwrap_or("the renderer refused").to_owned()))
    }
}

/// One request to the renderer and its answer: a line of JSON each way.
/// The error is the renderer's own words, or that it is not there or hangs.
pub fn call(request: Value) -> Result<Value, String> {
    ask(&socket(), &request, patience(&request)).map_err(|failure| failure.to_string())
}

/// The process listening on a socket, from the kernel (`SO_PEERCRED`): the
/// renderer, whoever started it. None when nothing listens, or when what
/// listens is not the user's own.
pub fn listener(socket: &Path) -> Option<i32> {
    use std::os::fd::AsRawFd;
    #[repr(C)]
    struct Credentials {
        pid: i32,
        uid: u32,
        gid: u32,
    }
    let stream = UnixStream::connect(socket).ok()?;
    let mut credentials = Credentials { pid: 0, uid: 0, gid: 0 };
    let mut length = std::mem::size_of::<Credentials>() as u32;
    let got = unsafe {
        getsockopt(
            stream.as_raw_fd(),
            SOL_SOCKET,
            SO_PEERCRED,
            (&raw mut credentials).cast(),
            &raw mut length,
        )
    };
    (got == 0 && credentials.pid > 0 && credentials.uid == unsafe { getuid() }).then_some(credentials.pid)
}

/// Whether anything still listens on a socket.
fn listening(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}

/// Waits up to `limit` for nothing to listen on `socket`; whether that came.
fn gone_within(socket: &Path, limit: Duration) -> bool {
    let since = Instant::now();
    loop {
        if !listening(socket) {
            return true;
        }
        if since.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// SIGTERM, then SIGKILL, `step` apart, to what listens on `socket`.
/// Whether nothing listens after.
fn signal_out(socket: &Path, step: Duration) -> bool {
    for signal in [SIGTERM, SIGKILL] {
        let Some(pid) = listener(socket) else { return !listening(socket) };
        unsafe {
            kill(pid, signal);
        }
        if gone_within(socket, step) {
            return true;
        }
    }
    false
}

/// Stop the renderer listening on `socket`, whoever started it and whatever
/// it is doing: asked to quit, then SIGTERM, then SIGKILL, `step` apart. A
/// renderer that hangs answers nothing and an old one does not even go on
/// SIGTERM, and two renderers must never draw and play at once. Whether it
/// is gone.
pub fn put_down(socket: &Path, step: Duration) -> bool {
    if !listening(socket) {
        return true;
    }
    let _ = ask(socket, &json!({ "cmd": "quit" }), step);
    gone_within(socket, step) || signal_out(socket, step)
}

/// The wallpaper was taken off: the renderer is to go within about three
/// seconds, whatever the daemon and the renderer are doing. When all is well
/// the daemon stops it in a moment; when it does not, it is stopped here.
pub fn see_off() {
    let socket = socket();
    if gone_within(&socket, Duration::from_millis(1200)) {
        return;
    }
    // Set again meanwhile: the daemon is to show that, not to lose it.
    let desktop = Desktop::load();
    if desktop.wallpaper.is_none() && desktop.outputs.is_empty() {
        signal_out(&socket, Duration::from_millis(700));
    }
}

/// Whether the renderer answers.
pub fn running() -> bool {
    call(json!({ "cmd": "version" })).is_ok()
}

/// What the renderer shows: monitor and wallpaper folder.
pub fn shown() -> Vec<(String, PathBuf)> {
    let Ok(outputs) = call(json!({ "cmd": "get", "path": "outputs" })) else { return Vec::new() };
    outputs
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, output)| {
            Some((name.clone(), PathBuf::from(output["wallpaper"]["path"].as_str()?)))
        })
        .collect()
}

/// The monitors the renderer draws on, with their size: what tells the
/// monitors apart where the compositor does not (anything but Hyprland).
pub fn outputs() -> Vec<(String, u32, u32)> {
    let Ok(outputs) = call(json!({ "cmd": "get", "path": "outputs" })) else { return Vec::new() };
    outputs
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, output)| {
            (
                name.clone(),
                output["size"][0].as_u64().unwrap_or(0) as u32,
                output["size"][1].as_u64().unwrap_or(0) as u32,
            )
        })
        .collect()
}

/// Tell the renderer the new value of a property of `wallpaper`, on every
/// monitor that shows it: another monitor has a wallpaper of its own, and a
/// property of the same name there (every wallpaper has a `schemecolor`) is
/// not this one.
pub fn tell_property(wallpaper: &Path, name: &str, value: &Value) {
    let (wallpaper, name, value) = (wallpaper.to_owned(), name.to_owned(), value.clone());
    // Off the interface's thread: a socket is quick, but never instant.
    std::thread::spawn(move || {
        for (output, _) in shown().into_iter().filter(|(_, path)| *path == wallpaper) {
            // The alignment of a video is how the renderer fills the monitor.
            if name == "alignment" {
                let fill = crate::props::fill_of(&value).map_or(Value::Null, Value::from);
                let _ =
                    call(json!({ "cmd": "set", "path": format!("outputs/{output}/fill"), "value": fill }));
                continue;
            }
            let path = format!("outputs/{output}/wallpaper/properties/{name}");
            let _ = call(json!({ "cmd": "set", "path": path, "value": value }));
        }
    });
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

/// What is over a monitor's desktop, on Hyprland.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Over {
    /// A window has the focus there.
    focused: bool,
    /// 0 nothing, 1 a window maximised over it, 2 a fullscreen window.
    covered: u8,
    /// The monitor sleeps (its power is off).
    asleep: bool,
}

/// Hyprland's monitors, each with what is over its desktop.
fn covered_monitors() -> Vec<(String, Over)> {
    let (Some(monitors), Some(clients)) = (hypr_json("monitors"), hypr_json("clients")) else {
        return Vec::new();
    };
    let active = hypr_json("activewindow").and_then(|w| w["monitor"].as_i64());
    let covering: Vec<(i64, u8)> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let how = c["fullscreen"].as_i64().unwrap_or(0).clamp(0, 2) as u8;
            (how > 0).then_some((c["workspace"]["id"].as_i64()?, how))
        })
        .collect();
    monitors
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let covered = [m["activeWorkspace"]["id"].as_i64(), m["specialWorkspace"]["id"].as_i64()]
                .into_iter()
                .flatten()
                .filter(|id| *id != 0)
                .flat_map(|id| {
                    covering.iter().filter(move |(workspace, _)| *workspace == id).map(|(_, how)| *how)
                })
                .max()
                .unwrap_or(0);
            let over = Over {
                focused: active.is_some() && active == m["id"].as_i64(),
                covered,
                asleep: m["dpmsStatus"].as_bool() == Some(false),
            };
            (m["name"].as_str().unwrap_or("").to_owned(), over)
        })
        .collect()
}

/// What the original's "Воспроизведение" asks of a wallpaper, weakest first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Playback {
    Run,
    Mute,
    Pause,
    PauseAll,
    Stop,
}

impl Playback {
    fn from_name(name: &str) -> Self {
        match name {
            "mute" => Playback::Mute,
            "pause" => Playback::Pause,
            "pauseall" => Playback::PauseAll,
            "stop" => Playback::Stop,
            _ => Playback::Run,
        }
    }
}

/// What a monitor's wallpaper is to do with this over it, and while another
/// application plays sound (`sound`): the strongest of what applies.
fn under(settings: &Settings, over: Over, sound: bool) -> Playback {
    let mut wanted = Playback::Run;
    let mut take = |applies: bool, choice: &str| {
        if applies {
            wanted = wanted.max(Playback::from_name(choice));
        }
    };
    take(over.covered == 2, &settings.playback_fullscreen);
    take(over.covered == 1, &settings.playback_maximized);
    take(over.focused && over.covered == 0, &settings.playback_focus);
    take(over.asleep, &settings.playback_sleep);
    take(sound, &settings.playback_audio);
    wanted
}

/// Whether an application other than the renderer (`renderer`, its pid)
/// plays sound now: a stream of the sound server that is not paused.
fn others_play_sound(renderer: i32) -> bool {
    let Some(text) = output("pactl", &["list", "sink-inputs"]) else { return false };
    text.split("Sink Input #").skip(1).any(|stream| {
        let playing = stream.lines().any(|l| l.trim() == "Corked: no");
        let pid = stream
            .lines()
            .find_map(|l| l.trim().strip_prefix("application.process.id = "))
            .map(|v| v.trim_matches('"').to_owned());
        let ours = pid.is_some_and(|p| p == renderer.to_string())
            || stream.contains("application.process.binary = \"wepaper\"");
        playing && !ours
    })
}

/// The size of the largest monitor (Hyprland's, else the renderer's), else
/// 4K: stills are rendered at it.
fn largest_monitor() -> (u32, u32) {
    hypr_json("monitors")
        .and_then(|m| {
            m.as_array()?
                .iter()
                .filter_map(|m| Some((m["width"].as_u64()? as u32, m["height"].as_u64()? as u32)))
                .max_by_key(|(w, h)| w * h)
        })
        .or_else(|| {
            outputs()
                .into_iter()
                .map(|(_, w, h)| (w, h))
                .filter(|(w, h)| w * h > 0)
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
    let lua = format!(
        r#"hl.layer_rule({{ match = {{ namespace = "^({NAMESPACE})$" }}, order = -1, animation = "fade" }})"#
    );
    let _ = run("hyprctl", &["eval", &lua])
        || run(
            "hyprctl",
            &["keyword", "layerrule", &format!("match:namespace ^({NAMESPACE})$, order -1, animation fade")],
        )
        || (run("hyprctl", &["keyword", "layerrule", &format!("order -1, {NAMESPACE}")])
            && run("hyprctl", &["keyword", "layerrule", &format!("animation fade, {NAMESPACE}")]));
}

// ------------------------------------------------------------------ stills

fn project(dir: &Path) -> serde_json::Value {
    std::fs::read(dir.join("project.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// A still of a wallpaper, for the static wallpaper under the renderer and for
/// the hook: a frame the renderer draws of a scene, the first frame of a
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
        && let (Some(engine), Some(assets)) = (bin(ENGINE), crate::library::assets())
    {
        let (w, h) = largest_monitor();
        let out = dir.join(format!("{name}-{w}x{h}.png"));
        if fresh(&out) {
            return Some(out);
        }
        let part = dir.join(format!("{name}.{}.part.png", std::process::id()));
        let mut command = Command::new("timeout");
        command.arg("30").arg(engine).arg("render").arg(wallpaper).arg("--assets").arg(assets);
        command.args(["--size", &format!("{w}x{h}"), "--time", "2", "--frames", "4", "--output"]).arg(&part);
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
            &[
                "-v",
                "error",
                "-y",
                "-i",
                &video.to_string_lossy(),
                "-frames:v",
                "1",
                "-q:v",
                "2",
                &part.to_string_lossy(),
            ],
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
        || (run("hyprctl", &["hyprpaper", "preload", &path])
            && run("hyprctl", &["hyprpaper", "wallpaper", &format!(",{path}")]))
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
    let images: Vec<(String, PathBuf)> = shown_images()
        .into_iter()
        .filter(|(_, image)| !image.starts_with(&stills) && image.is_file())
        .collect();
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

/// Where the output of the user's hook goes: the hook of the last change.
pub fn hook_log() -> PathBuf {
    runtime_dir().join("hook.log")
}

/// The user's hook: a shell command told what is on the desktop now.
fn hook(settings: &Settings, wallpaper: &Path, still: Option<&Path>) {
    if settings.hook.trim().is_empty() {
        return;
    }
    let title = project(wallpaper)["title"].as_str().unwrap_or("").to_owned();
    let _ = std::fs::create_dir_all(runtime_dir());
    let _ = run_hook(&settings.hook, wallpaper, still, &title, &hook_log());
}

/// Run a hook with its output in `log`, started anew: what it says is not for
/// the terminal the daemon was started from, where it would land at every
/// change of the wallpaper.
fn run_hook(
    command: &str,
    wallpaper: &Path,
    still: Option<&Path>,
    title: &str,
    log: &Path,
) -> std::io::Result<std::process::ExitStatus> {
    let out = std::fs::File::create(log)?;
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("WE_LINUX_WALLPAPER", wallpaper)
        .env("WE_LINUX_STILL", still.unwrap_or(Path::new("")))
        .env("WE_LINUX_TITLE", title)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        // Waited for (this runs on a thread of its own): no leftover process.
        .status()
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
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(runtime_dir().join("daemon.lock"))
        .ok()
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

/// What a monitor is to show: the wallpaper of its own, else the wallpaper of all.
fn wanted(desktop: &Desktop, monitor: &str) -> Option<PathBuf> {
    desktop
        .outputs
        .get(monitor)
        .filter(|p| p.is_dir())
        .or(desktop.wallpaper.as_ref().filter(|p| p.is_dir()))
        .cloned()
}

/// The user's values of a wallpaper's properties, as the renderer takes them.
fn properties(wallpaper: &Path) -> Value {
    let id = wallpaper.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u64>().ok());
    Value::Object(id.map(crate::props::overrides).unwrap_or_default().into_iter().collect())
}

/// The settings the renderer is told: a change of them is sent, nothing
/// restarts. `muted`: "Воспроизведение" asks for silence now.
fn told(settings: &Settings, muted: bool) -> [(&'static str, Value); 5] {
    [
        ("fps", json!(settings.fps.clamp(10, 240))),
        ("mute", json!(!settings.audio || muted)),
        ("volume", json!(settings.volume.clamp(0, 100))),
        ("video_hardware", json!(settings.video_gpu)),
        ("texture_max_side", json!(if settings.texture_half { 2048 } else { 0 })),
    ]
}

fn spawn(settings: &Settings) -> Result<Child, String> {
    let engine = bin(ENGINE).ok_or("wepaper (the renderer) is not installed")?;
    let assets = crate::library::assets().ok_or("Wallpaper Engine is not installed in Steam: no assets")?;
    let dir = runtime_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // A new file, not the old one emptied: what reads it sees a new renderer.
    let _ = std::fs::remove_file(dir.join("engine.log"));
    let log = std::fs::File::create(dir.join("engine.log")).map_err(|e| e.to_string())?;
    // The renderer keeps what it showed last there: ours is in desktop.json,
    // so it starts with nothing and is told.
    let state = dir.join("engine-state.json");
    let _ = std::fs::remove_file(&state);
    let mut command = Command::new(engine);
    command
        .arg("-v")
        .arg("daemon")
        .arg("--socket")
        .arg(socket())
        .arg("--state")
        .arg(state)
        .arg("--assets")
        .arg(assets);
    // Its own config file is not ours to read: the settings are told.
    command.arg("--config").arg(dir.join("engine-config.json"));
    for (key, value) in told(settings, false) {
        command.arg("--set").arg(format!("{key}={value}"));
    }
    command.stdin(Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    // The renderer goes when the daemon does, however that ends.
    unsafe {
        command.pre_exec(|| {
            prctl(PR_SET_PDEATHSIG, SIGTERM);
            Ok(())
        });
    }
    command.spawn().map_err(|e| e.to_string())
}

/// Stop our renderer: asked to quit (no longer than a moment: one that hangs
/// does not answer), then killed if it is still there.
fn stop(child: &mut Child) {
    let _ = ask(&socket(), &json!({ "cmd": "quit" }), Duration::from_secs(1));
    for _ in 0..30 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The pid of the renderer the daemon runs, 0 while none: what the watch of
/// [`watch_stop`] kills when the daemon's loop cannot.
static RENDERER: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// How long the daemon told to go may take over it before its renderer is
/// killed from outside the loop, and how long after that it exits anyway.
const GRACE: Duration = Duration::from_secs(2);

/// The daemon's loop sees that it was told to go between two requests to the
/// renderer, and a request may take long. This thread wakes it at once, kills
/// the renderer when the loop is not done after [`GRACE`] (the request it
/// waits on fails then), and leaves for it after another.
fn watch_stop(wake: Sender<()>) {
    std::thread::spawn(move || {
        while !STOP.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = wake.send(());
        std::thread::sleep(GRACE);
        let pid = RENDERER.load(std::sync::atomic::Ordering::Relaxed);
        if pid > 0 {
            unsafe {
                kill(pid, SIGKILL);
            }
        }
        std::thread::sleep(GRACE);
        restore_images();
        std::process::exit(0);
    });
}

/// Waits for a renderer just started to answer and to have found the
/// monitors: the wallpaper then goes up in this round of the daemon and not
/// seconds later, in the next. Gives up when the renderer exits, or takes
/// longer than a renderer ever does.
fn await_renderer(child: &mut Child) {
    let since = Instant::now();
    while since.elapsed() < Duration::from_secs(2) {
        if !outputs().is_empty() || !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A renderer that goes sooner than this after its start did not get to work.
const SHORT_LIFE: Duration = Duration::from_secs(30);
/// So many short lives in a row and the renderer is not started again.
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
    let (Some(runtime), Some(signature)) =
        (std::env::var_os("XDG_RUNTIME_DIR"), std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE"))
    else {
        return;
    };
    let socket = PathBuf::from(runtime).join("hypr").join(signature).join(".socket2.sock");
    std::thread::spawn(move || {
        loop {
            if let Ok(stream) = UnixStream::connect(&socket) {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let relevant = [
                        "fullscreen",
                        "workspace",
                        "focusedmon",
                        "activespecial",
                        "activewindow",
                        "openwindow",
                        "closewindow",
                        "movewindow",
                        "monitoradded",
                        "monitorremoved",
                    ];
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
            [desktop::desktop_file(), desktop::settings_file()]
                .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
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
    // Until something changes; a renderer that died is looked at again in a
    // few seconds.
    let rest = || {
        let _ = woken.recv_timeout(Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(120));
        while woken.try_recv().is_ok() {}
    };
    watch_hyprland(wake.clone());
    watch_stop(wake.clone());
    watch_files(wake);
    std::thread::spawn(desktop::run_playlist);
    hypr_layer_rule();

    let mut renderer: Option<(Child, Instant)> = None;
    let mut announced: Option<PathBuf> = None;
    // What could not be shown (monitor and wallpaper) and why: not tried
    // again until the user sets something.
    let mut broken: HashMap<(String, PathBuf), String> = HashMap::new();
    // Why the renderer itself does not run, when it does not.
    let mut down: Option<String> = None;
    // That a renderer hung and was killed, until the user sets something.
    let mut hung: Option<String> = None;
    let mut short_lives = 0;
    let mut desktop_stamp = None;
    // What the errors file holds (None: not known, it is written anew). The
    // file is written when this changes only: its time says how new it is.
    let mut reported: Option<String> = None;
    // What an earlier daemon left: a renderer that outlived it (one that hung
    // does not go with it) is stopped before its socket goes, else it would
    // draw and play next to ours.
    if !put_down(&socket(), Duration::from_secs(1)) {
        eprintln!("we-linux: the renderer an earlier daemon left does not stop");
    }
    let _ = std::fs::remove_file(socket());
    let _ = std::fs::remove_file(runtime_dir().join("engine.log"));
    // The settings the running renderer has.
    let mut settings_told: Option<String> = None;
    // Monitors whose wallpaper is frozen under a fullscreen window.
    let mut frozen: HashMap<String, bool> = HashMap::new();
    unsafe {
        signal(SIGTERM, on_stop);
        signal(SIGINT, on_stop);
    }
    loop {
        // Told to go: the renderer stops and the desktop gets its picture back.
        if STOP.load(std::sync::atomic::Ordering::Relaxed) {
            if let Some((mut child, _)) = renderer.take() {
                stop(&mut child);
            }
            RENDERER.store(0, std::sync::atomic::Ordering::Relaxed);
            restore_images();
            std::process::exit(0);
        }
        let desktop = Desktop::load();
        let settings = Settings::load();
        let live = desktop.wallpaper.as_ref().is_some_and(|p| p.is_dir())
            || desktop.outputs.values().any(|p| p.is_dir());
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
            short_lives = 0;
            down = None;
            hung = None;
            reported = None;
        }
        let covered = if hyprland() { covered_monitors() } else { Vec::new() };
        let sound = settings.playback_audio != "run"
            && others_play_sound(RENDERER.load(std::sync::atomic::Ordering::Relaxed));
        let playbacks: Vec<(String, Playback)> =
            covered.iter().map(|(name, over)| (name.clone(), under(&settings, *over, sound))).collect();
        // Every monitor is to stop: the renderer goes, and its memory with it.
        let hidden =
            !playbacks.is_empty() && playbacks.iter().all(|(_, playback)| *playback == Playback::Stop);
        // One monitor pausing all of them, one muting all of them.
        let pause_all = playbacks.iter().any(|(_, playback)| *playback == Playback::PauseAll);
        let muted = playbacks.iter().any(|(_, playback)| *playback == Playback::Mute)
            || (sound && Playback::from_name(&settings.playback_audio) == Playback::Mute);

        // A renderer that went on its own is started again (the compositor
        // may have come back), but not one that goes every time.
        if let Some((child, since)) = &mut renderer
            && let Ok(Some(status)) = child.try_wait()
        {
            short_lives = if since.elapsed() < SHORT_LIFE { short_lives + 1 } else { 0 };
            if short_lives >= SHORT_LIVES {
                down = Some(format!("the renderer keeps exiting ({status}): see `we-linux doctor`"));
                // No scene to stand in for: the user's own picture is better
                // than a still of what did not load.
                restore_images();
            }
            renderer = None;
            RENDERER.store(0, std::sync::atomic::Ordering::Relaxed);
        }
        if !live || hidden {
            if let Some((mut child, _)) = renderer.take() {
                stop(&mut child);
                RENDERER.store(0, std::sync::atomic::Ordering::Relaxed);
                let _ = std::fs::remove_file(runtime_dir().join("engine.log"));
            }
        } else if renderer.is_none() && short_lives < SHORT_LIVES {
            match spawn(&settings) {
                Ok(mut child) => {
                    RENDERER.store(child.id() as i32, std::sync::atomic::Ordering::Relaxed);
                    await_renderer(&mut child);
                    renderer = Some((child, Instant::now()));
                    down = None;
                    // A new renderer plays on every monitor, with these settings.
                    frozen.clear();
                    settings_told = Some(format!("{:?}", told(&settings, false)));
                }
                Err(e) => {
                    if down.as_ref() != Some(&e) {
                        eprintln!("we-linux: {e}");
                    }
                    down = Some(e);
                }
            }
        }

        SILENT.set(false);
        // A monitor that was being given its wallpaper when the renderer hung.
        let mut loading: Option<(String, PathBuf)> = None;
        if renderer.is_some() && running() {
            // Settings that changed are told; nothing restarts.
            let now = format!("{:?}", told(&settings, muted));
            if settings_told.as_ref() != Some(&now) {
                let all = told(&settings, muted).into_iter().all(|(key, value)| {
                    call(json!({ "cmd": "set", "path": format!("config/{key}"), "value": value })).is_ok()
                });
                if all {
                    settings_told = Some(now);
                }
            }
            // Each monitor gets what it is to show, when it shows something else.
            let showing: HashMap<String, PathBuf> = shown().into_iter().collect();
            for (monitor, _, _) in outputs() {
                // A renderer that hangs answers nothing more this round.
                if SILENT.get() {
                    break;
                }
                let want = wanted(&desktop, &monitor);
                if want.as_ref() == showing.get(&monitor) {
                    continue;
                }
                match want {
                    Some(wallpaper) => {
                        let key = (monitor.clone(), wallpaper.clone());
                        if broken.contains_key(&key) {
                            continue;
                        }
                        let request = json!({ "cmd": "show", "wallpaper": wallpaper, "output": monitor, "properties": properties(&wallpaper) });
                        match call(request) {
                            Ok(_) => {
                                frozen.remove(&monitor);
                                // A video's alignment, as the user set it (else the setting).
                                let fill = properties(&wallpaper)
                                    .get("alignment")
                                    .and_then(crate::props::fill_of)
                                    .map_or(Value::Null, Value::from);
                                let _ = call(
                                    json!({ "cmd": "set", "path": format!("outputs/{monitor}/fill"), "value": fill }),
                                );
                            }
                            Err(_) if SILENT.get() => loading = Some(key),
                            Err(why) => {
                                eprintln!("we-linux: {}: {why}", wallpaper.display());
                                broken.insert(key, why);
                                // The monitor keeps what it showed; with nothing there,
                                // the user's own picture is better than a still.
                                if !showing.contains_key(&monitor) {
                                    restore_images();
                                }
                            }
                        }
                    }
                    None => {
                        let _ = call(json!({ "cmd": "hide", "output": monitor }));
                    }
                }
            }
            // A monitor under a fullscreen window keeps its frame: the scene
            // stops there (no work for nothing) and goes on from that frame after.
            for (monitor, playback) in &playbacks {
                if SILENT.get() {
                    break;
                }
                let freeze = pause_all || *playback >= Playback::Pause;
                if frozen.get(monitor).copied().unwrap_or(false) == freeze {
                    continue;
                }
                let told = call(json!({ "cmd": if freeze { "pause" } else { "resume" }, "output": monitor }))
                    .is_ok();
                // A monitor the renderer does not have yet is told on the next round.
                if told {
                    frozen.insert(monitor.clone(), freeze);
                }
            }
        }

        // A renderer that does not answer in time hangs: it is killed (it would
        // hold the daemon up at every request and may play on) and counts as
        // one that did not get to work. One that hung loading a wallpaper is
        // not given that wallpaper again.
        if SILENT.get()
            && let Some((mut child, _)) = renderer.take()
        {
            let _ = child.kill();
            let _ = child.wait();
            RENDERER.store(0, std::sync::atomic::Ordering::Relaxed);
            let _ = std::fs::remove_file(socket());
            short_lives += 1;
            let why = "the renderer stopped answering and was killed: see `we-linux doctor`".to_owned();
            eprintln!("we-linux: {why}");
            match loading {
                Some(key) => {
                    broken.insert(key, "the renderer hung loading it and was killed".to_owned());
                }
                None => hung = Some(why),
            }
            if short_lives >= SHORT_LIVES {
                down = Some("the renderer keeps hanging: see `we-linux doctor`".to_owned());
                restore_images();
            }
        }

        // Every failure with its wallpaper: who asks about another wallpaper
        // (another monitor's) is not told this one's.
        let mut errors: Vec<String> =
            broken.iter().map(|((_, wallpaper), why)| format!("{}\t{why}", wallpaper.display())).collect();
        if let Some(why) = down.as_ref().or(hung.as_ref()) {
            let wallpapers = desktop.wallpaper.iter().chain(desktop.outputs.values());
            errors.extend(wallpapers.map(|wallpaper| format!("{}\t{why}", wallpaper.display())));
        }
        errors.sort();
        errors.dedup();
        let report = errors.join("\n");
        if reported.as_ref() != Some(&report) {
            if report.is_empty() {
                let _ = std::fs::remove_file(errors_file());
            } else {
                let _ = std::fs::write(errors_file(), &report);
            }
            reported = Some(report);
        }
        rest();
    }
}

/// How far a wallpaper set a moment ago has come.
pub enum Stage {
    /// The renderer is starting or loading it.
    Loading,
    /// The renderer shows it.
    Shown,
    Failed(String),
}

/// The stage of `wallpaper`: what the renderer says it shows, and what the
/// daemon could not get it to show.
pub fn stage(wallpaper: &Path) -> Stage {
    if shown().iter().any(|(_, path)| path == wallpaper) {
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
    Stage::Loading
}

/// `we-linux doctor`: what a bug report needs, in one paste.
pub fn doctor() -> String {
    let found =
        |path: Option<PathBuf>| path.map_or_else(|| "NOT FOUND".to_owned(), |p| p.display().to_string());
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
    out.push(format!(
        "session: {} / {}, WAYLAND_DISPLAY={}",
        env("XDG_CURRENT_DESKTOP"),
        env("XDG_SESSION_TYPE"),
        env("WAYLAND_DISPLAY")
    ));
    if hyprland() {
        out.push(format!(
            "hyprland: {}",
            output("hyprctl", &["version"]).unwrap_or_else(|| "hyprctl does not answer".into())
        ));
        let monitors: Vec<String> =
            desktop::monitors().into_iter().map(|m| format!("{} {}", m.name, m.detail)).collect();
        out.push(format!("monitors: {}", monitors.join(", ")));
    }
    if let Some(memory) =
        output("nvidia-smi", &["--query-gpu=name,memory.used,memory.total", "--format=csv,noheader"])
    {
        out.push(format!("gpu: {memory}"));
    }
    out.push(String::new());
    out.push(format!("{ENGINE}: {}", found(bin(ENGINE))));
    if let Some(version) = bin(ENGINE).and_then(|engine| output(&engine.to_string_lossy(), &["--version"])) {
        out.push(format!("  {version}"));
    }
    out.push(format!(
        "ffmpeg: {}",
        output("ffmpeg", &["-version"]).unwrap_or_else(|| {
            "NOT FOUND (videos are not optimised for the monitor, no stills of videos)".into()
        })
    ));
    out.push(format!(
        "steam libraries: {}",
        if libraries.is_empty() {
            "NONE FOUND".to_owned()
        } else {
            libraries.iter().map(|l| l.display().to_string()).collect::<Vec<_>>().join(", ")
        }
    ));
    out.push(format!("wallpaper engine assets: {}", found(crate::library::assets())));
    out.push(format!("wallpapers on disk: {}", crate::library::installed().len()));
    out.push(String::new());
    let lock = lock_file();
    let daemon = lock.is_some_and(|l| l.try_lock().is_err());
    out.push(format!(
        "daemon: {}",
        if daemon { "running" } else { "NOT RUNNING (start it: we-linux daemon)" }
    ));
    out.push(format!(
        "wallpaper: {}",
        desktop.wallpaper.as_ref().map_or_else(|| "none".to_owned(), |p| p.display().to_string())
    ));
    if let Some(wallpaper) = &desktop.wallpaper {
        let json = project(wallpaper);
        out.push(format!(
            "  type: {}, title: {}",
            json["type"].as_str().unwrap_or("scene"),
            json["title"].as_str().unwrap_or("")
        ));
    }
    for (name, path) in &desktop.outputs {
        out.push(format!("  {name}: {}", path.display()));
    }
    match call(json!({ "cmd": "version" })) {
        Ok(version) => {
            out.push(format!(
                "renderer: running, {} on {}",
                version["version"].as_str().unwrap_or("?"),
                version["gpu"].as_str().unwrap_or("?")
            ));
            for (monitor, path) in shown() {
                out.push(format!("  {monitor} shows {}", path.display()));
            }
            if let Ok(memory) = call(json!({ "cmd": "get", "path": "memory" })) {
                let mb = |key: &str| memory[key].as_f64().unwrap_or(0.0) / 1048576.0;
                out.push(format!(
                    "  video memory: textures {:.0} MB, buffers {:.0} MB, video {:.0} MB",
                    mb("textures"),
                    mb("buffers"),
                    mb("video")
                ));
            }
        }
        Err(why) => out.push(format!("renderer: {why}")),
    }
    for (wallpaper, why) in errors() {
        out.push(format!("last error: {why} ({})", wallpaper.display()));
    }
    if !Settings::load().hook.trim().is_empty() {
        out.push(format!("hook output: {}", hook_log().display()));
    }
    out.push(format!(
        "settings: {}",
        std::fs::read_to_string(desktop::settings_file())
            .map_or_else(|_| "(defaults)".to_owned(), |s| s.split_whitespace().collect::<Vec<_>>().join(" "))
    ));
    if let Ok(log) = std::fs::read_to_string(runtime_dir().join("engine.log")) {
        out.push(String::new());
        out.push("--- renderer log, last lines ---".to_owned());
        let lines: Vec<&str> = log.lines().collect();
        out.extend(lines[lines.len().saturating_sub(40)..].iter().map(|l| strip_colours(l)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A folder of its own for one test (short: a socket's path is at most 108 bytes).
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("we-linux-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Set by the test that runs this binary as a renderer that hangs: the
    /// socket it listens on without ever answering.
    const HANG_ON: &str = "WE_LINUX_TEST_HANG_ON";
    /// And whether it shrugs off SIGTERM, as an old renderer stuck in a call does.
    const DEAF: &str = "WE_LINUX_TEST_DEAF";

    /// Not a test: the renderer that hangs, run by the tests below.
    #[test]
    #[ignore]
    fn hanging_renderer() {
        let Some(socket) = std::env::var_os(HANG_ON) else { return };
        if std::env::var_os(DEAF).is_some() {
            extern "C" fn ignore(_: i32) {}
            unsafe {
                signal(SIGTERM, ignore);
            }
        }
        let listener = UnixListener::bind(socket).unwrap();
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    }

    fn start_hanging(socket: &Path, deaf: bool) -> Child {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "engine::tests::hanging_renderer", "--ignored", "--nocapture"])
            .env(HANG_ON, socket);
        if deaf {
            command.env(DEAF, "1");
        }
        let child = command.stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let since = Instant::now();
        while !listening(socket) {
            assert!(since.elapsed() < Duration::from_secs(10), "the hanging renderer did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        child
    }

    #[test]
    fn questions_wait_seconds_and_loading_waits_longer() {
        assert_eq!(patience(&json!({ "cmd": "version" })), Duration::from_secs(5));
        assert_eq!(
            patience(&json!({ "cmd": "set", "path": "config/fps", "value": 60 })),
            Duration::from_secs(5)
        );
        assert_eq!(patience(&json!({ "cmd": "quit" })), Duration::from_secs(5));
        assert_eq!(patience(&json!({ "cmd": "show", "wallpaper": "/w" })), Duration::from_secs(30));
        assert_eq!(patience(&json!({ "cmd": "snapshot" })), Duration::from_secs(30));
    }

    #[test]
    fn nothing_listening_is_absent_at_once() {
        let socket = scratch("absent").join("none.sock");
        let since = Instant::now();
        assert_eq!(ask(&socket, &json!({ "cmd": "version" }), Duration::from_secs(5)), Err(Failure::Absent));
        assert!(since.elapsed() < Duration::from_secs(1));
        assert!(!SILENT.get());
    }

    #[test]
    fn a_renderer_that_does_not_answer_is_silent_in_time() {
        let socket = scratch("silent").join("wepaper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let held = std::thread::spawn(move || listener.accept().map(|(stream, _)| stream));
        let since = Instant::now();
        assert_eq!(
            ask(&socket, &json!({ "cmd": "version" }), Duration::from_millis(300)),
            Err(Failure::Silent)
        );
        assert!(since.elapsed() < Duration::from_secs(2));
        assert!(SILENT.get(), "the daemon's loop is told the renderer hangs");
        drop(held.join());
    }

    #[test]
    fn an_answer_and_a_refusal() {
        let socket = scratch("answer").join("wepaper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            for answer in
                [r#"{"ok":true,"value":{"version":"1"}}"#, r#"{"ok":false,"error":"no such output"}"#, ""]
            {
                let (stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                (&stream).write_all(format!("{answer}\n").as_bytes()).unwrap();
            }
        });
        let patience = Duration::from_secs(5);
        assert_eq!(ask(&socket, &json!({ "cmd": "version" }), patience), Ok(json!({ "version": "1" })));
        assert_eq!(
            ask(&socket, &json!({ "cmd": "hide" }), patience),
            Err(Failure::Said("no such output".to_owned()))
        );
        assert_eq!(
            ask(&socket, &json!({ "cmd": "hide" }), patience),
            Err(Failure::Said("the renderer gave no answer".to_owned()))
        );
        server.join().unwrap();
        assert!(!SILENT.get());
    }

    #[test]
    fn the_listener_is_known_by_its_socket() {
        let socket = scratch("listener").join("wepaper.sock");
        assert_eq!(listener(&socket), None);
        let _bound = UnixListener::bind(&socket).unwrap();
        assert_eq!(listener(&socket), Some(std::process::id() as i32));
    }

    #[test]
    fn a_hanging_renderer_is_put_down() {
        let socket = scratch("put-down").join("wepaper.sock");
        let mut child = start_hanging(&socket, false);
        let since = Instant::now();
        assert!(put_down(&socket, Duration::from_millis(500)));
        assert!(since.elapsed() < Duration::from_secs(3));
        assert!(!listening(&socket));
        let _ = child.wait();
    }

    #[test]
    fn a_renderer_deaf_to_sigterm_is_killed() {
        let socket = scratch("deaf").join("wepaper.sock");
        let mut child = start_hanging(&socket, true);
        let since = Instant::now();
        assert!(put_down(&socket, Duration::from_millis(500)));
        assert!(since.elapsed() < Duration::from_secs(3));
        let status = child.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(SIGKILL));
    }

    #[test]
    fn the_hook_says_its_piece_to_its_log() {
        let dir = scratch("hook");
        let log = dir.join("hook.log");
        std::fs::write(&log, "what the hook of before said\n").unwrap();
        let status = run_hook(r#"echo "$WE_LINUX_TITLE"; echo oops >&2"#, &dir, None, "Rain", &log).unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "Rain\noops\n");
    }
}
