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

slint::include_modules!();

const INSTALLED: i32 = 0;
const RECOMMENDED: i32 = 1;
const WORKSHOP: i32 = 2;

/// Label, order and (for trends) the window in days.
const WORKSHOP_SORTS: [(&str, Sort, u32); 9] = [
    ("Лидеры рейтинга", Sort::Vote, 0),
    ("Популярные сегодня", Sort::Trend, 1),
    ("Популярные за неделю", Sort::Trend, 7),
    ("Популярные за месяц", Sort::Trend, 30),
    ("Популярные за 3 месяца", Sort::Trend, 90),
    ("Популярные за год", Sort::Trend, 365),
    ("Самые новые", Sort::Newest, 0),
    ("По числу подписчиков", Sort::Subscriptions, 0),
    ("Последнее обновление", Sort::Updated, 0),
];
const LOCAL_SORTS: [&str; 3] = ["Дата подписки", "Название", "Тип"];
const VIEWS: [&str; 2] = ["Установлено", "Избранное"];

const ONLY: [(&str, &str); 3] = [("Одобренные", "Approved"), ("Реагирующие на звук", "Audio responsive"), ("Настраиваемый", "Customizable")];
const TYPES: [(&str, &str); 4] = [("Сцена", "Scene"), ("Видео", "Video"), ("Веб", "Web"), ("Приложение", "Application")];
const RATINGS: [(&str, &str); 3] = [("Для всех", "Everyone"), ("Сомнительный", "Questionable"), ("Для взрослых", "Mature")];
const RESOLUTIONS: [(&str, &str); 14] = [
    ("Стандартное разрешение", "Standard Definition"),
    ("1280 x 720", "1280 x 720"),
    ("1920 x 1080", "1920 x 1080"),
    ("2560 x 1440", "2560 x 1440"),
    ("3840 x 2160", "3840 x 2160"),
    ("Сверхширокий 2560 x 1080", "Ultrawide 2560 x 1080"),
    ("Сверхширокий 3440 x 1440", "Ultrawide 3440 x 1440"),
    ("Два монитора 3840 x 1080", "Dual 3840 x 1080"),
    ("Два монитора 5120 x 1440", "Dual 5120 x 1440"),
    ("Три монитора 5760 x 1080", "Triple 5760 x 1080"),
    ("Вертикальный 1080 x 1920", "Portrait 1080 x 1920"),
    ("Вертикальный 1440 x 2560", "Portrait 1440 x 2560"),
    ("Динамическое разрешение", "Dynamic resolution"),
    ("Другое разрешение", "Other resolution"),
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
    ("MikuMikuDance", "MMD"),
    ("Музыка", "Music"),
    ("Природа", "Nature"),
    ("Пиксельная графика", "Pixel art"),
    ("Расслабляющее", "Relaxing"),
    ("Ретро", "Retro"),
    ("Научная фантастика", "Sci-Fi"),
    ("Спорт", "Sports"),
    ("Технологии", "Technology"),
    ("Телевидение", "Television"),
    ("Транспорт", "Vehicle"),
    ("Без метки", "Unspecified"),
];
const FEATURES: [(&str, &str); 5] = [
    ("HDR", "HDR"),
    ("3D", "3D"),
    ("Интеграция с медиа", "Media Integration"),
    ("Кукольная деформация", "Puppet Warp"),
    ("Видеотекстура", "Video Texture"),
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
}

/// The groups project.json answers: the rest is known to the workshop only
/// and is left out of the panel over the installed wallpapers.
const LOCAL_GROUPS: [&str; 4] = ["Показать только", "Тип", "Возрастной рейтинг", "Метки"];

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
                .map(|&(label, tag)| Choice { label, tag, checked: default(tag), default: default(tag) })
                .collect(),
        }
    }

    fn checked(&self) -> Vec<&'static str> {
        self.choices.iter().filter(|c| c.checked).map(|c| c.tag).collect()
    }

    /// Whether the installed wallpapers can be asked what the group asks.
    fn local(&self) -> bool {
        LOCAL_GROUPS.contains(&self.title)
    }

    /// Whether the group leaves anything out.
    fn narrows(&self) -> bool {
        let n = self.checked().len();
        match self.kind {
            GroupKind::AnyOf => n > 0 && n < self.choices.len(),
            GroupKind::AllOf => n > 0,
        }
    }
}

