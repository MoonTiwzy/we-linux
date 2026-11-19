//! Hyprland, through its two sockets (wiki.hypr.land/IPC): `.socket.sock`
//! answers one request a connection, written as hyprctl's arguments with `j/`
//! for JSON; `.socket2.sock` streams `EVENT>>DATA` lines.

use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::Value;

use super::{Change, Compositor, Monitor, Over, Tells};

/// The layer-shell namespace of the renderer's surfaces.
const NAMESPACE: &str = "wepaper";

pub struct Hyprland {
    /// `$XDG_RUNTIME_DIR/hypr/<its instance signature>`.
    dir: PathBuf,
}

impl Hyprland {
    /// The Hyprland of this session: the one the environment names, when it
    /// answers. One that crashed leaves its folder and its socket behind, and
    /// an environment that outlived it (systemd's, a terminal's) still names
    /// it; start-hyprland starts Hyprland anew, with another signature, on the
    /// same display. Then it is the Hyprland that answers on the session's
    /// display, as its lock file says.
    pub fn find() -> Option<Self> {
        let hypr = super::runtime()?.join("hypr");
        let named = hypr.join(std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?);
        if answers(&named) {
            return Some(Self { dir: named });
        }
        answering_on(&hypr, &super::display_name()).map(|dir| Self { dir })
    }

    /// One request and its answer. Hyprland handles the socket on its main
    /// thread: the connection is opened for the request and closed at once.
    fn ask(&self, request: &str) -> Option<String> {
        let mut stream = UnixStream::connect(self.dir.join(".socket.sock")).ok()?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        stream.write_all(request.as_bytes()).ok()?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer).ok()?;
        Some(answer)
    }

    fn json(&self, what: &str) -> Option<Value> {
        serde_json::from_str(&self.ask(&format!("j/{what}"))?).ok()
    }

    /// Its signature, which hyprctl is told: the environment may name a
    /// Hyprland that went.
    fn signature(&self) -> &std::ffi::OsStr {
        self.dir.file_name().unwrap_or_default()
    }
}

/// Whether the Hyprland of the folder `dir` is there to answer on its socket:
/// it is not gone (one that is busy may not take the connection at once).
fn answers(dir: &Path) -> bool {
    !super::socket_gone(&dir.join(".socket.sock"))
}

/// What a Hyprland's lock file says: its process and the name of its Wayland
/// display, a line each (what `hyprctl instances` reads).
fn lock_of(dir: &Path) -> Option<(i32, String)> {
    let text = std::fs::read_to_string(dir.join("hyprland.lock")).ok()?;
    let mut lines = text.lines();
    let pid = lines.next()?.trim().parse().ok()?;
    let display = lines.next()?.trim();
    (!display.is_empty()).then(|| (pid, display.to_owned()))
}

/// The folders of the Hyprlands in `hypr` ($XDG_RUNTIME_DIR/hypr) that drew
/// on `display`, as their lock files say.
fn on_display<'a>(hypr: &Path, display: &'a str) -> impl Iterator<Item = (PathBuf, i32)> + 'a {
    std::fs::read_dir(hypr).into_iter().flatten().flatten().filter_map(move |entry| {
        let (pid, on) = lock_of(&entry.path())?;
        (on == display).then(|| (entry.path(), pid))
    })
}

/// The Hyprland in `hypr` that answers and draws on `display`.
fn answering_on(hypr: &Path, display: &str) -> Option<PathBuf> {
    on_display(hypr, display).map(|(dir, _)| dir).find(|dir| answers(dir))
}

/// Whether a Hyprland in `hypr` is starting on `display`. Hyprland binds its
/// sockets first (the event socket, then the display's and the request
/// socket), and writes its lock file half a second later: until then one
/// that answers on either socket has no say of its display, and is taken for
/// the one starting on the session's. Or its process runs (`alive`) and has
/// named the display, and its socket does not answer yet.
fn starting_on(hypr: &Path, display: &str, alive: impl Fn(i32) -> bool) -> bool {
    std::fs::read_dir(hypr).into_iter().flatten().flatten().any(|entry| {
        let dir = entry.path();
        match lock_of(&dir) {
            None => answers(&dir) || !super::socket_gone(&dir.join(".socket2.sock")),
            Some((pid, on)) => on == display && alive(pid) && !answers(&dir),
        }
    })
}

/// Whether the process `pid` is a Hyprland.
fn is_hyprland(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|name| name.trim() == "Hyprland")
}

