//! Several wallpapers picked at once, as the original's grid has them: a
//! plain click selects one (and sets it), Ctrl adds a tile to the picked ones
//! or takes it out, Shift picks a run of them, Ctrl+A all; the menu's lines,
//! Delete and the playlist then act on all of them. Items are known by id
//! here: the grid's cards are made anew whenever the list is sorted or filtered.

/// What an item is to an action: on the disk, still to be had from the
/// workshop, or in between (subscribed to, on its way down).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Installed,
    Downloadable,
    Pending,
}

/// Which of the items an action is about it takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Takes {
    /// Everything but what is not subscribed to: there is nothing to
    /// unsubscribe from or to play in those.
    Subscribed,
    Only(Status),
    Any,
}

fn add(picked: &mut Vec<u64>, id: u64) {
    if !picked.contains(&id) {
        picked.push(id);
    }
}

/// Ctrl and a click: the item comes out of what is picked when it is in it
/// or is the selection (which goes then), and goes in otherwise. Returns the
/// selection after it.
pub fn toggle(picked: &mut Vec<u64>, current: Option<u64>, id: u64) -> Option<u64> {
    if picked.contains(&id) || current == Some(id) {
        picked.retain(|p| *p != id);
        if current == Some(id) { None } else { current }
    } else {
        picked.push(id);
        current
    }
}

/// Shift and a click: what was picked goes, and the item and everything
/// shown between it and the selection come in. The run starts at whichever of
/// the two comes first and ends before the other (the item is in it either
/// way); when there is no selection, or the item is the selection, it goes
/// on to the end of the list, as the original's does.
pub fn extend(picked: &mut Vec<u64>, current: Option<u64>, id: u64, shown: &[u64]) {
    picked.clear();
    add(picked, id);
    let mut inside = false;
    for &item in shown {
        if item == id || Some(item) == current {
            inside = !inside;
        }
        if inside {
            add(picked, item);
        }
    }
}

/// Ctrl+A: these come in, besides what is picked already.
pub fn add_all(picked: &mut Vec<u64>, ids: &[u64]) {
    for &id in ids {
        add(picked, id);
    }
}

/// The grid's tiles as they lie: `cols` a row, `step` from one to the next
/// across, `row_step` down, each `side` square, the first at (1, 1).
pub struct Grid {
    pub cols: usize,
    pub step: f32,
    pub row_step: f32,
    pub side: f32,
}

/// The tiles of the first `count` a rectangle dragged over the grid touches.
pub fn in_band(grid: &Grid, count: usize, (x, y, width, height): (f32, f32, f32, f32)) -> Vec<usize> {
    let cols = grid.cols.max(1);
    (0..count)
        .filter(|i| {
            let left = 1.0 + (i % cols) as f32 * grid.step;
            let top = 1.0 + (i / cols) as f32 * grid.row_step;
            left < x + width && left + grid.side > x && top < y + height && top + grid.side > y
        })
        .collect()
}

/// A rectangle let go of over `ids`: with Ctrl each is put in or taken out
/// as a Ctrl click would, with Shift they join what is picked, and without
/// either they are all that is picked and the selection goes. Returns the
/// selection after it.
pub fn band(
    picked: &mut Vec<u64>,
    current: Option<u64>,
    ids: &[u64],
    ctrl: bool,
    shift: bool,
) -> Option<u64> {
    if ctrl {
        return ids.iter().fold(current, |current, &id| toggle(picked, current, id));
    }
    let current = if shift { current } else { None };
    if !shift {
        picked.clear();
    }
    add_all(picked, ids);
    current
}

/// What an action started on `target` (or on nothing: a key) is for. With
/// nothing picked it is the target alone. Otherwise the target, the
/// selection and what is picked, in that order, of the items shown only (a
/// filter or another page may hide some), and only of the kind it `takes`.
pub fn set_of(
    picked: &[u64],
    current: Option<u64>,
    target: Option<u64>,
    shown: &[u64],
    status: impl Fn(u64) -> Status,
    takes: Takes,
) -> Vec<u64> {
    if picked.is_empty()
        && let Some(target) = target
    {
        return vec![target];
    }
    let mut set = Vec::new();
    for id in target.into_iter().chain(current).chain(picked.iter().copied()) {
        add(&mut set, id);
    }
    set.retain(|&id| {
        shown.contains(&id)
            && match takes {
                Takes::Subscribed => status(id) != Status::Downloadable,
                Takes::Only(only) => status(id) == only,
                Takes::Any => true,
            }
    });
    set
}

/// The presets that depend on wallpapers being unsubscribed from: they go
/// with them. `installed` is every item on the disk with the wallpaper it is
/// a preset of.
pub fn presets_of(going: &[u64], installed: &[(u64, Option<u64>)]) -> Vec<u64> {
    installed
        .iter()
        .filter(|(id, of)| of.is_some_and(|of| going.contains(&of)) && !going.contains(id))
        .map(|(id, _)| *id)
        .collect()
}

