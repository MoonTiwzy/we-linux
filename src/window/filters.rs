//! The filter panel at the left of the grid: its groups of boxes, as the
//! original has them, and what they ask of a list (of the workshop's query,
//! of the installed wallpapers). A change waits a moment before the list
//! follows: several boxes are usually ticked in a row.

use slint::{Color, ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use we_linux_ui::{AppWindow, FilterGroup, FilterOption, Glyph, Icons};

use super::saved::Saved;
use super::tags::{CATEGORIES, GENRES, ONLY, RATINGS, RESOLUTION_GROUPS, SOURCES, TYPES, tag_label};
use super::{App, slot};
use crate::lang::{self, t};
use crate::{desktop, desktop::Monitor};

#[derive(Clone, Copy, PartialEq)]
enum GroupKind {
    /// An item needs one of the checked tags; all or none checked: no limit.
    AnyOf,
    /// An item needs every checked tag.
    AllOf,
}

struct Choice {
    label: &'static str,
    tag: &'static str,
    checked: bool,
    default: bool,
    /// The part of its group it is in: "Тип" has the categories after a
    /// divider, "Разрешение экрана" its sub-groups.
    section: usize,
    /// Of the installed tab only (favourites, applications, the source).
    installed_only: bool,
    /// Something this side can tell; else greyed, as "Совместимые с Android".
    usable: bool,
}

/// The groups of the installed tab only.
const INSTALLED_GROUPS: [&str; 1] = ["Источник"];

struct Group {
    title: &'static str,
    kind: GroupKind,
    open: bool,
    shown: bool,
    boxed: bool,
    choices: Vec<Choice>,
}

impl Group {
    fn new(
        title: &'static str,
        kind: GroupKind,
        open: bool,
        list: &[(&'static str, &'static str)],
        default: impl Fn(&str) -> bool,
    ) -> Self {
        Self {
            title,
            kind,
            open,
            shown: true,
            boxed: false,
            choices: list
                .iter()
                .map(|&(label, tag)| Choice {
                    label,
                    tag,
                    checked: default(tag),
                    default: default(tag),
                    section: 0,
                    installed_only: false,
                    usable: true,
                })
                .collect(),
        }
    }

    fn checked(&self) -> Vec<&'static str> {
        self.choices.iter().filter(|c| c.checked).map(|c| c.tag).collect()
    }

    /// Whether the group is in the panel of the tab (`installed` or not).
    fn applies(&self, installed: bool) -> bool {
        installed || !INSTALLED_GROUPS.contains(&self.title)
    }

    /// The choices the panel of the tab shows.
    fn choices_for(&self, installed: bool) -> impl Iterator<Item = &Choice> {
        self.choices.iter().filter(move |c| installed || !c.installed_only)
    }

    /// Whether the group leaves anything out (on the tab).
    fn narrows_on(&self, installed: bool) -> bool {
        let all = self.choices_for(installed).count();
        let n = self.choices_for(installed).filter(|c| c.checked).count();
        match self.kind {
            GroupKind::AnyOf => n < all,
            GroupKind::AllOf => n > 0,
        }
    }
}

fn default_groups() -> Vec<Group> {
    let mut only = Group::new("Показать только", GroupKind::AllOf, true, &ONLY, |_| false);
    only.boxed = true;
    for choice in &mut only.choices {
        choice.installed_only = choice.tag == "Favorite";
        // The phones the Android app pairs with: not here.
        choice.usable = choice.tag != "Mobile compatible";
    }
    let mut kinds = Group::new("Тип", GroupKind::AnyOf, false, &TYPES, |_| true);
    for choice in &mut kinds.choices {
        choice.installed_only = choice.tag == "Application";
    }
    kinds.choices.extend(CATEGORIES.iter().map(|&(label, tag)| Choice {
        label,
        tag,
        checked: true,
        default: true,
        section: 1,
        installed_only: false,
        usable: true,
    }));
    let resolutions = Group {
        choices: RESOLUTION_GROUPS
            .iter()
            .enumerate()
            .flat_map(|(section, (_, _, tags))| {
                tags.iter().map(move |&tag| Choice {
                    label: tag,
                    tag,
                    checked: true,
                    default: true,
                    section,
                    installed_only: false,
                    usable: true,
                })
            })
            .collect(),
        ..Group::new("Разрешение экрана", GroupKind::AnyOf, false, &[], |_| true)
    };
    let mut sources = Group::new("Источник", GroupKind::AnyOf, true, &SOURCES, |_| true);
    for choice in &mut sources.choices {
        choice.installed_only = true;
    }
    vec![
        only,
        kinds,
        // Like the Windows client: adult content is opt-in.
        Group::new("Возрастной рейтинг", GroupKind::AnyOf, false, &RATINGS, |tag| {
            tag != "Mature"
        }),
        resolutions,
        sources,
        Group::new("Метки", GroupKind::AnyOf, true, &GENRES, |_| true),
    ]
}

