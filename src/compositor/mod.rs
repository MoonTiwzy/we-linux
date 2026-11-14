//! The compositor: what the daemon and the window ask it and tell it.
//!
//! Each compositor has its own way of saying which monitors there are and what
//! covers them, and of being told things: Hyprland its two sockets, Sway the i3
//! IPC socket, niri its JSON socket, KWin D-Bus and a script of its own. Anything
//! else is asked through the Wayland protocols wlroots compositors share. One of
//! them is chosen when first needed, from the session's environment
//! (`WE_LINUX_COMPOSITOR` names one instead), and nothing outside this module
//! knows which.

mod hyprland;
mod kwin;
mod niri;
mod sway;
mod wayland;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::Sender;

/// A monitor as the compositor lays it out.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: String,
    /// Where it stands on the desk and how large it is there, in the
    /// compositor's logical pixels.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Its mode in device pixels, turned as the monitor is.
    pub pixels: (u32, u32),
}

/// What is over a monitor's desktop.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Over {
    /// A window has the focus there.
    pub focused: bool,
    /// 0 nothing, 1 a window maximised over it, 2 a fullscreen window.
    pub covered: u8,
    /// The monitor sleeps (its power is off).
    pub asleep: bool,
}

/// What a compositor can say of [`Over`]: a setting about what it cannot say
/// does nothing there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tells {
    pub focus: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub sleep: bool,
}

impl Tells {
    const ALL: Tells = Tells { focus: true, maximized: true, fullscreen: true, sleep: true };
    const NOTHING: Tells = Tells { focus: false, maximized: false, fullscreen: false, sleep: false };
}

/// What the compositor told the daemon's watch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    /// Something that may change what covers a monitor, or which monitors there are.
    Covers,
    /// Another program's layer may now be over the renderer's: it makes its
    /// surfaces anew, which puts them on top.
    Layers,
}

pub trait Compositor: Send + Sync {
    /// Its name, for `we-linux doctor`.
    fn name(&self) -> &'static str;

    /// Its version, as it says it.
    fn version(&self) -> Option<String> {
        None
    }

    /// Its monitors, or None when it cannot be asked.
    fn monitors(&self) -> Option<Vec<Monitor>>;

    /// What is over each monitor's desktop, or None when it cannot say.
    fn over(&self) -> Option<Vec<(String, Over)>>;

    /// What of that it can say.
    fn tells(&self) -> Tells;

    /// Hear of changes from now on, on threads of its own.
    fn watch(&self, changes: Sender<Change>);

    /// The daemon is going: what it set up in the compositor goes with it.
    fn leave(&self) {}

    /// The layer-shell layer the renderer's surfaces go on.
    fn layer(&self) -> &'static str {
        "background"
    }

    /// Keep the renderer's layer over other programs' background layers, where
    /// the compositor orders them by anything but who came last.
    fn keep_on_top(&self) {}

    /// Show a picture with the desktop's own wallpaper program, on one monitor
    /// or on all. False when there is none.
    fn show_picture(&self, _image: &Path, _monitor: Option<&str>) -> bool {
        false
    }

    /// What the desktop's own wallpaper program shows: monitor and picture.
    fn pictures(&self) -> Vec<(String, PathBuf)> {
        Vec::new()
    }

    /// Whether a picture shown with [`Compositor::show_picture`] lands over the
    /// renderer's layer (its program starts anew for it, and the newest layer
    /// is on top).
    fn picture_covers(&self) -> bool {
        false
    }

    /// The line of the compositor's own config that starts the daemon with the
    /// session, for a session that does not start systemd's
    /// graphical-session.target.
    fn autostart_line(&self) -> Option<String> {
        None
    }
}

/// The compositor of this session.
pub fn current() -> &'static dyn Compositor {
    static CURRENT: OnceLock<Box<dyn Compositor>> = OnceLock::new();
    CURRENT.get_or_init(|| find(std::env::var("WE_LINUX_COMPOSITOR").ok().as_deref())).as_ref()
}

/// The compositor named, else the first whose interface the session has.
fn find(named: Option<&str>) -> Box<dyn Compositor> {
    let hyprland = || hyprland::Hyprland::find().map(|c| Box::new(c) as Box<dyn Compositor>);
    let sway = || sway::Sway::find().map(|c| Box::new(c) as Box<dyn Compositor>);
    let niri = || niri::Niri::find().map(|c| Box::new(c) as Box<dyn Compositor>);
    let kwin = || kwin::Kwin::find().map(|c| Box::new(c) as Box<dyn Compositor>);
    let found = match named {
        Some("hyprland") => hyprland(),
        Some("sway") => sway(),
        Some("niri") => niri(),
        Some("kwin") => kwin(),
        Some("wayland") => None,
        _ => hyprland().or_else(sway).or_else(niri).or_else(kwin),
    };
    found.unwrap_or_else(|| Box::new(wayland::Wayland::default()))
}

/// How a window of `window` logical pixels covers a monitor of `monitor`: 2
/// when it is as large as the monitor, 1 when it leaves less than a tenth of
/// it each way (a maximised window, give or take a bar and the gaps), else 0.
/// For compositors that say how large a window is but not whether it is
/// fullscreen.
fn covering(window: (f64, f64), monitor: (f64, f64)) -> u8 {
    let (width, height) = window;
    let (full_width, full_height) = monitor;
    if full_width <= 0.0 || full_height <= 0.0 {
        return 0;
    }
    if width >= full_width - 1.0 && height >= full_height - 1.0 {
        2
    } else if width >= full_width * 0.9 && height >= full_height * 0.9 {
        1
    } else {
        0
    }
}

/// Whether systemd's graphical-session.target is up in this session: a user
/// service wanted by it starts with the next session too.
pub fn session_target() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "graphical-session.target"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Read a compositor's reply saved from a real session, for the tests.
#[cfg(test)]
fn reply(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/compositor/replies").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_the_size_of_the_monitor_is_fullscreen_and_one_nearly_so_maximised() {
        let monitor = (1912.0, 1048.0);
        assert_eq!(covering((1912.0, 1048.0), monitor), 2);
        // niri's maximised column: the gaps around it.
        assert_eq!(covering((1880.0, 1016.0), monitor), 1);
        // Half a screen, niri's default column.
        assert_eq!(covering((932.0, 1016.0), monitor), 0);
        assert_eq!(covering((1912.0, 1048.0), (0.0, 0.0)), 0);
    }

    #[test]
    fn a_name_nothing_answers_to_is_the_wayland_protocols() {
        assert_eq!(find(Some("wayland")).name(), "Wayland");
    }
}
