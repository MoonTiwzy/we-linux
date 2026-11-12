//! User properties of a wallpaper ("Свойства"): what its project.json offers,
//! what the user changed (kept per item), and telling the running engines.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Bool,
    Slider,
    Color,
    Combo,
    Text,
    Label,
    Group,
}

#[derive(Clone, Debug)]
pub struct Property {
    pub name: String,
    pub label: String,
    pub kind: Kind,
    pub default: Value,
    pub min: f64,
    pub max: f64,
    /// Combo choices: label and value.
    pub options: Vec<(String, Value)>,
    /// "other.value == something": shown only while it holds.
    pub condition: String,
}

/// Labels Wallpaper Engine translates itself.
fn known(label: &str) -> Option<&'static str> {
    use crate::lang::t;
    Some(match label {
        "ui_browse_properties_scheme_color" => t("Scheme color", "Цвет схемы"),
        "ui_browse_properties_volume" => t("Volume", "Громкость"),
        "ui_browse_properties_playback_rate" => t("Playback rate", "Скорость воспроизведения"),
        "ui_browse_properties_alignment" => t("Alignment", "Выравнивание"),
        "ui_browse_properties_position" => t("Position", "Положение"),
        _ => return None,
    })
}

/// A label without its HTML.
fn clean(label: &str) -> String {
    if let Some(text) = known(label.trim()) {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut tag = false;
    for c in label.replace("<br>", "\n").replace("<br/>", "\n").chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").replace("&amp;", "&").trim().to_owned()
}

pub fn load(dir: &Path) -> Vec<Property> {
    let Ok(bytes) = std::fs::read(dir.join("project.json")) else { return Vec::new() };
    let Ok(json) = serde_json::from_slice::<Value>(&bytes) else { return Vec::new() };
    let Some(map) = json.pointer("/general/properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut list: Vec<(i64, Property)> = map
        .iter()
        .filter_map(|(name, p)| {
            let kind = match p.get("type").and_then(Value::as_str).unwrap_or("text") {
                "bool" => Kind::Bool,
                "slider" => Kind::Slider,
                "color" => Kind::Color,
                "combo" => Kind::Combo,
                "textinput" => Kind::Text,
                "group" => Kind::Group,
                "text" => Kind::Label,
                _ => return None,
            };
            let label = clean(p.get("text").and_then(Value::as_str).unwrap_or(name));
            if label.is_empty() || (kind == Kind::Label && label.len() > 400) {
                return None;
            }
            let number = |key: &str, or: f64| p.get(key).and_then(Value::as_f64).unwrap_or(or);
            Some((
                p.get("order").and_then(Value::as_i64).unwrap_or(i64::MAX),
                Property {
                    name: name.clone(),
                    label,
                    kind,
                    default: p.get("value").cloned().unwrap_or(Value::Null),
                    min: number("min", 0.0),
                    max: number("max", 1.0),
                    options: p
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .map(|o| {
                                    (
                                        clean(o.get("label").and_then(Value::as_str).unwrap_or("")),
                                        o.get("value").cloned().unwrap_or(Value::Null),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    condition: p.get("condition").and_then(Value::as_str).unwrap_or("").to_owned(),
                },
            ))
        })
        .collect();
    list.sort_by_key(|(order, _)| *order);
    list.into_iter().map(|(_, p)| p).collect()
}

fn loose(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.as_f64().map_or(n.to_string(), |f| if f.fract() == 0.0 { format!("{f:.0}") } else { f.to_string() }),
        _ => String::new(),
    }
}

/// Whether "name.value == x" (or !=) holds for `values`; anything else: yes.
pub fn holds(condition: &str, values: &BTreeMap<String, Value>) -> bool {
    let condition = condition.trim();
    let (negate, parts) = match (condition.split_once("=="), condition.split_once("!=")) {
        (Some(p), _) if !p.0.contains('!') => (false, p),
        (_, Some(p)) => (true, p),
        _ => return true,
    };
    let Some(name) = parts.0.trim().strip_suffix(".value") else { return true };
    if condition.contains("&&") || condition.contains("||") {
        return true;
    }
    let Some(value) = values.get(name.trim()) else { return true };
    let wanted = parts.1.trim().trim_matches(|c| c == '\'' || c == '"');
    let wanted = if wanted == "1" && value.is_boolean() { "true" } else if wanted == "0" && value.is_boolean() { "false" } else { wanted };
    (loose(value) == wanted) != negate
}

fn overrides_file(id: u64) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join(format!("we-linux/properties/{id}.json"))
}

/// What the user changed for item `id`: name -> value. The daemon hands
/// them to the engine when it starts.
pub fn overrides(id: u64) -> BTreeMap<String, Value> {
    std::fs::read(overrides_file(id))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_overrides(id: u64, values: &BTreeMap<String, Value>) {
    let path = overrides_file(id);
    if values.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(values) {
        let _ = std::fs::write(path, json);
    }
}

/// Tell every running engine the new value of a property.
pub fn tell_engines(name: &str, value: &Value) {
    let Some(penguinctl) = crate::engine::bin("penguinctl") else { return };
    let text = value.to_string();
    for socket in crate::engine::sockets() {
        let child = std::process::Command::new(&penguinctl)
            .arg("--socket")
            .arg(&socket)
            .args(["property", name, &text])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        // Waited for, off the interface's thread: a child nobody waits for
        // stays in the process table for as long as the window is open.
        if let Ok(mut child) = child {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

/// "r g b" (0..1 each) -> (r, g, b) 0..255.
pub fn rgb(value: &Value) -> (u8, u8, u8) {
    let parts: Vec<f64> = value.as_str().unwrap_or("").split_whitespace().filter_map(|p| p.parse().ok()).collect();
    let c = |i: usize| (parts.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0) * 255.0).round() as u8;
    (c(0), c(1), c(2))
}

pub fn hex(value: &Value) -> String {
    let (r, g, b) = rgb(value);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// "#rrggbb" -> "r g b" (0..1 each).
pub fn from_hex(text: &str) -> Option<Value> {
    let digits = text.trim().trim_start_matches('#');
    if digits.len() != 6 {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok().map(|v| f64::from(v) / 255.0);
    Some(Value::String(format!("{:.4} {:.4} {:.4}", c(0)?, c(2)?, c(4)?)))
}