/// What the groups ask of the workshop: groups of tags of which an item
/// needs one each, and tags it needs all of. On "Рекомендации" the genres
/// are its own to pick.
fn workshop_terms(groups: &[Group], recommended: bool) -> (Vec<Vec<String>>, Vec<String>) {
    let (mut any_of, mut all_of) = (Vec::new(), Vec::new());
    for group in groups {
        if !group.applies(false) || !group.narrows_on(false) {
            continue;
        }
        // Recommendations pick the genres themselves.
        if recommended && group.title == "Метки" {
            continue;
        }
        match group.kind {
            // Each part of "Тип" (the kinds, the categories) is a choice of its own.
            GroupKind::AnyOf if group.title == "Тип" => {
                for section in 0..2 {
                    let part: Vec<&Choice> =
                        group.choices_for(false).filter(|c| c.section == section).collect();
                    if part.iter().any(|c| !c.checked) {
                        any_of.push(part.iter().filter(|c| c.checked).map(|c| c.tag.to_owned()).collect());
                    }
                }
            }
            GroupKind::AnyOf => any_of
                .push(group.choices_for(false).filter(|c| c.checked).map(|c| c.tag.to_owned()).collect()),
            GroupKind::AllOf => all_of
                .extend(group.choices_for(false).filter(|c| c.checked && c.usable).map(|c| c.tag.to_owned())),
        }
    }
    (any_of, all_of)
}

/// The sub-groups of resolutions that fit the monitors' shape (wide, ultra
/// wide, portrait): what "recommended" ticks.
fn fitting_sections(monitors: &[Monitor]) -> Vec<usize> {
    let shape = |m: &Monitor| {
        let ratio = m.width / m.height.max(1.0);
        if ratio < 1.0 {
            4
        } else if ratio > 2.2 {
            1
        } else {
            0
        }
    };
    if monitors.is_empty() { vec![0] } else { monitors.iter().map(shape).collect() }
}

pub(super) struct Filters {
    groups: Vec<Group>,
    /// The list follows the boxes a moment after the last is ticked.
    timer: Timer,
}

impl Default for Filters {
    fn default() -> Self {
        Self { groups: default_groups(), timer: Timer::default() }
    }
}

