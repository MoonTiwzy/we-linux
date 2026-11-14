//! WE Linux's own log, beside the renderer's `engine.log` in the runtime
//! folder: what the daemon was asked, what it asked of the renderer and what
//! came back, every refusal with its reason, the renderer starting, exiting
//! and hanging; and what went wrong in the window. The daemon's stderr is
//! nowhere when the window started it, and the window's status line fades:
//! without this file a wallpaper that did not show leaves no word behind.
//!
//! A line is `14:03:27.518 daemon: warn: ...`, as the renderer writes its own,
//! so the two read side by side. The daemon and the window write the same
//! file, a whole line at a time. It is bounded: past [`LIMIT`] it becomes
//! `we-linux.log.1` (the one before is dropped) and a new one starts.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// How large the log grows before it is set aside for a new one.
pub const LIMIT: u64 = 256 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// Who writes: the daemon, the window, or a command of the command line.
static ROLE: OnceLock<&'static str> = OnceLock::new();

/// Name what this process is in the lines it writes. Once, at its start.
pub fn as_role(role: &'static str) {
    let _ = ROLE.set(role);
}

fn role() -> &'static str {
    ROLE.get().copied().unwrap_or("we-linux")
}

/// The log, in the runtime folder.
pub fn file() -> PathBuf {
    crate::engine::runtime_dir().join("we-linux.log")
}

/// The log before the current one, once it grew past [`LIMIT`].
pub fn older(log: &Path) -> PathBuf {
    let mut name = log.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

pub fn info(text: impl AsRef<str>) {
    write(Level::Info, text.as_ref());
}

pub fn warn(text: impl AsRef<str>) {
    write(Level::Warn, text.as_ref());
}

pub fn error(text: impl AsRef<str>) {
    write(Level::Error, text.as_ref());
}

fn write(level: Level, text: &str) {
    let words = words(role(), level, text);
    // A terminal the daemon was started from, or the journal of a service,
    // gets it too.
    let _ = std::io::stderr().write_all(format!("we-linux {words}").as_bytes());
    let _ = std::fs::create_dir_all(crate::engine::runtime_dir());
    let _ = append(&file(), &format!("{} {words}", clock()), LIMIT);
}

/// What a line says after its time: who, how bad, what. A text of several
/// lines stays one line: the reader goes by lines, and so does `logs -f`.
fn words(role: &str, level: Level, text: &str) -> String {
    let text: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
    format!("{role}: {}: {}\n", level.name(), text.join(" / "))
}

/// One line of the log, as it is written.
#[cfg(test)]
fn line(clock: &str, role: &str, level: Level, text: &str) -> String {
    format!("{clock} {}", words(role, level, text))
}

/// Add `line` to `log`, first setting the log aside when the line would take
/// it past `limit`. The file is opened for each line: the daemon and the
/// window both write, and one of them may have just set it aside.
pub fn append(log: &Path, line: &str, limit: u64) -> std::io::Result<()> {
    let size = std::fs::metadata(log).map_or(0, |m| m.len());
    if size > 0 && size + line.len() as u64 > limit {
        std::fs::rename(log, older(log))?;
    }
    // One write of the whole line: with O_APPEND, two writers' lines never mix.
    std::fs::OpenOptions::new().create(true).append(true).open(log)?.write_all(line.as_bytes())
}

/// The C library's broken-down time (glibc's layout, as on every Linux this runs on).
#[repr(C)]
struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
    gmtoff: i64,
    zone: *const std::ffi::c_char,
}

unsafe extern "C" {
    fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
}

/// Now, in the local time zone: the date and the time of day to the millisecond.
fn local_now() -> Option<(Tm, u32)> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?;
    let seconds = now.as_secs() as i64;
    let mut tm = Tm {
        sec: 0,
        min: 0,
        hour: 0,
        mday: 0,
        mon: 0,
        year: 0,
        wday: 0,
        yday: 0,
        isdst: 0,
        gmtoff: 0,
        zone: std::ptr::null(),
    };
    // SAFETY: both pointers are to locals of the right type.
    let done = unsafe { localtime_r(&seconds, &mut tm) };
    (!done.is_null()).then_some((tm, now.subsec_millis()))
}

/// The time of day a line starts with: `14:03:27.518`, as the renderer's lines.
pub fn clock() -> String {
    local_now().map_or_else(
        || "--:--:--.---".to_owned(),
        |(tm, ms)| format!("{:02}:{:02}:{:02}.{ms:03}", tm.hour, tm.min, tm.sec),
    )
}

/// Today, for the line that starts a run: the lines themselves have the time only.
pub fn today() -> String {
    local_now()
        .map_or_else(String::new, |(tm, _)| format!("{}-{:02}-{:02}", tm.year + 1900, tm.mon + 1, tm.mday))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("we-linux-test-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_line_has_the_time_who_and_how_bad() {
        assert_eq!(
            line("14:03:27.518", "daemon", Level::Warn, "DP-1: the renderer refused it"),
            "14:03:27.518 daemon: warn: DP-1: the renderer refused it\n"
        );
        // Several lines of text stay one line of the log.
        assert_eq!(
            line("14:03:27.518", "window", Level::Error, "first\n\nsecond  \n"),
            "14:03:27.518 window: error: first / second\n"
        );
    }

    #[test]
    fn the_clock_reads_as_the_renderers() {
        let clock = clock();
        assert_eq!(clock.len(), 12, "{clock}");
        assert_eq!(clock.as_bytes()[2], b':');
        assert_eq!(clock.as_bytes()[8], b'.');
        assert_eq!(today().len(), 10);
    }

    #[test]
    fn the_log_is_set_aside_when_full_and_the_one_before_goes() {
        let log = scratch("bound").join("we-linux.log");
        let line = "12:00:00.000 daemon: info: 0123456789\n"; // 38 bytes
        for _ in 0..3 {
            append(&log, line, 100).unwrap();
        }
        // Two lines fit in 100 bytes; the third starts a new file.
        assert_eq!(std::fs::read_to_string(&log).unwrap(), line);
        assert_eq!(std::fs::read_to_string(older(&log)).unwrap(), line.repeat(2));
        for _ in 0..4 {
            append(&log, line, 100).unwrap();
        }
        // However much is written, the two files hold no more than twice the limit.
        let total = std::fs::metadata(&log).unwrap().len() + std::fs::metadata(older(&log)).unwrap().len();
        assert!(total <= 200, "{total}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), line);
    }

    #[test]
    fn a_line_longer_than_the_limit_is_still_written() {
        let log = scratch("long").join("we-linux.log");
        append(&log, "a long line\n", 4).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "a long line\n");
        assert!(!older(&log).exists());
    }
}