fn default_groups() -> Vec<Group> {
    let mut only = Group::new("Показать только", GroupKind::AllOf, true, &ONLY, |_| false);
    only.boxed = true;
    vec![
        only,
        Group::new("Тип", GroupKind::AnyOf, false, &TYPES, |_| true),
        // Like the Windows client: adult content is opt-in.
        Group::new("Возрастной рейтинг", GroupKind::AnyOf, false, &RATINGS, |tag| tag != "Mature"),
        Group::new("Разрешение экрана", GroupKind::AnyOf, false, &RESOLUTIONS, |_| true),
        Group::new("Метки", GroupKind::AnyOf, true, &GENRES, |_| true),
        Group::new("Особенности", GroupKind::AllOf, false, &FEATURES, |_| false),
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
    subscriptions: u64,
    updated: u64,
    description: String,
    picture: Picture,
    dir: Option<PathBuf>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Saved {
    tab: Option<i32>,
    workshop_sort: Option<usize>,
    local_sort: Option<usize>,
    filters_open: Option<bool>,
    /// Checked tags by group title.
    filters: HashMap<String, Vec<String>>,
    /// Titles of the groups the filter settings hide.
    hidden_groups: Vec<String>,
    favorites: Vec<u64>,
    /// "en" or "ru"; nothing until the first run asked.
    language: Option<String>,
}

fn config_file() -> PathBuf {
    desktop::config_dir().join("we-linux/state.json")
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
    view: usize,
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
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

fn megabytes(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        tf!("{:.1} GB", "{:.1} ГБ", bytes as f64 / (1u64 << 30) as f64)
    } else {
        tf!("{} MB", "{} МБ", (bytes >> 20).max(1))
    }
}

/// dd.mm.yyyy of seconds since the epoch (UTC).
fn date(seconds: u64) -> String {
    let days = (seconds / 86400) as i64 + 719468;
    let era = days.div_euclid(146097);
    let doe = days.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day:02}.{month:02}.{year}")
}

/// A workshop description without its BBCode.
fn plain(description: &str) -> String {
    let mut out = String::with_capacity(description.len());
    let mut depth = 0;
    for c in description.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            '\r' => {}
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let mut compact = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        compact.push_str(line.trim_end());
        compact.push('\n');
    }
    compact.trim().chars().take(4000).collect()
}

