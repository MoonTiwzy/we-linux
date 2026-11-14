//! we-linux: Wallpaper Engine's library and workshop on Linux, laid out like
//! the Windows client.
//!
//! "Установлено" lists the workshop items Steam has on disk; "Мастерская" and
//! "Рекомендации" query the workshop through Steam itself (steam.rs), with the
//! tags the Windows client filters by. Subscribing goes through Steam too,
//! which then downloads the item. Setting a wallpaper, its properties, the
//! monitors, the engine's settings and the playlist are written to files
//! (desktop.rs, props.rs) the daemon reads.
//!
//! The window is `window/` (its `mod.rs` says how it is laid out);
//! `we-linux daemon` keeps the wallpaper on the desktop (engine.rs); `apply`,
//! `off` and `status` are the command line (cli.rs).

mod cli;
mod compositor;
mod desktop;
mod engine;
mod lang;
mod library;
mod previews;
mod props;
mod selection;
mod steam;
mod units;
mod window;

fn main() -> Result<(), slint::PlatformError> {
    steam::init_env();
    if let Some(code) = cli::run() {
        std::process::exit(code);
    }
    window::run()
}