/// Whether the session's Hyprland is starting: the environment says the
/// session is Hyprland's, none answers on the session's display as far as
/// the lock files say, and one is starting (see [`starting_on`]).
pub(super) fn starting() -> bool {
    let Some(runtime) = super::runtime() else {
        return false;
    };
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        && Hyprland::find().is_none()
        && starting_on(&runtime.join("hypr"), &super::display_name(), is_hyprland)
}

/// The monitors of `j/monitors`: `x`, `y` are logical, `width`, `height` the
/// mode, which the scale divides and a turn of 90 or 270 degrees swaps.
fn monitors_of(monitors: &Value) -> Vec<Monitor> {
    monitors
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["disabled"].as_bool() != Some(true))
        .map(|m| {
            let number = |key: &str| m[key].as_f64().unwrap_or(0.0);
            let scale = if number("scale") > 0.0 { number("scale") } else { 1.0 };
            let turned = m["transform"].as_i64().unwrap_or(0) % 2 == 1;
            let (width, height) = if turned {
                (number("height"), number("width"))
            } else {
                (number("width"), number("height"))
            };
            Monitor {
                name: m["name"].as_str().unwrap_or("").to_owned(),
                x: number("x") as f32,
                y: number("y") as f32,
                width: (width / scale) as f32,
                height: (height / scale) as f32,
                pixels: (width as u32, height as u32),
            }
        })
        .collect()
}

