//! niri, through its socket at `$NIRI_SOCKET` (niri's wiki, "IPC"; the
//! niri-ipc crate's documentation): a request is a line of JSON, its reply a
//! line wrapping the answer in `Ok` or `Err`. `"EventStream"` keeps the
//! connection and streams events, the whole state first.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::Value;

use super::{Change, Compositor, Monitor, Over, Tells};

pub struct Niri {
    socket: PathBuf,
}

impl Niri {
    pub fn find() -> Option<Self> {
        let socket = PathBuf::from(std::env::var_os("NIRI_SOCKET")?);
        UnixStream::connect(&socket).is_ok().then_some(Self { socket })
    }

    fn connect(&self, request: &str) -> Option<BufReader<UnixStream>> {
        let mut stream = UnixStream::connect(&self.socket).ok()?;
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        stream.write_all(format!("{request}\n").as_bytes()).ok()?;
        Some(BufReader::new(stream))
    }

    /// The answer to a request such as `"Outputs"`: what `Ok` wraps.
    fn ask(&self, request: &str) -> Option<Value> {
        let mut reader = self.connect(request)?;
        let _ = reader.get_ref().set_read_timeout(Some(Duration::from_secs(2)));
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let reply: Value = serde_json::from_str(&line).ok()?;
        reply.get("Ok").cloned()
    }
}

/// The outputs that are on (a disabled one has no `logical`).
fn monitors_of(outputs: &Value) -> Vec<Monitor> {
    let mut monitors: Vec<Monitor> = outputs["Outputs"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, o)| o["logical"].is_object())
        .map(|(name, o)| {
            let logical = |key: &str| o["logical"][key].as_f64().unwrap_or(0.0) as f32;
            let mode = o["current_mode"].as_u64().and_then(|i| o["modes"].get(i as usize));
            let size = |key: &str| mode.and_then(|m| m[key].as_u64()).unwrap_or(0) as u32;
            let turned = matches!(
                o["logical"]["transform"].as_str(),
                Some("_90" | "_270" | "Flipped90" | "Flipped270")
            );
            Monitor {
                name: name.clone(),
                x: logical("x"),
                y: logical("y"),
                width: logical("width"),
                height: logical("height"),
                pixels: if turned {
                    (size("height"), size("width"))
                } else {
                    (size("width"), size("height"))
                },
            }
        })
        .collect();
    monitors.sort_by(|a, b| (a.x, a.y).partial_cmp(&(b.x, b.y)).unwrap_or(std::cmp::Ordering::Equal));
    monitors
}

/// What is over each output, from the replies to `Outputs`, `Workspaces` and
/// `Windows`. niri does not say whether a window is fullscreen or maximised,
/// only how large its tile is: one the size of the output is taken for
/// fullscreen, one nearly so for maximised. A workspace scrolls, so of its
/// tiled windows only the one it shows (its active window) counts; floating
/// windows float over it all.
fn over_of(outputs: &Value, workspaces: &Value, windows: &Value) -> Vec<(String, Over)> {
    let workspaces: Vec<&Value> = workspaces["Workspaces"].as_array().into_iter().flatten().collect();
    let windows: Vec<&Value> = windows["Windows"].as_array().into_iter().flatten().collect();
    monitors_of(outputs)
        .into_iter()
        .map(|monitor| {
            let workspace = workspaces.iter().find(|w| {
                w["output"].as_str() == Some(&monitor.name) && w["is_active"].as_bool() == Some(true)
            });
            let shown: Vec<&&Value> = windows
                .iter()
                .filter(|window| {
                    workspace.is_some_and(|ws| {
                        window["workspace_id"] == ws["id"]
                            && (window["is_floating"].as_bool() == Some(true)
                                || window["id"] == ws["active_window_id"])
                    })
                })
                .collect();
            let size = (f64::from(monitor.width), f64::from(monitor.height));
            let covered = shown
                .iter()
                .map(|window| {
                    let tile = &window["layout"]["tile_size"];
                    super::covering((tile[0].as_f64().unwrap_or(0.0), tile[1].as_f64().unwrap_or(0.0)), size)
                })
                .max()
                .unwrap_or(0);
            let focused = windows.iter().any(|window| {
                window["is_focused"].as_bool() == Some(true)
                    && workspaces.iter().any(|ws| {
                        ws["id"] == window["workspace_id"] && ws["output"].as_str() == Some(&monitor.name)
                    })
            });
            (monitor.name, Over { focused, covered, asleep: false })
        })
        .collect()
}

