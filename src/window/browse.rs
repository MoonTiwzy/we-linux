//! Which list the grid shows and how it gets it: the tab, the sorting, the
//! search, the page, an author's wallpapers only. The installed tab is
//! worked out here from the disk (listing.rs); the workshop and
//! "Рекомендации" are questions to Steam, whose answers come back to
//! `page_arrived`.

use slint::{ModelRc, SharedString, Timer, TimerMode, VecModel};
use we_linux_ui::{ExploreRow, PageButton};

use super::card::Card;
use super::listing::{self, Wanted};
use super::saved::Saved;
use super::search::search_words;
use super::tags::tag_label;
use super::{App, slot};
use crate::lang::t;
use crate::steam::{self, Command, Sort};
use crate::tf;
use crate::units::grouped;

pub(super) const INSTALLED: i32 = 0;
pub(super) const RECOMMENDED: i32 = 1;
pub(super) const WORKSHOP: i32 = 2;
/// The wallpapers of a row of "Рекомендации".
const EXPLORE_ROW: usize = 20;

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

pub(super) struct Browse {
    tab: i32,
    workshop_sort: usize,
    local_sort: usize,
    /// The arrow beside the sorting of installed wallpapers points down.
    sort_descending: bool,
    workshop_search: String,
    local_search: String,
    page: u32,
    total: u32,
    /// Installed wallpapers a page: 50, 100 or 200 (the grid's menu, "Вид").
    per_page: usize,
    /// The workshop shows one author's wallpapers only.
    author: Option<u64>,
    /// Of the workshop query in flight; older answers are dropped.
    generation: u64,
    /// "Рекомендации": its rows (title and cards) and the generation of the
    /// query of the first (the others follow it).
    explore: Vec<(String, Vec<Card>)>,
    explore_base: u64,
    /// The search follows the typing, a moment after it.
    search_timer: Timer,
}

impl Default for Browse {
    fn default() -> Self {
        Self {
            tab: WORKSHOP,
            workshop_sort: 0,
            local_sort: 0,
            sort_descending: false,
            workshop_search: String::new(),
            local_search: String::new(),
            page: 1,
            total: 0,
            per_page: 100,
            author: None,
            generation: 0,
            explore: Vec::new(),
            explore_base: 0,
            search_timer: Timer::default(),
        }
    }
}

/// The ids of the row of "Рекомендации" the tile `at` (counted through all
/// the rows) is in.
fn row_of(rows: &[(String, Vec<Card>)], at: usize) -> Vec<u64> {
    let mut first = 0;
    for (_, cards) in rows {
        if (first..first + cards.len()).contains(&at) {
            return cards.iter().map(|c| c.id).collect();
        }
        first += cards.len();
    }
    Vec::new()
}

impl Browse {
    /// The workshop or "Рекомендации", not the installed tab.
    pub(super) fn online(&self) -> bool {
        self.tab != INSTALLED
    }

    pub(super) fn tab(&self) -> i32 {
        self.tab
    }

    pub(super) fn per_page(&self) -> usize {
        self.per_page
    }

    pub(super) fn set_per_page(&mut self, n: usize) {
        self.per_page = n;
    }

    /// The order of the installed tab (an index into its orders).
    pub(super) fn local_sort(&self) -> usize {
        self.local_sort
    }

    pub(super) fn author(&self) -> Option<u64> {
        self.author
    }

    pub(super) fn sort_descending(&self) -> bool {
        self.sort_descending
    }

    /// The ids of the row of "Рекомендации" the tile `at` is in.
    pub(super) fn explore_row(&self, at: usize) -> Vec<u64> {
        row_of(&self.explore, at)
    }

    pub(super) fn save_into(&self, saved: &mut Saved) {
        saved.tab = Some(self.tab);
        saved.workshop_sort = Some(self.workshop_sort);
        saved.local_sort = Some(self.local_sort);
        saved.sort_descending = Some(self.sort_descending);
        saved.per_page = Some(self.per_page);
    }

    pub(super) fn restore(&mut self, saved: &Saved) {
        self.tab = saved.tab.unwrap_or(WORKSHOP).clamp(INSTALLED, WORKSHOP);
        self.workshop_sort = saved.workshop_sort.unwrap_or(0).min(WORKSHOP_SORTS.len() - 1);
        self.local_sort = saved.local_sort.unwrap_or(0).min(LOCAL_SORTS.len() - 1);
        self.sort_descending = saved.sort_descending.unwrap_or(false);
        self.per_page = saved.per_page.filter(|n| [50, 100, 200].contains(n)).unwrap_or(100);
    }
}