impl Filters {
    /// The panel of the tab: the workshop's or the installed one's groups.
    fn show(&self, ui: &AppWindow, online: bool) {
        let installed = !online;
        let icons = ui.global::<Icons>();
        let groups: Vec<FilterGroup> = self
            .groups
            .iter()
            .map(|g| {
                let mut options = Vec::new();
                let mut section = 0;
                for (index, c) in g.choices.iter().enumerate().filter(|(_, c)| installed || !c.installed_only)
                {
                    // A divider before "Тип"'s categories; a heading with "Все" and
                    // "Ничего" before each sub-group of the resolutions.
                    if c.section != section || (index == 0 && g.title == "Разрешение экрана")
                    {
                        section = c.section;
                        let heading = RESOLUTION_GROUPS
                            .get(c.section)
                            .filter(|_| g.title == "Разрешение экрана")
                            .map_or("", |(ru, en, _)| if lang::russian() { ru } else { en });
                        if g.title == "Разрешение экрана" && index > 0 {
                            options.push(FilterOption { kind: 1, choice: -1, ..Default::default() });
                        }
                        if g.title == "Тип" {
                            options.push(FilterOption { kind: 1, choice: -1, ..Default::default() });
                        } else if !heading.is_empty() {
                            options.push(FilterOption {
                                label: heading.into(),
                                kind: 2,
                                section: c.section as i32,
                                choice: -1,
                                ..Default::default()
                            });
                        }
                    }
                    let label = if g.title == "Разрешение экрана" {
                        c.tag.replace("Standard Definition", "SD")
                    } else {
                        tag_label(c.tag).unwrap_or(if lang::russian() { c.label } else { c.tag }).to_owned()
                    };
                    let (icon, tint) = match c.tag {
                        "Approved" if g.boxed => (icons.get_trophy(), Color::from_rgb_u8(0xbe, 0xff, 0xb2)),
                        "Favorite" => (icons.get_heart(), Color::from_rgb_u8(0xff, 0x36, 0xd2)),
                        "Mobile compatible" => (icons.get_mobile(), Color::from_rgb_u8(0xe1, 0xad, 0x01)),
                        _ => (Glyph::default(), Color::default()),
                    };
                    options.push(FilterOption {
                        label: label.into(),
                        checked: c.checked,
                        kind: 0,
                        section: c.section as i32,
                        choice: index as i32,
                        icon,
                        tint,
                        usable: c.usable,
                    });
                }
                FilterGroup {
                    title: lang::label(g.title).into(),
                    open: g.open,
                    filtered: g.narrows_on(installed),
                    shown: g.shown && g.applies(installed),
                    wanted: g.shown,
                    boxed: g.boxed,
                    options: ModelRc::new(VecModel::from(options)),
                }
            })
            .collect();
        ui.set_groups(ModelRc::new(VecModel::from(groups)));
        let filtered = self.groups.iter().any(|g| g.applies(installed) && g.narrows_on(installed));
        ui.set_filtered(filtered);
        // The workshop says only that it is filtered; the installed tab counts (show_installed).
        if online {
            ui.set_filter_count(if filtered { t("(Filtered)", "(Отфильтровано)") } else { "" }.into());
        }
    }

    /// What the groups ask of the workshop (see `workshop_terms`).
    pub(super) fn workshop_terms(&self, recommended: bool) -> (Vec<Vec<String>>, Vec<String>) {
        workshop_terms(&self.groups, recommended)
    }

    /// The ratings ticked: what "Рекомендации" keeps to.
    pub(super) fn ratings(&self) -> Vec<String> {
        self.groups
            .iter()
            .find(|g| g.title == "Возрастной рейтинг")
            .map(|g| g.checked().iter().map(|t| (*t).to_owned()).collect())
            .unwrap_or_default()
    }

    /// What the groups ask of the installed wallpapers: the tags left
    /// unticked, and the ticked boxes of "Показать только".
    pub(super) fn installed_terms(&self) -> (Vec<&'static str>, Vec<&'static str>) {
        let unticked = self
            .groups
            .iter()
            .filter(|g| !g.boxed && g.applies(true))
            .flat_map(|g| g.choices.iter().filter(|c| !c.checked).map(|c| c.tag))
            .collect();
        let only = self
            .groups
            .iter()
            .filter(|g| g.boxed)
            .flat_map(|g| g.choices.iter().filter(|c| c.checked && c.usable).map(|c| c.tag))
            .collect();
        (unticked, only)
    }

    /// A box clicked: whether there was one.
    fn toggle(&mut self, group: i32, option: i32) -> bool {
        match self.groups.get_mut(group as usize).and_then(|g| g.choices.get_mut(option as usize)) {
            Some(choice) => {
                choice.checked = !choice.checked;
                true
            }
            None => false,
        }
    }

    /// Ticks for the saved state: by group title, the tags checked, every tag
    /// the group has, and the groups hidden.
    pub(super) fn save_into(&self, saved: &mut Saved) {
        saved.filters = self
            .groups
            .iter()
            .map(|g| (g.title.to_owned(), g.checked().iter().map(|t| (*t).to_owned()).collect()))
            .collect();
        saved.filter_tags = self
            .groups
            .iter()
            .map(|g| (g.title.to_owned(), g.choices.iter().map(|c| c.tag.to_owned()).collect()))
            .collect();
        saved.hidden_groups = self.groups.iter().filter(|g| !g.shown).map(|g| g.title.to_owned()).collect();
    }

