//! The workshop's tags as the original lists and names them: the filter
//! panel's boxes, the details' pills, the kind and rating of a card. Each
//! table is Russian label, then the tag itself (which is also the English).

use crate::lang;

/// "Показать только": the original's box. Favourites are the installed tab's
/// own; "Совместимые с Android" is about the Android app, not here.
pub(super) const ONLY: [(&str, &str); 5] = [
    ("Одобренные", "Approved"),
    ("Избранное", "Favorite"),
    ("Совместимые с Android", "Mobile compatible"),
    ("Реагирующие на звук", "Audio responsive"),
    ("Настраиваемый", "Customizable"),
];
pub(super) const TYPES: [(&str, &str); 4] =
    [("Сцена", "Scene"), ("Видео", "Video"), ("Веб", "Web"), ("Приложение", "Application")];
/// What "Тип" lists after its divider: the categories of the workshop.
pub(super) const CATEGORIES: [(&str, &str); 2] = [("Обои", "Wallpaper"), ("Пресет", "Preset")];
/// "Источник", of the installed only.
pub(super) const SOURCES: [(&str, &str); 3] =
    [("Официальный", "Official"), ("Мастерская", "Workshop"), ("Мои обои", "Local")];
pub(super) const RATINGS: [(&str, &str); 3] = [
    ("0+ (для всех возрастов)", "Everyone"),
    ("12+ (с ограничениями)", "Questionable"),
    ("18+ (для взрослых)", "Mature"),
];
/// The ratings' English labels, as the original has them.
const RATINGS_EN: [&str; 3] = ["G - All ages", "PG13 - Parental guidance", "R - Mature"];
/// The workshop's resolutions, in the original's sub-groups (its title, Russian
/// and English, and its tags); the last group has no title. The original shows
/// the tags themselves, "Standard Definition" as "SD".
pub(super) const RESOLUTION_GROUPS: [(&str, &str, &[&str]); 6] = [
    (
        "Широкий экран",
        "Widescreen",
        &["Standard Definition", "1280 x 720", "1366 x 768", "1920 x 1080", "2560 x 1440", "3840 x 2160"],
    ),
    (
        "Широкий экран «Ультра»",
        "Ultrawide",
        &["Ultrawide Standard Definition", "Ultrawide 2560 x 1080", "Ultrawide 3440 x 1440"],
    ),
    (
        "Два монитора",
        "Dual",
        &["Dual Standard Definition", "Dual 3840 x 1080", "Dual 5120 x 1440", "Dual 7680 x 2160"],
    ),
    (
        "Три монитора",
        "Triple",
        &[
            "Triple Standard Definition",
            "Triple 4096 x 768",
            "Triple 5760 x 1080",
            "Triple 7680 x 1440",
            "Triple 11520 x 2160",
        ],
    ),
    (
        "Портретная ориентация экрана",
        "Portrait",
        &[
            "Portrait Standard Definition",
            "Portrait 720 x 1280",
            "Portrait 1080 x 1920",
            "Portrait 1440 x 2560",
            "Portrait 2160 x 3840",
        ],
    ),
    ("", "", &["Dynamic resolution", "Other resolution"]),
];
pub(super) const GENRES: [(&str, &str); 25] = [
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

/// The label of a tag of `list`, in the interface language, if the list has it.
pub(super) fn label_of(list: &[(&'static str, &'static str)], tag: &str) -> Option<&'static str> {
    list.iter()
        .find(|(_, t)| t.eq_ignore_ascii_case(tag))
        .map(|(label, tag)| if lang::russian() { *label } else { *tag })
}

/// A workshop tag as the original names it where it translates tags: the
/// genres, the box's tags and the ratings. Others (resolutions, features)
/// it shows as they are.
pub(super) fn tag_label(tag: &str) -> Option<&'static str> {
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

/// Whether `tags` hold `tag`, the case left aside (Steam and project.json
/// do not agree on it).
pub(super) fn has_tag(tags: &[String], tag: &str) -> bool {
    tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_found_whatever_their_case() {
        let tags = vec!["anime".to_owned(), "Scene".to_owned()];
        assert!(has_tag(&tags, "Anime"));
        assert!(has_tag(&tags, "scene"));
        assert!(!has_tag(&tags, "Video"));
        // Whatever the language, a tag of the table is found and one that is not is not.
        assert!(label_of(&TYPES, "video").is_some());
        assert!(label_of(&TYPES, "Wallpaper").is_none());
    }

    #[test]
    fn only_some_tags_are_translated() {
        assert!(tag_label("Mature").is_some());
        assert!(tag_label("MMD").is_some());
        assert!(tag_label("Approved").is_some());
        // Resolutions and features are shown as they are.
        assert!(tag_label("1920 x 1080").is_none());
        assert!(tag_label("HDR").is_none());
    }
}
