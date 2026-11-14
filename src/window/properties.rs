//! The selected wallpaper's properties (the details' sliders, boxes and
//! colours) and its presets: named sets of them. A change is kept on the
//! disk and told to the renderers that play the wallpaper at once.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use slint::{Color, ModelRc, SharedString, VecModel};
use we_linux_ui::{AppWindow, Prop};

use super::App;
use super::card::Card;
use crate::lang::t;
use crate::{desktop, props, tf};

#[derive(Default)]
pub(super) struct Properties {
    /// The selected item's properties and what the user changed of them.
    list: Vec<props::Property>,
    overrides: BTreeMap<String, Value>,
    /// Rows of the properties shown (conditions hide some), by index into `list`.
    shown: Vec<usize>,
}

impl Properties {
    /// The value of every property now: the user's or the wallpaper's own.
    fn values(&self) -> BTreeMap<String, Value> {
        self.list
            .iter()
            .map(|p| (p.name.clone(), self.overrides.get(&p.name).unwrap_or(&p.default).clone()))
            .collect()
    }

    /// The property of a row shown.
    fn at(&self, row: usize) -> Option<&props::Property> {
        self.shown.get(row).and_then(|&i| self.list.get(i))
    }

    fn show(&mut self, ui: &AppWindow) {
        let values = self.values();
        self.shown.clear();
        let mut rows = Vec::new();
        for (index, property) in self.list.iter().enumerate() {
            if !props::holds(&property.condition, &values) {
                continue;
            }
            let value = &values[&property.name];
            let (r, g, b) = props::rgb(value);
            self.shown.push(index);
            rows.push(Prop {
                label: property.label.as_str().into(),
                kind: match property.kind {
                    props::Kind::Bool => 0,
                    props::Kind::Slider => 1,
                    props::Kind::Color => 2,
                    props::Kind::Combo => 3,
                    props::Kind::Text => 4,
                    props::Kind::Label => 5,
                    props::Kind::Group => 6,
                },
                on: value.as_bool().unwrap_or(false),
                number: value.as_f64().unwrap_or(0.0) as f32,
                min: property.min as f32,
                max: property.max.max(property.min + 0.0001) as f32,
                text: match property.kind {
                    props::Kind::Color => props::hex(value),
                    _ => value.as_str().map(str::to_owned).unwrap_or_default(),
                }
                .into(),
                options: ModelRc::new(VecModel::from(
                    property
                        .options
                        .iter()
                        .map(|(label, _)| SharedString::from(label.as_str()))
                        .collect::<Vec<_>>(),
                )),
                index: property
                    .options
                    .iter()
                    .position(|(_, v)| props::same(v, value))
                    .map_or(-1, |i| i as i32),
                swatch: Color::from_rgb_u8(r, g, b),
                usable: property.usable,
                hint: property.hint.as_str().into(),
                usable_options: ModelRc::new(VecModel::from(property.usable_options.clone())),
            });
        }
        ui.set_props(ModelRc::new(VecModel::from(rows)));
    }
}

impl App {
    /// A wallpaper newly selected: its properties (when it is on the disk)
    /// and what the user set of them.
    pub(super) fn load_properties(&mut self, card: &Card, dir: Option<&Path>) {
        self.properties.list = dir.map(props::load).unwrap_or_default();
        self.properties.overrides = props::overrides(card.id);
        self.show_properties();
    }

    fn show_properties(&mut self) {
        if let Some(ui) = self.ui() {
            self.properties.show(&ui);
        }
    }