    pub(super) fn restore(&mut self, saved: &Saved) {
        for group in &mut self.groups {
            // Ticks saved before the groups were the original's (no list of the
            // tags they knew) do not fit them: the groups start as they come.
            if let Some(checked) = saved.filters.get(group.title).filter(|_| !saved.filter_tags.is_empty()) {
                let known = saved.filter_tags.get(group.title);
                for choice in
                    group.choices.iter_mut().filter(|c| known.is_none_or(|k| k.iter().any(|t| t == c.tag)))
                {
                    choice.checked = checked.iter().any(|t| t == choice.tag);
                }
            }
            group.shown = !saved.hidden_groups.iter().any(|t| t == group.title);
        }
    }
}

impl App {
    pub(super) fn show_groups(&self) {
        if let Some(ui) = self.ui() {
            self.filters.show(&ui, self.browse.online());
        }
    }

    /// The filters changed: show them and search again after a short pause
    /// (several boxes are usually ticked in a row).
    fn filters_changed(&mut self) {
        self.show_groups();
        self.save();
        self.filters.timer.start(TimerMode::SingleShot, std::time::Duration::from_millis(400), || {
            slot::with(|app| app.first_page_again());
        });
    }

    pub(super) fn filter_toggled(&mut self, group: i32, option: i32) {
        if self.filters.toggle(group, option) {
            self.filters_changed();
        }
    }

    /// "Все" or "Ничего" of a group.
    pub(super) fn filter_group_all(&mut self, group: i32, checked: bool) {
        if let Some(group) = self.filters.groups.get_mut(group as usize) {
            group.choices.iter_mut().for_each(|c| c.checked = checked);
            self.filters_changed();
        }
    }

    /// "Все" or "Ничего" of a sub-group (the resolutions').
    pub(super) fn filter_section_all(&mut self, group: i32, section: i32, checked: bool) {
        if let Some(group) = self.filters.groups.get_mut(group as usize) {
            group
                .choices
                .iter_mut()
                .filter(|c| c.section == section as usize)
                .for_each(|c| c.checked = checked);
            self.filters_changed();
        }
    }

    /// A group's title clicked: it opens or closes.
    pub(super) fn group_toggled(&mut self, group: i32) {
        if let Some(group) = self.filters.groups.get_mut(group as usize) {
            group.open = !group.open;
            self.show_groups();
        }
    }

    /// A group ticked or not in the filter settings: in the panel or not.
    pub(super) fn group_shown(&mut self, group: i32) {
        if let Some(group) = self.filters.groups.get_mut(group as usize) {
            group.shown = !group.shown;
            self.show_groups();
            self.save();
        }
    }

    /// The age settings of the Steam account, on Steam's site.
    pub(super) fn content_preferences(&mut self) {
        let result =
            desktop::open_steam("steam://openurl/https://store.steampowered.com/account/preferences");
        self.report(result, t("Steam's content preferences are open", "Настройки контента Steam открыты"));
    }

    /// The filter settings' answer about anime.
    pub(super) fn anime_changed(&mut self, shown: bool) {
        // As the original: the answer ticks "Аниме" among the tags, or not.
        if let Some(choice) = self
            .filters
            .groups
            .iter_mut()
            .find(|g| g.title == "Метки")
            .and_then(|g| g.choices.iter_mut().find(|c| c.tag == "Anime"))
        {
            choice.checked = shown;
        }
        if let Some(ui) = self.ui() {
            ui.set_anime_shown(shown);
        }
        self.filters_changed();
    }

