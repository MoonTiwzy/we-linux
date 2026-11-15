//! The device editor: a dialog over the window, opened by clicking a device in
//! "Choose monitor" or from Settings > Devices. It edits the shape and the
//! picture (fill, zoom, shifts) of one saved device.
//!
//! The field shows a snapshot of the device taken wider than the device sees
//! (`framing::WIDE`), placed for the current look by `framing::placement`, so a
//! drag or the wheel moves it at once; a new snapshot is asked whenever the last
//! one is back, a few times a second. Every change goes to the device at once;
//! "OK" keeps it in the settings, "Cancel" puts the saved picture back.

use slint::Model;
use we_linux_ui::{DeviceEntry, DeviceView};

use super::App;
use super::framing::{self, Look};
use super::settings::{device_entry, device_of};
use crate::desktop::{Device, Settings};
use crate::engine;

/// What the editor asks of the renderer: a picture a device shows at once, and
/// a snapshot of a device for the field.
pub(super) enum Live {
    Picture(String, Vec<(&'static str, serde_json::Value)>),
    /// A snapshot for the device's look `asked`, `k` times wider than the device.
    Shot {
        name: String,
        asked: Look,
        k: f32,
    },
}

/// A snapshot as it came back: the renderer's answer and the picture.
type Shot = Option<(serde_json::Value, image::RgbaImage)>;

/// The thread that asks: the window never waits for the renderer. Of what
/// piled up while it was busy, the newest picture of a device and one snapshot count.
pub(super) fn live() -> &'static std::sync::mpsc::Sender<Live> {
    static LIVE: std::sync::OnceLock<std::sync::mpsc::Sender<Live>> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Live>();
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut pictures: Vec<(String, Vec<(&'static str, serde_json::Value)>)> = Vec::new();
                let mut shot = None;
                for job in std::iter::once(first).chain(rx.try_iter()) {
                    match job {
                        Live::Picture(name, picture) => {
                            pictures.retain(|(other, _)| *other != name);
                            pictures.push((name, picture));
                        }
                        Live::Shot { name, asked, k } => shot = Some((name, asked, k)),
                    }
                }
                for (name, picture) in pictures {
                    for (key, value) in picture {
                        let _ = engine::call(serde_json::json!({
                            "cmd": "set", "path": format!("outputs/{name}/{key}"), "value": value
                        }));
                    }
                }
                if let Some((name, asked, k)) = shot {
                    let taken = snapshot(&name, framing::view_of(asked, k));
                    super::slot::on_ui(move |app| app.device_shot_arrived(&name, asked, k, taken));
                }
            }
        });
        tx
    })
}

/// What the device `name` would show with the look `view`; a renderer without
/// `snapshot-look` gives what it shows and answers nothing.
fn snapshot(name: &str, view: Look) -> Shot {
    let file = engine::runtime_dir().join("device-preview.png");
    let reply = engine::call(serde_json::json!({
        "cmd": "snapshot",
        "file": file,
        "output": name,
        "look": { "zoom": view.zoom / 100.0, "shift_x": view.shift_x / 100.0, "shift_y": view.shift_y / 100.0 },
    }))
    .ok()?;
    let picture = image::open(&file).ok()?.to_rgba8();
    Some((reply, picture))
}

thread_local! {
    /// Asks for a snapshot a few times a second while the editor is open.
    static SHOTS: slint::Timer = slint::Timer::default();
}

/// The editor's own state; what it edits is the window's `device-editor`.
pub(super) struct Editor {
    /// The row of the saved devices that is open, and the name the renderer knows it by.
    open: Option<(usize, String)>,
    /// How much wider than the device the field is: `WIDE`, or 1 with a renderer
    /// that cannot snapshot a framing of its own.
    k: f32,
    /// The device's look the snapshot on show was taken for.
    shot: Option<Look>,
    /// The look when the drag started.
    from: Look,
    /// A snapshot is asked and not back yet.
    asking: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            open: None,
            k: framing::WIDE,
            shot: None,
            from: Look { zoom: 100.0, shift_x: 0.0, shift_y: 0.0 },
            asking: false,
        }
    }
}

