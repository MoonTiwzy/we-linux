//! A compositor with no IPC of its own, asked through Wayland protocols:
//! `wl_output` and xdg-output for the monitors, wlr-foreign-toplevel-management
//! for the windows (their states and the outputs they are on), and
//! wlr-output-power-management for a monitor's power. Each says what it says
//! in its protocol's XML; what a compositor does not offer is not known.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{
    self, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{
    self, ZwlrForeignToplevelManagerV1,
};
use wayland_protocols_wlr::output_power_management::v1::client::zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1;
use wayland_protocols_wlr::output_power_management::v1::client::zwlr_output_power_v1::{
    self, ZwlrOutputPowerV1,
};

use super::{Change, Compositor, Monitor, Over, Tells};

/// The states of a toplevel, as wlr-foreign-toplevel-management numbers them.
const MAXIMIZED: u32 = 0;
const MINIMIZED: u32 = 1;
const ACTIVATED: u32 = 2;
const FULLSCREEN: u32 = 3;

#[derive(Default)]
pub struct Wayland {
    /// What the watch saw last; None until it runs.
    seen: Arc<Mutex<Option<Covers>>>,
}

/// What is over each monitor, by its name.
type Covers = Vec<(String, Over)>;

/// A monitor as its events describe it.
struct Output {
    /// The registry's name of its global: what its objects carry.
    global: u32,
    name: String,
    /// xdg-output's logical place and size.
    logical: Option<(i32, i32, i32, i32)>,
    /// `wl_output.geometry`'s place, for a compositor without xdg-output.
    place: (i32, i32),
    mode: (i32, i32),
    turned: bool,
    scale: i32,
    asleep: Option<bool>,
    xdg: Option<ZxdgOutputV1>,
    power: Option<ZwlrOutputPowerV1>,
}

impl Output {
    fn monitor(&self) -> Monitor {
        let (width, height) = if self.turned { (self.mode.1, self.mode.0) } else { self.mode };
        let scale = self.scale.max(1);
        let (x, y, logical_width, logical_height) =
            self.logical.unwrap_or((self.place.0, self.place.1, width / scale, height / scale));
        Monitor {
            name: self.name.clone(),
            x: x as f32,
            y: y as f32,
            width: logical_width as f32,
            height: logical_height as f32,
            pixels: (width.max(0) as u32, height.max(0) as u32),
        }
    }
}

#[derive(Default)]
struct Toplevel {
    /// The globals of the outputs it is on.
    outputs: Vec<u32>,
    states: Vec<u32>,
}

#[derive(Default)]
struct State {
    outputs: Vec<Output>,
    toplevels: HashMap<ObjectId, Toplevel>,
    xdg: Option<ZxdgOutputManagerV1>,
    power: Option<ZwlrOutputPowerManagerV1>,
    /// Held for its events: the toplevels.
    _windows: Option<ZwlrForeignToplevelManagerV1>,
}

impl State {
    fn add_output(&mut self, registry: &WlRegistry, global: u32, version: u32, qh: &QueueHandle<State>) {
        let wl: WlOutput = registry.bind(global, version.min(4), qh, global);
        let xdg = self.xdg.as_ref().map(|manager| manager.get_xdg_output(&wl, qh, global));
        let power = self.power.as_ref().map(|manager| manager.get_output_power(&wl, qh, global));
        self.outputs.push(Output {
            global,
            name: String::new(),
            logical: None,
            place: (0, 0),
            mode: (0, 0),
            turned: false,
            scale: 1,
            asleep: None,
            xdg,
            power,
        });
    }

    fn output(&mut self, global: u32) -> Option<&mut Output> {
        self.outputs.iter_mut().find(|o| o.global == global)
    }

    fn over(&self) -> Vec<(String, Over)> {
        let outputs: Vec<(u32, String, bool)> =
            self.outputs.iter().map(|o| (o.global, o.name.clone(), o.asleep == Some(true))).collect();
        let windows: Vec<(Vec<u32>, Vec<u32>)> =
            self.toplevels.values().map(|t| (t.outputs.clone(), t.states.clone())).collect();
        over_of(&outputs, &windows)
    }
}

