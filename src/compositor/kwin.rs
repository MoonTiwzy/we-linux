//! KWin, through D-Bus. KWin has no request that lists the windows; a script
//! loaded into it ("KWin scripting API", develop.kde.org) sees them and calls
//! the daemon over D-Bus whenever what covers a screen may have changed.
//! The monitors are Wayland's (xdg-output), and the picture under the
//! renderer is Plasma's own wallpaper (`org.kde.PlasmaShell.setWallpaper`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedValue;

use super::{Change, Compositor, Monitor, Over, Tells};

const SCRIPT: &str = include_str!("kwin.js");
/// The name KWin knows the script by: loading it again replaces it.
const PLUGIN: &str = "we-linux";
const PATH: &str = "/io/github/MoonTiwzy/WeLinux";
/// Plasma's picture wallpaper.
const IMAGE: &str = "org.kde.image";

pub struct Kwin {
    /// What the script reported last.
    report: Arc<Mutex<Option<Value>>>,
}

impl Kwin {
    /// A Plasma session: KWin is its compositor.
    pub fn find() -> Option<Self> {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok()?;
        desktop
            .split(':')
            .any(|name| name.eq_ignore_ascii_case("KDE"))
            .then(|| Self { report: Arc::new(Mutex::new(None)) })
    }
}

/// The session bus, one connection for all.
fn bus() -> Option<&'static Connection> {
    static BUS: OnceLock<Option<Connection>> = OnceLock::new();
    BUS.get_or_init(|| Connection::session().ok()).as_ref()
}

fn proxy<'a>(bus: &'a Connection, service: &'a str, path: &'a str, interface: &'a str) -> Option<Proxy<'a>> {
    Proxy::new(bus, service, path.to_owned(), interface).ok()
}

/// What the script calls.
struct Listener {
    report: Arc<Mutex<Option<Value>>>,
    changes: Sender<Change>,
}

#[zbus::interface(name = "io.github.MoonTiwzy.WeLinux.Desktop")]
impl Listener {
    fn report(&self, report: String) {
        let Ok(report) = serde_json::from_str::<Value>(&report) else { return };
        if let Ok(mut last) = self.report.lock() {
            *last = Some(report);
        }
        let _ = self.changes.send(Change::Covers);
    }
}

