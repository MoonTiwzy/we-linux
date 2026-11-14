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
//! `we-linux daemon` keeps the wallpaper on the desktop (engine.rs); `apply`,
//! `off` and `status` are the command line (see `usage`).

mod desktop;
mod engine;
mod lang;
mod library;
mod previews;
mod props;
mod steam;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use serde_json::Value;
use slint::{Color, ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel, Weak};

use desktop::{Playlist, Settings};
use lang::t;
use previews::{Decoded, Job, Previews, Source};

unsafe extern "C" {
    /// glibc: give the freed heap back to the system.
    fn malloc_trim(pad: usize) -> i32;
}
use steam::{Command, Reply, Sort};

use we_linux_ui::*;

const INSTALLED: i32 = 0;
const RECOMMENDED: i32 = 1;
/// The wallpapers of a row of "Рекомендации".
const EXPLORE_ROW: usize = 20;
const WORKSHOP: i32 = 2;

/// The workshop's orders, as the original lists them: English, Russian, the
/// order and (for trends) the window in days. The line with no label is the
/// divider before the lists of the user's own.
const WORKSHOP_SORTS: [(&str, &str, Sort, u32); 14] = [
    ("Top Rated", "Лидеры рейтинга", Sort::Vote, 0),
    ("Most Popular (Year)", "Популярные (Год)", Sort::Trend, 365),
    ("Most Popular (Month)", "Популярные (Месяц)", Sort::Trend, 30),
    ("Most Popular (Week)", "Популярные (Неделя)", Sort::Trend, 7),
    ("Most Popular (Today)", "Популярные (Сегодня)", Sort::Trend, 1),
    ("Most Recent", "Недавно добавленные", Sort::Newest, 0),
    ("Most Up Votes", "Больше всего оценок", Sort::UpVotes, 0),
    ("Most Subscriptions", "Больше всего подписчиков", Sort::Subscriptions, 0),
    ("", "", Sort::Vote, 0),
    ("My Favorites", "Избранные", Sort::Favorited, 0),
    ("Voted Up", "Оценённые", Sort::VotedUp, 0),
    ("Friends' Favorites", "Избранное друзей", Sort::FriendsFavorites, 0),
    ("Created By Friends", "Создано друзьями", Sort::FriendsCreated, 0),
    ("Created By Followed Users", "Создано теми, на кого вы подписаны", Sort::FollowedCreated, 0),
];
/// The orders of the installed wallpapers, as the original lists them.
const LOCAL_SORTS: [(&str, &str); 6] = [
    ("Name", "Имя"),
    ("Rating", "Рейтинг"),
    ("Favorite", "Избранное"),
    ("File Size", "Размер файла"),
    ("Subscription Date", "Дата подписки"),
    ("Last Updated", "Последнее обновление"),
];

/// "Показать только": the original's box. Favourites are the installed tab's
/// own; "Совместимые с Android" is about the Android app, not here.
const ONLY: [(&str, &str); 5] = [
    ("Одобренные", "Approved"),
    ("Избранное", "Favorite"),
    ("Совместимые с Android", "Mobile compatible"),
    ("Реагирующие на звук", "Audio responsive"),
    ("Настраиваемый", "Customizable"),
];
const TYPES: [(&str, &str); 4] = [("Сцена", "Scene"), ("Видео", "Video"), ("Веб", "Web"), ("Приложение", "Application")];
/// What "Тип" lists after its divider: the categories of the workshop.
const CATEGORIES: [(&str, &str); 2] = [("Обои", "Wallpaper"), ("Пресет", "Preset")];
/// "Источник", of the installed only.
const SOURCES: [(&str, &str); 3] = [("Официальный", "Official"), ("Мастерская", "Workshop"), ("Мои обои", "Local")];
const RATINGS: [(&str, &str); 3] = [
    ("0+ (для всех возрастов)", "Everyone"),
    ("12+ (с ограничениями)", "Questionable"),
    ("18+ (для взрослых)", "Mature"),
];
/// The ratings' English labels, as the original has them.
const RATINGS_EN: [&str; 3] = ["G - All ages", "PG13 - Parental guidance", "R - Mature"];
/// The workshop's resolutions, in the original's sub-groups (its title, Russian
/// and English, and its tags); the last group has no title. The original shows
/// the tags themselves, "Standard Definition" as "SD".
const RESOLUTION_GROUPS: [(&str, &str, &[&str]); 6] = [
    ("Широкий экран", "Widescreen", &["Standard Definition", "1280 x 720", "1366 x 768", "1920 x 1080", "2560 x 1440", "3840 x 2160"]),
    ("Широкий экран «Ультра»", "Ultrawide", &["Ultrawide Standard Definition", "Ultrawide 2560 x 1080", "Ultrawide 3440 x 1440"]),
    ("Два монитора", "Dual", &["Dual Standard Definition", "Dual 3840 x 1080", "Dual 5120 x 1440", "Dual 7680 x 2160"]),
    ("Три монитора", "Triple", &["Triple Standard Definition", "Triple 4096 x 768", "Triple 5760 x 1080", "Triple 7680 x 1440", "Triple 11520 x 2160"]),
    ("Портретная ориентация экрана", "Portrait", &["Portrait Standard Definition", "Portrait 720 x 1280", "Portrait 1080 x 1920", "Portrait 1440 x 2560", "Portrait 2160 x 3840"]),
    ("", "", &["Dynamic resolution", "Other resolution"]),
];
const GENRES: [(&str, &str); 25] = [
    ("Абстракция", "Abstract"),
    ("Животные", "Animal"),
    ("Аниме", "Anime"),
    ("Мультфильм", "Cartoon"),
    ("Комп. графика", "CGI"),
    ("Киберпанк", "Cyberpunk"),
    ("Фантастика", "Fantasy"),
    ("Игра", "Game"),
    ("Девушки", "Girls"),
    ("Парни", "Guys"),
    ("Пейзаж", "Landscape"),
    ("Средневековье", "Medieval"),
    ("Мемы", "Memes"),
    ("MikuMikuDance (анимация)", "MMD"),
    ("Музыка", "Music"),
    ("Природа", "Nature"),
    ("Пиксель-арт", "Pixel art"),
    ("Расслабляющий", "Relaxing"),
    ("Ретро", "Retro"),
    ("Научная фантастика", "Sci-Fi"),
    ("Спорт", "Sports"),
    ("Технологии", "Technology"),
    ("Телевидение", "Television"),
    ("Транспорт", "Vehicle"),
    ("Неопределенный жанр", "Unspecified"),
];

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
    fn new(title: &'static str, kind: GroupKind, open: bool, list: &[(&'static str, &'static str)], default: impl Fn(&str) -> bool) -> Self {
        Self {
            title,
            kind,
            open,
            shown: true,
            boxed: false,
            choices: list
                .iter()
                .map(|&(label, tag)| Choice { label, tag, checked: default(tag), default: default(tag), section: 0, installed_only: false, usable: true })
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
    kinds.choices.extend(CATEGORIES.iter().map(|&(label, tag)| Choice { label, tag, checked: true, default: true, section: 1, installed_only: false, usable: true }));
    let resolutions = Group {
        choices: RESOLUTION_GROUPS
            .iter()
            .enumerate()
            .flat_map(|(section, (_, _, tags))| {
                tags.iter().map(move |&tag| Choice { label: tag, tag, checked: true, default: true, section, installed_only: false, usable: true })
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
        Group::new("Возрастной рейтинг", GroupKind::AnyOf, false, &RATINGS, |tag| tag != "Mature"),
        resolutions,
        sources,
        Group::new("Метки", GroupKind::AnyOf, true, &GENRES, |_| true),
    ]
}

#[derive(Clone)]
enum Picture {
    Url(String),
    File(PathBuf),
    None,
}

/// One cell of the grid, from the workshop or from the disk.
#[derive(Clone)]
struct Card {
    id: u64,
    title: String,
    kind: String,
    rating: String,
    tags: Vec<String>,
    size: u64,
    up: u32,
    down: u32,
    updated: u64,
    picture: Picture,
    dir: Option<PathBuf>,
    /// The author's Steam id (0: not known yet).
    owner: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Saved {
    tab: Option<i32>,
    workshop_sort: Option<usize>,
    local_sort: Option<usize>,
    filters_open: Option<bool>,
    /// Checked tags by group title.
    filters: HashMap<String, Vec<String>>,
    /// Every tag a group had when it was saved: a tag added since keeps its default.
    #[serde(default)]
    filter_tags: HashMap<String, Vec<String>>,
    /// Titles of the groups the filter settings hide.
    hidden_groups: Vec<String>,
    favorites: Vec<u64>,
    /// "en" or "ru"; nothing until the first run asked.
    language: Option<String>,
    /// "Choose monitor" comes down when the window opens.
    monitors_on_start: Option<bool>,
    /// The installed wallpapers are sorted the other way round.
    sort_descending: Option<bool>,
    /// The grid's menu, "Вид": the tiles' size and the installed a page.
    icon_size: Option<i32>,
    per_page: Option<usize>,
}

fn config_file() -> PathBuf {
    desktop::config_dir().join("we-linux/state.json")
}

/// A grid cell that plays: its frames with their delays, the one shown, and
/// when the next is due.
struct Cell {
    frames: Vec<(Image, u32)>,
    frame: usize,
    due: std::time::Instant,
}

struct App {
    ui: Weak<AppWindow>,
    /// "en" or "ru", once picked.
    language: Option<String>,
    /// A wallpaper was set and is not on the desktop yet: its title, where
    /// it goes (a monitor, or all) and when it was set.
    applying: Option<(String, Option<String>, PathBuf, std::time::Instant)>,
    steam: Sender<Command>,
    previews: Previews,
    tab: i32,
    cards: Vec<Card>,
    model: Rc<VecModel<Item>>,
    selected: Option<usize>,
    installed: Vec<library::Installed>,
    /// `EItemState` by id, as Steam last told.
    subscribed: HashMap<u64, u32>,
    progress: HashMap<u64, f32>,
    busy: HashSet<u64>,
    /// The wallpaper subscribed to last: it is set once Steam has it, if it
    /// is still the selected one.
    set_when_ready: Option<u64>,
    favorites: HashSet<u64>,
    groups: Vec<Group>,
    workshop_sort: usize,
    local_sort: usize,
    /// The arrow beside the sorting of installed wallpapers points down.
    sort_descending: bool,
    monitors_on_start: bool,
    workshop_search: String,
    local_search: String,
    page: u32,
    total: u32,
    /// Of the workshop query in flight; older answers are dropped.
    generation: u64,
    picture_generation: u64,
    /// What every monitor shows, and what it showed when the window opened ("Отмена").
    current: Option<PathBuf>,
    opened_with: desktop::Desktop,
    /// The monitor wallpapers are set on: None is all of them.
    monitor: Option<String>,
    steam_ok: bool,
    filter_timer: Timer,
    /// The selected item's animated preview: frames with their delays, the
    /// one showing, and the timer that turns them.
    frames: Vec<(Image, u32)>,
    frame: usize,
    frame_timer: Timer,
    /// The selected item's properties and what the user changed of them.
    properties: Vec<props::Property>,
    overrides: BTreeMap<String, Value>,
    /// Rows of the properties shown (conditions hide some), by index into `properties`.
    shown_properties: Vec<usize>,
    playlist: Playlist,
    playlist_model: Rc<VecModel<PlaylistEntry>>,
    /// The workshop's details of installed items (votes, size, author), by id,
    /// and the ids asked for already.
    workshop_info: HashMap<u64, steam::Entry>,
    asked_details: HashSet<u64>,
    /// Authors: name and avatar, by Steam id.
    personas: HashMap<u64, (String, Option<Image>)>,
    /// What installed folders take on the disk, counted when sorted by it.
    sizes: HashMap<u64, u64>,
    /// Installed wallpapers a page: 50, 100 or 200 (the grid's menu, "Вид").
    per_page: usize,
    /// The grid's tiles: 0 small, 1 medium, 2 large (the grid's menu, "Вид").
    icon_size: i32,
    /// The search follows the typing, a moment after it.
    search_timer: Timer,
    /// The tile whose menu is open.
    context: Option<usize>,
    /// "Рекомендации": its rows (title and cards) and the generation of the
    /// query of the first (the others follow it).
    explore: Vec<(String, Vec<Card>)>,
    explore_base: u64,
    /// Grid cells whose animated preview plays, by id; the rows in view; the
    /// ids whose animation was asked for.
    cells: HashMap<u64, Cell>,
    cell_timer: Timer,
    visible: (usize, usize),
    animations_asked: HashSet<u64>,
    /// The workshop shows one author's wallpapers only.
    author: Option<u64>,
    /// The user's votes (1 up, -1 down), by item, as Steam told; the authors asked for.
    votes: HashMap<u64, i32>,
    asked_personas: HashSet<u64>,
}

thread_local! {
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
}

/// Run `f` on the app from any thread, through the event loop.
fn on_ui(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        APP.with(|slot| {
            if let Some(app) = slot.borrow().as_ref() {
                f(&mut app.borrow_mut());
            }
        });
    });
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// A size as the original writes it, in either language: "19 MB".
fn megabytes(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", (bytes >> 20).max(1))
    }
}

/// What the settings' "About" tab says.
fn about() -> String {
    let renderer = engine::bin("wepaper").map_or_else(|| t("not found", "не найден").to_owned(), |path| path.display().to_string());
    [
        format!("WE Linux {}", env!("CARGO_PKG_VERSION")),
        t(
            "Wallpaper Engine's library, workshop and live wallpapers, native on Linux.",
            "Библиотека, мастерская и живые обои Wallpaper Engine, нативно на Linux.",
        )
        .to_owned(),
        String::new(),
        format!("{}: {renderer}", t("Renderer", "Рендерер")),
        "https://github.com/MoonTiwzy/we-linux".to_owned(),
        "https://github.com/MoonTiwzy/wepaper".to_owned(),
        String::new(),
        t(
            "WE Linux is not made by the Wallpaper Engine team. Wallpapers and the assets they are drawn with come from your own copy of Wallpaper Engine in Steam.",
            "WE Linux сделан не командой Wallpaper Engine. Обои и ассеты, которыми они рисуются, берутся из твоей копии Wallpaper Engine в Steam.",
        )
        .to_owned(),
        String::new(),
        t("License: MIT.", "Лицензия: MIT.").to_owned(),
        t(
            "Icons: Font Awesome Free 6.7.2 by Fonticons, Inc. (CC BY 4.0), https://fontawesome.com",
            "Иконки: Font Awesome Free 6.7.2, Fonticons, Inc. (CC BY 4.0), https://fontawesome.com",
        )
        .to_owned(),
    ]
    .join("\n")
}

fn label_of(list: &[(&'static str, &'static str)], tag: &str) -> Option<&'static str> {
    list.iter().find(|(_, t)| t.eq_ignore_ascii_case(tag)).map(|(label, tag)| if lang::russian() { *label } else { *tag })
}

/// A workshop tag as the original names it where it translates tags: the
/// genres, the box's tags and the ratings. Others (resolutions, features)
/// it shows as they are.
fn tag_label(tag: &str) -> Option<&'static str> {
    if let Some(i) = RATINGS.iter().position(|(_, t)| *t == tag) {
        return Some(if lang::russian() { RATINGS[i].0 } else { RATINGS_EN[i] });
    }
    let english = match tag {
        "MMD" => Some("MMD (Miku-Miku Dance)"),
        "Unspecified" => Some("Unspecified genre"),
        _ => None,
    };
    GENRES
        .iter()
        .chain(ONLY.iter())
        .find(|(_, t)| *t == tag)
        .map(|(label, t)| if lang::russian() { *label } else { english.unwrap_or(t) })
}