impl App {
    /// Opens the editor on the saved device of that row.
    pub(super) fn open_device_editor(&mut self, index: i32) {
        let Some(ui) = self.ui() else { return };
        let Some((index, device)) = usize::try_from(index)
            .ok()
            .and_then(|i| Settings::load().devices.into_iter().nth(i).map(|d| (i, d)))
        else {
            return;
        };
        let k = self.device_editor.k;
        self.device_editor = Editor { open: Some((index, device.name.clone())), k, ..Editor::default() };
        ui.set_device_editor(device_entry(&device));
        ui.set_device_editor_shown(false);
        ui.set_device_editor_view(DeviceView { scale: 1.0, x: 0.0, y: 0.0, wide: k });
        ui.set_device_editor_open(true);
        self.ask_device_shot();
        SHOTS.with(|timer| {
            timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(300), || {
                super::slot::try_with(App::ask_device_shot);
            });
        });
    }

    /// The device as the editor has it now.
    fn edited_device(&self) -> Option<Device> {
        self.ui().map(|ui| device_of(&ui.get_device_editor()))
    }

    /// A snapshot of the device, unless one is on its way.
    fn ask_device_shot(&mut self) {
        let (Some((_, name)), Some(device)) = (self.device_editor.open.clone(), self.edited_device()) else {
            return;
        };
        if self.device_editor.asking {
            return;
        }
        self.device_editor.asking = true;
        let _ = live().send(Live::Shot { name, asked: Look::from(&device), k: self.device_editor.k });
    }

    /// A snapshot came back (nothing: the device shows nothing, or is gone).
    fn device_shot_arrived(&mut self, name: &str, asked: Look, k: f32, taken: Shot) {
        self.device_editor.asking = false;
        let Some(ui) = self.ui() else { return };
        // Closed meanwhile, or opened on another device: not this one's.
        if self.device_editor.open.as_ref().is_none_or(|(_, open)| open != name) {
            return;
        }
        let Some((reply, picture)) = taken else {
            ui.set_device_editor_shown(false);
            return;
        };
        let (shot, k) = framing::drawn(&reply, asked, k);
        self.device_editor.k = k;
        self.device_editor.shot = Some(shot);
        let (width, height) = picture.dimensions();
        ui.set_device_editor_shot(slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
            picture.as_raw(),
            width,
            height,
        )));
        ui.set_device_editor_shown(true);
        self.place_device_shot();
    }

    /// The snapshot on show, placed for the look the device has now.
    fn place_device_shot(&self) {
        let (Some(ui), Some(shot), Some(device)) = (self.ui(), self.device_editor.shot, self.edited_device())
        else {
            return;
        };
        let k = self.device_editor.k;
        let at = framing::placement(shot, Look::from(&device), k);
        ui.set_device_editor_view(DeviceView { scale: at.scale, x: at.x, y: at.y, wide: k });
    }

    /// The device got a new look: the field follows and the device shows it.
    fn set_device_look(&mut self, look: Look) {
        let Some(ui) = self.ui() else { return };
        let mut entry = ui.get_device_editor();
        entry.zoom = look.zoom;
        entry.shift_x = look.shift_x;
        entry.shift_y = look.shift_y;
        ui.set_device_editor(entry);
        self.device_edited();
    }

    pub(super) fn device_drag_start(&mut self) {
        if let Some(device) = self.edited_device() {
            self.device_editor.from = Look::from(&device);
        }
    }

    pub(super) fn device_drag(&mut self, dx: f32, dy: f32) {
        self.set_device_look(framing::drag(self.device_editor.from, dx, dy, self.device_editor.k));
    }

    pub(super) fn device_wheel(&mut self, at_x: f32, at_y: f32, delta: f32) {
        if let Some(device) = self.edited_device() {
            self.set_device_look(framing::wheel(
                Look::from(&device),
                at_x,
                at_y,
                delta,
                self.device_editor.k,
            ));
        }
    }

    /// Something in the editor changed the device: it shows it at once.
    pub(super) fn device_edited(&mut self) {
        let (Some((_, name)), Some(device)) = (self.device_editor.open.clone(), self.edited_device()) else {
            return;
        };
        self.place_device_shot();
        let _ = live().send(Live::Picture(name, device.picture().into()));
    }

    /// "OK": the picture is kept with the device, in the settings and in the
    /// settings' own rows when they are open.
    pub(super) fn device_editor_ok(&mut self) {
        if let (Some((index, name)), Some(edited)) = (self.device_editor.open.clone(), self.edited_device()) {
            let mut settings = Settings::load();
            if let Some(saved) = settings.devices.get_mut(index) {
                *saved = saved.clone().with_picture(&edited);
                if let Err(e) = settings.save() {
                    self.warn(e);
                }
            }
            // The settings' row of that device, wherever the unsaved rows put it.
            if let Some(ui) = self.ui()
                && ui.get_settings_open()
                && let Some((at, row)) =
                    ui.get_devices().iter().enumerate().find(|(_, row)| row.name == name.as_str())
            {
                let kept =
                    DeviceEntry { saved: row.saved, ..device_entry(&device_of(&row).with_picture(&edited)) };
                ui.get_devices().set_row_data(at, kept);
            }
        }
        self.close_device_editor();
    }

    /// "Cancel": the device shows its saved picture again.
    pub(super) fn device_editor_cancel(&mut self) {
        if let Some((index, name)) = self.device_editor.open.clone()
            && let Some(saved) = Settings::load().devices.into_iter().nth(index)
        {
            let _ = live().send(Live::Picture(name, saved.picture().into()));
        }
        self.close_device_editor();
    }

    fn close_device_editor(&mut self) {
        self.device_editor.open = None;
        self.device_editor.shot = None;
        SHOTS.with(slint::Timer::stop);
        if let Some(ui) = self.ui() {
            ui.set_device_editor_open(false);
        }
    }
}
