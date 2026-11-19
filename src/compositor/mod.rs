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

use std::collections::HashSet;
use std::io::BufRead;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

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
    /// The compositor went (it exited or crashed): the monitors went with it,
    /// and nothing more is heard from it.
    Gone,
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

    /// Hear of changes from now on, on threads of its own, and of the
    /// compositor going ([`Change::Gone`]) where its interface tells.
    fn watch(&self, changes: Sender<Change>);

    /// Whether the compositor is gone already, where its interface tells (as
    /// [`Compositor::watch`] would tell it went).
    fn gone(&self) -> bool {
        false
    }

    /// What the user's hook is to find in its environment to talk to this
    /// compositor, where the daemon's own environment may say otherwise.
    fn environment(&self) -> Vec<(&'static str, std::ffi::OsString)> {
        Vec::new()
    }

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

    /// Whether [`Compositor::pictures`] is what is on the monitors now, and
    /// not what a config gives the wallpaper program when it starts.
    fn pictures_are_shown(&self) -> bool {
        true
    }

    /// Whether `name`, the monitor a picture is kept for (as
    /// [`Compositor::pictures`], awww or swww name it), is one of `monitors`,
    /// the monitors there are now. A name it cannot tell is there.
    fn has_monitor(&self, name: &str, monitors: &[Monitor]) -> bool {
        among(name, monitors)
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

/// Wait, no longer than `limit`, for the compositor that is still to come: the
/// daemon starts with the session, or anew after its compositor went (systemd
/// starts it again a moment later, while start-hyprland or the session is
/// still starting the compositor anew). In the moment before Hyprland's socket
/// answers the daemon would take the Wayland protocols for good, and with
/// nothing on the session's display it has nothing to draw on. Returns at once
/// when the session's compositor answers.
pub fn settle(limit: Duration) {
    let hyprland = std::env::var("WE_LINUX_COMPOSITOR").ok().is_none_or(|named| named == "hyprland");
    let since = Instant::now();
    let mut said = None;
    while since.elapsed() < limit {
        let Some(why) = waiting(|| hyprland && hyprland::starting(), display_answers) else {
            return;
        };
        if said != Some(why) {
            crate::log::info(why);
            said = Some(why);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// What the daemon waits for, when it does: a Hyprland that is starting, else
/// anything on the session's display.
fn waiting(starting: impl Fn() -> bool, display_answers: impl Fn() -> bool) -> Option<&'static str> {
    if starting() {
        Some("Hyprland is starting: the daemon waits for it to answer")
    } else if !display_answers() {
        Some("nothing answers on the session's Wayland display yet: the daemon waits for a compositor")
    } else {
        None
    }
}

/// Whether a failure to connect to a compositor's socket says that it is gone:
/// nothing listens there (it crashed and left the file) or the socket is not
/// there (it took it with it). Any other failure (no file descriptors or
/// memory left, a full queue of connections) says nothing of the compositor.
fn gone(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound)
}

/// Whether the compositor listening on `socket` is gone, as [`gone`] tells.
fn socket_gone(socket: &Path) -> bool {
    UnixStream::connect(socket).err().is_some_and(|error| gone(&error))
}

/// Whether one of `monitors` has the name `name` (a connector's, "DP-1").
fn among(name: &str, monitors: &[Monitor]) -> bool {
    monitors.iter().any(|monitor| monitor.name == name)
}

/// The lines of a stream of events, until it ends or fails. A line that is not
/// UTF-8 (a window title of other bytes) is read as well as it goes, not taken
/// for the end of the stream.
fn lines(mut reader: impl BufRead) -> impl Iterator<Item = String> {
    let mut line = Vec::new();
    std::iter::from_fn(move || {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                if line.last() == Some(&b'\n') {
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                }
                Some(String::from_utf8_lossy(&line).into_owned())
            }
        }
    })
}

/// The session's runtime directory, where compositors put their sockets.
fn runtime() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
}

