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
    /// Whether the renderer can do what it asks; a row it cannot do is shown
    /// greyed, with `hint` saying so.
    pub usable: bool,
    pub hint: String,
    /// Combo choices the renderer can do (empty: all).
    pub usable_options: Vec<bool>,
}

/// Labels Wallpaper Engine translates itself.
fn known(label: &str) -> Option<&'static str> {
    use crate::lang::t;
    Some(match label {
        "ui_browse_properties_scheme_color" => t("Scheme color", "Цветовая схема"),
        "ui_browse_properties_volume" => t("Volume", "Громкость"),
        "ui_browse_properties_playback_rate" => t("Playback rate", "Скорость воспроизведения"),
        "ui_browse_properties_alignment" => t("Alignment", "Выравнивание"),
        "ui_browse_properties_alignment_position" => t("Position", "Позиция"),
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

/// The properties Wallpaper Engine gives every video wallpaper of its own,
/// before the wallpaper's: how it sits on the monitor, how fast and loud it
/// plays, its colours. Of these the renderer does the fill (cover or stretch).
fn video_properties() -> Vec<Property> {
    use crate::lang::t;
    let row = |name: &str, label: &str, kind: Kind, default: Value, min: f64, max: f64| Property {
        name: name.to_owned(),
        label: label.to_owned(),
        kind,
        default,
        min,
        max,
        options: Vec::new(),
        condition: String::new(),
        usable: false,
        hint: t("The renderer does not do this yet.", "Рендерер пока этого не умеет.").to_owned(),
        usable_options: Vec::new(),
    };
    let alignment = Property {
        options: [
            t("Fill", "Заполнение"),
            t("Center", "Центр"),
            t("Cover", "Заполнить"),
            t("Stretch", "Растянуть"),
            t("Free", "Свободный"),
        ]
        .iter()
        .enumerate()
        .map(|(i, label)| ((*label).to_owned(), Value::from(i)))
        .collect(),
        usable: true,
        hint: String::new(),
        usable_options: vec![true, false, true, true, false],
        ..row("alignment", t("Alignment", "Выравнивание"), Kind::Combo, Value::from(0), 0.0, 4.0)
    };
    vec![
        alignment,
        row("alignmentposition", t("Position", "Позиция"), Kind::Slider, Value::from(50), 0.0, 100.0),
        row("alignmentfliph", t("Flip", "Отразить"), Kind::Bool, Value::Bool(false), 0.0, 1.0),
        row(
            "rate",
            t("Playback rate", "Скорость воспроизведения"),
            Kind::Slider,
            Value::from(100),
            25.0,
            200.0,
        ),
        row("volume", t("Volume", "Громкость"), Kind::Slider, Value::from(50), 0.0, 100.0),
        row(
            "showcoloroptions",
            t("Show color options", "Показать настройки цвета"),
            Kind::Bool,
            Value::Bool(false),
            0.0,
            1.0,
        ),
    ]
}

/// What the renderer makes of a wallpaper's "alignment" (the original's
/// choices: fill, center, cover, stretch, free): its fill of the monitor.
pub fn fill_of(alignment: &Value) -> Option<&'static str> {
    match alignment.as_f64().map(|n| n as i64) {
        Some(0 | 2) => Some("cover"),
        Some(3) => Some("stretch"),
        _ => None,
    }
}

pub fn load(dir: &Path) -> Vec<Property> {
    let Ok(bytes) = std::fs::read(dir.join("project.json")) else { return Vec::new() };
    let Ok(json) = serde_json::from_slice::<Value>(&bytes) else { return Vec::new() };
    let builtin = if json["type"].as_str().is_some_and(|t| t.eq_ignore_ascii_case("video")) {
        video_properties()
    } else {
        Vec::new()
    };
    let Some(map) = json.pointer("/general/properties").and_then(Value::as_object) else {
        return builtin;
    };
    let mut list: Vec<(i64, Property)> = map
        .iter()
        // The scheme colour is the editor's: the browser leaves it out.
        .filter(|(name, _)| *name != "schemecolor")
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
                    usable: true,
                    hint: String::new(),
                    usable_options: Vec::new(),
                },
            ))
        })
        .collect();
    list.sort_by_key(|(order, _)| *order);
    builtin.into_iter().chain(list.into_iter().map(|(_, p)| p)).collect()
}

fn loose(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n
            .as_f64()
            .map_or(n.to_string(), |f| if f.fract() == 0.0 { format!("{f:.0}") } else { f.to_string() }),
        _ => String::new(),
    }
}

/// Whether two property values are the same setting: `50` and `50.0` are
/// (serde_json tells an integer from a float), and so are `1` and `"1"`,
/// which a combo's choices and its value are written as in turn.
pub fn same(a: &Value, b: &Value) -> bool {
    let scalar = |v: &Value| v.is_string() || v.is_number() || v.is_boolean();
    a == b || (scalar(a) && scalar(b) && loose(a) == loose(b))
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
    let wanted = if wanted == "1" && value.is_boolean() {
        "true"
    } else if wanted == "0" && value.is_boolean() {
        "false"
    } else {
        wanted
    };
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
    std::fs::read(overrides_file(id)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
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

fn presets_dir(id: u64) -> PathBuf {
    overrides_file(id).with_file_name(format!("../presets/{id}"))
}

/// A name a preset can have as a file: no folders in it.
fn preset_file(id: u64, name: &str) -> Option<PathBuf> {
    let name: String = name.trim().chars().map(|c| if c == '/' || c == '\0' { '_' } else { c }).collect();
    (!name.is_empty() && name != "." && name != "..").then(|| presets_dir(id).join(format!("{name}.json")))
}

/// The names of the presets saved for item `id`, in order.
pub fn presets(id: u64) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(presets_dir(id))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| Some(entry.file_name().to_str()?.strip_suffix(".json")?.to_owned()))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// The values of a preset.
pub fn preset(id: u64, name: &str) -> Option<BTreeMap<String, Value>> {
    serde_json::from_slice(&std::fs::read(preset_file(id, name)?).ok()?).ok()
}

/// Keep the values under a name.
pub fn save_preset(id: u64, name: &str, values: &BTreeMap<String, Value>) -> Result<(), String> {
    let path = preset_file(id, name).ok_or("a preset needs a name")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(values).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// Tell the renderer the new value of a property of `wallpaper`.
pub fn tell_engines(wallpaper: &Path, name: &str, value: &Value) {
    crate::engine::tell_property(wallpaper, name, value);
}

/// "r g b" (0..1 each) -> (r, g, b) 0..255.
pub fn rgb(value: &Value) -> (u8, u8, u8) {
    let parts: Vec<f64> =
        value.as_str().unwrap_or("").split_whitespace().filter_map(|p| p.parse().ok()).collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn same_values() {
        assert!(same(&json!(50), &json!(50.0)));
        assert!(same(&json!(1), &json!("1")));
        assert!(same(&json!("0.2 0.4 0.6"), &json!("0.2 0.4 0.6")));
        assert!(!same(&json!(50), &json!(50.5)));
        assert!(!same(&json!(true), &json!(false)));
        assert!(!same(&json!(null), &json!("")));
        assert!(same(&json!(null), &json!(null)));
    }
}
