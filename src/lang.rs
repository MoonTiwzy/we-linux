//! The interface language: English or Russian, picked on the first run and in
//! the settings. The Russian strings are the keys of the tables that hold
//! them (filter groups, sort orders), so a saved state reads the same in
//! either language.

use std::sync::atomic::{AtomicBool, Ordering};

static RUSSIAN: AtomicBool = AtomicBool::new(false);

pub fn set_russian(russian: bool) {
    RUSSIAN.store(russian, Ordering::Relaxed);
}

pub fn russian() -> bool {
    RUSSIAN.load(Ordering::Relaxed)
}

/// What the system's locale says (LC_ALL, LC_MESSAGES, LANG).
pub fn system_is_russian() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .is_some_and(|value| value.to_lowercase().starts_with("ru"))
}

/// One of two texts, by the language.
pub fn t(english: &'static str, russian_text: &'static str) -> &'static str {
    if russian() { russian_text } else { english }
}

/// `format!` in the interface language: the English text, the Russian one,
/// then the arguments of both.
#[macro_export]
macro_rules! tf {
    ($english:literal, $russian:literal $(, $arg:expr)* $(,)?) => {
        if $crate::lang::russian() { format!($russian $(, $arg)*) } else { format!($english $(, $arg)*) }
    };
}

/// The label of a table entry kept under its Russian text.
pub fn label(russian_text: &'static str) -> &'static str {
    if russian() {
        return russian_text;
    }
    match russian_text {
        "Лидеры рейтинга" => "Top rated",
        "Популярные сегодня" => "Popular today",
        "Популярные за неделю" => "Popular this week",
        "Популярные за месяц" => "Popular this month",
        "Популярные за 3 месяца" => "Popular in 3 months",
        "Популярные за год" => "Popular this year",
        "Самые новые" => "Most recent",
        "По числу подписчиков" => "Most subscribed",
        "Последнее обновление" => "Last updated",
        "Дата подписки" => "Subscription date",
        "Название" => "Name",
        "Тип" => "Type",
        "Установлено" => "Installed",
        "Избранное" => "Favourites",
        "Показать только" => "Show only",
        "Возрастной рейтинг" => "Age rating",
        "Разрешение экрана" => "Resolution",
        "Метки" => "Tags",
        "Особенности" => "Features",
        other => other,
    }
}