/// What is over each monitor, from `j/monitors`, `j/clients` and
/// `j/activewindow`. A client's `fullscreen` is its mode: 1 maximised, 2
/// fullscreen, 3 both.
fn over_of(monitors: &Value, clients: &Value, active: &Value) -> Vec<(String, Over)> {
    let active = active["monitor"].as_i64();
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
            // The special workspace shows over the regular one when it is open.
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

/// What an event of `.socket2.sock` means to the daemon. Hyprland says
/// nothing when a monitor's power goes off: the daemon's round finds that.
fn change_of(line: &str) -> Option<Change> {
    let event = line.split_once(">>").map_or(line, |(event, _)| event);
    match event {
        "fullscreen" | "workspace" | "workspacev2" | "focusedmon" | "focusedmonv2" | "activespecial"
        | "activespecialv2" | "activewindowv2" | "openwindow" | "closewindow" | "movewindow"
        | "movewindowv2" | "moveworkspace" | "moveworkspacev2" | "monitoradded" | "monitoraddedv2"
        | "monitorremoved" | "monitorremovedv2" | "changefloatingmode" | "minimized" => Some(Change::Covers),
        // A reload forgets the layer rule set at run time.
        "configreloaded" => Some(Change::Layers),
        _ => None,
    }
}

/// `hyprctl hyprpaper listactive`: a monitor and its picture a line, as
/// "DP-1 = /a/b.png" in older hyprpaper and "DP-1: /a/b.png" from 0.8 on.
fn hyprpaper_pictures(text: &str) -> Vec<(String, PathBuf)> {
    text.lines()
        .filter_map(|line| line.split_once(" = ").or_else(|| line.split_once(": ")))
        .filter(|(_, image)| image.trim().starts_with('/'))
        .map(|(monitor, image)| (monitor.trim().to_owned(), PathBuf::from(image.trim())))
        .collect()
}

/// hyprpaper speaks its own protocol, and hyprctl is its client. hyprctl
/// gives hyprpaper 5 s to take the connection, then waits for the answer as
/// long as it takes: a hyprpaper stuck on a picture would hold up whoever
/// asks (the daemon's loop, the window) with it. So hyprctl has 5 s in all.
/// hyprpaper's socket is in the folder of the Hyprland of `signature`.
fn hyprpaper(signature: &std::ffi::OsStr, args: &[&str]) -> Option<String> {
    let mut child = Command::new("hyprctl")
        .env("HYPRLAND_INSTANCE_SIGNATURE", signature)
        .arg("hyprpaper")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let since = std::time::Instant::now();
    // The answer is a line a monitor: the pipe holds it until hyprctl is done.
    while matches!(child.try_wait(), Ok(None)) {
        if since.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !text.starts_with("error") && !text.contains("Couldn't connect")).then_some(text)
}

impl Compositor for Hyprland {
    fn name(&self) -> &'static str {
        "Hyprland"
    }

    fn version(&self) -> Option<String> {
        let version = self.json("version")?;
        version["tag"].as_str().or(version["version"].as_str()).map(str::to_owned)
    }

    fn monitors(&self) -> Option<Vec<Monitor>> {
        Some(monitors_of(&self.json("monitors")?))
    }

    fn over(&self) -> Option<Vec<(String, Over)>> {
        let (monitors, clients) = (self.json("monitors")?, self.json("clients")?);
        let active = self.json("activewindow").unwrap_or(Value::Null);
        Some(over_of(&monitors, &clients, &active))
    }

    fn tells(&self) -> Tells {
        Tells::ALL
    }

    /// Nothing listens on its socket. It takes the socket with it when it
    /// exits; a crash leaves the file behind.
    fn gone(&self) -> bool {
        !answers(&self.dir)
    }

    /// Its signature: the daemon's environment may name a Hyprland that went
    /// (see [`Hyprland::find`]), and hyprctl in the hook is to talk to this one.
    fn environment(&self) -> Vec<(&'static str, std::ffi::OsString)> {
        vec![("HYPRLAND_INSTANCE_SIGNATURE", self.signature().to_owned())]
    }

    /// When this Hyprland goes, the daemon is told ([`Change::Gone`]) and
    /// leaves (see the daemon's loop). A Hyprland that comes after it
    /// (start-hyprland restarts one that crashed) has another signature, and
    /// the daemon would stay blind to what covers its monitors and hold the
    /// lock the new Hyprland's own daemon needs.
    fn watch(&self, changes: Sender<Change>) {
        let hyprland = Hyprland { dir: self.dir.clone() };
        std::thread::spawn(move || {
            loop {
                if let Ok(stream) = UnixStream::connect(hyprland.dir.join(".socket2.sock")) {
                    for line in super::lines(BufReader::new(stream)) {
                        match change_of(&line) {
                            // The rule is all it takes here: the surfaces stay as they are.
                            Some(Change::Layers) => hyprland.keep_on_top(),
                            Some(change) if changes.send(change).is_err() => return,
                            _ => {}
                        }
                    }
                }
                if hyprland.gone() {
                    crate::log::info("Hyprland went");
                    let _ = changes.send(Change::Gone);
                    return;
                }
                // The event socket closed and Hyprland still answers (or cannot
                // be told from here, or is not quite gone yet): look again in a
                // moment.
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    /// Hyprland stacks the layers of one level by their `order`, lower on top:
    /// -1 keeps the renderer over a static wallpaper's layer (awww, swww,
    /// hyprpaper: 0) whichever of them started last. The layer fades in and out.
    fn keep_on_top(&self) {
        // The Lua config (0.55+), the block syntax (0.53+), the old one.
        let lua = format!(
            r#"eval hl.layer_rule({{ match = {{ namespace = "^({NAMESPACE})$" }}, order = -1, animation = "fade" }})"#
        );
        let ok = |request: &str| self.ask(request).is_some_and(|answer| answer.trim() == "ok");
        let _ = ok(&lua)
            || ok(&format!("keyword layerrule match:namespace ^({NAMESPACE})$, order -1, animation fade"))
            || (ok(&format!("keyword layerrule order -1, {NAMESPACE}"))
                && ok(&format!("keyword layerrule animation fade, {NAMESPACE}")));
    }

    fn show_picture(&self, image: &Path, monitor: Option<&str>) -> bool {
        let path = image.to_string_lossy();
        let target = format!("{},{path}", monitor.unwrap_or(""));
        let told = |args: &[&str]| hyprpaper(self.signature(), args).is_some();
        // Older hyprpaper loads a picture before it shows it; newer has no `preload`.
        (told(&["preload", &path]) && told(&["wallpaper", &target])) || told(&["wallpaper", &target])
    }

    fn pictures(&self) -> Vec<(String, PathBuf)> {
        hyprpaper(self.signature(), &["listactive"]).map(|text| hyprpaper_pictures(&text)).unwrap_or_default()
    }

    /// hyprpaper also knows a monitor by its description ("desc:...") and
    /// every monitor by none (""): of those it cannot be told they went.
    fn has_monitor(&self, name: &str, monitors: &[Monitor]) -> bool {
        name.is_empty() || name.starts_with("desc:") || super::among(name, monitors)
    }

    fn autostart_line(&self) -> Option<String> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
        Some(if config.join("hypr/hyprland.lua").exists() {
            r#"hl.on("hyprland.start", function() hl.exec_cmd("we-linux daemon") end)"#.to_owned()
        } else {
            "exec-once = we-linux daemon".to_owned()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::reply;
    use super::super::tests::closed;
    use super::*;

    fn json(name: &str) -> Value {
        serde_json::from_str(&reply(&format!("hyprland/{name}"))).unwrap()
    }

    #[test]
    fn monitors_are_logical_pixels_where_the_scale_puts_them() {
        // The nested Hyprland of the tests runs its one monitor at scale 2.
        let monitors = monitors_of(&json("monitors.json"));
        assert_eq!(
            monitors,
            vec![Monitor {
                name: "WAYLAND-1".into(),
                x: 0.0,
                y: 0.0,
                width: 960.0,
                height: 540.0,
                pixels: (1920, 1080)
            }]
        );
    }

    #[test]
    fn a_tiled_window_is_focus_and_no_cover() {
        let over = over_of(&json("monitors.json"), &json("clients-tiled.json"), &json("activewindow.json"));
        assert_eq!(over, vec![("WAYLAND-1".into(), Over { focused: true, covered: 0, asleep: false })]);
    }

    #[test]
    fn fullscreen_and_maximised_are_the_clients_mode() {
        let fullscreen =
            over_of(&json("monitors.json"), &json("clients-fullscreen.json"), &json("activewindow.json"));
        assert_eq!(fullscreen[0].1.covered, 2);
        let maximized =
            over_of(&json("monitors.json"), &json("clients-maximized.json"), &json("activewindow.json"));
        assert_eq!(maximized[0].1.covered, 1);
    }

    #[test]
    fn a_window_on_another_workspace_covers_nothing() {
        let over = over_of(
            &json("monitors-other-workspace.json"),
            &json("clients-fullscreen.json"),
            &json("activewindow-none.json"),
        );
        assert_eq!(over, vec![("WAYLAND-1".into(), Over::default())]);
    }

    #[test]
    fn a_monitor_whose_power_is_off_sleeps() {
        let over = over_of(&json("monitors-dpms-off.json"), &json("clients-tiled.json"), &Value::Null);
        assert!(over[0].1.asleep);
    }

    #[test]
    fn events_that_change_the_cover_and_a_reload_that_forgets_the_rule() {
        let changes: Vec<Option<Change>> = reply("hyprland/events.txt").lines().map(change_of).collect();
        let line = |text: &str| change_of(text);
        assert!(changes.contains(&Some(Change::Covers)));
        assert_eq!(line("fullscreen>>1"), Some(Change::Covers));
        assert_eq!(line("activewindowv2>>55c4a571d0a0"), Some(Change::Covers));
        assert_eq!(line("windowtitlev2>>55c4a571d0a0,zsh"), None);
        assert_eq!(line("configreloaded>>"), Some(Change::Layers));
    }

    #[test]
    fn a_hyprland_is_gone_when_nothing_listens_on_its_socket() {
        let dir = std::env::temp_dir().join(format!("we-linux-test-hyprland-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hyprland = Hyprland { dir: dir.clone() };
        // It exited and took its socket with it.
        assert!(hyprland.gone());
        let listening = std::os::unix::net::UnixListener::bind(dir.join(".socket.sock")).unwrap();
        assert!(!hyprland.gone());
        // It crashed: the socket is still there, nothing listens on it.
        drop(listening);
        closed(&dir.join(".socket.sock"));
        assert!(hyprland.gone());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Hyprland's folder in `hypr`, as it leaves it: its lock file, and its
    /// socket when it answers.
    fn instance(hypr: &Path, signature: &str, lock: &str) -> PathBuf {
        let dir = hypr.join(signature);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hyprland.lock"), lock).unwrap();
        dir
    }

    #[test]
    fn the_hyprland_that_answers_on_the_display_is_found_when_the_named_one_crashed() {
        let hypr = std::env::temp_dir().join(format!("we-linux-test-hypr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&hypr);
        // The one that crashed: its socket is there, nothing listens on it.
        let crashed = instance(&hypr, "a_1", "100\nwayland-1\n");
        drop(std::os::unix::net::UnixListener::bind(crashed.join(".socket.sock")).unwrap());
        closed(&crashed.join(".socket.sock"));
        // A nested one on another display.
        let nested = instance(&hypr, "b_2", "200\nwayland-2\n");
        let _nested = std::os::unix::net::UnixListener::bind(nested.join(".socket.sock")).unwrap();
        // A folder without a lock file, as older ones are.
        std::fs::create_dir_all(hypr.join("c_3")).unwrap();
        // The processes that run: the crashed one's is not among them.
        let running = |pid| pid == 200 || pid == 300;
        assert_eq!(answering_on(&hypr, "wayland-1"), None);
        assert!(!starting_on(&hypr, "wayland-1", running), "a crashed one is not starting");
        // Started anew on the same display, with another signature.
        let anew = instance(&hypr, "d_4", "300\nwayland-1\n");
        assert_eq!(answering_on(&hypr, "wayland-1"), None, "its socket is not there yet");
        assert!(starting_on(&hypr, "wayland-1", running));
        let _anew = std::os::unix::net::UnixListener::bind(anew.join(".socket.sock")).unwrap();
        assert_eq!(answering_on(&hypr, "wayland-1"), Some(anew));
        assert!(!starting_on(&hypr, "wayland-1", running), "it answers: it has started");
        let _ = std::fs::remove_dir_all(&hypr);
    }

    #[test]
    fn a_hyprland_that_answers_before_it_wrote_its_lock_file_is_starting() {
        let hypr = std::env::temp_dir().join(format!("we-linux-test-hypr-unlocked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&hypr);
        // The one that crashed, and a nested one on another display.
        let crashed = instance(&hypr, "a_1", "100\nwayland-1\n");
        drop(std::os::unix::net::UnixListener::bind(crashed.join(".socket.sock")).unwrap());
        closed(&crashed.join(".socket.sock"));
        let nested = instance(&hypr, "b_2", "200\nwayland-2\n");
        let _nested = std::os::unix::net::UnixListener::bind(nested.join(".socket.sock")).unwrap();
        let running = |pid| pid == 200 || pid == 300;
        assert!(!starting_on(&hypr, "wayland-1", running));
        // Started anew: its event socket answers first, then its request
        // socket; its lock file is not written yet.
        let anew = hypr.join("d_4");
        std::fs::create_dir_all(&anew).unwrap();
        let _events = std::os::unix::net::UnixListener::bind(anew.join(".socket2.sock")).unwrap();
        assert!(starting_on(&hypr, "wayland-1", running), "the daemon waits for its sockets");
        let _anew = std::os::unix::net::UnixListener::bind(anew.join(".socket.sock")).unwrap();
        assert_eq!(answering_on(&hypr, "wayland-1"), None);
        assert!(starting_on(&hypr, "wayland-1", running), "the daemon waits for its lock file");
        // Half written.
        std::fs::write(anew.join("hyprland.lock"), "300\n").unwrap();
        assert_eq!(answering_on(&hypr, "wayland-1"), None);
        assert!(starting_on(&hypr, "wayland-1", running));
        // Written: it is the session's Hyprland.
        std::fs::write(anew.join("hyprland.lock"), "300\nwayland-1\n").unwrap();
        assert_eq!(answering_on(&hypr, "wayland-1"), Some(anew));
        assert!(!starting_on(&hypr, "wayland-1", running));
        let _ = std::fs::remove_dir_all(&hypr);
    }

    #[test]
    fn the_hook_is_told_the_hyprland_the_daemon_found() {
        let hyprland = Hyprland { dir: PathBuf::from("/run/user/1000/hypr/d_4") };
        assert_eq!(hyprland.environment(), vec![("HYPRLAND_INSTANCE_SIGNATURE", "d_4".into())]);
    }

    #[test]
    fn a_picture_of_a_monitor_hyprpaper_names_otherwise_may_be_there() {
        let hyprland = Hyprland { dir: PathBuf::from("/run/user/1000/hypr/d_4") };
        let monitors = [super::super::monitor("DP-1")];
        assert!(hyprland.has_monitor("DP-1", &monitors));
        assert!(!hyprland.has_monitor("HDMI-A-1", &monitors));
        assert!(hyprland.has_monitor("", &monitors));
        assert!(hyprland.has_monitor("desc:Dell Inc. DELL U2720Q ABC123", &monitors));
    }

    #[test]
    fn a_lock_file_names_the_process_and_the_display() {
        let hypr = std::env::temp_dir().join(format!("we-linux-test-hypr-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&hypr);
        assert_eq!(lock_of(&instance(&hypr, "a", "3127\nwayland-1\n")), Some((3127, "wayland-1".into())));
        assert_eq!(lock_of(&instance(&hypr, "b", "3127\n")), None);
        assert_eq!(lock_of(&instance(&hypr, "c", "")), None);
        let _ = std::fs::remove_dir_all(&hypr);
    }

    #[test]
    fn hyprpaper_says_monitor_equals_picture() {
        assert_eq!(
            hyprpaper_pictures("DP-1 = /a/b.png\nHDMI-A-1 = /c d.jpg\nnothing here"),
            vec![("DP-1".into(), PathBuf::from("/a/b.png")), ("HDMI-A-1".into(), PathBuf::from("/c d.jpg"))]
        );
    }

    #[test]
    fn newer_hyprpaper_says_monitor_colon_picture() {
        // As 0.8.5 answered.
        assert_eq!(
            hyprpaper_pictures("WAYLAND-1: /home/u/pictures/green.png\n"),
            vec![("WAYLAND-1".into(), PathBuf::from("/home/u/pictures/green.png"))]
        );
    }
}