/// Whether an event of the stream may change what covers an output.
fn matters(event: &Value) -> bool {
    let Some(name) = event.as_object().and_then(|o| o.keys().next()) else {
        return false;
    };
    matches!(
        name.as_str(),
        "WorkspacesChanged"
            | "WorkspaceActivated"
            | "WorkspaceActiveWindowChanged"
            | "WindowsChanged"
            | "WindowOpenedOrChanged"
            | "WindowClosed"
            | "WindowFocusChanged"
            | "WindowLayoutsChanged"
    )
}

impl Compositor for Niri {
    fn name(&self) -> &'static str {
        "niri"
    }

    fn version(&self) -> Option<String> {
        self.ask(r#""Version""#)?["Version"].as_str().map(str::to_owned)
    }

    fn monitors(&self) -> Option<Vec<Monitor>> {
        Some(monitors_of(&self.ask(r#""Outputs""#)?))
    }

    fn over(&self) -> Option<Vec<(String, Over)>> {
        let outputs = self.ask(r#""Outputs""#)?;
        let workspaces = self.ask(r#""Workspaces""#)?;
        let windows = self.ask(r#""Windows""#)?;
        Some(over_of(&outputs, &workspaces, &windows))
    }

    /// niri's IPC has no monitor power, and niri has no wlr-output-power.
    fn tells(&self) -> Tells {
        Tells { sleep: false, ..Tells::ALL }
    }

    fn watch(&self, changes: Sender<Change>) {
        let niri = Niri { socket: self.socket.clone() };
        std::thread::spawn(move || {
            loop {
                if let Some(reader) = niri.connect(r#""EventStream""#) {
                    for line in reader.lines().map_while(Result::ok) {
                        let event: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                        if matters(&event) && changes.send(Change::Covers).is_err() {
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    fn autostart_line(&self) -> Option<String> {
        Some(r#"spawn-at-startup "we-linux" "daemon""#.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::super::reply;
    use super::*;

    fn json(name: &str) -> Value {
        let reply: Value = serde_json::from_str(&reply(&format!("niri/{name}"))).unwrap();
        reply["Ok"].clone()
    }

    fn over(windows: &str) -> Over {
        over_of(&json("outputs.json"), &json("workspaces.json"), &json(windows))[0].1
    }

    #[test]
    fn the_output_is_its_logical_size_and_its_mode() {
        // The nested niri of the tests: one window output, turned upside down.
        assert_eq!(
            monitors_of(&json("outputs.json")),
            vec![Monitor {
                name: "winit".into(),
                x: 0.0,
                y: 0.0,
                width: 1912.0,
                height: 1048.0,
                pixels: (1912, 1048)
            }]
        );
    }

    #[test]
    fn a_half_width_column_is_focus_and_no_cover() {
        assert_eq!(over("windows-tiled.json"), Over { focused: true, covered: 0, asleep: false });
    }

    #[test]
    fn a_tile_as_large_as_the_output_is_fullscreen() {
        assert_eq!(over("windows-fullscreen.json").covered, 2);
        // Maximised to the edges with no bar is the whole output too: niri does
        // not tell the two apart.
        assert_eq!(over("windows-maximized-to-edges.json").covered, 2);
    }

    #[test]
    fn a_maximised_column_leaves_the_gaps_and_is_maximised() {
        assert_eq!(over("windows-maximized-column.json").covered, 1);
    }

    #[test]
    fn a_small_floating_window_covers_nothing() {
        assert_eq!(over("windows-floating.json").covered, 0);
    }

    #[test]
    fn the_other_workspace_shows_the_desktop() {
        let over = over_of(
            &json("outputs.json"),
            &json("workspaces-other.json"),
            &json("windows-other-workspace.json"),
        );
        assert_eq!(over[0].1, Over::default());
    }

    #[test]
    fn window_events_matter_and_keyboard_layouts_do_not() {
        let text = reply("niri/events.jsonl");
        let events: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let names = |pick: bool| -> Vec<String> {
            events
                .iter()
                .filter(|e| matters(e) == pick)
                .filter_map(|e| e.as_object()?.keys().next().cloned())
                .collect()
        };
        assert!(names(true).contains(&"WindowLayoutsChanged".to_owned()));
        assert!(names(true).contains(&"WorkspaceActivated".to_owned()));
        assert!(names(false).contains(&"KeyboardLayoutsChanged".to_owned()));
        assert!(names(false).contains(&"ConfigLoaded".to_owned()));
    }
}
