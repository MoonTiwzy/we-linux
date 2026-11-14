//! `--show key=value,...`: the window opens in a state of its own, for
//! screenshots and checks. Not in the usage: it is a tool of development.
//!
//! tab=N, filters=0|1, select=N, settings=N (its tab), monitors, dialog=N,
//! menu=N, mini, pick=a.b.c (tiles picked as Ctrl would), ask (the question
//! Delete puts), band (a rectangle drawn over the grid).

use std::collections::HashMap;
use std::time::Duration;

use slint::{Timer, TimerMode};
use we_linux_ui::AppWindow;

use super::{App, slot};
use crate::selection::Takes;

fn shown_state() -> HashMap<String, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    args.iter()
        .position(|a| a == "--show")
        .and_then(|i| args.get(i + 1))
        .map(|spec| {
            spec.split(',')
                .filter_map(|pair| pair.split_once('=').or(Some((pair, "1"))).filter(|(k, _)| !k.is_empty()))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Shape the window as `--show` asks. `start_monitors` is the monitor
/// chooser due to come down by itself: any state asked for keeps it up,
/// unless it is the chooser that is asked for. The timers returned must
/// live as long as the window.
pub(super) fn apply(ui: &AppWindow, start_monitors: &Timer) -> [Timer; 2] {
    let state = shown_state();
    let number = |key: &str| state.get(key).and_then(|v| v.parse::<i32>().ok());
    let pending_select = Timer::default();
    let pending_monitors = Timer::default();
    slot::with(|app| {
        if let Some(tab) = number("tab") {
            app.switch_tab(tab);
        }
        if let Some(open) = number("filters") {
            ui.set_filters_open(open != 0);
        }
        if let Some(tab) = number("settings") {
            app.show_settings();
            ui.set_settings_tab(tab);
            ui.set_settings_open(true);
        }
    });
    if number("monitors").is_some() {
        // The blurred window behind it needs the window on the screen first.
        ui.set_monitors_open(false);
        pending_monitors.start(TimerMode::SingleShot, Duration::from_millis(2000), || {
            slot::with(|app| app.blur_backdrop());
        });
    } else if !state.is_empty() {
        start_monitors.stop();
        ui.set_monitors_open(false);
    }
    if let Some(dialog) = number("dialog") {
        ui.set_dialog(dialog);
    }
    if let Some(menu) = number("menu") {
        ui.set_debug_menu(menu);
    }
    if number("mini").is_some() {
        ui.set_mini(true);
    }
    // A selection waits for the grid (the workshop answers later); it is
    // shown without going on the desktop. `pick=2.5.6` picks those tiles
    // besides, as Ctrl would; `ask` puts the question Delete would, for
    // the selection and the picked ones (only OK, which nothing presses
    // here, would unsubscribe).
    let select = number("select");
    let picks: Vec<usize> = state
        .get("pick")
        .map(|list| list.split('.').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_default();
    let ask = state.contains_key("ask");
    if state.contains_key("band") {
        ui.set_debug_band(true);
    }
    if select.is_some() || !picks.is_empty() || ask {
        pending_select.start(TimerMode::SingleShot, Duration::from_millis(2500), move || {
            slot::with(|app| app.show_picked(select, &picks, ask));
        });
    }
    [pending_select, pending_monitors]
}

impl App {
    /// The selection and the picked tiles `--show` asked for, and the question.
    fn show_picked(&mut self, select: Option<i32>, picks: &[usize], ask: bool) {
        if let Some(index) = select.map(|i| i as usize).filter(|i| *i < self.grid.cards().len()) {
            self.grid.select(Some(index));
            self.show_selection(false);
            self.show_presets();
        }
        let ids: Vec<u64> = picks.iter().filter_map(|i| self.grid.card(*i)).map(|c| c.id).collect();
        self.grid.add_picks(&ids);
        self.refresh_marks();
        if ask {
            let set = self.set_for(None, Takes::Subscribed);
            if !set.is_empty() {
                self.ask(set);
            }
        }
    }
}
