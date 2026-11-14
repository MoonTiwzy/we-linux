//! `we-linux logs`: everything a person debugging needs, in one go and in the
//! order it reads as a story: what was found and what runs (`doctor`'s
//! summary), what WE Linux was asked and did, what the renderer did, what the
//! hook printed. `we-linux logs -f` follows the logs live instead, each line
//! marked with the log it comes from.

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{engine, log};

/// A log that is part of the story.
pub struct Source {
    /// What marks its lines when followed.
    pub tag: &'static str,
    /// Its heading in the story.
    pub title: &'static str,
    pub path: PathBuf,
    /// A part set aside (the log before it grew full, the renderer before
    /// the one running now): only some runs have one, and it is not followed.
    pub older: bool,
    /// Only its last lines are worth reading (the renderer before: what it
    /// said before it went).
    pub tail: Option<usize>,
}

/// The logs, in the order of the story: WE Linux's (its older part first),
/// the renderer's (the one before the running one first), the hook's.
pub fn sources() -> Vec<Source> {
    let ours = log::file();
    let source = |tag, title, path, older, tail| Source { tag, title, path, older, tail };
    vec![
        source("we-linux", "we-linux log, the part before", log::older(&ours), true, None),
        source("we-linux", "we-linux log (the daemon and the window)", ours, false, None),
        source(
            "renderer",
            "renderer log, the renderer before this one",
            engine::engine_log_before(),
            true,
            Some(40),
        ),
        source("renderer", "renderer log", engine::engine_log(), false, None),
        source("hook", "hook log (the last change)", engine::hook_log(), false, None),
    ]
}

/// The story: the summary, then each log under its heading. A log that is
/// not there is said to be missing, except the older parts, which only some
/// runs have.
pub fn story(summary: &str, logs: &[(&Source, Option<String>)]) -> String {
    let mut out = vec![summary.trim_end().to_owned()];
    for (source, text) in logs {
        let text = match text {
            Some(text) if !text.trim().is_empty() => text,
            _ if source.older => continue,
            _ => {
                out.push(String::new());
                out.push(format!("=== {}: nothing yet ({}) ===", source.title, source.path.display()));
                continue;
            }
        };
        out.push(String::new());
        out.push(format!("=== {} ({}) ===", source.title, source.path.display()));
        let lines: Vec<String> = text.lines().map(engine::strip_colours).collect();
        let from = source.tail.map_or(0, |n| lines.len().saturating_sub(n));
        out.extend(lines[from..].iter().cloned());
    }
    out.join("\n")
}

/// `we-linux logs`.
pub fn print() {
    let sources = sources();
    let logs: Vec<(&Source, Option<String>)> =
        sources.iter().map(|source| (source, std::fs::read_to_string(&source.path).ok())).collect();
    let _ = writeln!(std::io::stdout(), "{}", story(&engine::summary(), &logs));
}

/// A log being followed: how far it was read, and which file that was (a
/// log set aside or a renderer started anew is a new file at the same path).
pub struct Follower {
    path: PathBuf,
    file: Option<(u64, u64)>,
    offset: u64,
    /// The end of a line still being written.
    partial: String,
}

impl Follower {
    /// Follow `path` from its end: what is there already was printed before.
    pub fn from_end(path: &Path) -> Self {
        let meta = std::fs::metadata(path).ok();
        Self {
            path: path.to_owned(),
            file: meta.as_ref().map(|m| (m.dev(), m.ino())),
            offset: meta.map_or(0, |m| m.len()),
            partial: String::new(),
        }
    }

    /// The whole lines written since the last look.
    pub fn new_lines(&mut self) -> Vec<String> {
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return Vec::new();
        };
        let file = Some((meta.dev(), meta.ino()));
        // Another file, or this one emptied: read it from its start.
        if file != self.file || meta.len() < self.offset {
            self.file = file;
            self.offset = 0;
            self.partial.clear();
        }
        if meta.len() == self.offset {
            return Vec::new();
        }
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&self.path).and_then(|mut f| {
            f.seek(SeekFrom::Start(self.offset))?;
            f.read_to_end(&mut bytes)
        });
        if read.is_err() {
            return Vec::new();
        }
        self.offset += bytes.len() as u64;
        self.partial.push_str(&String::from_utf8_lossy(&bytes));
        let Some(end) = self.partial.rfind('\n') else {
            return Vec::new();
        };
        let rest = self.partial.split_off(end + 1);
        let lines = std::mem::replace(&mut self.partial, rest);
        lines.lines().map(engine::strip_colours).collect()
    }
}

