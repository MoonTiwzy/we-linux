//! The workshop items Steam has on disk (app 431960): what "Установлено" shows.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Installed {
    pub id: u64,
    pub dir: PathBuf,
    pub title: String,
    /// scene, video, web, application.
    pub kind: String,
    pub preview: Option<PathBuf>,
    pub tags: Vec<String>,
    /// What "Показать только" asks about, as project.json has it.
    pub approved: bool,
    pub audio_responsive: bool,
    /// It has properties of its own (the scheme colour is every wallpaper's).
    pub customizable: bool,
    /// Everyone, Questionable, Mature.
    pub rating: String,
    /// Seconds since the epoch.
    pub modified: u64,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// The Steam library folders: the client's own and the ones libraryfolders.vdf names.
pub fn libraries() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if let Ok(real) = p.canonicalize()
            && real.join("steamapps").is_dir()
            && !out.contains(&real)
        {
            out.push(real);
        }
    };
    // The client's own folder: native, the old link, Flatpak, Snap.
    let roots = [
        home().join(".local/share/Steam"),
        home().join(".steam/steam"),
        home().join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home().join("snap/steam/common/.local/share/Steam"),
    ];
    for root in roots {
        let vdf = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
        add(root);
        for line in vdf.lines() {
            let mut parts = line.split('"').filter(|p| !p.trim().is_empty());
            if parts.next() == Some("path")
                && let Some(path) = parts.next()
            {
                add(PathBuf::from(path));
            }
        }
    }
    out
}

/// Wallpaper Engine's own assets: without them no scene renders.
pub fn assets() -> Option<PathBuf> {
    libraries()
        .into_iter()
        .map(|l| l.join("steamapps/common/wallpaper_engine/assets"))
        .find(|p| p.is_dir())
}

fn read_item(dir: &Path) -> Option<Installed> {
    let id = dir.file_name()?.to_str()?.parse::<u64>().ok()?;
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("project.json")).ok()?).ok()?;
    let string = |key: &str| json.get(key).and_then(|v| v.as_str()).unwrap_or("").to_owned();
    let preview = Some(dir.join(string("preview"))).filter(|p| p.is_file());
    let modified = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let mut kind = string("type").to_lowercase();
    if kind.is_empty() {
        kind = "scene".into();
    }
    Some(Installed {
        id,
        dir: dir.to_owned(),
        title: Some(string("title")).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| id.to_string()),
        kind,
        preview,
        tags: json
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        approved: json.get("approved").and_then(|v| v.as_bool()).unwrap_or(false),
        audio_responsive: json["general"]["supportsaudioprocessing"].as_bool().unwrap_or(false),
        customizable: json["general"]["properties"].as_object().is_some_and(|p| p.keys().any(|k| k != "schemecolor")),
        rating: Some(string("contentrating")).filter(|r| !r.is_empty()).unwrap_or_else(|| "Everyone".into()),
        modified,
    })
}

pub fn installed() -> Vec<Installed> {
    let mut out = Vec::new();
    for library in libraries() {
        let Ok(dirs) = std::fs::read_dir(library.join(format!("steamapps/workshop/content/{}", crate::steam::APP))) else {
            continue;
        };
        out.extend(dirs.flatten().filter_map(|d| read_item(&d.path())));
    }
    out
}
