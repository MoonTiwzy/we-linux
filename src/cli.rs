//! The command line: `apply`, `off`, `status`, `doctor` and the rest (see
//! `USAGE`). With no command (or with `--show`, which only shapes the window)
//! the window opens instead.

use crate::steam::{self, Sort, Workshop};
use crate::units::megabytes;
use crate::{desktop, engine, library};

const USAGE: &str = "\
we-linux                          the window: library, workshop, playlist
we-linux apply <ID|PATH> [--output NAME]
                                  set a wallpaper (a workshop id, a wallpaper
                                  folder, a video or a picture) on every
                                  monitor, or on one
we-linux off                      no live wallpaper
we-linux status                   what is on the desktop
we-linux clear-cache              empty the caches: previews, stills, copies of
                                  videos, compiled shaders
we-linux doctor                   everything a bug report needs: versions, what
                                  was found, the settings, the engine's log
we-linux workshop [TEXT] [--sort top|trend|new|subs|updated] [--type scene|video|web]
                  [--page N]      search the workshop: id, size, votes, title
we-linux subscribe ID...          subscribe and wait for Steam to download
we-linux unsubscribe ID...        unsubscribe and remove the files
we-linux daemon                   keep the wallpaper on the desktop: run it
                                  when the session starts (the window and
                                  `apply` start it when it is not running)";

/// println! that does not panic when the reader went away (`| grep -q`).
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// Runs the command it was given and returns its exit code; None opens the window.
pub fn run() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--show") {
        return None;
    }
    let done = |result: Result<(), String>| match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("we-linux: {e}");
            1
        }
    };
    Some(match args.first()?.as_str() {
        "daemon" => {
            engine::run_daemon();
            0
        }
        "apply" => {
            let flag = args.iter().position(|a| a == "--output");
            let output = flag.and_then(|i| args.get(i + 1)).filter(|name| !name.starts_with("--"));
            let target = args
                .iter()
                .enumerate()
                .skip(1)
                .find(|(i, a)| !a.starts_with("--") && flag != Some(i - 1))
                .map(|(_, a)| a);
            // --output without a monitor is not "every monitor".
            let Some(target) = target.filter(|_| flag.is_none() || output.is_some()) else {
                eprintln!("{USAGE}");
                return Some(2);
            };
            done(desktop::resolve(target).and_then(|path| match output {
                Some(output) => desktop::apply_to(output, &path),
                None => desktop::clear_outputs().and_then(|()| desktop::apply(&path)),
            }))
        }
        "off" => done(desktop::off()),
        "status" => {
            let desktop = desktop::Desktop::load();
            match &desktop.wallpaper {
                Some(path) => say!("wallpaper: {}", path.display()),
                None => say!("wallpaper: none"),
            }
            for (output, path) in &desktop.outputs {
                say!("{output}: {}", path.display());
            }
            say!("renderer: {}", if engine::running() { "running" } else { "not running" });
            if let Some(wallpaper) = desktop.wallpaper.as_ref().or(desktop.outputs.values().next()) {
                say!(
                    "state: {}",
                    match engine::stage(wallpaper) {
                        engine::Stage::Shown => "shown",
                        engine::Stage::Loading => "loading",
                        engine::Stage::Failed(_) => "failed",
                    }
                );
            }
            for (wallpaper, why) in engine::errors() {
                say!("error: {why} ({})", wallpaper.display());
            }
            0
        }
        "workshop" => {
            let flag =
                |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
            // A flag's value by where it stands: the same word may be searched for.
            let values: Vec<usize> = ["--sort", "--type", "--page"]
                .iter()
                .filter_map(|n| args.iter().position(|a| a == n))
                .map(|i| i + 1)
                .collect();
            let text = args
                .iter()
                .enumerate()
                .skip(1)
                .filter(|(i, a)| !a.starts_with("--") && !values.contains(i))
                .map(|(_, a)| a.clone())
                .collect::<Vec<_>>()
                .join(" ");
            let (sort, trend_days) = match flag("--sort").as_deref() {
                Some("trend") => (Sort::Trend, 7),
                Some("new") => (Sort::Newest, 0),
                Some("subs") => (Sort::Subscriptions, 0),
                Some("updated") => (Sort::Updated, 0),
                _ => (Sort::Vote, 0),
            };
            let kind = match flag("--type").as_deref() {
                Some("video") => "Video",
                Some("web") => "Web",
                _ => "Scene",
            };
            let query = steam::Query {
                sort,
                text,
                page: flag("--page").and_then(|p| p.parse().ok()).unwrap_or(1),
                groups: vec![vec![kind.to_owned()], vec!["Everyone".to_owned()]],
                required: Vec::new(),
                excluded: Vec::new(),
                trend_days,
                author: 0,
            };
            match steam::Steam::open().and_then(|mut steam| steam.query(&query)) {
                Ok(page) => {
                    for e in &page.entries {
                        say!("{}\t{} MB\t+{}\t{}", e.id, (e.size >> 20).max(1), e.up, e.title);
                    }
                    0
                }
                Err(e) => {
                    eprintln!("we-linux: {e}");
                    1
                }
            }
        }
        "subscribe" | "unsubscribe" => {
            let ids: Vec<u64> = args.iter().skip(1).filter_map(|a| a.parse().ok()).collect();
            if ids.is_empty() {
                eprintln!("{USAGE}");
                return Some(2);
            }
            let mut steam = match steam::Steam::open() {
                Ok(steam) => steam,
                Err(e) => {
                    eprintln!("we-linux: {e}");
                    return Some(1);
                }
            };
            let subscribing = args[0] == "subscribe";
            // What Steam took: a refused one has nothing coming to wait for.
            let mut taken = Vec::new();
            for &id in &ids {
                let result = if subscribing { steam.subscribe(id) } else { steam.unsubscribe(id) };
                if subscribing && result.is_ok() {
                    steam.download(id);
                    taken.push(id);
                }
                match &result {
                    Ok(()) => say!("{id}: {}", if subscribing { "subscribed" } else { "unsubscribed" }),
                    Err(why) => say!("{id}: Steam refused: {why}"),
                }
                if result.is_ok() && !subscribing {
                    for item in library::installed().into_iter().filter(|item| item.id == id) {
                        if desktop::current().as_deref() == Some(item.dir.as_path()) {
                            let _ = desktop::off();
                        }
                        if let Err(why) = library::remove(&item.dir) {
                            eprintln!("we-linux: {why}");
                        }
                    }
                }
            }
            if subscribing {
                // Until Steam has them all (or ten minutes).
                let started = std::time::Instant::now();
                loop {
                    steam.pump();
                    let left = taken.iter().filter(|&&id| !steam.progress(id).state.finished()).count();
                    if left == 0 || started.elapsed().as_secs() > 600 {
                        say!("downloaded: {} of {}", taken.len() - left, ids.len());
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            } else {
                // Give the client a moment to take the requests.
                for _ in 0..20 {
                    steam.pump();
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            0
        }
        "clear-cache" => {
            say!("{} freed", megabytes(engine::clear_cache()));
            0
        }
        "doctor" => {
            say!("{}", engine::doctor());
            0
        }
        "-h" | "--help" | "help" => {
            say!("{USAGE}");
            0
        }
        "-V" | "--version" => {
            say!("we-linux {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    })
}
