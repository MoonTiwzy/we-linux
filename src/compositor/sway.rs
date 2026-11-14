//! Sway, through its i3 IPC socket (sway-ipc(7)): a message is `i3-ipc`, its
//! payload's length and its type as native 32-bit numbers, then the payload; a
//! reply is the same with a JSON payload. A connection that subscribed gets
//! events, their type with the highest bit set.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::Value;

use super::{Change, Compositor, Monitor, Over, Tells};

const RUN_COMMAND: u32 = 0;
const SUBSCRIBE: u32 = 2;
const GET_OUTPUTS: u32 = 3;
const GET_TREE: u32 = 4;
const GET_VERSION: u32 = 7;
const GET_CONFIG: u32 = 9;
const MAGIC: &[u8] = b"i3-ipc";

pub struct Sway {
    socket: PathBuf,
}

/// A message of type `kind`.
fn message(kind: u32, payload: &str) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    bytes.extend_from_slice(&kind.to_ne_bytes());
    bytes.extend_from_slice(payload.as_bytes());
    bytes
}

/// The next message on `stream`: its type and payload.
fn receive(stream: &mut impl Read) -> Option<(u32, Vec<u8>)> {
    let mut head = [0u8; 14];
    stream.read_exact(&mut head).ok()?;
    if &head[..6] != MAGIC {
        return None;
    }
    let length = u32::from_ne_bytes(head[6..10].try_into().ok()?) as usize;
    let kind = u32::from_ne_bytes(head[10..14].try_into().ok()?);
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).ok()?;
    Some((kind, payload))
}

impl Sway {
    pub fn find() -> Option<Self> {
        let socket = PathBuf::from(std::env::var_os("SWAYSOCK")?);
        UnixStream::connect(&socket).is_ok().then_some(Self { socket })
    }

    fn connect(&self) -> Option<UnixStream> {
        let stream = UnixStream::connect(&self.socket).ok()?;
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        Some(stream)
    }

    fn ask(&self, kind: u32, payload: &str) -> Option<Value> {
        let mut stream = self.connect()?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        stream.write_all(&message(kind, payload)).ok()?;
        let (_, reply) = receive(&mut stream)?;
        serde_json::from_slice(&reply).ok()
    }

    /// Run sway commands; whether every one of them worked.
    fn run(&self, command: &str) -> bool {
        self.ask(RUN_COMMAND, command).and_then(|reply| reply.as_array().cloned()).is_some_and(|replies| {
            !replies.is_empty() && replies.iter().all(|r| r["success"].as_bool() == Some(true))
        })
    }
}

/// The outputs that are on: `rect` is logical, `current_mode` the mode.
fn monitors_of(outputs: &Value) -> Vec<Monitor> {
    outputs
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["active"].as_bool() != Some(false))
        .map(|o| {
            let rect = |key: &str| o["rect"][key].as_f64().unwrap_or(0.0) as f32;
            let mode = |key: &str| o["current_mode"][key].as_u64().unwrap_or(0) as u32;
            let turned = matches!(o["transform"].as_str(), Some("90" | "270" | "flipped-90" | "flipped-270"));
            Monitor {
                name: o["name"].as_str().unwrap_or("").to_owned(),
                x: rect("x"),
                y: rect("y"),
                width: rect("width"),
                height: rect("height"),
                pixels: if turned {
                    (mode("height"), mode("width"))
                } else {
                    (mode("width"), mode("height"))
                },
            }
        })
        .collect()
}

/// The windows under a node: the nodes without children of their own that
/// are views (they have a pid), each with whether it floats.
fn windows<'a>(node: &'a Value, floating: bool, into: &mut Vec<(&'a Value, bool)>) {
    let tiled = node["nodes"].as_array().into_iter().flatten();
    let floats = node["floating_nodes"].as_array().into_iter().flatten();
    let mut leaf = true;
    for child in tiled {
        leaf = false;
        windows(child, floating, into);
    }
    for child in floats {
        leaf = false;
        windows(child, true, into);
    }
    if leaf && node["pid"].is_number() {
        into.push((node, floating));
    }
}