fn label_of(list: &[(&'static str, &'static str)], tag: &str) -> Option<&'static str> {
    list.iter().find(|(_, t)| t.eq_ignore_ascii_case(tag)).map(|(label, tag)| if lang::russian() { *label } else { *tag })
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
            hidden_groups: self.groups.iter().filter(|g| !g.shown).map(|g| g.title.to_owned()).collect(),
            favorites,
            language: self.language.clone(),
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
            if let Some(checked) = saved.filters.get(group.title) {
                for choice in &mut group.choices {
                    choice.checked = checked.iter().any(|t| t == choice.tag);
                }
            }
            group.shown = !saved.hidden_groups.iter().any(|t| t == group.title);
        }
        if let (Some(ui), Some(open)) = (self.ui(), saved.filters_open) {
            ui.set_filters_open(open);
        }
    }

    // ------------------------------------------------------------- filters

    fn show_groups(&self) {
        let Some(ui) = self.ui() else { return };
        let applies = |g: &Group| self.online() || g.local();
        let groups: Vec<FilterGroup> = self
            .groups
            .iter()
            .map(|g| FilterGroup {
                title: lang::label(g.title).into(),
                open: g.open,
                filtered: g.narrows(),
                shown: g.shown && applies(g),
                wanted: g.shown,
                boxed: g.boxed,
                options: ModelRc::new(VecModel::from(
                    g.choices
                        .iter()
                        .map(|c| FilterOption { label: if lang::russian() { c.label } else { c.tag }.into(), checked: c.checked })
                        .collect::<Vec<_>>(),
                )),
            })
            .collect();
        ui.set_groups(ModelRc::new(VecModel::from(groups)));
        ui.set_filtered(self.groups.iter().any(|g| applies(g) && g.narrows()));
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
                WORKSHOP_SORTS.iter().map(|(label, ..)| lang::label(label).into()).collect()
            } else {
                LOCAL_SORTS.iter().map(|label| lang::label(label).into()).collect()
            };
            ui.set_sort_options(ModelRc::new(VecModel::from(sorts)));
            ui.set_sort_index(if self.online() { self.workshop_sort } else { self.local_sort } as i32);
            ui.set_search(if self.online() { self.workshop_search.as_str() } else { self.local_search.as_str() }.into());
            ui.set_view_index(self.view as i32);
        }
        // Which groups there are depends on the tab.
        self.show_groups();
        self.reload();
    }

    fn reload(&mut self) {
        if self.online() { self.query_workshop() } else { self.show_installed() }
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
            subscriptions: 0,
            updated: item.modified,
            description: item.description.clone(),
            picture: item.preview.clone().map_or(Picture::None, Picture::File),
            dir: Some(item.dir.clone()),
        }
    }

    fn show_installed(&mut self) {
        self.generation += 1;
        self.installed = library::installed();
        let words: Vec<String> = self.local_search.to_lowercase().split_whitespace().map(str::to_owned).collect();
        let by = |title: &str| self.groups.iter().find(|g| g.title == title);
        let allows = |title: &str, value: &dyn Fn(&str) -> bool| {
            by(title).is_none_or(|g| !g.narrows() || g.checked().iter().any(|tag| value(tag)))
        };
        let mut cards: Vec<Card> = self
            .installed
            .iter()
            .filter(|item| {
                let title = item.title.to_lowercase();
                words.iter().all(|w| title.contains(w))
                    && (self.view == 0 || self.favorites.contains(&item.id))
                    && allows("Тип", &|tag| tag.eq_ignore_ascii_case(&item.kind))
                    && allows("Возрастной рейтинг", &|tag| tag.eq_ignore_ascii_case(&item.rating))
                    && allows("Метки", &|tag| has_tag(&item.tags, tag))
                    // Every box ticked there must hold.
                    && by("Показать только").is_none_or(|g| {
                        g.checked().iter().all(|tag| match *tag {
                            "Approved" => item.approved,
                            "Audio responsive" => item.audio_responsive,
                            "Customizable" => item.customizable,
                            _ => true,
                        })
                    })
            })
            .map(Self::installed_card)
            .collect();
        match self.local_sort {
            1 => cards.sort_by_key(|c| c.title.to_lowercase()),
            2 => cards.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))),
            _ => cards.sort_by(|a, b| b.updated.cmp(&a.updated)),
        }
        self.total = cards.len() as u32;
        self.set_cards(cards);
        if let Some(ui) = self.ui() {
            ui.set_loading(false);
            ui.set_error("".into());
            ui.set_pages(ModelRc::new(VecModel::from(Vec::<PageButton>::new())));
        }
        self.status(tf!("Wallpapers installed: {}", "Установлено обоев: {}", grouped(u64::from(self.total))));
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
        let (_, sort, days) = WORKSHOP_SORTS[self.workshop_sort];
        let mut query = steam::Query {
            sort,
            text: self.workshop_search.clone(),
            page: self.page.max(1),
            groups: Vec::new(),
            required: Vec::new(),
            excluded: Vec::new(),
            trend_days: days.max(1),
        };
        for group in &self.groups {
            if !group.narrows() {
                continue;
            }
            // Recommendations pick the genres themselves.
            if self.tab == RECOMMENDED && group.title == "Метки" {
                continue;
            }
            let tags: Vec<String> = group.checked().iter().map(|t| (*t).to_owned()).collect();
            match group.kind {
                GroupKind::AnyOf => query.groups.push(tags),
                GroupKind::AllOf => query.required.extend(tags),
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
        let recommended = self.tab == RECOMMENDED;
        let cards = page
            .entries
            .into_iter()
            // What is already here needs no recommending.
            .filter(|e| !recommended || self.subscribed.get(&e.id).copied().unwrap_or(0) & steam::SUBSCRIBED == 0)
            .map(|e| {
                let find = |list: &[(&'static str, &'static str)]| {
                    e.tags.iter().find(|t| label_of(list, t).is_some()).cloned().unwrap_or_default()
                };
                Card {
                    id: e.id,
                    title: e.title,
                    kind: find(&TYPES).to_lowercase(),
                    rating: find(&RATINGS),
                    size: e.size,
                    up: e.up,
                    down: e.down,
                    subscriptions: e.subscriptions,
                    updated: u64::from(e.updated),
                    description: e.description,
                    picture: if e.preview_url.is_empty() { Picture::None } else { Picture::Url(e.preview_url) },
                    dir: self.installed.iter().find(|i| i.id == e.id).map(|i| i.dir.clone()),
                    tags: e.tags,
                }
            })
            .collect();
        self.set_cards(cards);
        let pages = self.total.div_ceil(steam::PAGE_SIZE).max(1);
        ui.set_pages(ModelRc::new(VecModel::from(page_buttons(self.page.max(1), pages))));
        self.status(tf!("Wallpapers found: {}", "Найдено обоев: {}", grouped(u64::from(self.total))));
    }

    fn item_of(&self, card: &Card) -> Item {
        let state = self.subscribed.get(&card.id).copied().unwrap_or(0);
        let uhd = has_tag(&card.tags, "3840 x 2160");
        let hdr = has_tag(&card.tags, "HDR");
        Item {
            title: card.title.as_str().into(),
            image: Image::default(),
            loaded: false,
            kind: card.kind.as_str().into(),
            badge: match (uhd, hdr) {
                (true, true) => "4K HDR",
                (true, false) => "4K",
                (false, true) => "HDR",
                _ => "",
            }
            .into(),
            approved: has_tag(&card.tags, "Approved"),
            subscribed: self.online() && state & steam::SUBSCRIBED != 0,
            favorite: self.favorites.contains(&card.id),
            current: card.dir.is_some() && card.dir == self.current,
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
            self.previews.request(Job { id: card.id, source, size: 268, generation: self.picture_generation, large: false });
        }
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
                self.frames = std::iter::once((image, 100))
                    .chain(picture.frames.into_iter().map(|(rgba, delay)| {
                        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, picture.width, picture.height);
                        (Image::from_rgba8(buffer), delay)
                    }))
                    .collect();
                self.frame = 0;
                // The frames of the preview shown before go back to the system.
                unsafe { malloc_trim(0) };
                if self.frames.len() > 1 {
                    self.next_frame();
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
        if let Some(row) = self.cards.iter().position(|c| c.id == picture.id)
            && let Some(mut item) = self.model.row_data(row)
        {
            item.image = image;
            item.loaded = true;
            self.model.set_row_data(row, item);
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
                || item.current != fresh.current
                || item.progress != fresh.progress
                || item.favorite != fresh.favorite
            {
                item.subscribed = fresh.subscribed;
                item.current = fresh.current;
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
        ui.set_sel_stars(if card.up + card.down > 0 { card.up as f32 / (card.up + card.down) as f32 * 5.0 } else { 0.0 });

        let mut first = Vec::new();
        if let Some(kind) = label_of(&TYPES, &card.kind) {
            first.push(kind.to_owned());
        }
        if let Some(rating) = label_of(&RATINGS, &card.rating) {
            first.push(rating.to_owned());
        }
        if let Some(resolution) = card.tags.iter().find(|t| label_of(&RESOLUTIONS, t).is_some()) {
            first.push(label_of(&RESOLUTIONS, resolution).unwrap_or(resolution).to_owned());
        }
        let mut second = Vec::new();
        if card.size > 0 {
            second.push(megabytes(card.size));
        }
        if card.up + card.down > 0 {
            second.push(tf!("ratings: {}", "оценок: {}", grouped(u64::from(card.up + card.down))));
        }
        if card.subscriptions > 0 {
            second.push(tf!("subscribers: {}", "подписчиков: {}", grouped(card.subscriptions)));
        }
        let mut facts = vec![first.join("  ·  "), second.join("  ·  ")];
        if card.updated > 0 {
            facts.push(format!("{} {}", if self.online() { t("Updated", "Обновлено") } else { t("On disk since", "На диске с") }, date(card.updated)));
        }
        facts.retain(|line| !line.is_empty());
        ui.set_sel_facts(facts.join("\n").into());
        let genres: Vec<&str> = card
            .tags
            .iter()
            .filter_map(|t| label_of(&GENRES, t).or_else(|| label_of(&FEATURES, t)).or_else(|| label_of(&ONLY, t)))
            .collect();
        ui.set_sel_tags(genres.join(", ").into());
        ui.set_sel_description(plain(&card.description).into());

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
        self.previews.request(Job { id: card.id, source, size: 400, generation: 0, large: true });
    }

    fn select(&mut self, index: usize) {
        if index < self.cards.len() {
            self.selected = Some(index);
            self.show_selection(false);
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
                index: property.options.iter().position(|(_, v)| v == value || v.to_string().trim_matches('"') == value.to_string().trim_matches('"')).map_or(-1, |i| i as i32),
                swatch: Color::from_rgb_u8(r, g, b),
            });
        }
        ui.set_props(ModelRc::new(VecModel::from(rows)));
    }

    /// The user changed the property in row `row`: keep it, tell the running
    /// engines (when this wallpaper is the one playing) and redraw.
    fn set_property(&mut self, row: usize, value: Value) {
        let Some(card) = self.selected_card() else { return };
        let Some(property) = self.shown_properties.get(row).and_then(|&i| self.properties.get(i)).cloned() else { return };
        if value == property.default {
            self.overrides.remove(&property.name);
        } else {
            self.overrides.insert(property.name.clone(), value.clone());
        }
        props::save_overrides(card.id, &self.overrides);
        if self.dir_of(&card).is_some_and(|dir| Some(dir) == self.current) {
            props::tell_engines(&property.name, &value);
        }
        self.show_properties();
    }

    fn reset_properties(&mut self) {
        let Some(card) = self.selected_card() else { return };
        let playing = self.dir_of(&card).is_some_and(|dir| Some(dir) == self.current);
        for property in &self.properties {
            if playing && self.overrides.contains_key(&property.name) {
                props::tell_engines(&property.name, &property.default);
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
            engine::Stage::Preparing => self.status(tf!(
                "\"{title}\": preparing its video for this monitor, only the first time… {waited} s",
                "«{title}»: подготовка видео под монитор, только в первый раз… {waited} с"
            )),
            engine::Stage::Loading => self.status(tf!("\"{title}\": loading…", "«{title}»: загрузка…")),
        }
    }

    /// A double click: set what is on the disk, subscribe to what is not.
    fn activate(&mut self, index: usize) {
        self.select(index);
        let Some(card) = self.selected_card() else { return };
        if self.dir_of(&card).is_some() {
            self.apply();
        } else if self.subscribed.get(&card.id).copied().unwrap_or(0) & steam::SUBSCRIBED == 0 {
            self.subscribe();
        }
    }

    fn toggle_favorite(&mut self) {
        let Some(card) = self.selected_card() else { return };
        if !self.favorites.remove(&card.id) {
            self.favorites.insert(card.id);
        }
        self.save();
        self.refresh_marks();
    }

    fn open_file(&mut self) {
        match desktop::pick_file() {
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
                None => desktop::off(),
            }
        };
        self.current = desktop::current();
    }

    // ------------------------------------------------------------- monitors, settings

    fn show_monitors(&self) {
        let Some(ui) = self.ui() else { return };
        // One render has one wallpaper: a monitor's own is not shown.
        let overrides = if Settings::load().mirror { Vec::new() } else { desktop::outputs() };
        let title = |path: &PathBuf| {
            self.installed
                .iter()
                .find(|i| &i.dir == path)
                .map_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), |i| i.title.clone())
        };
        let monitors = desktop::monitors();
        let rows: Vec<MonitorEntry> = monitors
            .iter()
            .map(|m| MonitorEntry {
                name: m.name.as_str().into(),
                detail: m.detail.as_str().into(),
                wallpaper: overrides
                    .iter()
                    .find(|(name, _)| name == &m.name)
                    .map(|(_, path)| tf!("Its own: {}", "Свои обои: {}", title(path)))
                    .or_else(|| self.current.as_ref().map(|path| tf!("Same as all: {}", "Как у всех: {}", title(path))))
                    .unwrap_or_else(|| t("A plain picture", "Обычная картинка").to_owned())
                    .into(),
            })
            .collect();
        let index = self.monitor.as_ref().and_then(|name| monitors.iter().position(|m| &m.name == name));
        ui.set_monitor_index(index.map_or(-1, |i| i as i32));
        ui.set_monitor_title(index.map_or_else(|| t("All monitors", "Все мониторы").to_owned(), |i| tf!("Monitor {}", "Монитор {}", i + 1)).into());
        ui.set_monitors(ModelRc::new(VecModel::from(rows)));
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
            ui.set_set_lang(i32::from(russian));
            ui.set_view_options(ModelRc::new(VecModel::from(
                VIEWS.iter().map(|v| SharedString::from(lang::label(v))).collect::<Vec<_>>(),
            )));
            ui.set_view_index(self.view as i32);
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
        ui.set_set_lang(i32::from(lang::russian()));
        ui.set_set_fps(settings.fps);
        ui.set_set_video(i32::from(settings.video_gpu));
        ui.set_set_mirror(settings.mirror);
        ui.set_set_audio(settings.audio);
        ui.set_set_volume(settings.volume);
        ui.set_set_freeze(settings.pause_fullscreen);
        ui.set_set_stop(settings.stop_fullscreen);
        ui.set_set_still(settings.still);
    }

    fn save_settings(&mut self) {
        let Some(ui) = self.ui() else { return };
        if (ui.get_set_lang() == 1) != lang::russian() {
            self.set_language(ui.get_set_lang() == 1);
        }
        let settings = Settings {
            fps: ui.get_set_fps(),
            video_gpu: ui.get_set_video() == 1,
            mirror: ui.get_set_mirror(),
            audio: ui.get_set_audio(),
            volume: ui.get_set_volume(),
            pause_fullscreen: ui.get_set_freeze(),
            stop_fullscreen: ui.get_set_stop(),
            still: ui.get_set_still(),
            ..Settings::load()
        };
        if settings == Settings::load() {
            return;
        }
        let result = settings.save().and_then(|()| engine::ensure_daemon());
        self.report(result, t("Settings applied", "Настройки применены"));
        self.show_monitors();
    }

    // ------------------------------------------------------------- playlist

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
            })
            .collect();
        self.playlist_model.set_vec(entries);
        for (row, path) in self.playlist.items.iter().enumerate() {
            if let (Some(id), Some(preview)) = (
                self.playlist_id(row),
                self.installed.iter().find(|i| &i.dir == path).and_then(|i| i.preview.clone()),
            ) {
                self.previews.request(Job { id, source: Source::File(preview), size: 96, generation: 0, large: false });
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
            Reply::Subscribed { id, ok } => {
                self.busy.remove(&id);
                if ok {
                    *self.subscribed.entry(id).or_insert(0) |= steam::SUBSCRIBED;
                    self.progress.insert(id, 0.0);
                    self.status(t("Subscribed, Steam is downloading the wallpaper", "Подписка оформлена, Steam загружает обои"));
                } else {
                    self.status(t("Steam did not take the subscription", "Steam не принял подписку"));
                }
                self.refresh_marks();
            }
            Reply::Unsubscribed { id, ok } => {
                self.busy.remove(&id);
                self.progress.remove(&id);
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
fn page_buttons(current: u32, pages: u32) -> Vec<PageButton> {
    let button = |label: String, page: u32| PageButton { label: label.into(), page: page as i32, current: page == current };
    let mut out = vec![PageButton { label: "«".into(), page: if current > 1 { 1 } else { -1 }, current: false }];
    let from = current.saturating_sub(2).max(1).min(pages.saturating_sub(4).max(1));
    let to = (from + 4).min(pages);
    for page in from..=to {
        out.push(button(page.to_string(), page));
    }
    if to < pages {
        out.push(PageButton { label: "…".into(), page: 0, current: false });
        out.push(button(pages.to_string(), pages));
    }
    out.push(PageButton { label: "»".into(), page: if current < pages { current as i32 + 1 } else { -1 }, current: false });
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

/// The command line; None opens the window.
fn command_line() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
            say!("engines: {}", engine::sockets().len());
            if let Some(wallpaper) = desktop.wallpaper.as_ref().or(desktop.outputs.values().next()) {
                say!(
                    "state: {}",
                    match engine::stage(wallpaper) {
                        engine::Stage::Shown => "shown",
                        engine::Stage::Preparing => "preparing a video (first time only)",
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
    ui.set_set_lang(i32::from(russian));
    if language.is_none() {
        ui.set_dialog(9);
    }

    let model = Rc::new(VecModel::<Item>::default());
    let playlist_model = Rc::new(VecModel::<PlaylistEntry>::default());
    ui.set_items(ModelRc::from(model.clone()));
    ui.set_playlist(ModelRc::from(playlist_model.clone()));
    ui.set_view_options(ModelRc::new(VecModel::from(VIEWS.iter().map(|v| SharedString::from(lang::label(v))).collect::<Vec<_>>())));

    let app = Rc::new(RefCell::new(App {
        ui: ui.as_weak(),
        language: language.clone(),
        applying: None,
        steam: steam::spawn(|reply| on_ui(move |app| app.steam_reply(reply))),
        previews: Previews::new(|picture| on_ui(move |app| app.picture_arrived(picture))),
        tab: WORKSHOP,
        view: 0,
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
    }));
    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));

    {
        let mut app = app.borrow_mut();
        app.restore();
        app.show_groups();
        app.show_monitors();
        app.show_settings();
        app.show_playlist();
        app.show_tab();
        if library::assets().is_none() {
            app.status(t("Wallpaper Engine is not installed in Steam: scenes will not start without its assets", "Wallpaper Engine не установлен в Steam: без его ассетов сцены не запустятся"));
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
        app.page = 1;
        app.selected = None;
        app.save();
        app.show_tab();
    });
    on!(on_view_changed, |app, view| {
        app.view = view.max(0) as usize;
        app.selected = None;
        app.reload();
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
    on!(on_filters_reset, |app| {
        for group in &mut app.groups {
            group.choices.iter_mut().for_each(|c| c.checked = c.default);
        }
        app.filters_changed();
    });
    on!(on_page_clicked, |app, page| {
        app.page = page.max(1) as u32;
        app.selected = None;
        app.reload();
    });
    on!(on_item_selected, |app, index| app.select(index.max(0) as usize));
    on!(on_item_activated, |app, index| app.activate(index.max(0) as usize));
    on!(on_subscribe, |app| app.subscribe());
    on!(on_unsubscribe, |app| app.unsubscribe());
    on!(on_apply, |app| app.apply());
    on!(on_favorite, |app| app.toggle_favorite());
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
    on!(on_settings_save, |app| app.save_settings());
    on!(on_settings_shown, |app| app.show_settings());
    on!(on_language_picked, |app, index| {
        app.set_language(index == 1);
        if let Some(ui) = app.ui() {
            ui.set_dialog(0);
        }
    });
    on!(on_open_file, |app| app.open_file());
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
