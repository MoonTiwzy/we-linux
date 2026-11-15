//! The command line: `apply`, `off`, `status`, `doctor`, `logs` and the rest (see
//! `USAGE`). With no command (or with `--show`, which only shapes the window)
//! the window opens instead.

use crate::stage::{self, Asked, Phase};
use crate::steam::{self, Sort, Workshop};
use crate::units::megabytes;
use crate::{desktop, engine, library, logs};

const USAGE: &str = "\
we-linux                          the window: library, workshop, playlist
we-linux apply <ID|PATH> [--output NAME]
                                  set a wallpaper (a workshop id, a wallpaper
                                  folder, a video or a picture) on every
                                  monitor, or on one
we-linux clone NAME on|off        whether the wallpaper of every monitor is cloned
                                  on that monitor or device (off: it shows its
                                  own, or nothing; a new device starts off)
we-linux off                      no live wallpaper
we-linux status                   what is on the desktop
we-linux clear-cache              empty the caches: previews, stills, copies of
                                  videos, compiled shaders
we-linux doctor                   everything a bug report needs: versions, what
                                  was found, the settings, the logs' last lines
we-linux logs [-f]                what was found, then everything the daemon,
                                  the window, the renderer and the hook said, in
                                  order; -f follows the logs live
we-linux workshop [TEXT] [--sort top|trend|new|subs|updated] [--type scene|video|web]
                  [--page N]      search the workshop: id, size, votes, title
we-linux subscribe ID...          subscribe and wait for Steam to download
we-linux unsubscribe ID...        unsubscribe and remove the files
we-linux devices                  what shows a wallpaper and is not a monitor
                                  (a cooler's panel, a keyboard's screen): each
                                  with its size, command and state
we-linux device add NAME WxH COMMAND [--fps N]
                                  a device: its frames go to COMMAND's standard
                                  input as raw RGB; then `apply ID --output NAME`
we-linux device set NAME KEY VALUE
                                  change one: name, size, fps, command, enabled,
                                  fill (cover|contain|stretch), and the part of
                                  the wallpaper it shows: zoom (percent, 100 as
                                  the fill has it, 5 to 2000), shift_x, shift_y
                                  (percent of the device, right and up), shape
                                  (rect, or circle for a round panel)
