//! What the installed tab shows of the wallpapers on the disk: which the
//! filters and the search leave, in which order, cut into which pages; and
//! the page numbers under the grid, of either tab. No window here: it is
//! all worked out from the lists.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use we_linux_ui::PageButton;

use super::card::Card;
use super::search::title_matches;
use super::tags::{GENRES, has_tag};
use crate::library::Installed;
use crate::steam;

/// What the filters and the search ask of an installed wallpaper.
pub(super) struct Wanted<'a> {
    /// Tags left unticked: an item carrying one goes.
    pub unticked: Vec<&'static str>,
    /// The ticked boxes of "Показать только": an item stays only with all of them.
    pub only: Vec<&'static str>,
    /// The search's words and the tags it named (see `search_words`).
    pub words: String,
    pub special: Vec<&'static str>,
    pub favorites: &'a HashSet<u64>,
}

/// Whether an installed wallpaper stays in the list, as the original decides:
/// it goes when it carries a tag left unticked, and stays only with every tag
/// of "Показать только" ticked. `workshop_tags` are what the workshop says of it.
pub(super) fn kept(item: &Installed, workshop_tags: &[String], wanted: &Wanted) -> bool {
    let carries = |tag: &str| {
        has_tag(&item.tags, tag)
            || has_tag(workshop_tags, tag)
            || item.kind.eq_ignore_ascii_case(tag)
            || item.rating.eq_ignore_ascii_case(tag)
            // Everything installed here came from the workshop as a wallpaper.
            || tag == "Workshop"
            || tag == "Wallpaper"
    };
    let held = wanted.only.iter().all(|tag| match *tag {
        "Approved" => item.approved,
        "Favorite" => wanted.favorites.contains(&item.id),
        "Audio responsive" => item.audio_responsive,
        "Customizable" => item.customizable,
        _ => true,
    });
    held && !wanted.unticked.iter().any(|tag| carries(tag))
        && wanted.special.iter().all(|tag| has_tag(&item.tags, tag))
        && title_matches(&item.title, &wanted.words)
}

/// What the orders of the installed tab go by, besides the card itself.
pub(super) struct Facts {
    /// The workshop's votes, as stars.
    pub rating: f32,
    pub favorite: bool,
    /// What its folder takes on the disk (counted only for that order).
    pub size: u64,
    /// When the workshop last saw it updated (else when it came).
    pub updated: u64,
}