/// The words of a search, as the original reads them: lower case, without
/// "?", "." and "!", with "hdr", "3d" and "puppet" (warp) taken as tags.
fn search_words(text: &str) -> (String, Vec<&'static str>) {
    let mut text: String = text.to_lowercase().chars().filter(|c| !matches!(c, '?' | '.' | '!')).collect();
    let mut tags = Vec::new();
    for (words, tag) in [(&["hdr"][..], "HDR"), (&["3d"][..], "3D"), (&["puppet warp", "puppetwarp", "puppet"][..], "Puppet Warp")] {
        for word in words {
            let found = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let padded = format!(" {found} ");
            if padded.contains(&format!(" {word} ")) {
                text = padded.replace(&format!(" {word} "), " ").trim().to_owned();
                if !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
        }
    }
    (text.trim().to_owned(), tags)
}

/// Whether a title answers a search: it holds the words, or is within two
/// edits of them (a typo, two letters swapped), as in the original.
fn title_matches(title: &str, words: &str) -> bool {
    if words.is_empty() {
        return true;
    }
    let title: String = title.to_lowercase().chars().filter(|c| !matches!(c, '?' | '.' | '!')).collect();
    title.contains(words) || edits(&title, words) < 3
}

/// The Damerau-Levenshtein distance (optimal string alignment) of two texts.
fn edits(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// The window behind a panel over it, blurred as the original's backdrop is:
/// drawn small, softened, and scaled back up by the image itself.
fn blurred(shot: &SharedPixelBuffer<Rgba8Pixel>) -> Image {
    let (width, height) = (shot.width(), shot.height());
    let Some(full) = image::RgbaImage::from_raw(width, height, shot.as_bytes().to_vec()) else { return Image::default() };
    let small = image::imageops::thumbnail(&full, (width / 8).max(1), (height / 8).max(1));
    let soft = image::imageops::blur(&small, 1.5);
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(soft.as_raw(), soft.width(), soft.height()))
}

/// A row of the settings' "Воспроизведение".
#[derive(Clone, Copy)]
enum Row {
    Focus,
    Maximized,
    Fullscreen,
    Audio,
    Sleep,
}

/// The choices of a row of "Воспроизведение", as the original lists them:
/// pausing per monitor or all of them where there are several.
fn playback_choices(row: Row, several: bool) -> Vec<&'static str> {
    let (mute, per_monitor, stop) = match row {
        Row::Focus => (true, several, false),
        Row::Maximized | Row::Fullscreen => (true, several, true),
        Row::Audio => (true, false, false),
        Row::Sleep => (false, false, true),
    };
    let mut out = vec!["run"];
    if mute {
        out.push("mute");
    }
    out.push("pause");
    if per_monitor {
        out.push("pauseall");
    }
    if stop {
        out.push("stop");
    }
    out
}

/// A title's place in the original's name order (the browser's collation):
/// spaces, punctuation, symbols, digits, then letters by script, the case
/// left aside.
fn collation_key(title: &str) -> Vec<(u8, char)> {
    title
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| {
            let class = if c.is_whitespace() {
                0
            } else if c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c) || matches!(c, '\u{2010}'..='\u{2027}' | '\u{3000}'..='\u{303f}' | '\u{ff01}'..='\u{ff0f}' | '\u{ff1a}'..='\u{ff20}' | '\u{ff3b}'..='\u{ff40}' | '\u{ff5b}'..='\u{ff65}') {
                1
            } else if c.is_numeric() {
                3
            } else if !c.is_alphabetic() {
                2
            } else if c.is_ascii() || matches!(c, '\u{00c0}'..='\u{024f}') {
                4
            } else if matches!(c, '\u{0370}'..='\u{03ff}') {
                5
            } else if matches!(c, '\u{0400}'..='\u{04ff}') {
                6
            } else {
                7
            };
            (class, c)
        })
        .collect()
}

/// The share of votes up, as five stars.
fn votes(up: u32, down: u32) -> f32 {
    if up + down == 0 { 0.0 } else { up as f32 / (up + down) as f32 * 5.0 }
}