/// Load the script into KWin and run it, the daemon's bus name written in.
fn load(bus: &Connection) -> Option<()> {
    let name = bus.unique_name()?.to_string();
    let file = crate::engine::runtime_dir().join("kwin.js");
    std::fs::create_dir_all(file.parent()?).ok()?;
    std::fs::write(&file, SCRIPT.replace("@DAEMON@", &name)).ok()?;
    let scripting = proxy(bus, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting")?;
    // One an earlier daemon left calls a name that is gone.
    let _: Result<bool, _> = scripting.call("unloadScript", &(PLUGIN,));
    let id: i32 = scripting.call("loadScript", &(file.to_string_lossy().as_ref(), PLUGIN)).ok()?;
    // KWin 6 puts the script at /Scripting/Script<id>, KWin 5 at /<id>.
    [format!("/Scripting/Script{id}"), format!("/{id}")].iter().find_map(|path| {
        let script = Proxy::new(bus, "org.kde.KWin", path.as_str(), "org.kde.kwin.Script").ok()?;
        script.call::<_, _, ()>("run", &()).ok()
    })
}

/// What is over each screen, from a report of the script. A window counts on
/// the screen KWin puts it on (where its middle is) when it is on the
/// screen's virtual desktop and activity and not minimised; KWin's
/// `maximizeMode` 3 is maximised both ways.
fn over_of(report: &Value) -> Vec<(String, Over)> {
    let activity = report["activity"].as_str().unwrap_or("");
    let windows: Vec<&Value> = report["windows"].as_array().into_iter().flatten().collect();
    report["screens"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|screen| {
            let name = screen["name"].as_str().unwrap_or("");
            let desktop = &screen["desktop"];
            let shown: Vec<&&Value> = windows
                .iter()
                .filter(|w| {
                    let listed = |key: &str, current: &Value| {
                        w[key].as_array().is_none_or(|list| list.is_empty() || list.contains(current))
                    };
                    w["output"].as_str() == Some(name)
                        && w["minimized"].as_bool() != Some(true)
                        && w["hidden"].as_bool() != Some(true)
                        && listed("desktops", desktop)
                        && (activity.is_empty() || listed("activities", &Value::from(activity)))
                })
                .collect();
            let covered = shown
                .iter()
                .map(|w| {
                    if w["fullscreen"].as_bool() == Some(true) {
                        2
                    } else {
                        u8::from(w["maximized"].as_bool() == Some(true))
                    }
                })
                .max()
                .unwrap_or(0);
            let over = Over {
                focused: shown.iter().any(|w| w["active"].as_bool() == Some(true)),
                covered,
                asleep: screen["asleep"].as_bool() == Some(true),
            };
            (name.to_owned(), over)
        })
        .collect()
}

/// The picture `org.kde.PlasmaShell.wallpaper` says a screen shows, when it
/// shows a picture.
fn picture_of(wallpaper: &HashMap<String, OwnedValue>) -> Option<PathBuf> {
    let plugin = String::try_from(wallpaper.get("wallpaperPlugin")?.try_clone().ok()?).ok()?;
    if plugin != IMAGE {
        return None;
    }
    let image = String::try_from(wallpaper.get("Image")?.try_clone().ok()?).ok()?;
    Some(PathBuf::from(image.strip_prefix("file://").unwrap_or(&image)))
}

impl Compositor for Kwin {
    fn name(&self) -> &'static str {
        "KWin"
    }

    fn version(&self) -> Option<String> {
        let kwin = proxy(bus()?, "org.kde.KWin", "/KWin", "org.kde.KWin")?;
        let support: String = kwin.call("supportInformation", &()).ok()?;
        support.lines().find_map(|line| line.strip_prefix("KWin version: ")).map(str::to_owned)
    }

    fn monitors(&self) -> Option<Vec<Monitor>> {
        super::wayland::monitors()
    }

    /// What the script said last; nothing before the watch loaded it.
    fn over(&self) -> Option<Vec<(String, Over)>> {
        self.report.lock().ok()?.as_ref().map(over_of)
    }

    fn tells(&self) -> Tells {
        Tells::ALL
    }

    fn watch(&self, changes: Sender<Change>) {
        let Some(bus) = bus() else { return };
        let listener = Listener { report: Arc::clone(&self.report), changes };
        if bus.object_server().at(PATH, listener).is_err() || load(bus).is_none() {
            eprintln!("we-linux: KWin did not take the script that says which windows cover the screens");
        }
        // KWin started anew (it crashed, or was replaced) has no script.
        std::thread::spawn(move || {
            let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(bus) else { return };
            let Ok(owners) = dbus.receive_name_owner_changed() else { return };
            for change in owners {
                if let Ok(args) = change.args()
                    && args.name() == "org.kde.KWin"
                    && args.new_owner().is_some()
                {
                    let _ = load(bus);
                }
            }
        });
    }

    fn leave(&self) {
        if let Some(scripting) =
            bus().and_then(|bus| proxy(bus, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting"))
        {
            let _: Result<bool, _> = scripting.call("unloadScript", &(PLUGIN,));
        }
    }

    /// Plasma's desktop is a window under everything in KWin's desktop layer,
    /// with the background layer-shell layer: on that layer the renderer would
    /// be under Plasma's own wallpaper. On the bottom layer it is over it, and
    /// over the desktop's icons too.
    fn layer(&self) -> &'static str {
        "bottom"
    }

    fn show_picture(&self, image: &Path, monitor: Option<&str>) -> bool {
        let Some(shell) =
            bus().and_then(|bus| proxy(bus, "org.kde.plasmashell", "/PlasmaShell", "org.kde.PlasmaShell"))
        else {
            return false;
        };
        let url = format!("file://{}", image.display());
        let screens: Vec<u32> = match monitor {
            Some(screen) => screen.parse().ok().into_iter().collect(),
            None => (0..self.monitors().map_or(1, |m| m.len().max(1)) as u32).collect(),
        };
        let mut shown = false;
        for screen in screens {
            let parameters = HashMap::from([("Image", zbus::zvariant::Value::from(url.as_str()))]);
            shown |= shell.call::<_, _, ()>("setWallpaper", &(IMAGE, parameters, screen)).is_ok();
        }
        shown
    }

    /// Plasma's screens by number, the way `setWallpaper` takes them.
    fn pictures(&self) -> Vec<(String, PathBuf)> {
        let Some(shell) =
            bus().and_then(|bus| proxy(bus, "org.kde.plasmashell", "/PlasmaShell", "org.kde.PlasmaShell"))
        else {
            return Vec::new();
        };
        (0..self.monitors().map_or(1, |m| m.len().max(1)) as u32)
            .filter_map(|screen| {
                let wallpaper: HashMap<String, OwnedValue> = shell.call("wallpaper", &(screen,)).ok()?;
                Some((screen.to_string(), picture_of(&wallpaper)?))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::reply;
    use super::*;

    fn over(name: &str) -> Over {
        let report: Value = serde_json::from_str(&reply(&format!("kwin/{name}"))).unwrap();
        let over = over_of(&report);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].0, "Virtual-0");
        over[0].1
    }

    #[test]
    fn a_window_in_the_middle_of_the_screen_has_the_focus_and_covers_nothing() {
        assert_eq!(over("report-normal.json"), Over { focused: true, covered: 0, asleep: false });
    }

    #[test]
    fn fullscreen_and_maximised_are_what_kwin_says() {
        assert_eq!(over("report-fullscreen.json").covered, 2);
        assert_eq!(over("report-maximized.json").covered, 1);
    }

    #[test]
    fn a_minimised_window_and_one_on_another_desktop_are_not_on_the_screen() {
        assert_eq!(over("report-minimized.json"), Over::default());
        assert_eq!(over("report-other-desktop.json"), Over::default());
    }

    #[test]
    fn a_window_of_another_activity_is_not_on_the_screen_and_a_sleeping_screen_says_so() {
        let mut report: Value = serde_json::from_str(&reply("kwin/report-fullscreen.json")).unwrap();
        report["activity"] = "a".into();
        report["windows"][0]["activities"] = serde_json::json!(["b"]);
        report["screens"][0]["asleep"] = true.into();
        assert_eq!(over_of(&report)[0].1, Over { focused: false, covered: 0, asleep: true });
    }

    #[test]
    fn the_script_is_given_the_daemons_name() {
        assert!(SCRIPT.contains(r#"const daemon = "@DAEMON@";"#));
        assert!(SCRIPT.contains(PATH));
        assert!(SCRIPT.contains("io.github.MoonTiwzy.WeLinux.Desktop"));
    }

    #[test]
    fn plasma_shows_a_picture_only_with_its_picture_wallpaper() {
        let value = |text: &str| OwnedValue::try_from(zbus::zvariant::Value::from(text)).unwrap();
        let picture = HashMap::from([
            ("wallpaperPlugin".to_owned(), value(IMAGE)),
            ("Image".to_owned(), value("file:///usr/share/wallpapers/Next/contents/images/5120x2880.png")),
        ]);
        assert_eq!(
            picture_of(&picture),
            Some(PathBuf::from("/usr/share/wallpapers/Next/contents/images/5120x2880.png"))
        );
        let colour = HashMap::from([("wallpaperPlugin".to_owned(), value("org.kde.color"))]);
        assert_eq!(picture_of(&colour), None);
    }
}