/// What is over each output, from GET_OUTPUTS and GET_TREE. A tiled window
/// fills the workspace's tiling area, so any one covers the desktop as a
/// maximised one does; a floating window has to be about as large as the
/// output. Sway's own fullscreen (mode 1) is that output's; global fullscreen
/// (mode 2) covers every output.
fn over_of(outputs: &Value, tree: &Value) -> Vec<(String, Over)> {
    let outputs: Vec<&Value> =
        outputs.as_array().into_iter().flatten().filter(|o| o["active"].as_bool() != Some(false)).collect();
    let mut all = Vec::new();
    windows(tree, false, &mut all);
    let global = all.iter().any(|(w, _)| w["fullscreen_mode"].as_i64() == Some(2));
    let nodes = tree["nodes"].as_array().cloned().unwrap_or_default();
    outputs
        .into_iter()
        .map(|output| {
            let name = output["name"].as_str().unwrap_or("").to_owned();
            let size = (
                output["rect"]["width"].as_f64().unwrap_or(0.0),
                output["rect"]["height"].as_f64().unwrap_or(0.0),
            );
            let mut shown = Vec::new();
            let mut fullscreen = global;
            if let Some(node) = nodes.iter().find(|n| n["name"].as_str() == Some(&name))
                && let Some(workspace) = node["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|w| w["name"].as_str().is_some() && w["name"] == output["current_workspace"])
            {
                windows(workspace, false, &mut shown);
                // A container can be fullscreen too, with the windows in it.
                let mut stack = vec![workspace];
                while let Some(node) = stack.pop() {
                    stack.extend(node["nodes"].as_array().into_iter().flatten());
                    stack.extend(node["floating_nodes"].as_array().into_iter().flatten());
                    fullscreen |=
                        !std::ptr::eq(node, workspace) && node["fullscreen_mode"].as_i64() == Some(1);
                }
            }
            let covered = if fullscreen {
                2
            } else {
                shown
                    .iter()
                    .map(|(window, floating)| {
                        if !floating {
                            return 1;
                        }
                        let rect = |key: &str| window["rect"][key].as_f64().unwrap_or(0.0);
                        super::covering((rect("width"), rect("height")), size).min(1)
                    })
                    .max()
                    .unwrap_or(0)
            };
            let over = Over {
                focused: shown.iter().any(|(window, _)| window["focused"].as_bool() == Some(true)),
                covered,
                asleep: output["power"].as_bool().or(output["dpms"].as_bool()) == Some(false),
            };
            (name, over)
        })
        .collect()
}

/// What an event means to the daemon: a reload of the config starts swaybg
/// anew, which puts it over the renderer.
fn change_of(event: &Value) -> Change {
    if event["change"].as_str() == Some("reload") { Change::Layers } else { Change::Covers }
}

/// The pictures of the config's `output ... bg` lines, its `set $name value`
/// variables put in: what sway showed before anything was set at run time.
fn config_pictures(config: &str) -> Vec<(String, PathBuf)> {
    let mut variables: Vec<(String, String)> = Vec::new();
    let mut pictures = Vec::new();
    for line in config.lines().map(str::trim) {
        let mut words = split(line);
        // A variable is put in everywhere but in the name a `set` gives.
        let from = if words.first().is_some_and(|w| w == "set") { 2 } else { 0 };
        for word in words.iter_mut().skip(from) {
            for (name, value) in &variables {
                *word = word.replace(name.as_str(), value);
            }
        }
        match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["set", name, value @ ..] if name.starts_with('$') => {
                variables.push(((*name).to_owned(), value.join(" ")));
                // The longest names first: $wallpaper before $wall.
                variables.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
            }
            ["output", output, "bg" | "background", image, ..] if !image.starts_with('#') => {
                let image = image.replacen('~', &std::env::var("HOME").unwrap_or_default(), 1);
                pictures.push(((*output).to_owned(), PathBuf::from(image)));
            }
            _ => {}
        }
    }
    pictures
}

/// A config line's words, a quoted one as one.
fn split(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    for c in line.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, ' ' | '\t') => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