    /// The user changed the property in row `row`: keep it, tell the engines
    /// that play this wallpaper (on every monitor or on one) and redraw.
    fn set_property(&mut self, row: usize, value: Value) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let Some(property) = self.properties.at(row).cloned() else {
            return;
        };
        let overrides = &mut self.properties.overrides;
        if props::same(&value, &property.default) {
            overrides.remove(&property.name);
        } else {
            overrides.insert(property.name.clone(), value.clone());
        }
        props::save_overrides(card.id, overrides);
        if let Some(dir) = self.dir_of(&card) {
            props::tell_engines(&dir, &property.name, &value);
        }
        self.show_properties();
    }

    pub(super) fn prop_bool(&mut self, row: i32, value: bool) {
        self.set_property(row.max(0) as usize, Value::Bool(value));
    }

    pub(super) fn prop_number(&mut self, row: i32, value: f32) {
        // Two decimals are what the sliders of the Windows client give.
        let rounded = (f64::from(value) * 100.0).round() / 100.0;
        self.set_property(row.max(0) as usize, serde_json::json!(rounded));
    }

    pub(super) fn prop_text(&mut self, row: i32, text: &str) {
        let row = row.max(0) as usize;
        match self.properties.at(row).map(|p| p.kind) {
            Some(props::Kind::Color) => match props::from_hex(text) {
                Some(value) => self.set_property(row, value),
                None => self.status(t("A colour is written as #rrggbb", "Цвет пишется как #rrggbb")),
            },
            Some(_) => self.set_property(row, Value::String(text.to_owned())),
            None => {}
        }
    }

    pub(super) fn prop_index(&mut self, row: i32, index: i32) {
        let row = row.max(0) as usize;
        let value = self
            .properties
            .at(row)
            .and_then(|p| p.options.get(index.max(0) as usize))
            .map(|(_, value)| value.clone());
        if let Some(value) = value {
            self.set_property(row, value);
        }
    }

    pub(super) fn reset_properties(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        if let Some(dir) = self.dir_of(&card) {
            let properties = &self.properties;
            for property in properties.list.iter().filter(|p| properties.overrides.contains_key(&p.name)) {
                props::tell_engines(&dir, &property.name, &property.default);
            }
        }
        self.properties.overrides.clear();
        props::save_overrides(card.id, &self.properties.overrides);
        self.show_properties();
        self.status(t("Properties reset", "Свойства сброшены"));
    }

    // ------------------------------------------------------------- presets

    /// The presets of the selected wallpaper: named sets of its properties.
    pub(super) fn show_presets(&self) {
        let Some(ui) = self.ui() else { return };
        let names: Vec<SharedString> = self
            .selected_card()
            .map(|card| props::presets(card.id))
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();
        ui.set_presets(ModelRc::new(VecModel::from(names)));
    }

    pub(super) fn preset_save(&mut self, name: &str) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let result = props::save_preset(card.id, name, &self.properties.overrides);
        self.report(result, &tf!("Preset \"{}\" saved", "Пресет «{}» сохранён", name.trim()));
        self.show_presets();
    }

    pub(super) fn preset_load(&mut self, row: usize) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let Some(name) = props::presets(card.id).into_iter().nth(row) else {
            return;
        };
        let Some(values) = props::preset(card.id, &name) else {
            return;
        };
        // The renderer is told every property that is another value now.
        if let Some(dir) = self.dir_of(&card) {
            for property in &self.properties.list {
                let before = self.properties.overrides.get(&property.name).unwrap_or(&property.default);
                let after = values.get(&property.name).unwrap_or(&property.default);
                if !props::same(before, after) {
                    props::tell_engines(&dir, &property.name, after);
                }
            }
        }
        self.properties.overrides = values;
        props::save_overrides(card.id, &self.properties.overrides);
        self.show_properties();
        self.status(tf!("Preset \"{name}\" loaded", "Пресет «{name}» загружен"));
    }

    /// Give every other installed wallpaper the values set here, for the
    /// properties it has by the same name and of the same kind.
    pub(super) fn preset_apply_all(&mut self) {
        let Some(card) = self.selected_card() else {
            return;
        };
        let mine = &self.properties;
        let mut changed = 0;
        for item in self.catalog.installed().iter().filter(|item| item.id != card.id) {
            let theirs = props::load(&item.dir);
            let mut values = props::overrides(item.id);
            let before = values.clone();
            for property in mine.list.iter().filter(|p| mine.overrides.contains_key(&p.name)) {
                if theirs.iter().any(|p| {
                    p.name == property.name
                        && std::mem::discriminant(&p.kind) == std::mem::discriminant(&property.kind)
                }) && let Some(value) = mine.overrides.get(&property.name)
                {
                    values.insert(property.name.clone(), value.clone());
                    props::tell_engines(&item.dir, &property.name, value);
                }
            }
            if values != before {
                props::save_overrides(item.id, &values);
                changed += 1;
            }
        }
        self.status(tf!("Applied to {changed} wallpapers", "Применено к обоям: {changed}"));
    }

    /// The values set here as JSON, on the clipboard.
    pub(super) fn preset_share(&mut self) {
        let json = serde_json::to_string(&self.properties.overrides).unwrap_or_default();
        self.report(
            desktop::copy(&json),
            t("The preset is on the clipboard as JSON", "Пресет в буфере обмена как JSON"),
        );
    }
}