    /// The resolution settings: the ones that fit the monitors, or all; and
    /// those of no known resolution.
    pub(super) fn resolution_changed(&mut self, recommended: bool, other: bool) {
        // Recommended: the resolutions of the shape of the monitors (wide,
        // ultra wide, portrait), dynamic ones, and unknown ones when wanted.
        let fitting = fitting_sections(&desktop::monitors());
        if let Some(group) = self.filters.groups.iter_mut().find(|g| g.title == "Разрешение экрана")
        {
            for choice in &mut group.choices {
                choice.checked = if choice.tag == "Other resolution" {
                    other
                } else {
                    !recommended || choice.section == 5 || fitting.contains(&choice.section)
                };
            }
        }
        if let Some(ui) = self.ui() {
            ui.set_resolution_recommended(recommended);
            ui.set_other_resolution(other);
        }
        self.filters_changed();
    }

    /// "Сбросить фильтры": every box as it comes.
    pub(super) fn filters_reset(&mut self) {
        for group in &mut self.filters.groups {
            group.choices.iter_mut().for_each(|c| c.checked = c.default);
        }
        self.filters_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group<'a>(groups: &'a mut [Group], title: &str) -> &'a mut Group {
        groups.iter_mut().find(|g| g.title == title).unwrap()
    }

    #[test]
    fn as_they_come_the_groups_ask_the_workshop_only_for_no_adult_content() {
        let groups = default_groups();
        let (any_of, all_of) = workshop_terms(&groups, false);
        assert_eq!(any_of, [vec!["Everyone".to_owned(), "Questionable".to_owned()]]);
        assert!(all_of.is_empty());
        // The source is the installed tab's; nothing in it narrows the workshop.
        assert!(groups.iter().all(|g| g.title != "Источник" || !g.applies(false)));
    }

    #[test]
    fn each_part_of_the_type_is_a_choice_of_its_own() {
        let mut groups = default_groups();
        let kinds = group(&mut groups, "Тип");
        for choice in &mut kinds.choices {
            choice.checked = choice.tag != "Video" && choice.tag != "Preset";
        }
        let (any_of, _) = workshop_terms(&groups, false);
        // "Application" is the installed tab's only: the workshop is not asked about it.
        assert_eq!(any_of[0], ["Scene", "Web"]);
        assert_eq!(any_of[1], ["Wallpaper"]);
    }

    #[test]
    fn the_box_asks_for_all_it_ticks_that_can_be_told() {
        let mut groups = default_groups();
        for choice in &mut group(&mut groups, "Показать только").choices {
            choice.checked = true;
        }
        let (_, all_of) = workshop_terms(&groups, false);
        // Favourites are the installed tab's own, the phones not something this side knows.
        assert_eq!(all_of, ["Approved", "Audio responsive", "Customizable"]);
    }

    #[test]
    fn recommendations_pick_the_genres_themselves() {
        let mut groups = default_groups();
        group(&mut groups, "Метки").choices[0].checked = false;
        assert_eq!(workshop_terms(&groups, false).0.len(), 2);
        assert_eq!(workshop_terms(&groups, true).0.len(), 1);
    }

    #[test]
    fn a_group_narrows_when_it_leaves_something_out_on_its_tab() {
        let mut groups = default_groups();
        let kinds = group(&mut groups, "Тип");
        assert!(!kinds.narrows_on(true));
        kinds.choices.iter_mut().find(|c| c.tag == "Application").unwrap().checked = false;
        assert!(kinds.narrows_on(true));
        // The workshop has no applications to leave out.
        assert!(!kinds.narrows_on(false));
        let only = group(&mut groups, "Показать только");
        assert!(!only.narrows_on(false));
        only.choices[0].checked = true;
        assert!(only.narrows_on(false));
    }

    fn monitor(width: f32, height: f32) -> Monitor {
        Monitor { name: String::new(), detail: String::new(), x: 0.0, y: 0.0, width, height }
    }

    #[test]
    fn the_resolutions_that_fit_go_by_the_shape_of_the_monitors() {
        assert_eq!(fitting_sections(&[]), [0]);
        assert_eq!(fitting_sections(&[monitor(2560.0, 1440.0)]), [0]);
        assert_eq!(fitting_sections(&[monitor(3440.0, 1440.0)]), [1]);
        assert_eq!(fitting_sections(&[monitor(1080.0, 1920.0), monitor(5120.0, 1440.0)]), [4, 1]);
    }
}