impl App {
    pub(super) fn show_tab(&mut self) {
        // Another list: what was picked in this one is not.
        self.grid.forget_list();
        if let Some(ui) = self.ui() {
            let browse = &self.browse;
            ui.set_tab(browse.tab);
            let sorts: Vec<SharedString> = if browse.online() {
                WORKSHOP_SORTS.iter().map(|(english, russian, ..)| t(english, russian).into()).collect()
            } else {
                LOCAL_SORTS.iter().map(|(english, russian)| t(english, russian).into()).collect()
            };
            ui.set_sort_options(ModelRc::new(VecModel::from(sorts)));
            ui.set_sort_index(if browse.online() { browse.workshop_sort } else { browse.local_sort } as i32);
            ui.set_search(
                if browse.online() { browse.workshop_search.as_str() } else { browse.local_search.as_str() }
                    .into(),
            );
            ui.set_workshop_sorting(browse.online());
            ui.set_icon_size(self.grid.icon_size());
            let author = browse.author.filter(|_| browse.tab == WORKSHOP);
            ui.set_notice(
                author
                    .map_or_else(String::new, |owner| {
                        let name = self.catalog.persona_name(owner);
                        tf!("Browsing wallpapers by {name}", "Смотрим все обои от {name}")
                    })
                    .into(),
            );
        }
        // Which groups there are depends on the tab.
        self.show_groups();
        self.reload();
    }

    pub(super) fn reload(&mut self) {
        if self.browse.tab == RECOMMENDED {
            self.query_explore()
        } else if self.browse.online() {
            self.query_workshop()
        } else {
            self.show_installed()
        }
    }

    /// The list again, from its first page (the search, the sorting or the filters changed).
    pub(super) fn first_page_again(&mut self) {
        self.browse.page = 1;
        self.reload();
    }

    /// A tab clicked.
    pub(super) fn tab_picked(&mut self, tab: i32) {
        self.browse.tab = tab;
        self.browse.author = None;
        self.browse.page = 1;
        self.grid.select(None);
        self.save();
        self.show_tab();
    }

    /// Another tab, kept to the three there are, without saving it (--show).
    pub(super) fn switch_tab(&mut self, tab: i32) {
        self.browse.tab = tab.clamp(INSTALLED, WORKSHOP);
        self.show_tab();
    }

    /// The cross of "Смотрим все обои от …": the whole workshop again.
    pub(super) fn notice_closed(&mut self) {
        self.browse.author = None;
        self.browse.page = 1;
        self.show_tab();
    }

    /// The workshop, an author's wallpapers only (the original's "author filter").
    pub(super) fn browse_author(&mut self, owner: u64) {
        if owner == 0 {
            return;
        }
        if self.catalog.persona(owner).is_none() {
            let _ = self.steam.send(Command::Persona(owner));
        }
        self.browse.author = Some(owner);
        self.browse.tab = WORKSHOP;
        self.browse.page = 1;
        self.grid.select(None);
        self.show_tab();
    }

    /// A letter typed in the search.
    pub(super) fn search_typed(&mut self, text: String) {
        // The original's field waits a tenth of a second for the next letter.
        self.browse.search_timer.start(
            TimerMode::SingleShot,
            std::time::Duration::from_millis(100),
            move || {
                let text = text.clone();
                slot::with(|app| app.search(text));
            },
        );
    }

    /// The search of the tab shown, from its first page.
    pub(super) fn search(&mut self, text: String) {
        if self.browse.online() {
            self.browse.workshop_search = text;
        } else {
            self.browse.local_search = text;
        }
        self.first_page_again();
    }

    pub(super) fn sort_picked(&mut self, index: i32) {
        let index = index.max(0) as usize;
        if self.browse.online() {
            self.browse.workshop_sort = index.min(WORKSHOP_SORTS.len() - 1);
        } else {
            self.browse.local_sort = index.min(LOCAL_SORTS.len() - 1);
        }
        self.browse.page = 1;
        self.save();
        self.reload();
    }

