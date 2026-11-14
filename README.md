<div align="center">

<img src="assets/we-linux.svg" width="96" alt="">

# WE Linux

**Wallpaper Engine on Linux, the way the Windows client feels:<br>browse the workshop, subscribe, tune, and put it on your desktop.**

[![License: MIT](https://img.shields.io/badge/license-MIT-3b9cff)](LICENSE)
![Rust](https://img.shields.io/badge/rust-2024-e6522c?logo=rust&logoColor=white)
![Wayland](https://img.shields.io/badge/wayland-native-ffbc00)
[![NixOS](https://img.shields.io/badge/NixOS-flake-5277c3?logo=nixos&logoColor=white)](#nixos)
![Wayland](https://img.shields.io/badge/Wayland-Hyprland%20%C2%B7%20Sway%20%C2%B7%20niri%20%C2%B7%20KDE-58e1ff)

[Install](#install) · [First run](#first-run) · [Compositors](#compositors) · [Command line](#command-line) · [Troubleshooting](#troubleshooting) · [Русский](README.ru.md)

<img src="assets/screenshot.jpg" alt="The workshop tab: search, filters, previews, the selected wallpaper on the right">

</div>

## What it is

A native app (Rust + [Slint](https://slint.dev), no Electron, no Proton) laid out like Wallpaper
Engine's own window, plus a small daemon that keeps the wallpaper on your desktop.

- **The real workshop.** It talks to your running Steam client through Steamworks, as Wallpaper
  Engine itself does: search, the client's filters (type, age rating, resolution, tags,
  features), nine sort orders, 50 items a page, approved and 4K/HDR badges.
- **Subscribe and unsubscribe in place.** Steam downloads, the app shows the progress.
- **Your library.** Everything installed across all Steam libraries, with search, filters,
  favourites and recommendations from the genres you already have.
- **Wallpaper properties.** Checkboxes, sliders, colours, drop-downs and text of a wallpaper,
  applied to the running wallpaper at once and remembered per wallpaper.
- **Monitors.** One wallpaper everywhere, or a different one per monitor. Every monitor runs
  its own copy of the scene, so scripts that follow the pointer work per monitor.
- **Playlists.** Timed or random, saved and loaded, running in the background.
- **Light on the machine.** Rendering is done by [wepaper](https://github.com/MoonTiwzy/wepaper),
  a renderer written for exactly this: one process and one OpenGL context for all monitors,
  textures and video decoders shared between them, nothing running that no wallpaper needs.
  Changing a setting or a property never restarts it. It is fast: a wallpaper is up in a
  fraction of a second and takes little memory, video memory and CPU. A video is decoded once
  however many monitors show it, on the CPU by default; decoding on the GPU is one switch in
  Settings.

English and Russian: the app asks on the first start, and the language box in Settings
changes it later.

## Requirements

- **Wallpaper Engine, bought and installed in Steam.** Its `assets` folder holds the shaders
  every scene needs. Nothing of Wallpaper Engine is shipped here.
- **Steam running**, for the workshop tabs. The library and your wallpapers work without it.
- **One native Linux game installed in Steam** (any free one will do). Every native game
  carries Valve's `libsteam_api.so`; WE Linux finds it by itself and keeps a copy.
- **Wayland** with layer-shell. Hyprland, Sway, niri and KDE Plasma are each asked through their
  own interface; other compositors through the wlroots protocols they have (see
  [Compositors](#compositors)).
- OpenGL 3.3, and at run time `libmpv` (video and sound) and `ffmpeg` (cheaper copies of
  oversized videos).

## Install

### Any distro

```sh
git clone https://github.com/MoonTiwzy/we-linux
cd we-linux
./install.sh
```

One command builds WE Linux and the renderer and puts `we-linux` and `wepaper` in
`~/.local/bin`, with a menu entry. It needs Rust ([rustup.rs](https://rustup.rs)) and the usual
build tools; if the renderer does not build, the script prints the package list for Arch,
Fedora and Debian. The whole build takes about two minutes.

Web wallpapers are a plugin of the renderer (`wepaper-web`), built when GTK 4, WebKitGTK 6 and
gtk4-layer-shell are installed; `./install.sh --no-web` leaves it out.
The renderer is the commit `flake.lock` names, the same one the Nix package is built with: a
pull that moves the pin rebuilds it on the next `./install.sh`, and one that does not leaves it
alone. `./install.sh --engine` rebuilds it whatever is installed.

### NixOS

Try it without installing anything:

```sh
nix run github:MoonTiwzy/we-linux
```

Or add it to your flake. The package brings the renderer, ffmpeg and the file dialog with it.

```nix
{
  inputs.we-linux.url = "github:MoonTiwzy/we-linux";

  outputs = { nixpkgs, we-linux, ... }: {
    nixosConfigurations.my-pc = nixpkgs.lib.nixosSystem {
      modules = [
        we-linux.nixosModules.default
        {
          programs.we-linux.enable = true;
          # Hyprland without UWSM: turn the service off and use exec-once (see below).
          # programs.we-linux.autostart = false;
        }
      ];
    };
  };
}
```

Home Manager works the same way: `we-linux.homeManagerModules.default` and
`services.we-linux.enable = true;`.

`nix flake check` boots a NixOS VM, starts a compositor in it and opens the window. Two things
on NixOS to know:

- The package is built, and the VM test run, on every push (see the CI). If it does not
  build for you all the same, say so in an issue.
- The workshop tabs load Valve's `libsteam_api.so` from one of your games. The package puts
  `libstdc++` on its library path for it; if the workshop still says Steam does not answer, run
  WE Linux through `steam-run we-linux` and tell us in an issue.

## First run

1. Start Steam, then **WE Linux** from your menu (or `we-linux`).
2. Open **Workshop**, find something, press **Subscribe**.
3. When Steam has it, press **Set wallpaper**. Done.

To have the wallpaper come back after a login, start the daemon with your session:

The switch in Settings ("Start with the session") installs a systemd user service. It starts with
any session that starts `graphical-session.target`: KDE Plasma, niri through `niri-session`,
Hyprland and Sway through UWSM. In a session that does not (a compositor started by hand),
the switch says so and names the line for your config instead:

| Compositor | Add this |
|---|---|
| Hyprland, Lua config | `hl.on("hyprland.start", function() hl.exec_cmd("we-linux daemon") end)` |
| Hyprland, older config | `exec-once = we-linux daemon` |
| Sway | `exec we-linux daemon` |
| niri | `spawn-at-startup "we-linux" "daemon"` |
| NixOS module | nothing, the systemd user service does it |

You do not have to: the window and `we-linux apply` start the daemon when it is not running.

## Compositors

The daemon asks the compositor which monitors there are and what covers them, each through its
own interface, and hears of changes as they happen. It picks the interface from the session
when it starts; `WE_LINUX_COMPOSITOR=hyprland`, `sway`, `niri`, `kwin` or `wayland` picks one by
hand.

| | Hyprland | Sway | niri | KDE Plasma | Others |
|---|---|---|---|---|---|
| Asked through | its two sockets | its IPC socket | its IPC socket | a KWin script of the daemon's, over D-Bus | wlr-foreign-toplevel, wlr-output-power |
| Monitors, as they stand | yes | yes | yes | yes (xdg-output) | yes (xdg-output) |
| Fullscreen window | yes | yes | yes: a window as large as the monitor | yes | with wlr-foreign-toplevel |
| Maximised window | yes | any tiled window (it fills the screen), a floating one as large | a window nearly as large as the monitor | yes | with wlr-foreign-toplevel |
| Window in focus | yes | yes | yes | yes | with wlr-foreign-toplevel |
| Monitor asleep | yes, within three seconds | yes | no: niri does not say | not tried | with wlr-output-power |
| Live wallpaper over a static one | a layer rule the daemon sets | start the daemon after your wallpaper program | start the daemon after your wallpaper program | on the layer above Plasma's desktop (not tried) | start the daemon after your wallpaper program |
| Still frame for | awww, swww, hyprpaper | awww, swww, Sway's own background | awww, swww | awww, swww, Plasma's picture wallpaper (not tried) | awww, swww |

Hyprland 0.57, Sway 1.12, niri 26.04 and KWin 6.8 were run, headless or nested, with the daemon
in them, and the parsers are tested against their replies. "Not tried" is written from KDE's
documentation: Plasma's own shell was not there to try it with. What the table says per
compositor:

- **Hyprland** sends no event when a monitor's power goes off: the daemon sees it on its next
  look, within three seconds. Everything else it hears of at once.
- **Sway** has no maximised windows: a tiled window fills its part of the screen and leaves no
  desktop to see, so it counts as one. Sway restarts swaybg for a new background (`output * bg`,
  the still frame) and on `swaymsg reload`, and the newest layer is on top: the daemon has the
  renderer put its layer back on top both times.
- **niri** says how large a window is, not whether it is fullscreen or maximised: a window the
  size of the monitor counts as fullscreen (a window maximised to the edges with no bar too),
  one at least nine tenths of it each way as maximised. Of a workspace's columns only the one in
  view counts. niri says nothing of a monitor's power.
- **KDE Plasma**: KWin lists no windows over D-Bus, so the daemon loads a small script into KWin
  that tells it what covers each screen, and unloads it when it stops. Plasma's desktop is a
  window under everything, and a wallpaper on the background layer would be under it; the
  renderer goes on the layer above, which also puts it over the desktop's icons.
- **Others** (river, labwc, Wayfire...) are asked through the wlroots protocols. A window on a
  workspace that is not shown still counts there: the protocol does not say which workspace is.

**With awww, swww or the desktop's own wallpaper program.** Keep them. The live wallpaper is
drawn on the same background layer, and the daemon hands your wallpaper program a still frame of
the live wallpaper, so the desktop shows the same picture before the engine is up and while it
is stopped. The picture you had before is remembered and comes back on `we-linux off`, when the
daemon is stopped and when a wallpaper fails to load. Turn the still off with `"still": false` in
the settings file if you want your own picture under it.

**Fullscreen.** A monitor that shows a fullscreen window (a game, a video) keeps the
wallpaper's last frame and does no work; when the desktop is back, the wallpaper goes on from
that frame. Other monitors keep playing. Settings, "Playback", has the original's choices for
a fullscreen window, a maximised one, a focused one, another program's sound and a sleeping
monitor: keep running, mute, pause (that monitor or all of them) or stop, which gives a game the
video memory once every monitor is stopped (the scene starts over afterwards). A choice about
what the compositor does not say does nothing, and its hint says so.

**Colours from the wallpaper.** Set a hook and it runs on every wallpaper change with a still
frame to take colours from:

```jsonc
// ~/.config/we-linux/settings.json
{ "hook": "matugen image \"$WE_LINUX_STILL\"" }      // or: wal -i, wallust run
```

The hook gets `WE_LINUX_WALLPAPER` (the wallpaper's folder), `WE_LINUX_STILL` (a PNG or JPEG of
it) and `WE_LINUX_TITLE`. What it prints goes to `$XDG_RUNTIME_DIR/we-linux/hook.log` (the last
run's), not to the terminal the daemon was started from.

<details>
<summary>The layer rule the daemon sets on Hyprland</summary>

Hyprland stacks the layers of one level by their `order`, lower on top, and the daemon gives the
renderer's layer `order -1` (a static wallpaper's is 0), so it stays on top whichever started
last. It sets the rule again after every config reload, which forgets it. Over Hyprland's
socket, as `hyprctl` would:

```sh
# Lua config (Hyprland 0.55+)
hyprctl eval 'hl.layer_rule({ match = { namespace = "^(wepaper)$" }, order = -1, animation = "fade" })'
# older configs
hyprctl keyword layerrule "order -1, wepaper"
```

If your Hyprland takes neither, add the rule to your config by hand.
</details>

## Command line

```
we-linux                                  the window
we-linux apply <ID|PATH> [--output NAME]  a workshop id, a wallpaper folder, a video or a
                                          picture; on every monitor, or on one
we-linux off                              no live wallpaper
we-linux status                           what is on the desktop
we-linux doctor                           a full report for a bug: versions, what was found,
                                          the settings, the engine's log
we-linux workshop [TEXT] [--sort top|trend|new|subs|updated] [--type scene|video|web] [--page N]
                                          search the workshop: id, size, votes, title
we-linux subscribe ID...                  subscribe and wait for Steam to download
we-linux unsubscribe ID...                unsubscribe (Steam removes the files)
we-linux daemon                           keep the wallpaper on the desktop
```

```sh
we-linux apply 3409533530                 # a subscribed workshop item
we-linux apply ~/Videos/loop.mp4          # any video
we-linux apply 3409533530 --output DP-2   # only on one monitor
```

A picture is handed to awww or swww, whichever runs, else to the desktop's own wallpaper
program (hyprpaper, Sway's background, Plasma's picture wallpaper).

## Settings

The window's **Settings** covers the language, the frame rate, where scene videos are decoded,
sound, playback under other windows and the still frame, each with a hint under the pointer. The file has the
same switches and the hook:

```jsonc
// ~/.config/we-linux/settings.json
{
  "fps": 60,                 // frame rate cap
  "video_gpu": false,        // decode scene videos on the GPU: less CPU, more video memory
  "audio": false,
  "volume": 50,
  // "Playback": "run", "mute", "pause" (that monitor), "pauseall" or "stop"
  "playback_fullscreen": "pause",  // a fullscreen window on the monitor
  "playback_maximized": "run",     // a window maximised over it
  "playback_focus": "run",         // a window has the focus there
  "playback_audio": "run",         // another program plays sound (pactl)
  "playback_sleep": "run",         // the monitor's power is off
  "still": true,             // give the static wallpaper program a still of the wallpaper
  "hook": ""                 // a shell command run when the wallpaper changes
}
```

| What | Where |
|---|---|
| The wallpaper on the desktop, per-monitor wallpapers | `~/.config/we-linux/desktop.json` |
| Changed properties of a wallpaper | `~/.config/we-linux/properties/<id>.json` |
| The playlist and saved playlists | `~/.config/we-linux/playlist.json`, `playlists/` |
| Preview and still caches | `~/.cache/we-linux/` |
| The renderer's log | `$XDG_RUNTIME_DIR/we-linux/engine.log` |
| What the hook printed last | `$XDG_RUNTIME_DIR/we-linux/hook.log` |

## Troubleshooting

Something does not work? Run `we-linux doctor` and read what it says is `NOT FOUND` or
`NOT RUNNING`; paste its output into an issue if that does not explain it.

| You see | Do this |
|---|---|
| The workshop tab says Steam does not answer | Start Steam and log in. Your account must own Wallpaper Engine. |
| "libsteam_api.so was not found" | Install any native Linux game in Steam (a free one is fine) and open WE Linux again. |
| A wallpaper is set but the desktop does not change | `we-linux status`. It prints the error: usually Wallpaper Engine itself is not installed in Steam (no `assets`), or `wepaper` is not on the PATH. Then look at the renderer's log. |
| The static wallpaper covers the live one | Hyprland: the daemon's layer rule did not take, add it by hand (Compositors). Other compositors: start `we-linux daemon` after your wallpaper program. |
| The wallpaper is gone after a reboot | Start `we-linux daemon` with your session (First run). |
| The wallpaper freezes when a game starts | That is the fullscreen freeze, it goes on when the desktop is back. Turn it off in Settings to keep it running. |
| "did not start: not enough video memory" | The scene does not fit your graphics card with what else is open. Close something heavy (a browser with video, a game) and set it again, or pick a lighter wallpaper: 4K scenes with many layers are the expensive ones. |
| High CPU on a scene with a video | Lower the frame rate to 30, or switch video decoding to the GPU in Settings (costs video memory). |
| A scene looks wrong | Run `wepaper check <the wallpaper's folder>`: it names what the renderer leaves out and why. Report it to [wepaper](https://github.com/MoonTiwzy/wepaper/issues) with that output. |

## How it works

```mermaid
flowchart LR
    W["we-linux<br>the window"] -- Steamworks --> S["Steam client<br>workshop, downloads"]
    W -- writes --> F["desktop.json<br>settings.json"]
    C["we-linux apply / off"] -- writes --> F
    F --> D["we-linux daemon"]
    H["the compositor: sockets,<br>D-Bus, Wayland"] --> D
    D -- "tells over a socket" --> P["wepaper<br>one process, all monitors"]
    D -- still frame --> A["awww / swww /<br>the desktop's own"]
    D -- hook --> X["your colours:<br>matugen, wal, wallust"]
```

The window never draws a wallpaper. It writes what you picked to a file; the daemon reads it
and tells the renderer over its socket: this wallpaper on that monitor, this property, that
setting. The renderer keeps running through all of it; it starts once and stops when there is
nothing to show. The window itself renders in software: about 175 MB of RAM and no video
memory.

## Not there yet

- The wallpaper editor and sending wallpapers to a phone.
- Application wallpapers (Windows programs).

## Credits

- [wepaper](https://github.com/MoonTiwzy/wepaper): the renderer.
- [Slint](https://slint.dev) for the interface.
- The icons are [Font Awesome Free](https://fontawesome.com) 6.7.2 by Fonticons, Inc., licensed
  under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) (`ui/icons.slint` holds their
  outlines unchanged).
- [awww](https://codeberg.org/LGFae/awww), swww, hyprpaper, swaybg and Plasma's wallpaper, which
  it plays nicely with.

## License

[MIT](LICENSE). WE Linux is an independent project and is not affiliated with Wallpaper Engine
or Valve. Wallpaper Engine is a trademark of its owner; you need your own copy of it.