/// The session's Wayland display as compositors name it, the name of its
/// socket in the runtime directory: WAYLAND_DISPLAY, or libwayland's own
/// "wayland-0" when that is not set.
fn display_name() -> String {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_owned());
    let path = Path::new(&display);
    match (path.is_absolute(), runtime()) {
        (true, Some(runtime)) if path.parent() == Some(runtime.as_path()) => {
            path.file_name().map_or(display.clone(), |name| name.to_string_lossy().into_owned())
        }
        _ => display,
    }
}

/// The socket of the session's Wayland display.
fn display_path() -> Option<PathBuf> {
    let display = display_name();
    if Path::new(&display).is_absolute() {
        Some(PathBuf::from(display))
    } else {
        Some(runtime()?.join(display))
    }
}

/// Whether a compositor answers on the session's Wayland display.
fn display_answers() -> bool {
    display_path().is_some_and(|path| UnixStream::connect(path).is_ok())
}

/// Whether the session's Wayland display is gone, as [`gone`] tells.
fn display_gone() -> bool {
    display_path().is_none_or(|path| socket_gone(&path))
}

/// A socket a compositor answers on: where it is, and which file it is (its
/// device and inode). A compositor that takes the name of one that went
/// makes a file of its own: it removes the one left behind before it binds
/// the name, and the runtime directory's tmpfs numbers each new file anew.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Socket {
    path: PathBuf,
    file: (u64, u64),
}

impl Socket {
    /// The socket at `path`, when something listens on it.
    fn answering(path: PathBuf) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        UnixStream::connect(&path).ok()?;
        let metadata = std::fs::metadata(&path).ok()?;
        Some(Self { file: (metadata.dev(), metadata.ino()), path })
    }
}

/// The socket of the session's Wayland display, when a compositor answers on
/// it. Taken while the daemon's compositor is there, it tells that compositor
/// from one that takes the display's name after it went.
pub fn session_display() -> Option<Socket> {
    Socket::answering(display_path()?)
}

/// Whether the compositor that answered on the session's display as `ours`
/// left it: nothing listens there now, or another compositor took the name.
/// Looked at for up to 2 s: a compositor that exits closes its clients'
/// connections and its listening socket one after the other.
fn display_left(ours: Option<&Socket>) -> bool {
    display_path().is_none_or(|path| left(&path, ours, Duration::from_secs(2)))
}

fn left(path: &Path, ours: Option<&Socket>, limit: Duration) -> bool {
    let since = Instant::now();
    loop {
        let other =
            || Socket::answering(path.to_owned()).is_some_and(|now| ours.is_some_and(|ours| now != *ours));
        if socket_gone(path) || other() {
            return true;
        }
        if since.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The Wayland displays that answer when the compositor went, to tell a
/// compositor that comes after it from those that answered all along
/// (another session's, a headless one's for a VNC server) and from the one
/// that goes while it still answers.
pub struct Answering(HashSet<Socket>);

impl Answering {
    /// What answers now. `ours` is the session's display as the compositor
    /// that went answered on it (None: it did not): another compositor there
    /// came after it, though before the daemon was told that it went, which
    /// it may be a moment late (it hears of it when its socket stops
    /// answering, and looks again every 2 s while it answers).
    pub fn now(ours: Option<&Socket>) -> Self {
        Self::of(answering(), display_path().as_deref(), ours)
    }

    fn of(now: HashSet<Socket>, display: Option<&Path>, ours: Option<&Socket>) -> Self {
        Self(
            now.into_iter()
                .filter(|socket| Some(socket.path.as_path()) != display || Some(socket) == ours)
                .collect(),
        )
    }

    /// Whether a compositor answers that did not when this was taken: on the
    /// session's display or another, one that did not answer then or has
    /// stopped answering since (the next compositor takes the socket's name of
    /// the one that went, as a rule, with a socket of its own).
    pub fn one_came(&mut self) -> bool {
        self.came(&answering())
    }

    fn came(&mut self, now: &HashSet<Socket>) -> bool {
        self.0.retain(|socket| now.contains(socket));
        now.iter().any(|socket| !self.0.contains(socket))
    }
}

/// The Wayland displays that take a connection: the session's, and those in
/// the session's runtime directory. A compositor that crashed leaves its
/// socket there, which takes none.
fn answering() -> HashSet<Socket> {
    let mut displays = runtime().map(|runtime| answering_in(&runtime)).unwrap_or_default();
    displays.extend(session_display());
    displays
}

fn answering_in(dir: &Path) -> HashSet<Socket> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("wayland-") && !name.ends_with(".lock")
        })
        .filter_map(|entry| Socket::answering(entry.path()))
        .collect()
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