/// Titles as the question names them: the first three, then how many there are.
pub fn listing(titles: &[String]) -> String {
    let named = titles.len().min(3);
    let mut text = titles[..named].join(", ");
    if named < titles.len() {
        text.push_str(&format!(", ... ({})", titles.len()));
    }
    text
}

/// A title as plain text in Markdown: its punctuation backslashed, so that a
/// star or an angle bracket in it stays what it is.
fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// An empty line between paragraphs, which Markdown would close up: a
/// paragraph of a no-break space.
const GAP: &str = "\n\n\u{a0}\n\n";

/// The question before unsubscribing, in Markdown (the titles bold, as the
/// original has them): what goes and, when presets go with it, which. The
/// original's second sentence says Steam takes the files away once the
/// window closes; here they are taken off the disk at once, and it says so.
pub fn question(titles: &[String], presets: &[String], russian: bool) -> String {
    // A space at either end inside the stars would undo the bold.
    let bold = |list: &[String]| {
        let titles: Vec<String> = list.iter().map(|t| literal(t.trim())).collect();
        format!("**{}**", listing(&titles))
    };
    let mut text = if russian {
        format!(
            "Вы уверены, что хотите отписаться от: {}?{GAP}После отмены подписки файлы обоев сразу удаляются с диска.",
            bold(titles)
        )
    } else {
        format!(
            "Are you sure that you want to unsubscribe from: {}?{GAP}After unsubscribing, the wallpaper files are removed from the disk at once.",
            bold(titles)
        )
    };
    if !presets.is_empty() {
        text.push_str(&if russian {
            format!(
                "{GAP}Следующие пресеты зависят от обоев, на которые вы пытаетесь отменить подписку, и они также будут удалены: {}",
                bold(presets)
            )
        } else {
            format!(
                "{GAP}The following presets are depending on the wallpaper(s) you are trying to unsubscribe and will be removed as well: {}",
                bold(presets)
            )
        });
    }
    text
}