/// What is over each output (its global, its name, whether it sleeps), from
/// the toplevels' outputs and states. A minimised window is on no screen; a
/// window on a workspace not shown is on its output all the same, the
/// protocol has no word for that.
fn over_of(outputs: &[(u32, String, bool)], windows: &[(Vec<u32>, Vec<u32>)]) -> Vec<(String, Over)> {
    outputs
        .iter()
        .map(|(global, name, asleep)| {
            let on: Vec<&Vec<u32>> = windows
                .iter()
                .filter(|(outputs, states)| outputs.contains(global) && !states.contains(&MINIMIZED))
                .map(|(_, states)| states)
                .collect();
            let covered = on
                .iter()
                .map(
                    |states| {
                        if states.contains(&FULLSCREEN) { 2 } else { u8::from(states.contains(&MAXIMIZED)) }
                    },
                )
                .max()
                .unwrap_or(0);
            let focused = on.iter().any(|states| states.contains(&ACTIVATED));
            (name.clone(), Over { focused, covered, asleep: *asleep })
        })
        .collect()
}

/// A connection with the outputs bound, and with the windows and the power
/// too when `everything`, its first answers in.
fn connect(everything: bool) -> Option<(GlobalList, EventQueue<State>, State)> {
    let connection = Connection::connect_to_env().ok()?;
    let (globals, mut queue) = registry_queue_init::<State>(&connection).ok()?;
    let qh = queue.handle();
    let mut state = State { xdg: globals.bind(&qh, 1..=3, ()).ok(), ..State::default() };
    if everything {
        state.power = globals.bind(&qh, 1..=1, ()).ok();
    }
    for global in globals.contents().clone_list() {
        if global.interface == WlOutput::interface().name {
            state.add_output(globals.registry(), global.name, global.version, &qh);
        }
    }
    queue.roundtrip(&mut state).ok()?;
    // The toplevels after the outputs: a compositor tells which outputs a
    // toplevel is on only of the outputs the client has bound already.
    if everything {
        state._windows = globals.bind(&qh, 1..=3, ()).ok();
        queue.roundtrip(&mut state).ok()?;
    }
    queue.roundtrip(&mut state).ok()?;
    Some((globals, queue, state))
}

/// The monitors, as wl_output and xdg-output describe them.
pub(super) fn monitors() -> Option<Vec<Monitor>> {
    let (_, _, state) = connect(false)?;
    Some(state.outputs.iter().map(Output::monitor).collect())
}