    /// The arrow beside the installed tab's sorting.
    pub(super) fn sort_direction_changed(&mut self) {
        if let Some(ui) = self.ui() {
            self.browse.sort_descending = ui.get_sort_descending();
        }
        self.save();
        self.reload();
    }

    /// A page number under the grid.
    pub(super) fn page_picked(&mut self, page: i32) {
        let per_page = if self.browse.online() { steam::PAGE_SIZE } else { self.browse.per_page as u32 };
        self.browse.page = listing::page_within(page, self.browse.total, per_page);
        self.grid.select(None);
        self.reload();
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
        for genre in self.catalog.taste() {
            let label = tag_label(&genre).unwrap_or(&genre).to_owned();
            rows.push((tf!("Most Popular: {label}", "Популярные: {label}"), Sort::Trend, 30, vec![genre]));
        }
        self.browse.explore_base = self.browse.generation + 1;
        self.browse.explore = rows.iter().map(|(title, ..)| (title.clone(), Vec::new())).collect();
        self.set_cards(Vec::new());
        self.show_explore();
        if let Some(ui) = self.ui() {
            ui.set_loading(true);
            ui.set_error("".into());
            ui.set_pages(ModelRc::new(VecModel::from(Vec::<PageButton>::new())));
        }
        // What the filters leave out stays out (the ratings above all).
        let rating = self.filters.ratings();
        for (sort, days, tags) in rows.into_iter().map(|(_, sort, days, tags)| (sort, days, tags)) {
            self.browse.generation += 1;
            let mut groups = vec![rating.clone()];
            if !tags.is_empty() {
                groups.push(tags);
            }
            let query = steam::Query {
                sort,
                text: String::new(),
                page: 1,
                groups,
                required: Vec::new(),
                excluded: Vec::new(),
                trend_days: days.max(1),
                author: 0,
            };
            let _ = self.steam.send(Command::Query { generation: self.browse.generation, query });
        }
    }

    /// The rows of "Рекомендации" on the screen: their cards one after another
    /// in the grid's list, each row knowing where its own begin.
    fn show_explore(&mut self) {
        let cards: Vec<Card> =
            self.browse.explore.iter().flat_map(|(_, cards)| cards.iter().cloned()).collect();
        let mut first = 0;
        let rows: Vec<ExploreRow> = self
            .browse
            .explore
            .iter()
            .map(|(title, cards)| {
                let row = ExploreRow {
                    title: title.as_str().into(),
                    first: first as i32,
                    count: cards.len() as i32,
                };
                first += cards.len();
                row
            })
            .collect();
        self.set_cards(cards);
        if let Some(ui) = self.ui() {
            ui.set_explore_rows(ModelRc::new(VecModel::from(rows)));
        }
    }

    /// The installed tab: what is on the disk, filtered, searched, sorted and
    /// cut into pages.
    pub(super) fn show_installed(&mut self) {
        self.browse.generation += 1;
        self.catalog.reread();
        let all = self.catalog.installed().len();
        let (unticked, only) = self.filters.installed_terms();
        let (words, special) = search_words(&self.browse.local_search);
        let wanted = Wanted { unticked, only, words, special, favorites: self.catalog.favorites() };
        let mut cards: Vec<Card> = self
            .catalog
            .installed()
            .iter()
            .filter(|item| listing::kept(item, self.catalog.workshop_tags(item.id), &wanted))
            .map(Card::installed)
            .collect();
        let shown = cards.len();
        if self.browse.local_sort == 3 {
            self.catalog.count_sizes(&cards);
        }
        listing::sort_installed(&mut cards, self.browse.local_sort, self.browse.sort_descending, |c| {
            self.catalog.facts(c)
        });
        self.browse.total = shown as u32;
        // A hundred a page, the pages under the grid.
        let per_page = self.browse.per_page.max(1);
        let (page, pages, range) = listing::paged(shown, self.browse.page, per_page);
        self.browse.page = page;
        let on_page = cards[range].to_vec();
        self.grid.set_listed(cards);
        self.set_cards(on_page);
        if let Some(ui) = self.ui() {
            ui.set_loading(false);
            ui.set_error("".into());
            let buttons = if shown > per_page { listing::page_buttons(page, pages) } else { Vec::new() };
            ui.set_pages(ModelRc::new(VecModel::from(buttons)));
            ui.set_filter_count(
                if shown != all { tf!("({} of {})", "({} из {})", shown, all) } else { String::new() }.into(),
            );
        }
        // The details only the workshop has (votes, size, author), once.
        let unknown = self.catalog.ask_details();
        if !unknown.is_empty() {
            let _ = self.steam.send(Command::Details(unknown));
        }
    }