/// A line as `logs -f` prints it: marked with the log it comes from.
pub fn marked(tag: &str, line: &str) -> String {
    format!("{tag:<8} | {line}")
}

/// `we-linux logs -f`: the last lines of each log, then every line as it is
/// written, until the reader goes (Ctrl+C, or the end of a pipe).
pub fn follow() {
    let mut out = std::io::stdout();
    // The older parts are done with: only what is written now is followed.
    let live: Vec<Source> = sources().into_iter().filter(|s| !s.older).collect();
    let mut followers = Vec::new();
    for source in &live {
        let text = std::fs::read_to_string(&source.path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines[lines.len().saturating_sub(10)..] {
            if writeln!(out, "{}", marked(source.tag, &engine::strip_colours(line))).is_err() {
                return;
            }
        }
        followers.push((source.tag, Follower::from_end(&source.path)));
    }
    loop {
        for (tag, follower) in &mut followers {
            for line in follower.new_lines() {
                if writeln!(out, "{}", marked(tag, &line)).is_err() {
                    return;
                }
            }
        }
        if out.flush().is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("we-linux-test-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn source(tag: &'static str, title: &'static str, tail: Option<usize>) -> Source {
        let older = title.contains("before");
        Source { tag, title, path: PathBuf::from(format!("/run/{tag}.log")), older, tail }
    }

    #[test]
    fn the_story_is_the_summary_then_ours_then_the_renderers_then_the_hooks() {
        let ours = source("we-linux", "we-linux log", None);
        let renderer = source("renderer", "renderer log", None);
        let hook = source("hook", "hook log", None);
        let text = story(
            "we-linux 0.2.0\ndaemon: running\n",
            &[
                (&ours, Some("12:00:00.000 daemon: info: asking\n".into())),
                (&renderer, Some("12:00:00.100 wepaper: info: asked to show /w on DP-1\n".into())),
                (&hook, Some("matugen: done\n".into())),
            ],
        );
        let at = |what: &str| text.find(what).unwrap_or_else(|| panic!("{what} is missing from\n{text}"));
        assert!(at("daemon: running") < at("=== we-linux log"));
        assert!(at("=== we-linux log") < at("daemon: info: asking"));
        assert!(at("daemon: info: asking") < at("=== renderer log"));
        assert!(at("=== renderer log") < at("asked to show"));
        assert!(at("asked to show") < at("=== hook log"));
        assert!(at("=== hook log") < at("matugen: done"));
    }

    #[test]
    fn a_log_not_there_says_so_and_an_older_part_not_there_is_left_out() {
        let before = source("we-linux", "we-linux log, the part before", None);
        let ours = source("we-linux", "we-linux log", None);
        let previous = source("renderer", "renderer log, the renderer before this one", Some(40));
        let text = story("summary", &[(&before, None), (&ours, None), (&previous, None)]);
        assert_eq!(text, "summary\n\n=== we-linux log: nothing yet (/run/we-linux.log) ===");
    }

    #[test]
    fn of_the_renderer_before_only_its_last_lines_and_without_colours() {
        let previous = source("renderer", "renderer log, the renderer before this one", Some(2));
        let text = story("s", &[(&previous, Some("one\ntwo\n\u{1b}[31mthree\u{1b}[0m\n".into()))]);
        assert!(text.ends_with("===\ntwo\nthree"), "{text}");
    }

    #[test]
    fn following_reads_whole_new_lines_and_a_new_file_from_its_start() {
        let path = scratch("follow").join("engine.log");
        std::fs::write(&path, "old line\n").unwrap();
        let mut follower = Follower::from_end(&path);
        assert!(follower.new_lines().is_empty(), "what was there is not printed again");
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"first\nsecond, half").unwrap();
        assert_eq!(follower.new_lines(), ["first"]);
        file.write_all(b" and the rest\n").unwrap();
        assert_eq!(follower.new_lines(), ["second, half and the rest"]);
        // The renderer started anew: a new file where the old one was.
        std::fs::rename(&path, path.with_extension("log.1")).unwrap();
        std::fs::write(&path, "new renderer\n").unwrap();
        assert_eq!(follower.new_lines(), ["new renderer"]);
        // A log that went away is not an error: it is waited for.
        std::fs::remove_file(&path).unwrap();
        assert!(follower.new_lines().is_empty());
    }

    #[test]
    fn a_followed_line_says_where_it_comes_from() {
        assert_eq!(marked("hook", "done"), "hook     | done");
        assert_eq!(marked("renderer", "x"), "renderer | x");
    }
}