/// KWin, as a Plasma session finds it, for the tests: nothing is asked of it
/// but what needs no D-Bus.
#[cfg(test)]
pub fn plasma() -> Box<dyn Compositor> {
    Box::new(kwin::Kwin::new())
}

/// A monitor of `name` and of no size, for the tests.
#[cfg(test)]
pub fn monitor(name: &str) -> Monitor {
    Monitor { name: name.to_owned(), x: 0.0, y: 0.0, width: 0.0, height: 0.0, pixels: (0, 0) }
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

    /// Waits up to 2 s for nothing to listen on `socket`, whose listener this
    /// process dropped: a child another test is starting has the listener
    /// too, from its fork until it runs its program.
    pub(super) fn closed(socket: &Path) {
        let since = Instant::now();
        while !socket_gone(socket) {
            assert!(since.elapsed() < Duration::from_secs(2), "{} still answers", socket.display());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

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

    #[test]
    fn a_compositor_is_gone_when_nothing_listens_or_its_socket_went_and_not_otherwise() {
        use std::io::{Error, ErrorKind};
        // ECONNREFUSED, ENOENT.
        assert!(gone(&Error::from_raw_os_error(111)));
        assert!(gone(&Error::from_raw_os_error(2)));
        // EMFILE, ENFILE, ENOMEM, EACCES, EAGAIN (a full queue), ETIMEDOUT.
        for errno in [24, 23, 12, 13, 11, 110] {
            assert!(!gone(&Error::from_raw_os_error(errno)), "errno {errno}");
        }
        assert!(!gone(&Error::from(ErrorKind::Interrupted)));
    }

    #[test]
    fn a_socket_nothing_listens_on_or_none_at_all_is_gone() {
        let dir = std::env::temp_dir().join(format!("we-linux-test-socket-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("ipc.sock");
        assert!(socket_gone(&socket));
        let listening = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(!socket_gone(&socket));
        drop(listening);
        closed(&socket);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_of_other_bytes_does_not_end_the_events() {
        let stream: &[u8] = b"activewindow>>kitty,\xff\xfe title\nfullscreen>>1\r\nworkspace>>2";
        let read: Vec<String> = lines(stream).collect();
        assert_eq!(read, vec!["activewindow>>kitty,\u{fffd}\u{fffd} title", "fullscreen>>1", "workspace>>2"]);
    }

    #[test]
    fn a_display_answers_when_a_socket_named_wayland_takes_a_connection() {
        let dir = std::env::temp_dir().join(format!("we-linux-test-displays-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // What a compositor that crashed leaves.
        drop(std::os::unix::net::UnixListener::bind(dir.join("wayland-1")).unwrap());
        closed(&dir.join("wayland-1"));
        std::fs::write(dir.join("wayland-1.lock"), "").unwrap();
        let _other = std::os::unix::net::UnixListener::bind(dir.join("pipewire-0")).unwrap();
        assert!(answering_in(&dir).is_empty());
        let _started = std::os::unix::net::UnixListener::bind(dir.join("wayland-2")).unwrap();
        let started = Socket::answering(dir.join("wayland-2")).unwrap();
        assert_eq!(answering_in(&dir), HashSet::from([started]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_compositor_left_its_display_when_nothing_listens_there_or_another_took_it() {
        let dir = std::env::temp_dir().join(format!("we-linux-test-left-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let display = dir.join("wayland-1");
        let listening = std::os::unix::net::UnixListener::bind(&display).unwrap();
        let ours = Socket::answering(display.clone()).unwrap();
        // It still answers (its connection to the daemon broke for another reason).
        assert!(!left(&display, Some(&ours), Duration::from_millis(200)));
        // Not known what answered there before: what answers now may be it.
        assert!(!left(&display, None, Duration::ZERO));
        // Another one answers on the name (a socket file of its own).
        let other = Socket { file: (ours.file.0, ours.file.1 + 1), ..ours.clone() };
        assert!(left(&display, Some(&other), Duration::ZERO));
        // It crashed: the socket is there, nothing listens on it.
        drop(listening);
        assert!(left(&display, Some(&ours), Duration::from_secs(2)));
        // It exited and took its socket with it.
        std::fs::remove_file(&display).unwrap();
        assert!(left(&display, None, Duration::ZERO));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_daemon_says_it_waits_for_hyprland_only_while_one_is_starting() {
        assert_eq!(
            waiting(|| true, || false),
            Some("Hyprland is starting: the daemon waits for it to answer")
        );
        assert_eq!(
            waiting(|| true, || true),
            Some("Hyprland is starting: the daemon waits for it to answer")
        );
        // Nothing on the display, and no Hyprland starting (one that went, or none).
        assert_eq!(
            waiting(|| false, || false),
            Some("nothing answers on the session's Wayland display yet: the daemon waits for a compositor")
        );
        assert_eq!(waiting(|| false, || true), None);
    }

    /// A display's socket in the runtime directory, by its name and the
    /// number of its file.
    fn socket(name: &str, file: u64) -> Socket {
        Socket { path: PathBuf::from(format!("/run/user/1000/{name}")), file: (30, file) }
    }

    /// What answers now.
    fn now(sockets: &[&Socket]) -> HashSet<Socket> {
        sockets.iter().map(|socket| (*socket).clone()).collect()
    }

    #[test]
    fn a_compositor_came_when_a_display_answers_that_did_not_when_the_last_one_went() {
        let (going, headless) = (socket("wayland-1", 10), socket("wayland-7", 70));
        // As the session's compositor (wayland-1) goes: another one (a headless
        // one on wayland-7) answers, and so does the one going, for a moment.
        let mut before = Answering::of(now(&[&going, &headless]), None, None);
        assert!(!before.came(&now(&[&going, &headless])), "the one going still answers");
        assert!(!before.came(&now(&[&headless])), "the other one answered all along");
        assert!(!before.came(&now(&[&headless])));
        // The next one takes the name of the one that went.
        assert!(before.came(&now(&[&socket("wayland-1", 11), &headless])));
        // Or another name.
        let mut before = Answering::of(now(&[&headless]), None, None);
        assert!(!before.came(&now(&[&headless])));
        assert!(before.came(&now(&[&socket("wayland-2", 20), &headless])));
        // The other one going and coming back is a compositor that came, too.
        let mut before = Answering::of(now(&[&headless]), None, None);
        assert!(!before.came(&now(&[])));
        assert!(before.came(&now(&[&socket("wayland-7", 71)])));
    }

    #[test]
    fn a_compositor_that_took_the_display_before_the_daemon_heard_the_last_one_went_came() {
        let display = Path::new("/run/user/1000/wayland-1");
        let (ours, next, headless) =
            (socket("wayland-1", 10), socket("wayland-1", 11), socket("wayland-7", 70));
        // The next one has the session's display already when the daemon hears
        // that its compositor went.
        let mut before = Answering::of(now(&[&next, &headless]), Some(display), Some(&ours));
        assert!(before.came(&now(&[&next, &headless])));
        // Heard in time: the one that goes still answers there.
        let mut before = Answering::of(now(&[&ours, &headless]), Some(display), Some(&ours));
        assert!(!before.came(&now(&[&ours, &headless])));
        assert!(!before.came(&now(&[&headless])));
        assert!(before.came(&now(&[&next, &headless])));
        // Nothing answered on the session's display while the daemon's compositor
        // was there (the daemon started for a device): what answers there came.
        let mut before = Answering::of(now(&[&next, &headless]), Some(display), None);
        assert!(before.came(&now(&[&next, &headless])));
        assert!(!Answering::of(now(&[&headless]), Some(display), None).came(&now(&[&headless])));
    }
}