we-linux device remove NAME
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
            crate::log::as_role("command");
            let result = desktop::resolve(target).and_then(|path| match output {
                Some(output) => desktop::apply_to(output, &path),
                None => desktop::clear_outputs().and_then(|()| desktop::apply(&path)),
            });
            match &result {
                Ok(()) => crate::log::info(format!(
                    "we-linux apply {target}{}",
                    output.map_or_else(String::new, |o| format!(" --output {o}"))
                )),
                Err(why) => crate::log::warn(format!("we-linux apply {target}: {why}")),
            }
            done(result)
        }
        "off" => {
            crate::log::as_role("command");
            crate::log::info("we-linux off");
            done(desktop::off())
        }
        "clone" => {
            crate::log::as_role("command");
            let (Some(name), Some(state)) = (args.get(1), args.get(2)) else {
                eprintln!("{USAGE}");
                return Some(2);
            };
            let on = match state.as_str() {
                "on" | "true" | "yes" | "1" => true,
                "off" | "false" | "no" | "0" => false,
                _ => {
                    eprintln!("{USAGE}");
                    return Some(2);
                }
            };
            let result = if desktop::monitors().iter().any(|m| &m.name == name) {
                desktop::set_cloned(name, on)
                    .and_then(|()| if engine::daemon_running() { engine::ensure_daemon() } else { Ok(()) })
            } else {
                Err(format!("there is no monitor or device {name}"))
            };
            match &result {
                Ok(()) => crate::log::info(format!("we-linux clone {name} {state}")),
                Err(why) => crate::log::warn(format!("we-linux clone {name} {state}: {why}")),
            }
            done(result)
        }
        "devices" => {
            let settings = desktop::Settings::load();
            if settings.devices.is_empty() {
                say!("no devices: we-linux device add NAME WxH COMMAND");
            }
            let told = engine::call(serde_json::json!({ "cmd": "get", "path": "devices" })).ok();
            for device in &settings.devices {
                let state = match device.problem() {
                    Some(problem) => problem,
                    None if !device.enabled => "off".to_owned(),
                    None => match told.as_ref().map(|told| &told[&device.name]) {
                        Some(node) if node["error"].is_string() => {
                            format!("not running: {}", node["error"].as_str().unwrap_or_default())
                        }
                        // A plugin (web wallpapers) writes its frames to the
                        // device itself: the renderer has none to count.
                        Some(node) if node["running"] == true => match node["sent"].as_u64().unwrap_or(0) {
                            0 => "running".to_owned(),
                            sent => format!("running, {sent} frames sent"),
                        },
                        Some(node) if node.is_object() => "ready (nothing is shown on it)".to_owned(),
                        _ => "the renderer is not running".to_owned(),
                    },
                };
                say!(
                    "{}: {}x{}, {} fps, {}, fill {}, zoom {}%, shift {} {}, `{}`: {state}",
                    device.name,
                    device.width,
                    device.height,
                    device.fps,
                    device.shape,
                    device.fill,
                    device.zoom,
                    device.shift_x,
                    device.shift_y,
                    device.command
                );
            }
            0
        }
        "device" => {
            crate::log::as_role("command");
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            let mut settings = desktop::Settings::load();
            let result = match rest.as_slice() {
                ["add", name, size, command @ ..] if !command.is_empty() => (|| {
                    let fps = command.iter().position(|a| *a == "--fps");
                    let rate = match fps.and_then(|i| command.get(i + 1)) {
                        Some(n) => n.parse::<u32>().map_err(|_| format!("--fps {n}: not a number"))?,
                        None => 30,
                    };
                    let words: Vec<&str> = command
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| fps.is_none_or(|f| *i != f && *i != f + 1))
                        .map(|(_, a)| *a)
                        .collect();
                    let (width, height) = size
                        .split_once(['x', 'X'])
                        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                        .ok_or_else(|| format!("{size}: the size is WIDTHxHEIGHT, like 480x480"))?;
                    if settings.devices.iter().any(|d| d.name == *name) {
                        return Err(format!("there is a device {name} already"));
                    }
                    let device = desktop::Device {
                        name: (*name).to_owned(),
                        width,
                        height,
                        fps: rate,
                        command: words.join(" "),
                        ..desktop::Device::default()
                    };
                    if let Some(problem) = device.problem() {
                        return Err(problem);
                    }
                    settings.devices.push(device);
                    // A device is not one of the screens: the wallpaper of every
                    // monitor is not put on it until that is asked for.
                    desktop::set_cloned(name, false)
                })(),
                ["remove", name] => {
                    let before = settings.devices.len();
                    settings.devices.retain(|d| d.name != *name);
                    if settings.devices.len() == before {
                        Err(format!("there is no device {name}"))
                    } else {
                        // Its own wallpaper goes with it.
                        desktop::forget_output(name)
                    }
                }
                ["set", name, key, value] => (|| {
                    let device = settings
                        .devices
                        .iter_mut()
                        .find(|d| d.name == *name)
                        .ok_or_else(|| format!("there is no device {name}"))?;
                    let number =
                        || value.parse::<i64>().map_err(|_| format!("{key} takes a number, not {value}"));
                    let percent = |range: (f32, f32)| {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|n| n.is_finite())
                            .map(|n| n.clamp(range.0, range.1))
                            .ok_or_else(|| format!("{key} takes a number of percent, not {value}"))
                    };
                    match *key {
                        "name" => {
                            device.name = (*value).to_owned();
                            if device.problem().is_none() {
                                desktop::rename_output(name, value)?;
                            }
                        }
                        "size" => {
                            (device.width, device.height) = value
                                .split_once(['x', 'X'])
                                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                                .ok_or_else(|| format!("{value}: the size is WIDTHxHEIGHT"))?
                        }
                        "fps" => device.fps = number()?.clamp(1, 240) as u32,
                        "command" => device.command = (*value).to_owned(),
                        "enabled" => device.enabled = matches!(*value, "true" | "on" | "1" | "yes"),
                        "fill" if ["cover", "contain", "stretch"].contains(value) => {
                            device.fill = (*value).to_owned()
                        }
                        "fill" => return Err("fill takes cover, contain or stretch".to_owned()),
                        "zoom" => device.zoom = percent(desktop::ZOOM_RANGE)?,
                        "shift_x" => device.shift_x = percent(desktop::SHIFT_RANGE)?,
                        "shift_y" => device.shift_y = percent(desktop::SHIFT_RANGE)?,
                        "shape" if ["rect", "circle"].contains(value) => device.shape = (*value).to_owned(),
                        "shape" => return Err("shape takes rect or circle".to_owned()),
                        _ => {
                            return Err(format!(
                                "a device has no {key}: name, size, fps, command, enabled, fill, zoom, shift_x, shift_y, shape"
                            ));
                        }
                    }
                    device.problem().map_or(Ok(()), Err)
                })(),
                _ => {
                    eprintln!("{USAGE}");
                    return Some(2);
                }
            };
            let result = result.and_then(|()| settings.save()).and_then(|()| {
                // A daemon that runs reads the settings itself; none is started
                // for a device nothing is shown on.
                if engine::daemon_running() { engine::ensure_daemon() } else { Ok(()) }
            });
            match &result {
                Ok(()) => crate::log::info(format!("we-linux device {}", rest.join(" "))),
                Err(why) => crate::log::warn(format!("we-linux device {}: {why}", rest.join(" "))),
            }
            done(result)
        }
        "status" => {
            let desktop = desktop::Desktop::load();
            match &desktop.wallpaper {
                Some(path) => say!("wallpaper: {}", path.display()),
                None => say!("wallpaper: none"),
            }
            for (output, path) in &desktop.outputs {
                say!("{output}: {}", path.display());
            }
            if !desktop.apart.is_empty() {
                say!("not cloned on: {}", desktop.apart.join(", "));
            }
            let seen = engine::seen();
            say!(
                "renderer: {}",
                if seen.busy.is_some() {
                    "busy"
                } else if seen.shown.is_some() {
                    "running"
                } else {
                    "not running"
                }
            );
            if let Some(wallpaper) = desktop.wallpaper.as_ref().or(desktop.outputs.values().next()) {
                let asked =
                    Asked { wallpaper: wallpaper.clone(), output: None, waited: std::time::Duration::ZERO };
                say!(
                    "state: {}",
                    match stage::phase(&asked, &seen) {
                        Phase::Shown => "shown".to_owned(),
                        Phase::Optimising(n) => format!("shown, {n} video(s) being re-encoded"),
                        Phase::Starting => "the renderer is starting".to_owned(),
                        Phase::Loading => "loading".to_owned(),
                        Phase::Waiting(before) => format!("waiting for {} to load", before.display()),
                        Phase::Failed(_) => "failed".to_owned(),
                        Phase::Stuck => "stuck".to_owned(),
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
        "logs" => {
            if args.iter().skip(1).any(|a| a == "-f" || a == "--follow") {
                logs::follow();
            } else {
                logs::print();
            }
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