impl Compositor for Sway {
    fn name(&self) -> &'static str {
        "Sway"
    }

    fn version(&self) -> Option<String> {
        self.ask(GET_VERSION, "")?["human_readable"].as_str().map(str::to_owned)
    }

    fn monitors(&self) -> Option<Vec<Monitor>> {
        Some(monitors_of(&self.ask(GET_OUTPUTS, "")?))
    }

    fn over(&self) -> Option<Vec<(String, Over)>> {
        Some(over_of(&self.ask(GET_OUTPUTS, "")?, &self.ask(GET_TREE, "")?))
    }

    fn tells(&self) -> Tells {
        Tells::ALL
    }

    fn watch(&self, changes: Sender<Change>) {
        let sway = Sway { socket: self.socket.clone() };
        std::thread::spawn(move || {
            loop {
                if let Some(mut stream) = sway.connect()
                    && stream.write_all(&message(SUBSCRIBE, r#"["window","workspace","output"]"#)).is_ok()
                {
                    while let Some((kind, payload)) = receive(&mut stream) {
                        // The answer to the subscription itself is no event.
                        if kind & 0x8000_0000 == 0 {
                            continue;
                        }
                        let event: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
                        if changes.send(change_of(&event)).is_err() {
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    /// Sway's `output bg` runs swaybg anew for the picture.
    fn show_picture(&self, image: &Path, monitor: Option<&str>) -> bool {
        let path = image.to_string_lossy();
        let output = monitor.unwrap_or("*");
        // Sway's quoting has no escape for a quote inside a quoted word.
        if path.contains('"') || output.contains('"') {
            return false;
        }
        self.run(&format!(r#"output "{output}" bg "{path}" fill"#))
    }

    /// What the config gives swaybg: sway does not say what it shows now.
    fn pictures(&self) -> Vec<(String, PathBuf)> {
        self.ask(GET_CONFIG, "")
            .and_then(|reply| reply["config"].as_str().map(config_pictures))
            .unwrap_or_default()
    }

    fn picture_covers(&self) -> bool {
        true
    }

    fn autostart_line(&self) -> Option<String> {
        Some("exec we-linux daemon".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::super::reply;
    use super::*;

    fn json(name: &str) -> Value {
        serde_json::from_str(&reply(&format!("sway/{name}"))).unwrap()
    }

    #[test]
    fn a_message_is_the_magic_the_length_the_type_and_the_payload() {
        // sway-ipc(7)'s own example: `exit` as RUN_COMMAND.
        let bytes = message(RUN_COMMAND, "exit");
        assert_eq!(&bytes[..6], b"i3-ipc");
        assert_eq!(u32::from_ne_bytes(bytes[6..10].try_into().unwrap()), 4);
        assert_eq!(u32::from_ne_bytes(bytes[10..14].try_into().unwrap()), 0);
        assert_eq!(&bytes[14..], b"exit");
        assert_eq!(receive(&mut bytes.as_slice()), Some((0, b"exit".to_vec())));
        assert_eq!(receive(&mut &b"i3-ip"[..]), None);
    }

    #[test]
    fn outputs_are_where_their_rect_puts_them() {
        let monitors = monitors_of(&json("outputs.json"));
        let names: Vec<(&str, f32, f32, (u32, u32))> =
            monitors.iter().map(|m| (m.name.as_str(), m.x, m.width, m.pixels)).collect();
        assert_eq!(
            names,
            vec![("HEADLESS-2", 1920.0, 1280.0, (1280, 1024)), ("HEADLESS-1", 0.0, 1920.0, (1920, 1080))]
        );
    }

    #[test]
    fn a_tiled_window_covers_its_output_as_a_maximised_one() {
        let over = over_of(&json("outputs.json"), &json("tree-tiled.json"));
        assert_eq!(
            over,
            vec![
                ("HEADLESS-2".into(), Over { focused: true, covered: 1, asleep: false }),
                ("HEADLESS-1".into(), Over::default()),
            ]
        );
    }

    #[test]
    fn a_fullscreen_window_and_a_small_floating_one() {
        let fullscreen = over_of(&json("outputs.json"), &json("tree-fullscreen.json"));
        assert_eq!(fullscreen[0].1.covered, 2);
        assert_eq!(fullscreen[1].1.covered, 0);
        let floating = over_of(&json("outputs.json"), &json("tree-floating.json"));
        assert_eq!(floating[0].1, Over { focused: true, covered: 0, asleep: false });
    }

    #[test]
    fn the_focus_on_another_output_is_not_this_ones() {
        // Workspace 2, empty, on HEADLESS-1 has the focus; the window stays on HEADLESS-2.
        let over = over_of(&json("outputs.json"), &json("tree-other-workspace.json"));
        assert!(!over[0].1.focused && !over[1].1.focused);
    }

    #[test]
    fn an_output_whose_power_is_off_sleeps() {
        let over = over_of(&json("outputs-power-off.json"), &json("tree-floating.json"));
        assert!(over[0].1.asleep);
        assert!(!over[1].1.asleep);
    }

    #[test]
    fn a_reload_puts_swaybg_over_the_renderer_and_the_rest_may_change_the_cover() {
        let changes: Vec<Change> = reply("sway/events.jsonl")
            .lines()
            .map(|line| change_of(&serde_json::from_str(line).unwrap()))
            .collect();
        assert_eq!(changes.last(), Some(&Change::Layers));
        assert!(changes[..changes.len() - 1].iter().all(|c| *c == Change::Covers));
    }

    #[test]
    fn the_pictures_of_the_config_with_its_variables() {
        // The config sway installs: one picture for every output.
        let pictures = config_pictures(&reply("sway/config"));
        assert_eq!(
            pictures,
            vec![(
                "*".into(),
                PathBuf::from("/usr/share/backgrounds/sway/Sway_Wallpaper_Blue_1920x1080.png")
            )]
        );
        let own = "set $wall ~/Pictures\nset $wallpaper $wall/lake.png\noutput DP-1 bg $wallpaper fill\n\
                   output \"HDMI-A-1\" background \"/a b/c.jpg\" fit\noutput * bg #000000 solid_color";
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(
            config_pictures(own),
            vec![
                ("DP-1".into(), PathBuf::from(format!("{home}/Pictures/lake.png"))),
                ("HDMI-A-1".into(), PathBuf::from("/a b/c.jpg")),
            ]
        );
    }
}
