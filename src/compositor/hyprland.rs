//! Hyprland, through its two sockets (wiki.hypr.land/IPC): `.socket.sock`
//! answers one request a connection, written as hyprctl's arguments with `j/`
//! for JSON; `.socket2.sock` streams `EVENT>>DATA` lines.

use std::io::{BufRead, BufReader, Read, Write};
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
    /// `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE`.
    dir: PathBuf,
}

impl Hyprland {
    pub fn find() -> Option<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
        let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
        let dir = PathBuf::from(runtime).join("hypr").join(signature);
        dir.join(".socket.sock").exists().then_some(Self { dir })
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

/// `hyprctl hyprpaper listactive`: "DP-1 = /a/b.png" a line.
fn hyprpaper_pictures(text: &str) -> Vec<(String, PathBuf)> {
    text.lines()
        .filter_map(|line| line.split_once(" = "))
        .filter(|(_, image)| image.trim().starts_with('/'))
        .map(|(monitor, image)| (monitor.trim().to_owned(), PathBuf::from(image.trim())))
        .collect()
}

/// hyprpaper speaks its own protocol, and hyprctl is its client.
fn hyprpaper(args: &[&str]) -> Option<String> {
    let out = Command::new("hyprctl")
        .arg("hyprpaper")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
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

    fn watch(&self, changes: Sender<Change>) {
        let hyprland = Hyprland { dir: self.dir.clone() };
        std::thread::spawn(move || {
            loop {
                if let Ok(stream) = UnixStream::connect(hyprland.dir.join(".socket2.sock")) {
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        match change_of(&line) {
                            // The rule is all it takes here: the surfaces stay as they are.
                            Some(Change::Layers) => hyprland.keep_on_top(),
                            Some(change) if changes.send(change).is_err() => return,
                            _ => {}
                        }
                    }
                }
                // Hyprland went (or restarts): look again in a moment.
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
        // Older hyprpaper loads a picture before it shows it; newer has no `preload`.
        (hyprpaper(&["preload", &path]).is_some() && hyprpaper(&["wallpaper", &target]).is_some())
            || hyprpaper(&["wallpaper", &target]).is_some()
    }

    fn pictures(&self) -> Vec<(String, PathBuf)> {
        hyprpaper(&["listactive"]).map(|text| hyprpaper_pictures(&text)).unwrap_or_default()
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
    fn hyprpaper_says_monitor_equals_picture() {
        assert_eq!(
            hyprpaper_pictures("DP-1 = /a/b.png\nHDMI-A-1 = /c d.jpg\nnothing here"),
            vec![("DP-1".into(), PathBuf::from("/a/b.png")), ("HDMI-A-1".into(), PathBuf::from("/c d.jpg"))]
        );
    }
}