/// What a folder takes on the disk, its files counted.
fn folder_size(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => folder_size(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

fn has_tag(tags: &[String], tag: &str) -> bool {
    tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
}

impl App {
    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    fn status(&self, text: impl Into<SharedString>) {
        if let Some(ui) = self.ui() {
            ui.set_status_error(false);
        }
        if let Some(ui) = self.ui() {
            ui.set_status(text.into());
        }
    }

    fn report(&self, result: Result<(), String>, done: &str) {
        match result {
            Ok(()) => self.status(done),
            Err(e) => self.status(tf!("That did not work: {e}", "Не получилось: {e}")),
        }
    }

    fn save(&self) {
        let mut favorites: Vec<u64> = self.favorites.iter().copied().collect();
        favorites.sort_unstable();
        let saved = Saved {
            tab: Some(self.tab),
            workshop_sort: Some(self.workshop_sort),
            local_sort: Some(self.local_sort),
            filters_open: self.ui().map(|ui| ui.get_filters_open()),
            filters: self
                .groups
                .iter()
                .map(|g| (g.title.to_owned(), g.checked().iter().map(|t| (*t).to_owned()).collect()))
                .collect(),
            filter_tags: self.groups.iter().map(|g| (g.title.to_owned(), g.choices.iter().map(|c| c.tag.to_owned()).collect())).collect(),
            hidden_groups: self.groups.iter().filter(|g| !g.shown).map(|g| g.title.to_owned()).collect(),
            favorites,
            language: self.language.clone(),
            monitors_on_start: Some(self.monitors_on_start),
            sort_descending: Some(self.sort_descending),
            icon_size: Some(self.icon_size),
            per_page: Some(self.per_page),
        };
        let path = config_file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&saved) {
            let _ = std::fs::write(path, json);
        }
    }

    fn restore(&mut self) {
        let Ok(bytes) = std::fs::read(config_file()) else { return };
        let Ok(saved) = serde_json::from_slice::<Saved>(&bytes) else { return };
        self.tab = saved.tab.unwrap_or(WORKSHOP).clamp(INSTALLED, WORKSHOP);
        self.workshop_sort = saved.workshop_sort.unwrap_or(0).min(WORKSHOP_SORTS.len() - 1);
        self.local_sort = saved.local_sort.unwrap_or(0).min(LOCAL_SORTS.len() - 1);
        self.favorites = saved.favorites.into_iter().collect();
        for group in &mut self.groups {
            // Ticks saved before the groups were the original's (no list of the
            // tags they knew) do not fit them: the groups start as they come.
            if let Some(checked) = saved.filters.get(group.title).filter(|_| !saved.filter_tags.is_empty()) {
                let known = saved.filter_tags.get(group.title);
                for choice in group.choices.iter_mut().filter(|c| known.is_none_or(|k| k.iter().any(|t| t == c.tag))) {
                    choice.checked = checked.iter().any(|t| t == choice.tag);
                }
            }
            group.shown = !saved.hidden_groups.iter().any(|t| t == group.title);
        }
        self.monitors_on_start = saved.monitors_on_start.unwrap_or(true);
        self.sort_descending = saved.sort_descending.unwrap_or(false);
        self.icon_size = saved.icon_size.unwrap_or(1).clamp(0, 2);
        self.per_page = saved.per_page.filter(|n| [50, 100, 200].contains(n)).unwrap_or(100);
        if let Some(ui) = self.ui() {
            ui.set_monitors_on_start(self.monitors_on_start);
            ui.set_sort_descending(self.sort_descending);
            if let Some(open) = saved.filters_open {
                ui.set_filters_open(open);
            }
        }
    }

    // ------------------------------------------------------------- filters

    fn show_groups(&self) {
        let Some(ui) = self.ui() else { return };
        let installed = !self.online();
        let icons = ui.global::<Icons>();
        let groups: Vec<FilterGroup> = self
            .groups
            .iter()
            .map(|g| {
                let mut options = Vec::new();
                let mut section = 0;
                for (index, c) in g.choices.iter().enumerate().filter(|(_, c)| installed || !c.installed_only) {
                    // A divider before "Тип"'s categories; a heading with "Все" and
                    // "Ничего" before each sub-group of the resolutions.
                    if c.section != section || (index == 0 && g.title == "Разрешение экрана") {
                        section = c.section;
                        let heading = RESOLUTION_GROUPS.get(c.section).filter(|_| g.title == "Разрешение экрана").map_or("", |(ru, en, _)| if lang::russian() { ru } else { en });
                        if g.title == "Разрешение экрана" && index > 0 {
                            options.push(FilterOption { kind: 1, choice: -1, ..Default::default() });
                        }
                        if g.title == "Тип" {
                            options.push(FilterOption { kind: 1, choice: -1, ..Default::default() });
                        } else if !heading.is_empty() {
                            options.push(FilterOption { label: heading.into(), kind: 2, section: c.section as i32, choice: -1, ..Default::default() });
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
        if self.online() {
            ui.set_filter_count(if filtered { t("(Filtered)", "(Отфильтровано)") } else { "" }.into());
        }
    }

    /// The filters changed: show them and search again after a short pause
    /// (several boxes are usually ticked in a row).
    fn filters_changed(&mut self) {
        self.show_groups();
        self.save();
        self.filter_timer.start(TimerMode::SingleShot, std::time::Duration::from_millis(400), || {
            APP.with(|slot| {
                if let Some(app) = slot.borrow().as_ref() {
                    let mut app = app.borrow_mut();
                    app.page = 1;
                    app.reload();
                }
            });
        });
    }

    // ------------------------------------------------------------- lists

    fn online(&self) -> bool {
        self.tab != INSTALLED
    }

    fn show_tab(&mut self) {
        if let Some(ui) = self.ui() {
            ui.set_tab(self.tab);
            let sorts: Vec<SharedString> = if self.online() {
                WORKSHOP_SORTS.iter().map(|(english, russian, ..)| t(english, russian).into()).collect()
            } else {
                LOCAL_SORTS.iter().map(|(english, russian)| t(english, russian).into()).collect()
            };
            ui.set_sort_options(ModelRc::new(VecModel::from(sorts)));
            ui.set_sort_index(if self.online() { self.workshop_sort } else { self.local_sort } as i32);
            ui.set_search(if self.online() { self.workshop_search.as_str() } else { self.local_search.as_str() }.into());
            ui.set_workshop_sorting(self.online());
            ui.set_icon_size(self.icon_size);
            let author = self.author.filter(|_| self.tab == WORKSHOP);
            ui.set_notice(author.map_or_else(String::new, |owner| {
                let name = self.personas.get(&owner).map_or("?", |(name, _)| name.as_str());
                tf!("Browsing wallpapers by {name}", "Смотрим все обои от {name}")
            }).into());
        }
        // Which groups there are depends on the tab.
        self.show_groups();
        self.reload();
    }

    fn reload(&mut self) {
        if self.tab == RECOMMENDED {
            self.query_explore()
        } else if self.online() {
            self.query_workshop()
        } else {
            self.show_installed()
        }
    }

    /// "Рекомендации", as the original's home of it: rows of wallpapers, each
    /// a question to the workshop, side by side in a strip.
    fn query_explore(&mut self) {
        let mut rows: Vec<(String, Sort, u32, Vec<String>)> = vec![
            (t("Most Popular (Today)", "Популярные (Сегодня)").to_owned(), Sort::Trend, 1, Vec::new()),
            (t("Most Popular (Week)", "Популярные (Неделя)").to_owned(), Sort::Trend, 7, Vec::new()),
            (t("Most Recent", "Недавно добавленные").to_owned(), Sort::Newest, 0, Vec::new()),
            (t("Top Rated", "Лидеры рейтинга").to_owned(), Sort::Vote, 0, Vec::new()),
        ];
        // The genres the installed wallpapers carry most, a row each.
        for genre in self.taste() {
            let label = tag_label(&genre).unwrap_or(&genre).to_owned();
            rows.push((tf!("Most Popular: {label}", "Популярные: {label}"), Sort::Trend, 30, vec![genre]));
        }
        self.explore_base = self.generation + 1;
        self.explore = rows.iter().map(|(title, ..)| (title.clone(), Vec::new())).collect();
        self.set_cards(Vec::new());
        self.show_explore();
        if let Some(ui) = self.ui() {
            ui.set_loading(true);
            ui.set_error("".into());
            ui.set_pages(ModelRc::new(VecModel::from(Vec::<PageButton>::new())));
        }
        // What the filters leave out stays out (the ratings above all).
        let rating: Vec<String> = self.groups.iter().find(|g| g.title == "Возрастной рейтинг").map(|g| g.checked().iter().map(|t| (*t).to_owned()).collect()).unwrap_or_default();
        for (sort, days, tags) in rows.into_iter().map(|(_, sort, days, tags)| (sort, days, tags)) {
            self.generation += 1;
            let mut groups = vec![rating.clone()];
            if !tags.is_empty() {
                groups.push(tags);
            }
            let query = steam::Query { sort, text: String::new(), page: 1, groups, required: Vec::new(), excluded: Vec::new(), trend_days: days.max(1), author: 0 };
            let _ = self.steam.send(Command::Query { generation: self.generation, query });
        }
    }

    /// The rows of "Рекомендации" on the screen: their cards one after another
    /// in the grid's list, each row knowing where its own begin.
    fn show_explore(&mut self) {
        let cards: Vec<Card> = self.explore.iter().flat_map(|(_, cards)| cards.iter().cloned()).collect();
        let mut first = 0;
        let rows: Vec<ExploreRow> = self
            .explore
            .iter()
            .map(|(title, cards)| {
                let row = ExploreRow { title: title.as_str().into(), first: first as i32, count: cards.len() as i32 };
                first += cards.len();
                row
            })
            .collect();
        self.set_cards(cards);
        if let Some(ui) = self.ui() {
            ui.set_explore_rows(ModelRc::new(VecModel::from(rows)));
        }
    }

    fn installed_card(item: &library::Installed) -> Card {
        Card {
            id: item.id,
            title: item.title.clone(),
            kind: item.kind.clone(),
            rating: item.rating.clone(),
            tags: item.tags.clone(),
            size: 0,
            up: 0,
            down: 0,
            updated: item.modified,
            picture: item.preview.clone().map_or(Picture::None, Picture::File),
            dir: Some(item.dir.clone()),
            owner: 0,
        }
    }

    fn show_installed(&mut self) {
        self.generation += 1;
        self.installed = library::installed();
        let all = self.installed.len();
        // As the original: an item goes when it carries a tag left unticked,
        // and stays only with every tag of "Показать только" ticked.
        let unticked: Vec<&'static str> = self
            .groups
            .iter()
            .filter(|g| !g.boxed && g.applies(true))
            .flat_map(|g| g.choices.iter().filter(|c| !c.checked).map(|c| c.tag))
            .collect();
        let only: Vec<&'static str> = self.groups.iter().filter(|g| g.boxed).flat_map(|g| g.choices.iter().filter(|c| c.checked && c.usable).map(|c| c.tag)).collect();
        let (words, special) = search_words(&self.local_search);
        let favorites = &self.favorites;
        let mut cards: Vec<Card> = self
            .installed
            .iter()
            .filter(|item| {
                let workshop_tags = self.workshop_info.get(&item.id).map(|e| e.tags.as_slice()).unwrap_or(&[]);
                let carries = |tag: &str| {
                    has_tag(&item.tags, tag)
                        || has_tag(workshop_tags, tag)
                        || item.kind.eq_ignore_ascii_case(tag)
                        || item.rating.eq_ignore_ascii_case(tag)
                        // Everything installed here came from the workshop as a wallpaper.
                        || tag == "Workshop"
                        || tag == "Wallpaper"
                };
                let held = only.iter().all(|tag| match *tag {
                    "Approved" => item.approved,
                    "Favorite" => favorites.contains(&item.id),
                    "Audio responsive" => item.audio_responsive,
                    "Customizable" => item.customizable,
                    _ => true,
                });
                held && !unticked.iter().any(|tag| carries(tag))
                    && special.iter().all(|tag| has_tag(&item.tags, tag))
                    && title_matches(&item.title, &words)
            })
            .map(Self::installed_card)
            .collect();
        let shown = cards.len();
        // The original's orders, "up" first; the arrow turns them round.
        let info = &self.workshop_info;
        let rating = |c: &Card| info.get(&c.id).map_or(0.0, |e| votes(e.up, e.down));
        let name = |a: &Card, b: &Card| collation_key(&a.title).cmp(&collation_key(&b.title));
        let sizes = self.local_sort == 3;
        if sizes {
            for card in &cards {
                if let Some(dir) = &card.dir {
                    self.sizes.entry(card.id).or_insert_with(|| folder_size(dir));
                }
            }
        }
        let size = |c: &Card| self.sizes.get(&c.id).copied().unwrap_or(0);
        let updated = |c: &Card| info.get(&c.id).map_or(c.updated, |e| u64::from(e.updated));
        cards.sort_by(|a, b| match self.local_sort {
            1 => rating(b).total_cmp(&rating(a)).then_with(|| name(a, b)),
            2 => favorites.contains(&b.id).cmp(&favorites.contains(&a.id)).then_with(|| name(a, b)),
            3 => size(b).cmp(&size(a)).then_with(|| name(a, b)),
            4 => b.updated.cmp(&a.updated).then_with(|| name(a, b)),
            5 => updated(b).cmp(&updated(a)).then_with(|| name(a, b)),
            _ => name(a, b),
        });
        if self.sort_descending {
            cards.reverse();
        }
        self.total = shown as u32;
        // A hundred a page, the pages under the grid.
        let per_page = self.per_page.max(1);
        let pages = shown.div_ceil(per_page).max(1) as u32;
        self.page = self.page.clamp(1, pages);
        let from = (self.page as usize - 1) * per_page;
        let page: Vec<Card> = cards.into_iter().skip(from).take(per_page).collect();
        self.set_cards(page);
        if let Some(ui) = self.ui() {
            ui.set_loading(false);
            ui.set_error("".into());
            let buttons = if shown > per_page { page_buttons(self.page, pages) } else { Vec::new() };
            ui.set_pages(ModelRc::new(VecModel::from(buttons)));
            ui.set_filter_count(if shown != all { tf!("({} of {})", "({} из {})", shown, all) } else { String::new() }.into());
        }
        // The details only the workshop has (votes, size, author), once.
        let unknown: Vec<u64> = self.installed.iter().map(|i| i.id).filter(|id| !self.workshop_info.contains_key(id) && !self.asked_details.contains(id)).collect();
        if !unknown.is_empty() {
            self.asked_details.extend(unknown.iter().copied());
            let _ = self.steam.send(Command::Details(unknown));
        }
    }

    /// The genres the installed wallpapers carry most: what "Рекомендации" looks for.
    fn taste(&self) -> Vec<String> {
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for item in &self.installed {
            for (_, tag) in GENRES {
                if tag != "Unspecified" && has_tag(&item.tags, tag) {
                    *counts.entry(tag).or_insert(0) += 1;
                }
            }
        }
        let mut ranked: Vec<(&'static str, usize)> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        ranked.into_iter().take(3).map(|(tag, _)| tag.to_owned()).collect()
    }

    fn query_workshop(&mut self) {
        self.generation += 1;
        let (_, _, sort, days) = WORKSHOP_SORTS[self.workshop_sort];
        let mut query = steam::Query {
            sort,
            text: self.workshop_search.clone(),
            page: self.page.max(1),
            groups: Vec::new(),
            required: Vec::new(),
            excluded: Vec::new(),
            trend_days: days.max(1),
            author: self.author.map_or(0, |owner| owner as u32),
        };
        for group in &self.groups {
            if !group.applies(false) || !group.narrows_on(false) {
                continue;
            }
            // Recommendations pick the genres themselves.
            if self.tab == RECOMMENDED && group.title == "Метки" {
                continue;
            }
            match group.kind {
                // Each part of "Тип" (the kinds, the categories) is a choice of its own.
                GroupKind::AnyOf if group.title == "Тип" => {
                    for section in 0..2 {
                        let part: Vec<&Choice> = group.choices_for(false).filter(|c| c.section == section).collect();
                        if part.iter().any(|c| !c.checked) {
                            query.groups.push(part.iter().filter(|c| c.checked).map(|c| c.tag.to_owned()).collect());
                        }
                    }
                }
                GroupKind::AnyOf => query.groups.push(group.choices_for(false).filter(|c| c.checked).map(|c| c.tag.to_owned()).collect()),
                GroupKind::AllOf => query.required.extend(group.choices_for(false).filter(|c| c.checked && c.usable).map(|c| c.tag.to_owned())),
            }
        }
        if self.tab == RECOMMENDED {
            let taste = self.taste();
            if !taste.is_empty() {
                query.groups.push(taste);
            }
        }
        if let Some(ui) = self.ui() {
            ui.set_loading(true);
            ui.set_error("".into());
        }
        let _ = self.steam.send(Command::Query { generation: self.generation, query });
    }

    fn page_arrived(&mut self, generation: u64, result: Result<steam::Page, String>) {
        // A row of "Рекомендации".
        if self.tab == RECOMMENDED {
            let Some(row) = generation.checked_sub(self.explore_base).map(|r| r as usize).filter(|r| *r < self.explore.len()) else { return };
            if let Ok(page) = result {
                let cards: Vec<Card> = page.entries.into_iter().take(EXPLORE_ROW).map(|e| self.card_of(e)).collect();
                self.explore[row].1 = cards;
                self.show_explore();
            }
            if let Some(ui) = self.ui() {
                ui.set_loading(false);
            }
            return;
        }
        if generation != self.generation || !self.online() {
            return;
        }
        let Some(ui) = self.ui() else { return };
        ui.set_loading(false);
        let page = match result {
            Ok(page) => page,
            Err(e) => {
                ui.set_error(e.into());
                self.set_cards(Vec::new());
                return;
            }
        };
        self.total = page.total;
        let cards: Vec<Card> = page.entries.into_iter().map(|e| self.card_of(e)).collect();
        self.set_cards(cards);
        // Steam pages no further than 50 000 items: a thousand pages.
        let pages = self.total.div_ceil(steam::PAGE_SIZE).clamp(1, 1000);
        ui.set_pages(ModelRc::new(VecModel::from(page_buttons(self.page.max(1), pages))));
        self.status(tf!("Wallpapers found: {}", "Найдено обоев: {}", grouped(u64::from(self.total))));
    }

    /// A wallpaper of the workshop as a cell of the grid.
    fn card_of(&self, e: steam::Entry) -> Card {
        let find = |list: &[(&'static str, &'static str)]| e.tags.iter().find(|t| label_of(list, t).is_some()).cloned().unwrap_or_default();
        Card {
            id: e.id,
            title: e.title.clone(),
            kind: find(&TYPES).to_lowercase(),
            rating: find(&RATINGS),
            size: e.size,
            up: e.up,
            down: e.down,
            updated: u64::from(e.updated),
            picture: if e.preview_url.is_empty() { Picture::None } else { Picture::Url(e.preview_url.clone()) },
            dir: self.installed.iter().find(|i| i.id == e.id).map(|i| i.dir.clone()),
            owner: e.owner,
            tags: e.tags,
        }
    }

    fn item_of(&self, card: &Card) -> Item {
        let state = self.subscribed.get(&card.id).copied().unwrap_or(0);
        Item {
            title: card.title.as_str().into(),
            image: Image::default(),
            loaded: false,
            kind: card.kind.as_str().into(),
            approved: has_tag(&card.tags, "Approved"),
            subscribed: self.online() && state & steam::SUBSCRIBED != 0,
            favorite: self.favorites.contains(&card.id),
            progress: self.progress.get(&card.id).copied().unwrap_or(-1.0),
        }
    }

    fn set_cards(&mut self, cards: Vec<Card>) {
        let keep = self.selected.and_then(|i| self.cards.get(i)).map(|c| c.id);
        self.cards = cards;
        self.picture_generation = self.previews.next_generation();
        let items: Vec<Item> = self.cards.iter().map(|c| self.item_of(c)).collect();
        self.model.set_vec(items);
        for card in &self.cards {
            let source = match &card.picture {
                Picture::Url(url) => Source::Url(url.clone()),
                Picture::File(path) => Source::File(path.clone()),
                Picture::None => continue,
            };
            self.previews.request(Job { id: card.id, source, size: 268, generation: self.picture_generation, large: false, animate: false });
        }
        self.cells.clear();
        self.animations_asked.clear();
        self.cell_timer.stop();
        self.animate_visible();
        let selected = keep.and_then(|id| self.cards.iter().position(|c| c.id == id));
        let same = selected.is_some();
        self.selected = selected;
        self.show_selection(same);
    }

    fn picture_arrived(&mut self, picture: Decoded) {
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&picture.rgba, picture.width, picture.height);
        let image = Image::from_rgba8(buffer);
        if picture.large {
            if let (Some(ui), Some(card)) = (self.ui(), self.selected.and_then(|i| self.cards.get(i)))
                && card.id == picture.id
            {
                ui.set_sel_image(image.clone());
                ui.set_sel_loaded(true);
                // An animated preview plays, as in the Windows client.
                self.frames = std::iter::once((image, picture.delay))
                    .chain(picture.frames.into_iter().map(|(rgba, delay)| {
                        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, picture.width, picture.height);
                        (Image::from_rgba8(buffer), delay)
                    }))
                    .collect();
                self.frame = 0;
                // The frames of the preview shown before go back to the system.
                unsafe { malloc_trim(0) };
                if self.frames.len() > 1 {
                    // The first frame is on the screen: for its own time.
                    self.arm_frame(picture.delay);
                } else {
                    self.frames.clear();
                }
            }
            return;
        }
        // The playlist strip's thumbnails are asked for with generation 0.
        if picture.generation == 0 {
            for row in 0..self.playlist_model.row_count() {
                if self.playlist_id(row) == Some(picture.id)
                    && let Some(mut entry) = self.playlist_model.row_data(row)
                {
                    entry.image = image.clone();
                    entry.loaded = true;
                    self.playlist_model.set_row_data(row, entry);
                }
            }
            return;
        }
        if picture.generation != self.picture_generation {
            return;
        }
        let Some(row) = self.cards.iter().position(|c| c.id == picture.id) else { return };
        // An animated preview of a cell in view plays, as the original's do.
        if !picture.frames.is_empty() {
            let (first, last) = self.visible;
            if row < first || row >= last {
                return;
            }
            let (width, height) = (picture.width, picture.height);
            let frames: Vec<(Image, u32)> = std::iter::once((image.clone(), picture.delay))
                .chain(picture.frames.into_iter().map(|(rgba, delay)| {
                    (Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, width, height)), delay)
                }))
                .collect();
            let due = std::time::Instant::now() + std::time::Duration::from_millis(u64::from(picture.delay));
            self.cells.insert(picture.id, Cell { frames, frame: 0, due });
            self.arm_cells();
        }
        if let Some(mut item) = self.model.row_data(row) {
            item.image = image;
            item.loaded = true;
            self.model.set_row_data(row, item);
        }
    }

    /// The cells in view: their animated previews are asked for, the ones
    /// out of view stop (their frames go).
    fn visible_changed(&mut self, first: usize, last: usize) {
        self.visible = (first, last.min(self.cards.len()));
        self.animate_visible();
    }

    fn animate_visible(&mut self) {
        let (first, last) = self.visible;
        let shown: HashSet<u64> = self.cards.iter().skip(first).take(last.saturating_sub(first)).map(|c| c.id).collect();
        self.cells.retain(|id, _| shown.contains(id));
        // A cell out of view asks again when it comes back.
        self.animations_asked.retain(|id| shown.contains(id));
        let generation = self.picture_generation;
        for card in self.cards.iter().skip(first).take(last.saturating_sub(first)) {
            if !self.animations_asked.insert(card.id) {
                continue;
            }
            let source = match &card.picture {
                Picture::Url(url) => Source::Url(url.clone()),
                Picture::File(path) => Source::File(path.clone()),
                Picture::None => continue,
            };
            self.previews.request(Job { id: card.id, source, size: 240, generation, large: false, animate: true });
        }
    }

    /// One timer turns the frames of every cell that plays, each at its own pace.
    fn arm_cells(&mut self) {
        if self.cells.is_empty() || self.cell_timer.running() {
            return;
        }
        self.cell_timer.start(TimerMode::Repeated, std::time::Duration::from_millis(15), || {
            APP.with(|slot| {
                if let Some(app) = slot.borrow().as_ref()
                    && let Ok(mut app) = app.try_borrow_mut()
                {
                    app.turn_cells();
                }
            });
        });
    }

    fn turn_cells(&mut self) {
        if self.cells.is_empty() {
            self.cell_timer.stop();
            return;
        }
        let now = std::time::Instant::now();
        let mut turned = Vec::new();
        for (id, cell) in &mut self.cells {
            if cell.due > now {
                continue;
            }
            cell.frame = (cell.frame + 1) % cell.frames.len();
            let (image, delay) = cell.frames[cell.frame].clone();
            cell.due = now + std::time::Duration::from_millis(u64::from(delay));
            turned.push((*id, image));
        }
        for (id, image) in turned {
            if let Some(row) = self.cards.iter().position(|c| c.id == id)
                && let Some(mut item) = self.model.row_data(row)
            {
                item.image = image;
                self.model.set_row_data(row, item);
            }
        }
    }

    /// Show the next frame of the selected item's preview and arm the timer for the one after.
    fn next_frame(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.frame = (self.frame + 1) % self.frames.len();
        let (image, delay) = self.frames[self.frame].clone();
        if let Some(ui) = self.ui() {
            ui.set_sel_image(image);
        }
        self.arm_frame(delay);
    }

    fn arm_frame(&mut self, delay: u32) {
        self.frame_timer.start(TimerMode::SingleShot, std::time::Duration::from_millis(u64::from(delay)), || {
            APP.with(|slot| {
                if let Some(app) = slot.borrow().as_ref()
                    && let Ok(mut app) = app.try_borrow_mut()
                {
                    app.next_frame();
                }
            });
        });
    }

    /// Bring the marks of every cell (subscribed, on the desktop, progress) up to date.
    fn refresh_marks(&mut self) {
        for (row, card) in self.cards.iter().enumerate() {
            let Some(mut item) = self.model.row_data(row) else { continue };
            let fresh = self.item_of(card);
            if item.subscribed != fresh.subscribed
                || item.progress != fresh.progress
                || item.favorite != fresh.favorite
            {
                item.subscribed = fresh.subscribed;
                item.progress = fresh.progress;
                item.favorite = fresh.favorite;
                self.model.set_row_data(row, item);
            }
        }
        self.show_selection(true);
    }

    // ------------------------------------------------------------- selection

    fn dir_of(&self, card: &Card) -> Option<PathBuf> {
        card.dir.clone().or_else(|| self.installed.iter().find(|i| i.id == card.id).map(|i| i.dir.clone()))
    }

    /// Fill the side panel from the selected card. `same` keeps its picture
    /// and its properties (only the marks changed).
    fn show_selection(&mut self, same: bool) {
        let Some(ui) = self.ui() else { return };
        ui.set_selected(self.selected.map_or(-1, |i| i as i32));
        let Some(card) = self.selected.and_then(|i| self.cards.get(i)).cloned() else {
            ui.set_has_selection(false);
            ui.set_sel_installed(false);
            self.frame_timer.stop();
            self.frames.clear();
            return;
        };
        let state = self.subscribed.get(&card.id).copied().unwrap_or(0);
        let dir = self.dir_of(&card);
        ui.set_has_selection(true);
        ui.set_sel_title(card.title.as_str().into());
        ui.set_sel_subscribed(state & steam::SUBSCRIBED != 0);
        ui.set_sel_installed(dir.is_some());
        ui.set_sel_current(dir.is_some() && dir == self.current);
        ui.set_sel_favorite(self.favorites.contains(&card.id));
        ui.set_sel_busy(self.busy.contains(&card.id));
        ui.set_sel_progress(self.progress.get(&card.id).copied().unwrap_or(-1.0));
        // The workshop's word on it: from the page it came with, or (installed)
        // asked for by id.
        let info = self.workshop_info.get(&card.id);
        let (up, down) = info.map_or((card.up, card.down), |e| (e.up, e.down));
        ui.set_sel_stars(votes(up, down).round() as i32);
        ui.set_sel_vote(self.votes.get(&card.id).copied().unwrap_or(0));
        let owner = info.map_or(card.owner, |e| e.owner);
        match self.personas.get(&owner) {
            Some((name, avatar)) => {
                ui.set_sel_author(name.as_str().into());
                ui.set_sel_avatar(avatar.clone().unwrap_or_default());
            }
            None => {
                ui.set_sel_author("".into());
                ui.set_sel_avatar(Image::default());
                if owner != 0 && self.asked_personas.insert(owner) {
                    let _ = self.steam.send(Command::Persona(owner));
                }
            }
        }
        if card.id != 0 && !same && !self.votes.contains_key(&card.id) {
            let _ = self.steam.send(Command::Vote(card.id));
        }

        // The kind and the size on one line, the tags as pills below: as the
        // original has them.
        let mut facts = Vec::new();
        if let Some(kind) = label_of(&TYPES, &card.kind) {
            facts.push(kind.to_owned());
        }
        let size = info.map_or(card.size, |e| e.size);
        if size > 0 {
            facts.push(megabytes(size));
        }
        ui.set_sel_facts(facts.join("  ").into());
        // The tags the workshop has, in its order, without the ones that say
        // what it is; genres, the box's tags and the ratings translated.
        const NOT_SHOWN: [&str; 11] = ["Wallpaper", "Preset", "Web", "Video", "Scene", "Application", "Approved", "Multi-monitor optimized", "Workshop", "Official", "Local"];
        let mut tags: Vec<String> = info.map_or_else(|| card.tags.clone(), |e| e.tags.clone());
        if info.is_none() && !card.rating.is_empty() && !tags.contains(&card.rating) {
            tags.push(card.rating.clone());
        }
        let pills: Vec<String> = tags
            .iter()
            .filter(|t| !NOT_SHOWN.contains(&t.as_str()))
            .map(|t| tag_label(t).map_or_else(|| t.clone(), str::to_owned))
            .collect();
        // Rows of what fits the card's width: a letter is about seven wide, a pill
        // has twelve of padding and four between it and the next.
        let room = if self.ui().is_some_and(|ui| ui.get_mini()) { 280 } else { 300 };
        let mut rows: Vec<Vec<SharedString>> = Vec::new();
        let mut used = 0usize;
        for pill in pills {
            let width = pill.chars().count() * 7 + 16;
            if rows.is_empty() || used + width > room {
                rows.push(Vec::new());
                used = 0;
            }
            used += width;
            if let Some(row) = rows.last_mut() {
                row.push(pill.into());
            }
        }
        let rows: Vec<ModelRc<SharedString>> = rows.into_iter().map(|row| ModelRc::new(VecModel::from(row))).collect();
        ui.set_sel_tags(ModelRc::new(VecModel::from(rows)));
        self.mark_playlist();
        ui.set_sel_workshop(card.id != 0);

        if same {
            return;
        }
        self.properties = dir.as_deref().map(props::load).unwrap_or_default();
        self.overrides = props::overrides(card.id);
        self.show_properties();
        self.frame_timer.stop();
        self.frames.clear();
        ui.set_sel_loaded(false);
        let source = match &card.picture {
            Picture::Url(url) => Source::Url(url.clone()),
            Picture::File(path) => Source::File(path.clone()),
            Picture::None => return,
        };
        self.previews.request(Job { id: card.id, source, size: 400, generation: 0, large: true, animate: true });
    }

    fn select(&mut self, index: usize) {
        if index < self.cards.len() {
            self.selected = Some(index);
            self.show_selection(false);
            self.show_presets();
            // As in Wallpaper Engine: a wallpaper that is here goes on the desktop at
            // once, and "Cancel" puts back what was there.
            let here = self.selected_card().and_then(|card| self.dir_of(&card));
            if here.is_some() && here != self.on_target() {
                self.apply();
            }
        }
    }

    /// What the monitor wallpapers are being set on shows now.
    fn on_target(&self) -> Option<PathBuf> {
        match &self.monitor {
            Some(name) => {
                desktop::outputs().into_iter().find(|(output, _)| output == name).map(|(_, path)| path).or_else(|| self.current.clone())
            }
            None => self.current.clone(),
        }
    }

    fn selected_card(&self) -> Option<Card> {
        self.selected.and_then(|i| self.cards.get(i)).cloned()
    }

    // ------------------------------------------------------------- properties

    /// The value of every property now: the user's or the wallpaper's own.
    fn property_values(&self) -> BTreeMap<String, Value> {
        self.properties
            .iter()
            .map(|p| (p.name.clone(), self.overrides.get(&p.name).unwrap_or(&p.default).clone()))
            .collect()
    }

    fn show_properties(&mut self) {
        let Some(ui) = self.ui() else { return };
        let values = self.property_values();
        self.shown_properties.clear();
        let mut rows = Vec::new();
        for (index, property) in self.properties.iter().enumerate() {
            if !props::holds(&property.condition, &values) {
                continue;
            }
            let value = &values[&property.name];
            let (r, g, b) = props::rgb(value);
            self.shown_properties.push(index);
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
                    property.options.iter().map(|(label, _)| SharedString::from(label.as_str())).collect::<Vec<_>>(),
                )),
                index: property.options.iter().position(|(_, v)| props::same(v, value)).map_or(-1, |i| i as i32),
                swatch: Color::from_rgb_u8(r, g, b),
                usable: property.usable,
                hint: property.hint.as_str().into(),
                usable_options: ModelRc::new(VecModel::from(property.usable_options.clone())),
            });
        }
        ui.set_props(ModelRc::new(VecModel::from(rows)));
    }

    /// The user changed the property in row `row`: keep it, tell the engines
    /// that play this wallpaper (on every monitor or on one) and redraw.
    fn set_property(&mut self, row: usize, value: Value) {
        let Some(card) = self.selected_card() else { return };
        let Some(property) = self.shown_properties.get(row).and_then(|&i| self.properties.get(i)).cloned() else { return };
        if props::same(&value, &property.default) {
            self.overrides.remove(&property.name);
        } else {
            self.overrides.insert(property.name.clone(), value.clone());
        }
        props::save_overrides(card.id, &self.overrides);
        if let Some(dir) = self.dir_of(&card) {
            props::tell_engines(&dir, &property.name, &value);
        }
        self.show_properties();
    }

    fn reset_properties(&mut self) {
        let Some(card) = self.selected_card() else { return };
        if let Some(dir) = self.dir_of(&card) {
            for property in self.properties.iter().filter(|p| self.overrides.contains_key(&p.name)) {
                props::tell_engines(&dir, &property.name, &property.default);
            }
        }
        self.overrides.clear();
        props::save_overrides(card.id, &self.overrides);
        self.show_properties();
        self.status(t("Properties reset", "Свойства сброшены"));
    }

    // ------------------------------------------------------------- actions

    fn subscribe(&mut self) {
        let Some(card) = self.selected_card() else { return };
        if self.busy.insert(card.id) {
            self.status(tf!("Subscribing to \"{}\"…", "Подписка на «{}»…", card.title));
            let _ = self.steam.send(Command::Subscribe(card.id));
            self.set_when_ready = Some(card.id);
            self.show_selection(true);
        }
    }

    fn unsubscribe(&mut self) {
        let Some(card) = self.selected_card() else { return };
        if self.busy.insert(card.id) {
            self.status(tf!("Unsubscribing from \"{}\"…", "Отписка от «{}»…", card.title));
            let _ = self.steam.send(Command::Unsubscribe(card.id));
            if self.set_when_ready == Some(card.id) {
                self.set_when_ready = None;
            }
            self.show_selection(true);
        }
    }

    fn apply(&mut self) {
        let Some(card) = self.selected_card() else { return };
        let Some(dir) = self.dir_of(&card) else {
            self.status(t("This wallpaper is not downloaded yet", "Эти обои ещё не скачаны"));
            return;
        };
        // "On the desktop" is said when the engine shows it (show_applying).
        let result = match &self.monitor {
            Some(output) => desktop::apply_to(output, &dir),
            None => desktop::clear_outputs().and_then(|()| desktop::apply(&dir)).map(|()| self.current = Some(dir.clone())),
        };
        match result {
            Ok(()) => {
                self.applying = Some((card.title.clone(), self.monitor.clone(), dir, std::time::Instant::now()));
                self.show_applying();
            }
            Err(e) => self.status(tf!("That did not work: {e}", "Не получилось: {e}")),
        }
        self.refresh_marks();
        self.show_monitors();
    }

    /// Say how far the wallpaper set last has come, until it is on the desktop.
    fn show_applying(&mut self) {
        let Some((title, monitor, dir, since)) = self.applying.clone() else { return };
        let waited = since.elapsed().as_secs();
        match engine::stage(&dir) {
            engine::Stage::Shown => {
                self.applying = None;
                match monitor {
                    Some(output) => self.status(tf!("\"{title}\" is on monitor {output}", "«{title}» на мониторе {output}")),
                    None => self.status(tf!("\"{title}\" is on the desktop", "«{title}» на рабочем столе")),
                }
            }
            engine::Stage::Failed(error) => {
                self.applying = None;
                let error = if error.contains("video memory") {
                    t("not enough video memory for this wallpaper", "этим обоям не хватает видеопамяти").to_owned()
                } else {
                    error
                };
                self.status(tf!("\"{title}\" did not start: {error}", "«{title}» не запустились: {error}"));
            }
            _ if waited > 180 => {
                self.applying = None;
                self.status(tf!("\"{title}\" is taking too long: see `we-linux doctor`", "«{title}» грузятся слишком долго: смотри `we-linux doctor`"));
            }
            engine::Stage::Loading => self.status(tf!("\"{title}\": loading…", "«{title}»: загрузка…")),
        }
    }

    /// A double click: set what is on the disk, subscribe to what is not.
    fn activate(&mut self, index: usize) {
        // A wallpaper that is here was set by the click that selected it.
        self.select(index);
        let Some(card) = self.selected_card() else { return };
        if self.dir_of(&card).is_some() {
        } else if self.subscribed.get(&card.id).copied().unwrap_or(0) & steam::SUBSCRIBED == 0 {
            self.subscribe();
        } else if self.progress.contains_key(&card.id) {
            // Its download is watched: on the desktop it goes when it lands.
            self.set_when_ready = Some(card.id);
            self.status(tf!(
                "\"{}\" is still downloading: it goes on the desktop when it is here",
                "«{}» ещё скачиваются: встанут на рабочий стол, как только загрузятся",
                card.title
            ));
        } else {
            self.status(t("Steam has not downloaded this wallpaper yet", "Steam ещё не скачал эти обои"));
        }
    }

    fn toggle_favorite(&mut self) {
        let Some(card) = self.selected_card() else { return };
        let on = !self.favorites.remove(&card.id);
        if on {
            self.favorites.insert(card.id);
        }
        // The original's heart is the workshop's favourite.
        if card.id != 0 {
            let _ = self.steam.send(Command::Favorite { id: card.id, on });
        }
        self.save();
        self.refresh_marks();
    }

    /// The menu of a tile, as the original builds it for the item under the
    /// pointer (which stays unselected): playlist, subscription, favourite,
    /// the workshop, the folder, the grid's view.
    fn context_menu(&mut self, index: usize) {
        let Some(ui) = self.ui() else { return };
        let Some(card) = self.cards.get(index).cloned() else { return };
        self.context = Some(index);
        let icons = ui.global::<Icons>();
        let line = |label: &str, icon: Glyph, id: &str, enabled: bool| MenuEntry {
            label: label.into(),
            icon,
            enabled,
            checked: false,
            divider: false,
            sub: -1,
            id: id.into(),
        };
        let divider = || MenuEntry { divider: true, enabled: true, sub: -1, ..Default::default() };
        let submenu = |label: &str, icon: Glyph, sub: i32| MenuEntry { sub, ..line(label, icon, "", true) };
        let dir = self.dir_of(&card);
        let state = self.subscribed.get(&card.id).copied().unwrap_or(0);
        let downloadable = dir.is_none() && state & steam::SUBSCRIBED == 0;
        let mut entries = Vec::new();
        if !downloadable {
            if dir.as_ref().is_some_and(|d| self.playlist.items.contains(d)) {
                entries.push(line(t("Remove from playlist", "Удалить из плейлиста"), icons.get_minus(), "playlist-remove", true));
            } else {
                entries.push(line(t("Add to playlist", "Добавить в плейлист"), icons.get_plus(), "playlist-add", dir.is_some()));
            }
        }
        if card.id != 0 {
            if downloadable {
                entries.push(line(t("Subscribe", "Подписаться"), icons.get_download(), "subscribe", self.steam_ok));
            } else {
                entries.push(line(t("Unsubscribe", "Отписаться"), icons.get_xmark(), "unsubscribe", self.steam_ok));
            }
            if self.favorites.contains(&card.id) {
                entries.push(line(t("Remove from Favorites", "Удалить из избранного"), icons.get_heart_regular(), "unfavorite", true));
            } else {
                entries.push(line(t("Add to Favorites", "Добавить в избранное"), icons.get_heart(), "favorite", true));
            }
            entries.push(divider());
            entries.push(line(t("Open in Workshop", "Открыть в Мастерской"), icons.get_steam(), "workshop", true));
            entries.push(submenu(t("Related Wallpapers", "Связанные обои"), icons.get_image(), 0));
            entries.push(submenu(t("Report & Block", "Пожаловаться и заблокировать"), icons.get_triangle_exclamation(), 1));
        }
        if let Some(_dir) = &dir {
            entries.push(divider());
            // A key that sets the wallpaper from anywhere is the compositor's to bind.
            entries.push(line(t("Assign Hotkey", "Назначить горячую клавишу"), icons.get_keyboard(), "hotkey", false));
            entries.push(line(t("Open in Explorer", "Открыть в Проводнике"), icons.get_folder_open(), "folder", true));
            entries.push(submenu(t("Play in Window", "Воспроизвести в окне"), icons.get_window_restore(), 2));
        }
        entries.push(divider());
        entries.push(submenu(t("View", "Вид"), icons.get_eye(), 3));

        let owner = self.workshop_info.get(&card.id).map_or(card.owner, |e| e.owner);
        let author = self.personas.get(&owner).map_or("?".to_owned(), |(name, _)| name.clone());
        if owner != 0 && !self.personas.contains_key(&owner) {
            let _ = self.steam.send(Command::Persona(owner));
        }
        let related = vec![line(&tf!("Browse all by {author} (?)", "Просмотреть всё от {author} (?)"), icons.get_user(), "author", owner != 0 && self.steam_ok)];
        let report = vec![
            line(t("Report", "Сообщить"), icons.get_triangle_exclamation(), "report", true),
            line(&tf!("Block {author}", "Заблок. {author}"), icons.get_user_slash(), "block", false),
        ];
        // Playing a wallpaper in a window of its own is not something the renderer does.
        let window = vec![
            line(t("Full HD Preview", "Предпросмотр, Full HD"), icons.get_computer(), "", false),
            line(t("Phone Preview", "Предпросмотр, телефон"), icons.get_mobile_screen_button(), "", false),
            divider(),
            line(t("Maximized Preview", "Предпросмотр, на весь экран"), icons.get_window_maximize(), "", false),
        ];
        let checked = |entry: MenuEntry, on: bool| MenuEntry { checked: on, ..entry };
        let mut view = vec![
            checked(line(t("Small Icons", "Маленькие значки"), Glyph::default(), "icons-0", true), self.icon_size == 0),
            checked(line(t("Medium Icons", "Средние значки"), Glyph::default(), "icons-1", true), self.icon_size == 1),
            checked(line(t("Large Icons", "Большие значки"), Glyph::default(), "icons-2", true), self.icon_size == 2),
        ];
        if self.tab == INSTALLED {
            view.push(divider());
            for n in [50, 100, 200] {
                view.push(checked(line(&tf!("{n} per page", "{n} на страницу"), Glyph::default(), &format!("per-page-{n}"), true), self.per_page == n));
            }
        }
        let model = |list: Vec<MenuEntry>| ModelRc::new(VecModel::from(list));
        ui.set_context_entries(model(entries));
        ui.set_context_submenus(ModelRc::new(VecModel::from(vec![model(related), model(report), model(window), model(view)])));
    }

    /// A line of a tile's menu was picked.
    fn context_activated(&mut self, id: &str) {
        let Some(card) = self.context.and_then(|i| self.cards.get(i)).cloned() else { return };
        let dir = self.dir_of(&card);
        match id {
            "playlist-add" => {
                if let Some(dir) = dir
                    && !self.playlist.items.contains(&dir)
                {
                    self.playlist.items.push(dir);
                    self.playlist.save();
                    self.show_playlist();
                }
            }
            "playlist-remove" => {
                if let Some(row) = dir.and_then(|d| self.playlist.items.iter().position(|p| *p == d)) {
                    self.playlist_remove(row);
                }
            }
            "subscribe" | "unsubscribe" => {
                if self.busy.insert(card.id) {
                    let _ = self.steam.send(if id == "subscribe" { Command::Subscribe(card.id) } else { Command::Unsubscribe(card.id) });
                    self.refresh_marks();
                }
            }
            "favorite" | "unfavorite" => {
                if id == "favorite" {
                    self.favorites.insert(card.id);
                } else {
                    self.favorites.remove(&card.id);
                }
                self.save();
                self.refresh_marks();
            }
            "workshop" | "report" => {
                let result = desktop::open_steam(&format!("steam://url/CommunityFilePage/{}", card.id));
                self.report(result, t("The page is open in Steam", "Страница открыта в Steam"));
            }
            "author" => {
                let owner = self.workshop_info.get(&card.id).map_or(card.owner, |e| e.owner);
                self.browse_author(owner);
            }
            "folder" => {
                if let Some(dir) = dir {
                    let result = desktop::open(&dir.to_string_lossy());
                    self.report(result, t("The wallpaper folder is open", "Папка обоев открыта"));
                }
            }
            _ => {
                if let Some(size) = id.strip_prefix("icons-").and_then(|n| n.parse::<i32>().ok()) {
                    self.icon_size = size;
                    if let Some(ui) = self.ui() {
                        ui.set_icon_size(size);
                    }
                    self.save();
                } else if let Some(n) = id.strip_prefix("per-page-").and_then(|n| n.parse::<usize>().ok()) {
                    self.per_page = n;
                    self.save();
                    self.reload();
                }
            }
        }
    }

    /// The workshop, an author's wallpapers only (the original's "author filter").
    fn browse_author(&mut self, owner: u64) {
        if owner == 0 {
            return;
        }
        if !self.personas.contains_key(&owner) {
            let _ = self.steam.send(Command::Persona(owner));
        }
        self.author = Some(owner);
        self.tab = WORKSHOP;
        self.page = 1;
        self.selected = None;
        self.show_tab();
    }

    fn open_file(&mut self, picture: bool) {
        match desktop::pick_file(picture) {
            Ok(Some(path)) => {
                // A project.json stands for the wallpaper folder it is in.
                let target = if path.file_name().is_some_and(|n| n == "project.json") {
                    path.parent().map(PathBuf::from).unwrap_or(path)
                } else {
                    path
                };
                let result = desktop::apply(&target);
                if result.is_ok() {
                    self.current = desktop::current();
                }
                self.report(result, &tf!("{} is on the desktop", "{} на рабочем столе", target.display()));
                self.refresh_marks();
                self.show_monitors();
            }
            Ok(None) => {}
            Err(e) => self.status(tf!("That did not work: {e}", "Не получилось: {e}")),
        }
    }

    /// "Отмена": put back what the desktop showed when the window opened.
    fn revert(&mut self) {
        let before = self.opened_with.clone();
        if desktop::Desktop::load() == before {
            return;
        }
        let _ = if before.wallpaper.is_some() || !before.outputs.is_empty() {
            // The live wallpapers as they were, every monitor's own too.
            before.save().and_then(|()| engine::ensure_daemon())
        } else {
            // Nothing was live: the engine stops, and the picture set before
            // (if one was) shows again.
            match &before.image {
                Some(picture) => desktop::apply(picture),
                None => desktop::take_off().map(|()| drop(std::thread::spawn(engine::see_off))),
            }
        };
        self.current = desktop::current();
    }

    // ------------------------------------------------------------- monitors, settings

    fn show_monitors(&self) {
        let Some(ui) = self.ui() else { return };
        let overrides = desktop::outputs();
        let title = |path: &PathBuf| {
            self.installed
                .iter()
                .find(|i| &i.dir == path)
                .map_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), |i| i.title.clone())
        };
        let monitors = desktop::monitors();
        let left = monitors.iter().map(|m| m.x).fold(f32::MAX, f32::min);
        let top = monitors.iter().map(|m| m.y).fold(f32::MAX, f32::min);
        let rows: Vec<MonitorEntry> = monitors
            .iter()
            .map(|m| MonitorEntry {
                name: m.name.as_str().into(),
                detail: m.detail.as_str().into(),
                wallpaper: overrides
                    .iter()
                    .find(|(name, _)| name == &m.name)
                    .map(|(_, path)| path)
                    .or(self.current.as_ref())
                    .map(title)
                    .unwrap_or_default()
                    .into(),
                x: m.x - left,
                y: m.y - top,
                w: m.width,
                h: m.height,
                image: Image::default(),
                has_image: false,
            })
            .collect();
        ui.set_desk_width(monitors.iter().map(|m| m.x - left + m.width).fold(1.0, f32::max));
        ui.set_desk_height(monitors.iter().map(|m| m.y - top + m.height).fold(1.0, f32::max));
        let index = self.monitor.as_ref().and_then(|name| monitors.iter().position(|m| &m.name == name));
        ui.set_monitor_index(index.map_or(-1, |i| i as i32));
        // As the original: the row is there when there is a monitor to choose
        // (or one so wide it is likely several), the number when there are more.
        let lone_wide = monitors.len() == 1 && monitors.iter().all(|m| m.width / m.height.max(1.0) > 3.5 || m.height / m.width.max(1.0) > 3.5);
        ui.set_monitor_row(monitors.len() > 1 || lone_wide);
        ui.set_monitor_title(if monitors.len() > 1 { tf!("Display {}", "Монитор {}", index.unwrap_or(0) + 1) } else { t("Display", "Монитор").to_owned() }.into());
        ui.set_monitors(ModelRc::new(VecModel::from(rows)));
    }

    /// Take the wallpaper off a monitor, or off all of them.
    fn clear_monitor(&mut self, index: i32) {
        let name = usize::try_from(index).ok().and_then(|i| desktop::monitors().into_iter().nth(i)).map(|m| m.name);
        let result = match &name {
            // Its own wallpaper goes; without one, there is only everybody's to take off.
            Some(name) if desktop::outputs().iter().any(|(output, _)| output == name) => desktop::remove_from(name),
            _ => desktop::take_off().map(|()| drop(std::thread::spawn(engine::see_off))),
        };
        self.current = desktop::current();
        self.report(result, t("The wallpaper is off", "Обои убраны"));
        self.refresh_marks();
        self.show_monitors();
    }

    /// The monitor chooser comes down over the window, the window blurred behind it.
    fn open_monitors(&mut self) {
        let Some(ui) = self.ui() else { return };
        if ui.get_monitors_open() {
            ui.set_monitors_open(false);
            return;
        }
        self.blur_backdrop();
    }

    /// Bring the monitor chooser down over a blurred copy of the window.
    fn blur_backdrop(&mut self) {
        let Some(ui) = self.ui() else { return };
        self.show_monitors();
        if let Ok(shot) = ui.window().take_snapshot() {
            ui.set_backdrop(blurred(&shot));
        }
        ui.set_monitors_open(true);
    }

    fn pick_monitor(&mut self, index: i32) {
        self.monitor = usize::try_from(index).ok().and_then(|i| desktop::monitors().into_iter().nth(i)).map(|m| m.name);
        self.show_monitors();
    }

    /// Switch the interface to a language and make every label again.
    fn set_language(&mut self, russian: bool) {
        self.language = Some(if russian { "ru" } else { "en" }.to_owned());
        lang::set_russian(russian);
        if let Some(ui) = self.ui() {
            ui.global::<L>().set_russian(russian);
            let mut values = ui.get_settings();
            values.language = i32::from(russian);
            ui.set_settings(values);
            ui.set_about(about().into());
        }
        self.save();
        self.show_groups();
        self.show_monitors();
        self.show_playlist();
        self.show_tab();
    }

    fn show_settings(&self) {
        let Some(ui) = self.ui() else { return };
        let settings = Settings::load();
        let multi = desktop::monitors().len() > 1;
        let index = |row: Row, value: &str| playback_choices(row, multi).iter().position(|c| *c == value).unwrap_or(0) as i32;
        ui.set_settings(SettingsValues {
            focus: index(Row::Focus, &settings.playback_focus),
            maximized: index(Row::Maximized, &settings.playback_maximized),
            fullscreen: index(Row::Fullscreen, &settings.playback_fullscreen),
            playing: index(Row::Audio, &settings.playback_audio),
            sleep: index(Row::Sleep, &settings.playback_sleep),
            several: multi,
            fps: settings.fps,
            texture: i32::from(settings.texture_half),
            autostart: desktop::autostart(),
            language: i32::from(lang::russian()),
            still: settings.still,
            audio: settings.audio,
            volume: settings.volume,
            video_gpu: settings.video_gpu,
        });
        let plugins: Vec<PluginEntry> = engine::plugins()
            .into_iter()
            .map(|plugin| PluginEntry {
                name: plugin.name.into(),
                path: plugin.path.to_string_lossy().as_ref().into(),
                kind: plugin.kind.into(),
                loaded: plugin.enabled,
            })
            .collect();
        ui.set_plugins(ModelRc::new(VecModel::from(plugins)));
    }

    fn save_settings(&mut self) {
        let Some(ui) = self.ui() else { return };
        let values = ui.get_settings();
        if (values.language == 1) != lang::russian() {
            self.set_language(values.language == 1);
        }
        if values.autostart != desktop::autostart()
            && let Err(e) = desktop::set_autostart(values.autostart)
        {
            self.status(tf!("That did not work: {e}", "Не получилось: {e}"));
        }
        let multi = values.several;
        let choice = |row: Row, index: i32| playback_choices(row, multi).get(index.max(0) as usize).copied().unwrap_or("run").to_owned();
        let settings = Settings {
            fps: values.fps,
            video_gpu: values.video_gpu,
            audio: values.audio,
            volume: values.volume,
            playback_focus: choice(Row::Focus, values.focus),
            playback_maximized: choice(Row::Maximized, values.maximized),
            playback_fullscreen: choice(Row::Fullscreen, values.fullscreen),
            playback_audio: choice(Row::Audio, values.playing),
            playback_sleep: choice(Row::Sleep, values.sleep),
            texture_half: values.texture == 1,
            still: values.still,
            ..Settings::load()
        };
        if settings == Settings::load() {
            return;
        }
        let result = settings.save().and_then(|()| engine::ensure_daemon());
        self.report(result, t("Settings applied", "Настройки применены"));
        self.show_monitors();
    }

    /// "Сбросить": every setting as it comes.
    fn reset_settings(&mut self) {
        let fresh = Settings { hook: Settings::load().hook, ..Settings::default() };
        let result = fresh.save().and_then(|()| engine::ensure_daemon());
        self.report(result, t("Settings reset", "Настройки сброшены"));
        self.show_settings();
    }

    // ------------------------------------------------------------- presets

    /// The presets of the selected wallpaper: named sets of its properties.
    fn show_presets(&self) {
        let Some(ui) = self.ui() else { return };
        let names: Vec<SharedString> =
            self.selected_card().map(|card| props::presets(card.id)).unwrap_or_default().into_iter().map(Into::into).collect();
        ui.set_presets(ModelRc::new(VecModel::from(names)));
    }

    fn preset_save(&mut self, name: &str) {
        let Some(card) = self.selected_card() else { return };
        let result = props::save_preset(card.id, name, &self.overrides);
        self.report(result, &tf!("Preset \"{}\" saved", "Пресет «{}» сохранён", name.trim()));
        self.show_presets();
    }

    fn preset_load(&mut self, row: usize) {
        let Some(card) = self.selected_card() else { return };
        let Some(name) = props::presets(card.id).into_iter().nth(row) else { return };
        let Some(values) = props::preset(card.id, &name) else { return };
        // The renderer is told every property that is another value now.
        if let Some(dir) = self.dir_of(&card) {
            for property in &self.properties {
                let before = self.overrides.get(&property.name).unwrap_or(&property.default);
                let after = values.get(&property.name).unwrap_or(&property.default);
                if !props::same(before, after) {
                    props::tell_engines(&dir, &property.name, after);
                }
            }
        }
        self.overrides = values;
        props::save_overrides(card.id, &self.overrides);
        self.show_properties();
        self.status(tf!("Preset \"{name}\" loaded", "Пресет «{name}» загружен"));
    }

    /// Give every other installed wallpaper the values set here, for the
    /// properties it has by the same name and of the same kind.
    fn preset_apply_all(&mut self) {
        let Some(card) = self.selected_card() else { return };
        let mut changed = 0;
        for item in self.installed.iter().filter(|item| item.id != card.id) {
            let theirs = props::load(&item.dir);
            let mut values = props::overrides(item.id);
            let before = values.clone();
            for mine in self.properties.iter().filter(|p| self.overrides.contains_key(&p.name)) {
                if theirs.iter().any(|p| p.name == mine.name && std::mem::discriminant(&p.kind) == std::mem::discriminant(&mine.kind))
                    && let Some(value) = self.overrides.get(&mine.name)
                {
                    values.insert(mine.name.clone(), value.clone());
                    props::tell_engines(&item.dir, &mine.name, value);
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
    fn preset_share(&mut self) {
        let json = serde_json::to_string(&self.overrides).unwrap_or_default();
        self.report(desktop::copy(&json), t("The preset is on the clipboard as JSON", "Пресет в буфере обмена как JSON"));
    }

    // ------------------------------------------------------------- playlist    // ------------------------------------------------------------- playlist

    fn playlist_id(&self, row: usize) -> Option<u64> {
        self.playlist.items.get(row).and_then(|p| p.file_name()).and_then(|n| n.to_str()).and_then(|n| n.parse().ok())
    }

    fn show_playlist(&mut self) {
        let Some(ui) = self.ui() else { return };
        let entries: Vec<PlaylistEntry> = self
            .playlist
            .items
            .iter()
            .map(|path| PlaylistEntry {
                title: self.installed.iter().find(|i| &i.dir == path).map_or("", |i| i.title.as_str()).into(),
                image: Image::default(),
                loaded: false,
                selected: false,
            })
            .collect();
        self.playlist_model.set_vec(entries);
        self.mark_playlist();
        for (row, path) in self.playlist.items.iter().enumerate() {
            if let (Some(id), Some(preview)) = (
                self.playlist_id(row),
                self.installed.iter().find(|i| &i.dir == path).and_then(|i| i.preview.clone()),
            ) {
                self.previews.request(Job { id, source: Source::File(preview), size: 96, generation: 0, large: false, animate: false });
            }
        }
        ui.set_playlist_running(self.playlist.running);
        ui.set_playlist_minutes(self.playlist.minutes as i32);
        ui.set_playlist_order(i32::from(self.playlist.random));
        ui.set_playlist_saved(ModelRc::new(VecModel::from(
            Playlist::saved().into_iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
    }

    fn playlist_add(&mut self) {
        let Some(card) = self.selected_card() else { return };
        let Some(dir) = self.dir_of(&card) else { return };
        if self.playlist.items.contains(&dir) {
            self.status(t("This wallpaper is in the playlist already", "Эти обои уже в плейлисте"));
            return;
        }
        self.playlist.items.push(dir);
        self.playlist.save();
        self.show_playlist();
        self.status(tf!("\"{}\" added to the playlist", "«{}» в плейлисте", card.title));
    }

    /// The selected wallpaper in the playlist: its entry framed, and "Добавить
    /// обои" turned into "Удалить обои".
    fn mark_playlist(&mut self) {
        let dir = self.selected_card().and_then(|card| self.dir_of(&card));
        for (row, path) in self.playlist.items.iter().enumerate() {
            if let Some(mut entry) = self.playlist_model.row_data(row) {
                let selected = dir.as_ref() == Some(path);
                if entry.selected != selected {
                    entry.selected = selected;
                    self.playlist_model.set_row_data(row, entry);
                }
            }
        }
        if let Some(ui) = self.ui() {
            ui.set_sel_in_playlist(dir.is_some_and(|d| self.playlist.items.contains(&d)));
        }
    }

    fn playlist_remove(&mut self, row: usize) {
        if row < self.playlist.items.len() {
            self.playlist.items.remove(row);
            if self.playlist.items.is_empty() {
                self.playlist.running = false;
            }
            self.playlist.save();
            self.show_playlist();
        }
    }

    fn playlist_run(&mut self, running: bool) {
        if let Some(ui) = self.ui() {
            self.playlist.minutes = ui.get_playlist_minutes().max(1) as u32;
            self.playlist.random = ui.get_playlist_order() == 1;
        }
        self.playlist.running = running && !self.playlist.items.is_empty();
        self.playlist.save();
        if self.playlist.running {
            self.report(engine::ensure_daemon(), t("Playlist started", "Плейлист запущен"));
        } else {
            self.status(t("Playlist stopped", "Плейлист остановлен"));
        }
        self.show_playlist();
    }

    // ------------------------------------------------------------- Steam

    fn steam_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Session(result) => {
                self.steam_ok = result.is_ok();
                if let Some(ui) = self.ui() {
                    ui.set_steam_ok(self.steam_ok);
                    if let Err(e) = result
                        && self.online()
                        && self.cards.is_empty()
                    {
                        ui.set_loading(false);
                        ui.set_error(tf!("{e}. Start Steam: the list will load by itself.", "{e}. Запусти Steam: список загрузится сам.").into());
                    }
                }
            }
            Reply::Page { generation, result } => self.page_arrived(generation, result),
            Reply::Details(entries) => {
                let installed_tab = self.tab == INSTALLED;
                for entry in entries {
                    self.workshop_info.insert(entry.id, entry);
                }
                // The marks (approved) and the panel come from them now.
                if installed_tab && self.local_sort != 1 && self.local_sort != 5 {
                    self.show_selection(true);
                } else if installed_tab {
                    self.show_installed();
                }
            }
            Reply::Vote { id, vote } => {
                self.votes.insert(id, vote);
                self.show_selection(true);
            }
            Reply::Persona { id, persona } => {
                let avatar = persona.avatar.map(|(w, h, rgba)| Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, w, h)));
                self.personas.insert(id, (persona.name, avatar));
                self.show_selection(true);
                if self.author == Some(id)
                    && let Some(ui) = self.ui()
                {
                    let name = self.personas.get(&id).map_or("?", |(name, _)| name.as_str());
                    ui.set_notice(tf!("Browsing wallpapers by {name}", "Смотрим все обои от {name}").into());
                }
            }
            Reply::Subscribed { id, ok } => {
                self.busy.remove(&id);
                if ok {
                    *self.subscribed.entry(id).or_insert(0) |= steam::SUBSCRIBED;
                    self.progress.insert(id, 0.0);
                    self.status(t("Subscribed, Steam is downloading the wallpaper", "Подписка оформлена, Steam загружает обои"));
                } else {
                    self.progress.remove(&id);
                    // Not to be set by itself whenever it gets to the disk.
                    if self.set_when_ready == Some(id) {
                        self.set_when_ready = None;
                    }
                    self.status(t("Steam did not take the subscription", "Steam не принял подписку"));
                }
                self.refresh_marks();
            }
            Reply::Unsubscribed { id, ok } => {
                self.busy.remove(&id);
                self.progress.remove(&id);
                if self.set_when_ready == Some(id) {
                    self.set_when_ready = None;
                }
                if ok {
                    self.subscribed.remove(&id);
                    self.status(t("Unsubscribed, Steam will remove the wallpaper from the disk", "Подписка снята, Steam уберёт обои с диска"));
                } else {
                    self.status(t("Steam did not remove the subscription", "Steam не снял подписку"));
                }
                self.refresh_marks();
            }
            Reply::Subscriptions(list) => {
                self.subscribed = list.into_iter().collect();
                let on_disk = library::installed();
                let changed = on_disk.len() != self.installed.len();
                self.installed = on_disk;
                if self.tab == INSTALLED && changed {
                    self.show_installed();
                } else {
                    for card in &mut self.cards {
                        if card.dir.is_none() {
                            card.dir = self.installed.iter().find(|i| i.id == card.id).map(|i| i.dir.clone());
                        }
                    }
                    self.refresh_marks();
                }
                // What was just subscribed to is on the disk: on the desktop
                // it goes, when it is still what the user looks at.
                if let Some(id) = self.set_when_ready
                    && self.installed.iter().any(|i| i.id == id)
                {
                    self.set_when_ready = None;
                    if self.selected_card().is_some_and(|c| c.id == id) {
                        self.show_selection(true);
                        self.apply();
                    }
                }
            }
            Reply::Progress { id, state, done, total } => {
                self.subscribed.insert(id, state);
                let finished = state & steam::INSTALLED != 0 && state & (steam::DOWNLOADING | steam::PENDING) == 0;
                if finished {
                    self.progress.remove(&id);
                    self.status(t("The wallpaper is downloaded", "Обои загружены"));
                } else if total > 0 {
                    self.progress.insert(id, done as f32 / total as f32);
                    self.status(tf!("Steam is downloading: {} of {}", "Steam загружает обои: {} из {}", megabytes(done), megabytes(total)));
                }
                self.refresh_marks();
            }
        }
    }
}

/// « 1 2 3 4 5 … N » around the current page, as the Windows client lays them out.
/// The page numbers under the grid, as the original lays them out: the first
/// and the last always, four either side of the current one, "…" for the
/// rest (it asks for a number), the arrows at the ends. Page 0 is "…".
fn page_buttons(current: u32, pages: u32) -> Vec<PageButton> {
    let button = |label: String, page: u32| PageButton { label: label.into(), page: page as i32, current: page == current };
    let mut out = vec![PageButton { label: "«".into(), page: current.saturating_sub(1).max(1) as i32, current: false }];
    let (low, high) = (current.saturating_sub(4).max(1), (current + 4).min(pages));
    let mut page = 1;
    while page <= pages {
        if (page != 1 && page < low) || (page != pages && page > high) {
            out.push(PageButton { label: "…".into(), page: 0, current: false });
            page = if page < low { low } else { pages };
        }
        out.push(button(page.to_string(), page));
        page += 1;
    }
    out.push(PageButton { label: "»".into(), page: (current + 1).min(pages) as i32, current: false });
    out
}

const USAGE: &str = "\
we-linux                          the window: library, workshop, playlist
we-linux apply <ID|PATH> [--output NAME]
                                  set a wallpaper (a workshop id, a wallpaper
                                  folder, a video or a picture) on every
                                  monitor, or on one
we-linux off                      no live wallpaper
we-linux status                   what is on the desktop
we-linux doctor                   everything a bug report needs: versions, what
                                  was found, the settings, the engine's log
we-linux workshop [TEXT] [--sort top|trend|new|subs|updated] [--type scene|video|web]
                  [--page N]      search the workshop: id, size, votes, title
we-linux subscribe ID...          subscribe and wait for Steam to download
we-linux unsubscribe ID...        unsubscribe (Steam removes the files)
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

/// `--show key=value,...`: the window opens in a state of its own, for
/// screenshots and checks. Not in the usage: it is a tool of development.
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

/// The command line; None opens the window.
fn command_line() -> Option<i32> {
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
            let target = args.iter().enumerate().skip(1).find(|(i, a)| !a.starts_with("--") && flag != Some(i - 1)).map(|(_, a)| a);
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
            let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
            // A flag's value by where it stands: the same word may be searched for.
            let values: Vec<usize> = ["--sort", "--type", "--page"].iter().filter_map(|n| args.iter().position(|a| a == n)).map(|i| i + 1).collect();
            let text = args.iter().enumerate().skip(1).filter(|(i, a)| !a.starts_with("--") && !values.contains(i)).map(|(_, a)| a.clone()).collect::<Vec<_>>().join(" ");
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
            match steam::Steam::open().and_then(|steam| steam.query(&query)) {
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
            let steam = match steam::Steam::open() {
                Ok(steam) => steam,
                Err(e) => {
                    eprintln!("we-linux: {e}");
                    return Some(1);
                }
            };
            let subscribing = args[0] == "subscribe";
            for &id in &ids {
                let ok = if subscribing { steam.subscribe(id) } else { steam.unsubscribe(id) };
                say!("{id}: {}", if ok { if subscribing { "subscribed" } else { "unsubscribed" } } else { "Steam refused" });
            }
            if subscribing {
                // Until Steam has them all (or ten minutes).
                let started = std::time::Instant::now();
                loop {
                    steam.pump();
                    let left = ids.iter().filter(|&&id| steam.progress(id).0 & steam::INSTALLED == 0).count();
                    if left == 0 || started.elapsed().as_secs() > 600 {
                        say!("downloaded: {} of {}", ids.len() - left, ids.len());
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

fn main() -> Result<(), slint::PlatformError> {
    steam::init_env();
    if let Some(code) = command_line() {
        std::process::exit(code);
    }

    // A wallpaper is set and nothing shows it (the daemon was not started
    // with the session, or went away): it comes back with the window.
    if desktop::current().is_some() {
        let _ = engine::ensure_daemon();
    }

    let ui = AppWindow::new()?;
    // The window's Wayland app id: what compositor rules and the menu entry
    // match. Only after the window is made (there is no platform before that)
    // and before it shows.
    slint::set_xdg_app_id("we-linux")?;
    // The language first: every label made below is in it. Until the first
    // run asks, it is the system's.
    let language = std::fs::read(config_file())
        .ok()
        .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
        .and_then(|saved| saved.language);
    let russian = language.as_deref().map_or_else(lang::system_is_russian, |l| l == "ru");
    lang::set_russian(russian);
    ui.global::<L>().set_russian(russian);
    ui.global::<Theme>().set_font(desktop::ui_font().into());
    ui.set_caption(format!("WE Linux {}", env!("CARGO_PKG_VERSION")).into());
    ui.set_about(about().into());
    if language.is_none() {
        ui.set_dialog(9);
    }

    let model = Rc::new(VecModel::<Item>::default());
    let playlist_model = Rc::new(VecModel::<PlaylistEntry>::default());
    ui.set_items(ModelRc::from(model.clone()));
    ui.set_playlist(ModelRc::from(playlist_model.clone()));

    let app = Rc::new(RefCell::new(App {
        ui: ui.as_weak(),
        language: language.clone(),
        applying: None,
        steam: steam::spawn(|reply| on_ui(move |app| app.steam_reply(reply))),
        previews: Previews::new(|picture| on_ui(move |app| app.picture_arrived(picture))),
        tab: WORKSHOP,
        cards: Vec::new(),
        model,
        selected: None,
        installed: library::installed(),
        subscribed: HashMap::new(),
        progress: HashMap::new(),
        busy: HashSet::new(),
        set_when_ready: None,
        favorites: HashSet::new(),
        groups: default_groups(),
        workshop_sort: 0,
        local_sort: 0,
        sort_descending: false,
        monitors_on_start: true,
        workshop_search: String::new(),
        local_search: String::new(),
        page: 1,
        total: 0,
        generation: 0,
        picture_generation: 0,
        current: desktop::current(),
        opened_with: desktop::Desktop::load(),
        monitor: None,
        steam_ok: false,
        filter_timer: Timer::default(),
        frames: Vec::new(),
        frame: 0,
        frame_timer: Timer::default(),
        properties: Vec::new(),
        overrides: BTreeMap::new(),
        shown_properties: Vec::new(),
        playlist: Playlist::load(),
        playlist_model,
        workshop_info: HashMap::new(),
        asked_details: HashSet::new(),
        personas: HashMap::new(),
        sizes: HashMap::new(),
        per_page: 100,
        icon_size: 1,
        search_timer: Timer::default(),
        context: None,
        explore: Vec::new(),
        explore_base: 0,
        author: None,
        votes: HashMap::new(),
        asked_personas: HashSet::new(),
        cells: HashMap::new(),
        cell_timer: Timer::default(),
        visible: (0, 40),
        animations_asked: HashSet::new(),
    }));
    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));

    let start_monitors = Timer::default();
    let app_handle = app.clone();
    {
        let mut app = app.borrow_mut();
        app.restore();
        app.show_groups();
        app.show_monitors();
        app.show_settings();
        app.show_playlist();
        app.show_tab();
        // "Choose monitor" first, where there is more than one to choose from
        // (over the window once it is on the screen: it is blurred behind).
        if app.monitors_on_start && language.is_some() && desktop::monitors().len() > 1 {
            let handle = app_handle.clone();
            start_monitors.start(TimerMode::SingleShot, std::time::Duration::from_millis(300), move || handle.borrow_mut().blur_backdrop());
        }
        if library::assets().is_none() {
            app.status(t("Wallpaper Engine is not installed in Steam: scenes will not start without its assets", "Wallpaper Engine не установлен в Steam: без его ассетов сцены не запустятся"));
        }
    }

    // The state asked for with --show.
    let state = shown_state();
    let number = |key: &str| state.get(key).and_then(|v| v.parse::<i32>().ok());
    let pending_select = Timer::default();
    let pending_monitors = Timer::default();
    let handle = app.clone();
    {
        let mut app = app.borrow_mut();
        if let Some(tab) = number("tab") {
            app.tab = tab.clamp(INSTALLED, WORKSHOP);
            app.show_tab();
        }
        if let Some(open) = number("filters") {
            ui.set_filters_open(open != 0);
        }
        if let Some(tab) = number("settings") {
            app.show_settings();
            ui.set_settings_tab(tab);
            ui.set_settings_open(true);
        }
        if number("monitors").is_some() {
            // The blurred window behind it needs the window on the screen first.
            let handle = handle.clone();
            ui.set_monitors_open(false);
            pending_monitors.start(TimerMode::SingleShot, std::time::Duration::from_millis(2000), move || handle.borrow_mut().blur_backdrop());
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
        // shown without going on the desktop.
        if let Some(index) = number("select") {
            pending_select.start(TimerMode::SingleShot, std::time::Duration::from_millis(2500), move || {
                let mut app = handle.borrow_mut();
                if (index as usize) < app.cards.len() {
                    app.selected = Some(index as usize);
                    app.show_selection(false);
                    app.show_presets();
                }
            });
        }
    }

    // Each handler borrows the app for its own call only.
    macro_rules! on {
        ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {{
            let handle = app.clone();
            ui.$setter(move |$($arg),*| {
                #[allow(unused_mut)]
                let mut $app = handle.borrow_mut();
                $body
            });
        }};
    }
    on!(on_tab_changed, |app, tab| {
        app.tab = tab;
        app.author = None;
        app.page = 1;
        app.selected = None;
        app.save();
        app.show_tab();
    });
    on!(on_search_typed, |app, text| {
        // The original's field waits a tenth of a second for the next letter.
        let text = text.to_string();
        app.search_timer.start(TimerMode::SingleShot, std::time::Duration::from_millis(100), move || {
            let text = text.clone();
            APP.with(|slot| {
                if let Some(app) = slot.borrow().as_ref() {
                    let mut app = app.borrow_mut();
                    if app.online() {
                        app.workshop_search = text;
                    } else {
                        app.local_search = text;
                    }
                    app.page = 1;
                    app.reload();
                }
            });
        });
    });
    on!(on_open_monitors, |app| app.open_monitors());
    on!(on_item_context, |app, index| app.context_menu(index.max(0) as usize));
    on!(on_visible_changed, |app, first, last| app.visible_changed(first.max(0) as usize, last.max(0) as usize));
    on!(on_notice_closed, |app| {
        app.author = None;
        app.page = 1;
        app.show_tab();
    });
    on!(on_context_activated, |app, id| app.context_activated(&id));
    on!(on_monitor_profiles, |app, save| {
        if let Some(ui) = app.ui() {
            let names: Vec<SharedString> = desktop::profiles().into_iter().map(SharedString::from).collect();
            ui.set_profiles(ModelRc::new(VecModel::from(names)));
            ui.set_preset_name("".into());
            ui.set_dialog(if save { 11 } else { 10 });
        }
    });
    on!(on_profile_save, |app, name| {
        let result = desktop::save_profile(name.trim());
        app.report(result, t("The profile is saved", "Профиль сохранён"));
    });
    on!(on_profile_load, |app, row| {
        if let Some(name) = desktop::profiles().get(row.max(0) as usize) {
            let result = desktop::load_profile(name);
            app.current = desktop::current();
            app.report(result, t("The profile is on the desktop", "Профиль на рабочем столе"));
            app.refresh_marks();
            app.show_monitors();
        }
    });
    on!(on_search_submitted, |app, text| {
        if app.online() {
            app.workshop_search = text.to_string();
        } else {
            app.local_search = text.to_string();
        }
        app.page = 1;
        app.reload();
    });
    on!(on_sort_changed, |app, index| {
        let index = index.max(0) as usize;
        if app.online() {
            app.workshop_sort = index.min(WORKSHOP_SORTS.len() - 1);
        } else {
            app.local_sort = index.min(LOCAL_SORTS.len() - 1);
        }
        app.page = 1;
        app.save();
        app.reload();
    });
    on!(on_filter_toggled, |app, group, option| {
        if let Some(choice) = app.groups.get_mut(group as usize).and_then(|g| g.choices.get_mut(option as usize)) {
            choice.checked = !choice.checked;
            app.filters_changed();
        }
    });
    on!(on_filter_group_all, |app, group, checked| {
        if let Some(group) = app.groups.get_mut(group as usize) {
            group.choices.iter_mut().for_each(|c| c.checked = checked);
            app.filters_changed();
        }
    });
    on!(on_group_toggled, |app, group| {
        if let Some(group) = app.groups.get_mut(group as usize) {
            group.open = !group.open;
            app.show_groups();
        }
    });
    on!(on_group_shown, |app, group| {
        if let Some(group) = app.groups.get_mut(group as usize) {
            group.shown = !group.shown;
            app.show_groups();
            app.save();
        }
    });
    on!(on_filter_section_all, |app, group, section, checked| {
        if let Some(group) = app.groups.get_mut(group as usize) {
            group.choices.iter_mut().filter(|c| c.section == section as usize).for_each(|c| c.checked = checked);
            app.filters_changed();
        }
    });
    on!(on_content_preferences, |app| {
        let result = desktop::open_steam("steam://openurl/https://store.steampowered.com/account/preferences");
        app.report(result, t("Steam's content preferences are open", "Настройки контента Steam открыты"));
    });
    on!(on_anime_changed, |app, shown| {
        // As the original: the answer ticks "Аниме" among the tags, or not.
        if let Some(choice) = app.groups.iter_mut().find(|g| g.title == "Метки").and_then(|g| g.choices.iter_mut().find(|c| c.tag == "Anime")) {
            choice.checked = shown;
        }
        if let Some(ui) = app.ui() {
            ui.set_anime_shown(shown);
        }
        app.filters_changed();
    });
    on!(on_resolution_changed, |app, recommended, other| {
        // Recommended: the resolutions of the shape of the monitors (wide,
        // ultra wide, portrait), dynamic ones, and unknown ones when wanted.
        let monitors = desktop::monitors();
        let shape = |m: &desktop::Monitor| {
            let ratio = m.width / m.height.max(1.0);
            if ratio < 1.0 { 4 } else if ratio > 2.2 { 1 } else { 0 }
        };
        let fitting: Vec<usize> = if monitors.is_empty() { vec![0] } else { monitors.iter().map(shape).collect() };
        if let Some(group) = app.groups.iter_mut().find(|g| g.title == "Разрешение экрана") {
            for choice in &mut group.choices {
                choice.checked = if choice.tag == "Other resolution" {
                    other
                } else {
                    !recommended || choice.section == 5 || fitting.contains(&choice.section)
                };
            }
        }
        if let Some(ui) = app.ui() {
            ui.set_resolution_recommended(recommended);
            ui.set_other_resolution(other);
        }
        app.filters_changed();
    });
    on!(on_filters_reset, |app| {
        for group in &mut app.groups {
            group.choices.iter_mut().for_each(|c| c.checked = c.default);
        }
        app.filters_changed();
    });
    on!(on_page_clicked, |app, page| {
        // A number past the last page is the last page, as in the original.
        let last = if app.online() { app.total.div_ceil(steam::PAGE_SIZE) } else { app.total.div_ceil(app.per_page as u32) };
        app.page = (page.max(1) as u32).min(last.max(1));
        app.selected = None;
        app.reload();
    });
    on!(on_item_selected, |app, index| app.select(index.max(0) as usize));
    on!(on_item_activated, |app, index| app.activate(index.max(0) as usize));
    on!(on_subscribe, |app| app.subscribe());
    on!(on_unsubscribe, |app| app.unsubscribe());
    on!(on_apply, |app| app.apply());
    on!(on_favorite, |app| app.toggle_favorite());
    on!(on_vote, |app, up| {
        if let Some(card) = app.selected_card()
            && card.id != 0
        {
            let _ = app.steam.send(Command::SetVote { id: card.id, up });
        }
    });
    on!(on_browse_author, |app| {
        if let Some(card) = app.selected_card() {
            let owner = app.workshop_info.get(&card.id).map_or(card.owner, |e| e.owner);
            app.browse_author(owner);
        }
    });
    on!(on_open_author, |app| {
        if let Some(card) = app.selected_card() {
            let owner = app.workshop_info.get(&card.id).map_or(card.owner, |e| e.owner);
            let result = desktop::open_steam(&format!("steam://url/SteamIDPage/{owner}"));
            app.report(result, t("The author's page is open in Steam", "Страница автора открыта в Steam"));
        }
    });
    on!(on_navigate, |app, step| {
        // The small window's arrows: the wallpaper before or after in the list.
        let next = app.selected.map_or(0, |i| i as i64 + i64::from(step));
        if next >= 0 && (next as usize) < app.cards.len() {
            app.select(next as usize);
        }
    });
    on!(on_open_steam, |app| {
        if let Some(card) = app.selected_card() {
            let result = desktop::open_steam(&format!("steam://url/CommunityFilePage/{}", card.id));
            app.report(result, t("The page is open in Steam", "Страница открыта в Steam"));
        }
    });
    on!(on_open_folder, |app| {
        if let Some(dir) = app.selected_card().and_then(|card| app.dir_of(&card)) {
            let result = desktop::open(&dir.to_string_lossy());
            app.report(result, t("The wallpaper folder is open", "Папка обоев открыта"));
        }
    });
    on!(on_reload, |app| app.reload());
    on!(on_prop_bool, |app, row, value| app.set_property(row.max(0) as usize, Value::Bool(value)));
    on!(on_prop_number, |app, row, value| {
        // Two decimals are what the sliders of the Windows client give.
        let rounded = (f64::from(value) * 100.0).round() / 100.0;
        app.set_property(row.max(0) as usize, serde_json::json!(rounded));
    });
    on!(on_prop_text, |app, row, text| {
        let row = row.max(0) as usize;
        let kind = app.shown_properties.get(row).and_then(|&i| app.properties.get(i)).map(|p| p.kind);
        match kind {
            Some(props::Kind::Color) => match props::from_hex(&text) {
                Some(value) => app.set_property(row, value),
                None => app.status(t("A colour is written as #rrggbb", "Цвет пишется как #rrggbb")),
            },
            Some(_) => app.set_property(row, Value::String(text.to_string())),
            None => {}
        }
    });
    on!(on_prop_index, |app, row, index| {
        let row = row.max(0) as usize;
        let value = app
            .shown_properties
            .get(row)
            .and_then(|&i| app.properties.get(i))
            .and_then(|p| p.options.get(index.max(0) as usize))
            .map(|(_, value)| value.clone());
        if let Some(value) = value {
            app.set_property(row, value);
        }
    });
    on!(on_props_reset, |app| app.reset_properties());
    on!(on_playlist_add, |app| app.playlist_add());
    on!(on_playlist_remove, |app, row| app.playlist_remove(row.max(0) as usize));
    on!(on_playlist_save, |app| {
        // The first number no saved playlist has: one may have been removed.
        let saved = Playlist::saved();
        let name = (saved.len() + 1..)
            .map(|n| tf!("Playlist {}", "Плейлист {}", n))
            .find(|name| !saved.contains(name))
            .unwrap_or_default();
        app.playlist.save_as(&name);
        app.show_playlist();
        app.status(tf!("Saved as \"{name}\"", "Сохранён как «{name}»"));
    });
    on!(on_playlist_load, |app, row| {
        if let Some(loaded) = Playlist::saved().get(row.max(0) as usize).and_then(|name| Playlist::load_saved(name)) {
            let running = app.playlist.running;
            app.playlist = Playlist { running, ..loaded };
            app.playlist.save();
            app.show_playlist();
            app.status(t("Playlist loaded", "Плейлист загружен"));
        }
    });
    on!(on_playlist_start, |app| app.playlist_run(true));
    on!(on_playlist_stop, |app| app.playlist_run(false));
    on!(on_playlist_clear, |app| {
        app.playlist.items.clear();
        app.playlist.running = false;
        app.playlist.save();
        app.show_playlist();
    });
    on!(on_monitor_picked, |app, index| app.pick_monitor(index));
    on!(on_monitor_cleared, |app, index| app.clear_monitor(index));
    on!(on_monitors_on_start_changed, |app, on| {
        app.monitors_on_start = on;
        app.save();
    });
    on!(on_sort_direction_changed, |app| {
        if let Some(ui) = app.ui() {
            app.sort_descending = ui.get_sort_descending();
        }
        app.save();
        app.reload();
    });
    on!(on_copy_link, |app| {
        if let Some(card) = app.selected_card() {
            let link = format!("https://steamcommunity.com/sharedfiles/filedetails/?id={}", card.id);
            app.report(desktop::copy(&link), t("The link is on the clipboard", "Ссылка в буфере обмена"));
        }
    });
    on!(on_report, |app| {
        if let Some(card) = app.selected_card() {
            let result = desktop::open_steam(&format!("steam://url/CommunityFilePage/{}", card.id));
            app.report(result, t("Reports are filed on the wallpaper's page in Steam: it is open", "Жалобы подаются на странице обоев в Steam: она открыта"));
        }
    });
    on!(on_preset_save, |app, name| app.preset_save(&name));
    on!(on_preset_load, |app, row| app.preset_load(row.max(0) as usize));
    on!(on_preset_apply_all, |app| app.preset_apply_all());
    on!(on_preset_share, |app| app.preset_share());
    on!(on_open_editor, |app| {
        app.status(t(
            "The editor is a part of Wallpaper Engine for Windows; wallpapers made in it play here",
            "Редактор есть только в Wallpaper Engine для Windows; сделанные в нём обои здесь играют",
        ));
    });
    on!(on_settings_reset, |app| app.reset_settings());
    on!(on_settings_save, |app| app.save_settings());
    on!(on_settings_shown, |app| app.show_settings());
    on!(on_language_picked, |app, index| {
        app.set_language(index == 1);
        if let Some(ui) = app.ui() {
            ui.set_dialog(0);
        }
    });
    on!(on_open_file, |app, kind| app.open_file(kind == "static"));
    on!(on_playlist_toggle_selected, |app| {
        if let Some(row) = app.selected_card().and_then(|card| app.dir_of(&card)).and_then(|dir| app.playlist.items.iter().position(|p| *p == dir)) {
            app.playlist_remove(row);
        }
    });
    on!(on_playlist_select, |app, row| {
        // As in the original: the entry's wallpaper is selected in the grid, where it is.
        if let Some(path) = app.playlist.items.get(row.max(0) as usize).cloned()
            && let Some(index) = app.cards.iter().position(|c| app.dir_of(c).as_ref() == Some(&path))
        {
            app.select(index);
        }
    });
    {
        let weak = ui.as_weak();
        ui.on_window_close(move || {
            if let Some(ui) = weak.upgrade() {
                let _ = ui.hide();
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_window_minimize(move || {
            if let Some(ui) = weak.upgrade() {
                ui.window().set_minimized(true);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_window_maximize(move || {
            if let Some(ui) = weak.upgrade() {
                let window = ui.window();
                window.set_maximized(!window.is_maximized());
            }
        });
    }
    {
        // The small window: the card alone.
        let weak = ui.as_weak();
        ui.on_mini_toggled(move || {
            if let Some(ui) = weak.upgrade() {
                let mini = !ui.get_mini();
                ui.set_mini(mini);
                let height = ui.window().size().to_logical(ui.window().scale_factor()).height;
                ui.window().set_size(slint::LogicalSize::new(if mini { 340.0 } else { 1656.0 }, height));
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_accept(move || {
            if let Some(ui) = weak.upgrade() {
                let _ = ui.hide();
            }
        });
    }
    {
        let (weak, handle) = (ui.as_weak(), app.clone());
        ui.on_cancel(move || {
            handle.borrow_mut().revert();
            if let Some(ui) = weak.upgrade() {
                let _ = ui.hide();
            }
        });
    }

    // The wallpaper can change from outside (the command line, the playlist): follow it.
    let follow = Timer::default();
    {
        let handle = app.clone();
        follow.start(TimerMode::Repeated, std::time::Duration::from_millis(700), move || {
            let Ok(mut app) = handle.try_borrow_mut() else { return };
            app.show_applying();
            let current = desktop::current();
            if current != app.current {
                app.current = current;
                app.refresh_marks();
                app.show_monitors();
            }
        });
    }

    let result = ui.run();
    let _ = app.borrow().steam.send(Command::Quit);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(buttons: &[PageButton]) -> Vec<String> {
        buttons.iter().map(|b| b.label.to_string()).collect()
    }

    #[test]
    fn pages_as_the_original_lays_them_out() {
        assert_eq!(labels(&page_buttons(1, 2)), ["«", "1", "2", "»"]);
        assert_eq!(labels(&page_buttons(1, 1000)), ["«", "1", "2", "3", "4", "5", "…", "1000", "»"]);
        assert_eq!(labels(&page_buttons(10, 1000)), ["«", "1", "…", "6", "7", "8", "9", "10", "11", "12", "13", "14", "…", "1000", "»"]);
        assert_eq!(labels(&page_buttons(1000, 1000)), ["«", "1", "…", "996", "997", "998", "999", "1000", "»"]);
        let around = page_buttons(10, 1000);
        assert_eq!((around[0].page, around.last().unwrap().page), (9, 11));
        assert!(around.iter().any(|b| b.current && b.label == "10"));
        // The arrows stop at the ends.
        assert_eq!(page_buttons(1, 3)[0].page, 1);
        assert_eq!(page_buttons(3, 3).last().unwrap().page, 3);
    }

    #[test]
    fn search_takes_tags_and_typos() {
        assert_eq!(search_words("Rain HDR!"), ("rain".to_owned(), vec!["HDR"]));
        assert_eq!(search_words("puppet warp girl"), ("girl".to_owned(), vec!["Puppet Warp"]));
        assert!(title_matches("Purple Space", "space"));
        assert!(title_matches("Rain", "rian"));
        assert!(!title_matches("Purple Space", "forest"));
        assert!(title_matches("anything", ""));
        assert_eq!(edits("abc", "acb"), 1);
        assert_eq!(edits("", "abc"), 3);
    }

    #[test]
    fn names_sort_as_the_browser_does() {
        let mut titles = vec!["Anime girl", "150594984_p0", "[MLP] FS", "\u{3010}Blue Archive\u{3011}", "arch linux", "\u{8494}\u{84dd}"];
        titles.sort_by_key(|t| collation_key(t));
        assert_eq!(titles, ["[MLP] FS", "\u{3010}Blue Archive\u{3011}", "150594984_p0", "Anime girl", "arch linux", "\u{8494}\u{84dd}"]);
    }

    #[test]
    fn playback_rows_have_the_original_choices() {
        assert_eq!(playback_choices(Row::Fullscreen, true), ["run", "mute", "pause", "pauseall", "stop"]);
        assert_eq!(playback_choices(Row::Maximized, false), ["run", "mute", "pause", "stop"]);
        assert_eq!(playback_choices(Row::Focus, true), ["run", "mute", "pause", "pauseall"]);
        assert_eq!(playback_choices(Row::Audio, true), ["run", "mute", "pause"]);
        assert_eq!(playback_choices(Row::Sleep, true), ["run", "pause", "stop"]);
    }
}