    fn query_workshop(&mut self) {
        self.browse.generation += 1;
        let (_, _, sort, days) = WORKSHOP_SORTS[self.browse.workshop_sort];
        let recommended = self.browse.tab == RECOMMENDED;
        let (groups, required) = self.filters.workshop_terms(recommended);
        let mut query = steam::Query {
            sort,
            text: self.browse.workshop_search.clone(),
            page: self.browse.page.max(1),
            groups,
            required,
            excluded: Vec::new(),
            trend_days: days.max(1),
            author: self.browse.author.map_or(0, |owner| owner as u32),
        };
        if recommended {
            let taste = self.catalog.taste();
            if !taste.is_empty() {
                query.groups.push(taste);
            }
        }
        if let Some(ui) = self.ui() {
            ui.set_loading(true);
            ui.set_error("".into());
        }
        let _ = self.steam.send(Command::Query { generation: self.browse.generation, query });
    }

    /// A page of the workshop, or a row of "Рекомендации", from Steam.
    pub(super) fn page_arrived(&mut self, generation: u64, result: Result<steam::Page, String>) {
        // A row of "Рекомендации".
        if self.browse.tab == RECOMMENDED {
            let Some(row) = generation
                .checked_sub(self.browse.explore_base)
                .map(|r| r as usize)
                .filter(|r| *r < self.browse.explore.len())
            else {
                return;
            };
            if let Ok(page) = result {
                let cards: Vec<Card> =
                    page.entries.into_iter().take(EXPLORE_ROW).map(|e| self.card_of(e)).collect();
                self.browse.explore[row].1 = cards;
                self.show_explore();
            }
            if let Some(ui) = self.ui() {
                ui.set_loading(false);
            }
            return;
        }
        if generation != self.browse.generation || !self.browse.online() {
            return;
        }
        let Some(ui) = self.ui() else { return };
        ui.set_loading(false);
        let page = match result {
            Ok(page) => page,
            Err(e) => {
                let e = if self.catalog.steam_ok() { e } else { super::replies::offline(&e) };
                ui.set_error(e.into());
                self.set_cards(Vec::new());
                return;
            }
        };
        ui.set_error("".into());
        self.browse.total = page.total;
        let cards: Vec<Card> = page.entries.into_iter().map(|e| self.card_of(e)).collect();
        self.set_cards(cards);
        let pages = listing::workshop_pages(self.browse.total);
        ui.set_pages(ModelRc::new(VecModel::from(listing::page_buttons(self.browse.page.max(1), pages))));
        self.status(tf!("Wallpapers found: {}", "Найдено обоев: {}", grouped(u64::from(self.browse.total))));
    }

    /// A wallpaper of the workshop as a cell of the grid.
    fn card_of(&self, e: steam::Entry) -> Card {
        let dir = self.catalog.dir_of(e.id);
        Card::of_entry(e, dir)
    }
}

#[cfg(test)]
mod tests {
    use super::super::card::Picture;
    use super::*;

    fn cards(ids: &[u64]) -> Vec<Card> {
        ids.iter()
            .map(|&id| Card {
                id,
                title: String::new(),
                kind: String::new(),
                rating: String::new(),
                tags: Vec::new(),
                size: 0,
                up: 0,
                down: 0,
                updated: 0,
                picture: Picture::None,
                dir: None,
                owner: 0,
            })
            .collect()
    }

    #[test]
    fn ctrl_a_on_recommendations_takes_the_row_of_the_selection() {
        let rows = vec![
            ("a".to_owned(), cards(&[1, 2])),
            ("b".to_owned(), cards(&[])),
            ("c".to_owned(), cards(&[3, 4, 5])),
        ];
        assert_eq!(row_of(&rows, 0), [1, 2]);
        assert_eq!(row_of(&rows, 1), [1, 2]);
        assert_eq!(row_of(&rows, 2), [3, 4, 5]);
        assert_eq!(row_of(&rows, 4), [3, 4, 5]);
        assert!(row_of(&rows, 5).is_empty());
    }
}