/// In the small window the list does not go empty-handed when the selection
/// is unsubscribed from: what comes as many places after it as items go is
/// selected, or, at the end of the list, the one before it. None when that is
/// going as well, or when the selection is not on the page.
pub fn after_removal(page: &[u64], current: u64, going: &[u64]) -> Option<u64> {
    let at = page.iter().position(|&id| id == current)?;
    let next = if at + going.len() < page.len() {
        page[at + going.len()]
    } else if at > 0 {
        page[at - 1]
    } else {
        return None;
    };
    (!going.contains(&next)).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_puts_an_item_in_and_takes_it_out() {
        let mut picked = vec![];
        assert_eq!(toggle(&mut picked, Some(1), 2), Some(1));
        assert_eq!(toggle(&mut picked, Some(1), 3), Some(1));
        assert_eq!(picked, [2, 3]);
        // Picked already: it comes out, the selection stays.
        assert_eq!(toggle(&mut picked, Some(1), 2), Some(1));
        assert_eq!(picked, [3]);
        // The selection itself: it is unselected.
        assert_eq!(toggle(&mut picked, Some(1), 1), None);
        assert_eq!(picked, [3]);
        // Nothing selected: it goes in.
        assert_eq!(toggle(&mut picked, None, 1), None);
        assert_eq!(picked, [3, 1]);
    }

    #[test]
    fn shift_picks_the_run_between_the_selection_and_the_item() {
        let shown = [10, 11, 12, 13, 14, 15];
        let mut picked = vec![99];
        // Forward: the selection and what follows it up to the item.
        extend(&mut picked, Some(11), 14, &shown);
        assert_eq!(picked, [14, 11, 12, 13]);
        // Backward: from the item up to the selection, which is selected anyway.
        extend(&mut picked, Some(14), 11, &shown);
        assert_eq!(picked, [11, 12, 13]);
        // No selection, or the selection clicked: on to the end.
        extend(&mut picked, None, 13, &shown);
        assert_eq!(picked, [13, 14, 15]);
        extend(&mut picked, Some(12), 12, &shown);
        assert_eq!(picked, [12, 13, 14, 15]);
    }

    #[test]
    fn ctrl_a_adds_to_what_is_picked() {
        let mut picked = vec![3];
        add_all(&mut picked, &[1, 2, 3]);
        assert_eq!(picked, [3, 1, 2]);
    }

    #[test]
    fn a_rectangle_takes_the_tiles_it_touches() {
        // Three a row, 100 square, 10 apart.
        let grid = Grid { cols: 3, step: 110.0, row_step: 110.0, side: 100.0 };
        // Across the gap between the first two rows, over the second and third columns.
        assert_eq!(in_band(&grid, 8, (150.0, 90.0, 100.0, 30.0)), [1, 2, 4, 5]);
        // In a gap only: nothing.
        assert!(in_band(&grid, 8, (102.0, 0.0, 8.0, 300.0)).is_empty());
        // Past the last tile: the row is short.
        assert_eq!(in_band(&grid, 8, (0.0, 230.0, 400.0, 50.0)), [6, 7]);
    }

    #[test]
    fn a_rectangle_picks_as_the_keys_say() {
        let mut picked = vec![1];
        // Plain: what it covers is all that is picked, and nothing is selected.
        assert_eq!(band(&mut picked, Some(9), &[2, 3], false, false), None);
        assert_eq!(picked, [2, 3]);
        // Shift: added, the selection stays.
        assert_eq!(band(&mut picked, Some(9), &[3, 4], false, true), Some(9));
        assert_eq!(picked, [2, 3, 4]);
        // Ctrl: each turned over, the selection among them let go of.
        assert_eq!(band(&mut picked, Some(9), &[4, 5, 9], true, false), None);
        assert_eq!(picked, [2, 3, 5]);
    }

    #[test]
    fn an_action_is_for_the_target_alone_when_nothing_is_picked() {
        let status = |_| Status::Downloadable;
        assert_eq!(set_of(&[], Some(1), Some(5), &[1, 2], status, Takes::Subscribed), [5]);
        // A key: the selection, when it is of the kind.
        assert_eq!(set_of(&[], Some(1), None, &[1, 2], |_| Status::Installed, Takes::Subscribed), [1]);
        assert!(set_of(&[], Some(1), None, &[1, 2], status, Takes::Subscribed).is_empty());
        assert!(set_of(&[], None, None, &[1, 2], status, Takes::Any).is_empty());
    }

    #[test]
    fn an_action_is_for_the_target_the_selection_and_the_picked_ones_shown() {
        let status = |id| match id {
            1 | 2 => Status::Installed,
            3 => Status::Pending,
            _ => Status::Downloadable,
        };
        let shown = [1, 2, 3, 4, 5];
        // 6 is not shown (another page, filtered out).
        assert_eq!(set_of(&[3, 4, 6], Some(2), Some(1), &shown, status, Takes::Subscribed), [1, 2, 3]);
        assert_eq!(set_of(&[3, 4, 6], Some(2), Some(1), &shown, status, Takes::Any), [1, 2, 3, 4]);
        assert_eq!(
            set_of(&[3, 4, 5], Some(2), Some(4), &shown, status, Takes::Only(Status::Downloadable)),
            [4, 5]
        );
        // Each once, the target first.
        assert_eq!(set_of(&[1, 2], Some(2), Some(2), &shown, status, Takes::Any), [2, 1]);
    }

    #[test]
    fn presets_go_with_their_wallpapers() {
        let installed = [(1, None), (2, Some(1)), (3, Some(1)), (4, Some(9)), (5, None)];
        assert_eq!(presets_of(&[1, 5], &installed), [2, 3]);
        // A preset unsubscribed from itself is not named twice.
        assert_eq!(presets_of(&[1, 2], &installed), [3]);
        assert!(presets_of(&[5], &installed).is_empty());
    }

    #[test]
    fn the_question_names_three_and_counts_the_rest() {
        let titles = |n: usize| (1..=n).map(|i| format!("W{i}")).collect::<Vec<_>>();
        assert_eq!(listing(&titles(1)), "W1");
        assert_eq!(listing(&titles(3)), "W1, W2, W3");
        assert_eq!(listing(&titles(5)), "W1, W2, W3, ... (5)");
    }

    #[test]
    fn the_question_says_what_goes_and_which_presets_with_it() {
        let titles = vec!["Rain *night*".to_owned()];
        let text = question(&titles, &[], true);
        assert!(
            text.starts_with(&format!("Вы уверены, что хотите отписаться от: **Rain \\*night\\***?{GAP}"))
        );
        assert!(!text.contains("пресеты"));
        let text = question(&titles, &["Blue".to_owned(), "Red".to_owned()], false);
        assert!(text.starts_with("Are you sure that you want to unsubscribe from: **Rain \\*night\\***?"));
        assert!(text.ends_with("will be removed as well: **Blue, Red**"));
    }

    #[test]
    fn the_small_window_moves_on_to_the_next_item() {
        let page = [1, 2, 3, 4];
        assert_eq!(after_removal(&page, 2, &[2]), Some(3));
        // Two go: as many places on.
        assert_eq!(after_removal(&page, 2, &[2, 4]), None);
        assert_eq!(after_removal(&page, 1, &[1, 3]), None);
        assert_eq!(after_removal(&page, 1, &[1, 2]), Some(3));
        // At the end: the one before.
        assert_eq!(after_removal(&page, 4, &[4]), Some(3));
        assert_eq!(after_removal(&[7], 7, &[7]), None);
        assert_eq!(after_removal(&page, 9, &[9]), None);
    }
}