/// The installed wallpapers in the original's orders (`LOCAL_SORTS`), "up"
/// first and the name breaking ties; `descending` turns the list round.
pub(super) fn sort_installed(
    cards: &mut [Card],
    order: usize,
    descending: bool,
    facts: impl Fn(&Card) -> Facts,
) {
    let name = |a: &Card, b: &Card| collation_key(&a.title).cmp(&collation_key(&b.title));
    cards.sort_by(|a, b| {
        let by = |key: fn(&Facts, &Facts) -> Ordering| key(&facts(a), &facts(b)).then_with(|| name(a, b));
        match order {
            1 => by(|a, b| b.rating.total_cmp(&a.rating)),
            2 => by(|a, b| b.favorite.cmp(&a.favorite)),
            3 => by(|a, b| b.size.cmp(&a.size)),
            4 => b.updated.cmp(&a.updated).then_with(|| name(a, b)),
            5 => by(|a, b| b.updated.cmp(&a.updated)),
            _ => name(a, b),
        }
    });
    if descending {
        cards.reverse();
    }
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
pub(super) fn votes(up: u32, down: u32) -> f32 {
    if up + down == 0 { 0.0 } else { up as f32 / (up + down) as f32 * 5.0 }
}

/// The genres the installed wallpapers carry most: what "Рекомендации" looks for.
pub(super) fn taste(installed: &[Installed]) -> Vec<String> {
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for item in installed {
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

/// A list of `len` cut into pages of `per_page`: the page asked for, kept
/// within them, how many pages there are, and what of the list is on it.
pub(super) fn paged(len: usize, page: u32, per_page: usize) -> (u32, u32, Range<usize>) {
    let per_page = per_page.max(1);
    let pages = len.div_ceil(per_page).max(1) as u32;
    let page = page.clamp(1, pages);
    let from = (page as usize - 1) * per_page;
    (page, pages, from.min(len)..(from + per_page).min(len))
}

/// The workshop's pages for `total` results. Steam pages no further than
/// 50 000 items: a thousand pages.
pub(super) fn workshop_pages(total: u32) -> u32 {
    total.div_ceil(steam::PAGE_SIZE).clamp(1, 1000)
}

/// The page a click on a number (or one typed after "…") goes to: a number
/// past the last page is the last page, as in the original.
pub(super) fn page_within(asked: i32, total: u32, per_page: u32) -> u32 {
    (asked.max(1) as u32).min(total.div_ceil(per_page).max(1))
}

/// The page numbers under the grid, as the original lays them out: the first
/// and the last always, four either side of the current one, "…" for the
/// rest (it asks for a number), the arrows at the ends. Page 0 is "…".
pub(super) fn page_buttons(current: u32, pages: u32) -> Vec<PageButton> {
    let button = |label: String, page: u32| PageButton {
        label: label.into(),
        page: page as i32,
        current: page == current,
    };
    let mut out =
        vec![PageButton {
            label: "«".into(), page: current.saturating_sub(1).max(1) as i32, current: false
        }];
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::card::Picture;
    use super::*;

    fn labels(buttons: &[PageButton]) -> Vec<String> {
        buttons.iter().map(|b| b.label.to_string()).collect()
    }

    #[test]
    fn pages_as_the_original_lays_them_out() {
        assert_eq!(labels(&page_buttons(1, 2)), ["«", "1", "2", "»"]);
        assert_eq!(labels(&page_buttons(1, 1000)), ["«", "1", "2", "3", "4", "5", "…", "1000", "»"]);
        assert_eq!(
            labels(&page_buttons(10, 1000)),
            ["«", "1", "…", "6", "7", "8", "9", "10", "11", "12", "13", "14", "…", "1000", "»"]
        );
        assert_eq!(
            labels(&page_buttons(1000, 1000)),
            ["«", "1", "…", "996", "997", "998", "999", "1000", "»"]
        );
        let around = page_buttons(10, 1000);
        assert_eq!((around[0].page, around.last().unwrap().page), (9, 11));
        assert!(around.iter().any(|b| b.current && b.label == "10"));
        // The arrows stop at the ends.
        assert_eq!(page_buttons(1, 3)[0].page, 1);
        assert_eq!(page_buttons(3, 3).last().unwrap().page, 3);
    }

    #[test]
    fn names_sort_as_the_browser_does() {
        let mut titles = vec![
            "Anime girl",
            "150594984_p0",
            "[MLP] FS",
            "\u{3010}Blue Archive\u{3011}",
            "arch linux",
            "\u{8494}\u{84dd}",
        ];
        titles.sort_by_key(|t| collation_key(t));
        assert_eq!(
            titles,
            [
                "[MLP] FS",
                "\u{3010}Blue Archive\u{3011}",
                "150594984_p0",
                "Anime girl",
                "arch linux",
                "\u{8494}\u{84dd}"
            ]
        );
    }

    fn installed(id: u64, title: &str, tags: &[&str]) -> Installed {
        Installed {
            id,
            dir: PathBuf::from(format!("/w/{id}")),
            title: title.to_owned(),
            kind: "scene".to_owned(),
            preview: None,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            approved: false,
            audio_responsive: false,
            customizable: false,
            rating: "Everyone".to_owned(),
            modified: id,
            dependency: None,
        }
    }

    fn card(id: u64, title: &str) -> Card {
        Card {
            id,
            title: title.to_owned(),
            kind: String::new(),
            rating: String::new(),
            tags: Vec::new(),
            size: 0,
            up: 0,
            down: 0,
            updated: id,
            picture: Picture::None,
            dir: None,
            owner: 0,
        }
    }

    #[test]
    fn an_unticked_tag_hides_and_the_box_asks_for_all_of_its_ticks() {
        let favorites: HashSet<u64> = [2].into();
        let wanted = |unticked: Vec<&'static str>, only: Vec<&'static str>, words: &str| Wanted {
            unticked,
            only,
            words: words.to_owned(),
            special: Vec::new(),
            favorites: &favorites,
        };
        let anime = installed(1, "Rain", &["Anime"]);
        let nature = installed(2, "Forest", &["Nature"]);
        assert!(kept(&anime, &[], &wanted(vec![], vec![], "")));
        assert!(!kept(&anime, &[], &wanted(vec!["Anime"], vec![], "")));
        // What the workshop says of it counts as much as its own tags.
        assert!(!kept(&nature, &["Anime".to_owned()], &wanted(vec!["Anime"], vec![], "")));
        // Its kind and rating are tags too, and every installed one is a workshop wallpaper.
        assert!(!kept(&nature, &[], &wanted(vec!["Scene"], vec![], "")));
        assert!(!kept(&nature, &[], &wanted(vec!["Everyone"], vec![], "")));
        assert!(!kept(&nature, &[], &wanted(vec!["Workshop"], vec![], "")));
        assert!(kept(&nature, &[], &wanted(vec![], vec!["Favorite"], "")));
        assert!(!kept(&anime, &[], &wanted(vec![], vec!["Favorite"], "")));
        assert!(!kept(&nature, &[], &wanted(vec![], vec!["Favorite", "Approved"], "")));
        assert!(kept(&anime, &[], &wanted(vec![], vec![], "rian")));
        assert!(!kept(&anime, &[], &wanted(vec![], vec![], "forest")));
        let hdr = Wanted { special: vec!["HDR"], ..wanted(vec![], vec![], "") };
        assert!(!kept(&anime, &[], &hdr));
        assert!(kept(&installed(3, "Lights", &["hdr"]), &[], &hdr));
    }

    #[test]
    fn the_orders_go_by_their_fact_and_then_by_name() {
        let facts = |c: &Card| Facts {
            rating: [0.0, 4.0, 4.0, 1.0][c.id as usize],
            favorite: c.id == 3,
            size: c.id * 10,
            updated: 10 - c.id,
        };
        let order = |order: usize, descending: bool| {
            let mut cards = vec![card(1, "b"), card(2, "a"), card(3, "c")];
            sort_installed(&mut cards, order, descending, facts);
            cards.iter().map(|c| c.id).collect::<Vec<_>>()
        };
        assert_eq!(order(0, false), [2, 1, 3]);
        assert_eq!(order(0, true), [3, 1, 2]);
        // Equal ratings fall back on the name.
        assert_eq!(order(1, false), [2, 1, 3]);
        assert_eq!(order(2, false), [3, 2, 1]);
        assert_eq!(order(3, false), [3, 2, 1]);
        // The subscription date is the card's own, the last update the workshop's.
        assert_eq!(order(4, false), [3, 2, 1]);
        assert_eq!(order(5, false), [1, 2, 3]);
    }

    #[test]
    fn a_page_is_kept_within_the_list() {
        assert_eq!(paged(0, 1, 100), (1, 1, 0..0));
        assert_eq!(paged(250, 1, 100), (1, 3, 0..100));
        assert_eq!(paged(250, 3, 100), (3, 3, 200..250));
        assert_eq!(paged(250, 9, 100), (3, 3, 200..250));
        assert_eq!(paged(250, 0, 100), (1, 3, 0..100));
        assert_eq!(workshop_pages(0), 1);
        assert_eq!(workshop_pages(51), 2);
        assert_eq!(workshop_pages(10_000_000), 1000);
        assert_eq!(page_within(7, 250, 100), 3);
        assert_eq!(page_within(-2, 250, 100), 1);
        assert_eq!(page_within(2, 0, 50), 1);
    }

    #[test]
    fn taste_is_the_three_genres_carried_most() {
        let items = [
            installed(1, "a", &["Anime", "Nature"]),
            installed(2, "b", &["anime", "Game"]),
            installed(3, "c", &["Nature", "Unspecified"]),
            installed(4, "d", &["Unspecified", "Retro"]),
        ];
        assert_eq!(taste(&items), ["Anime", "Nature", "Game"]);
        assert!(taste(&[]).is_empty());
        assert_eq!(votes(0, 0), 0.0);
        assert_eq!(votes(3, 1), 3.75);
    }
}