impl Compositor for Wayland {
    fn name(&self) -> &'static str {
        "Wayland"
    }

    fn monitors(&self) -> Option<Vec<Monitor>> {
        monitors()
    }

    fn over(&self) -> Option<Vec<(String, Over)>> {
        if !self.tells().fullscreen {
            return None;
        }
        if let Some(seen) = self.seen.lock().ok()?.clone() {
            return Some(seen);
        }
        let (_, _, state) = connect(true)?;
        Some(state.over())
    }

    fn tells(&self) -> Tells {
        static TELLS: OnceLock<Tells> = OnceLock::new();
        *TELLS.get_or_init(|| {
            let Some((globals, _, _)) = connect(false) else {
                return Tells::NOTHING;
            };
            let has =
                |interface: &str| globals.contents().clone_list().iter().any(|g| g.interface == interface);
            let windows = has(ZwlrForeignToplevelManagerV1::interface().name);
            Tells {
                focus: windows,
                maximized: windows,
                fullscreen: windows,
                sleep: has(ZwlrOutputPowerManagerV1::interface().name),
            }
        })
    }

    fn watch(&self, changes: Sender<Change>) {
        let seen = Arc::clone(&self.seen);
        std::thread::spawn(move || {
            let Some((_globals, mut queue, mut state)) = connect(true) else {
                return;
            };
            loop {
                let over = state.over();
                let changed = seen.lock().is_ok_and(|mut seen| {
                    let changed = seen.as_ref() != Some(&over);
                    *seen = Some(over);
                    changed
                });
                if changed && changes.send(Change::Covers).is_err() {
                    return;
                }
                // The compositor went: nothing more is known.
                if queue.blocking_dispatch(&mut state).is_err() {
                    if let Ok(mut seen) = seen.lock() {
                        *seen = None;
                    }
                    return;
                }
            }
        });
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version }
                if interface == WlOutput::interface().name =>
            {
                state.add_output(registry, name, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(output) = state.outputs.iter().find(|o| o.global == name) {
                    if let Some(xdg) = &output.xdg {
                        xdg.destroy();
                    }
                    if let Some(power) = &output.power {
                        power.destroy();
                    }
                }
                state.outputs.retain(|o| o.global != name);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.output(*global) else { return };
        match event {
            wl_output::Event::Geometry { x, y, transform, .. } => {
                output.place = (x, y);
                output.turned = matches!(
                    transform,
                    WEnum::Value(
                        wl_output::Transform::_90
                            | wl_output::Transform::_270
                            | wl_output::Transform::Flipped90
                            | wl_output::Transform::Flipped270
                    )
                );
            }
            wl_output::Event::Mode { flags: WEnum::Value(flags), width, height, .. }
                if flags.contains(wl_output::Mode::Current) =>
            {
                output.mode = (width, height);
            }
            wl_output::Event::Scale { factor } => output.scale = factor,
            wl_output::Event::Name { name } => output.name = name,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.output(*global) else { return };
        let logical = output.logical.get_or_insert((0, 0, 0, 0));
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => (logical.0, logical.1) = (x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => (logical.2, logical.3) = (width, height),
            // wl_output version 4 names it too; this is for older compositors.
            zxdg_output_v1::Event::Name { name } if output.name.is_empty() => output.name = name,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.insert(toplevel.id(), Toplevel::default());
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let global = |output: &WlOutput| output.data::<u32>().copied();
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::State { state: bytes } => {
                let states = bytes.as_chunks::<4>().0.iter().map(|b| u32::from_ne_bytes(*b)).collect();
                state.toplevels.entry(handle.id()).or_default().states = states;
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputEnter { output } => {
                if let Some(global) = global(&output) {
                    state.toplevels.entry(handle.id()).or_default().outputs.push(global);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputLeave { output } => {
                if let (Some(global), Some(toplevel)) =
                    (global(&output), state.toplevels.get_mut(&handle.id()))
                {
                    toplevel.outputs.retain(|o| *o != global);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                state.toplevels.remove(&handle.id());
                handle.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputPowerV1,
        event: zwlr_output_power_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.output(*global) else { return };
        match event {
            zwlr_output_power_v1::Event::Mode { mode } => {
                output.asleep = Some(mode == WEnum::Value(zwlr_output_power_v1::Mode::Off));
            }
            zwlr_output_power_v1::Event::Failed => output.asleep = None,
            _ => {}
        }
    }
}

/// Managers send nothing that matters here.
macro_rules! silent {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, ()> for State {
            fn event(
                _: &mut Self,
                _: &$interface,
                _: <$interface as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}
silent!(ZxdgOutputManagerV1, ZwlrOutputPowerManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs() -> Vec<(u32, String, bool)> {
        vec![(10, "DP-1".into(), false), (11, "HDMI-A-1".into(), true)]
    }

    #[test]
    fn a_fullscreen_window_covers_its_output_only() {
        let over = over_of(&outputs(), &[(vec![10], vec![FULLSCREEN, ACTIVATED])]);
        assert_eq!(
            over,
            vec![
                ("DP-1".into(), Over { focused: true, covered: 2, asleep: false }),
                ("HDMI-A-1".into(), Over { focused: false, covered: 0, asleep: true }),
            ]
        );
    }

    #[test]
    fn maximised_beats_nothing_and_fullscreen_beats_maximised() {
        let windows =
            [(vec![10], vec![MAXIMIZED]), (vec![10, 11], vec![]), (vec![11], vec![MAXIMIZED, FULLSCREEN])];
        let over = over_of(&outputs(), &windows);
        assert_eq!((over[0].1.covered, over[1].1.covered), (1, 2));
        assert!(!over[0].1.focused);
    }

    #[test]
    fn a_minimised_window_is_on_no_screen() {
        let over = over_of(&outputs(), &[(vec![10], vec![FULLSCREEN, MINIMIZED, ACTIVATED])]);
        assert_eq!(over[0].1, Over::default());
    }
}
