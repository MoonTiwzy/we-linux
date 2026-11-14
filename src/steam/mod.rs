//! The Steam workshop of Wallpaper Engine, through the running Steam client
//! as app 431960: the same queries, tags and subscriptions the Windows client
//! uses.
//!
//! The rest of the program talks to Steam through [`Workshop`] alone, so that
//! what it does with Steam's answers can be driven by a fake in tests:
//!
//! - `sdk`: the Steamworks SDK's names, ids and struct layouts, checked as it builds.
//! - `flat`: the [`Workshop`] of a real Steam, through the flat API of a
//!   `libsteam_api.so` found in an installed game; it refuses a library it
//!   does not know before calling into it.
//! - `worker`: the thread the window asks ([`spawn`]): it keeps the session,
//!   notices Steam going away and coming back, and watches downloads.

mod flat;
mod sdk;
mod worker;

#[cfg(test)]
mod fake;

pub use flat::Steam;
pub use worker::{Command, Reply, spawn};

pub const APP: u32 = 431960;
/// What a page of the workshop holds: `kNumUGCResultsPerPage` (isteamugc.h).
pub const PAGE_SIZE: u32 = 50;

/// How the workshop sorts: an order over everything, or a list of the user's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Trend,
    Vote,
    Newest,
    Subscriptions,
    Updated,
    UpVotes,
    FriendsFavorites,
    FriendsCreated,
    FollowedCreated,
    /// The user's favourites.
    Favorited,
    /// What the user voted up.
    VotedUp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub sort: Sort,
    pub text: String,
    /// From 1.
    pub page: u32,
    /// An item must carry a tag of every group.
    pub groups: Vec<Vec<String>>,
    pub required: Vec<String>,
    pub excluded: Vec<String>,
    /// The window of [`Sort::Trend`], in days.
    pub trend_days: u32,
    /// One author's wallpapers only, newest first: their account (0: everyone's).
    pub author: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub title: String,
    pub tags: Vec<String>,
    pub size: u64,
    pub up: u32,
    pub down: u32,
    pub preview_url: String,
    pub updated: u32,
    /// The author's Steam id.
    pub owner: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub total: u32,
}

/// An item's `EItemState`, as Steam tells it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ItemState(pub u32);

impl ItemState {
    pub const SUBSCRIBED: Self = Self(sdk::item_state::SUBSCRIBED);
    pub const INSTALLED: Self = Self(sdk::item_state::INSTALLED);
    pub const DOWNLOADING: Self = Self(sdk::item_state::DOWNLOADING);
    pub const DOWNLOAD_PENDING: Self = Self(sdk::item_state::DOWNLOAD_PENDING);

    pub fn has(self, other: Self) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    pub fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn subscribed(self) -> bool {
        self.has(Self::SUBSCRIBED)
    }

    /// Steam is getting its files, or is about to.
    pub fn downloading(self) -> bool {
        self.has(Self::DOWNLOADING) || self.has(Self::DOWNLOAD_PENDING)
    }

    /// On the disk with nothing more to come.
    pub fn finished(self) -> bool {
        self.has(Self::INSTALLED) && !self.downloading()
    }
}

/// How far an item's download is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Progress {
    pub state: ItemState,
    pub done: u64,
    pub total: u64,
}

/// The user's vote on an item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vote {
    Up,
    Down,
    None,
}

impl Vote {
    /// As the details panel shows it: 1 up, -1 down, 0 none.
    pub fn sign(self) -> i32 {
        match self {
            Vote::Up => 1,
            Vote::Down => -1,
            Vote::None => 0,
        }
    }
}

/// A Steam user as the details panel shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Persona {
    pub name: String,
    /// 32x32 RGBA, when Steam has it.
    pub avatar: Option<(u32, u32, Vec<u8>)>,
}

/// Why Steam did not do what it was asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// There is no session: no usable libsteam_api.so, or Steam not running
    /// or not signed in. Says which, in words.
    Unavailable(String),
    /// The Steam client went away during the session.
    Gone,
    /// Asked, and no answer in time.
    NoAnswer,
    /// Steam answered with this `EResult`.
    Refused(i32),
    /// Steam's answer was not the struct we know (`ESteamAPICallFailure`
    /// MismatchedCallback): a library or a client this build does not know.
    Mismatch,
    /// The user's own lists without knowing who the user is.
    NoAccount,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::lang::t;
        use sdk::result;
        let text = match self {
            Error::Unavailable(why) => return f.write_str(why),
            Error::Gone => t("Steam was closed", "Steam закрыт"),
            Error::NoAnswer => t("Steam did not answer in time", "Steam не ответил вовремя"),
            Error::Mismatch => t(
                "Steam answered in a form this version of WE Linux does not know: update WE Linux",
                "Steam ответил в виде, который эта версия WE Linux не знает: обнови WE Linux",
            ),
            Error::NoAccount => t("Steam does not say who is signed in", "Steam не говорит, кто вошёл"),
            Error::Refused(code) => match *code {
                result::NO_CONNECTION | result::SERVICE_UNAVAILABLE => t(
                    "Steam is offline: no connection to its servers",
                    "Steam офлайн: нет связи с его серверами",
                ),
                result::NOT_LOGGED_ON => t("Steam is not signed in", "в Steam никто не вошёл"),
                result::ACCESS_DENIED => t(
                    "Steam says this account may not do that",
                    "Steam говорит, что этому аккаунту так нельзя",
                ),
                result::FILE_NOT_FOUND | result::NO_MATCH => {
                    t("the workshop has no such item", "в мастерской нет такого предмета")
                }
                result::TIMEOUT => t("the workshop did not answer in time", "мастерская не ответила вовремя"),
                result::BUSY | result::LIMIT_EXCEEDED | result::RATE_LIMIT_EXCEEDED => {
                    t("Steam is busy: try again in a minute", "Steam занят: попробуй через минуту")
                }
                result::DUPLICATE_REQUEST => t("Steam has that already", "у Steam это уже есть"),
                result::INVALID_PARAM => t("Steam did not take the request", "Steam не принял запрос"),
                code => {
                    return f.write_str(&crate::tf!(
                        "the workshop answered with error {code}",
                        "мастерская ответила ошибкой {code}"
                    ));
                }
            },
        };
        f.write_str(text)
    }
}

/// What the program asks of Steam. [`Steam`] is the real one; tests drive
/// the worker with a fake.
pub trait Workshop {
    /// Let Steam deliver what it has for us.
    fn pump(&mut self);
    /// Whether the Steam client is still there.
    fn running(&self) -> bool;
    /// A page of the workshop.
    fn query(&mut self, query: &Query) -> Result<Page, Error>;
    /// The workshop's details of items by id (fifty at most).
    fn details(&mut self, ids: &[u64]) -> Result<Page, Error>;
    fn subscribe(&mut self, id: u64) -> Result<(), Error>;
    fn unsubscribe(&mut self, id: u64) -> Result<(), Error>;
    /// Ask Steam to fetch a subscribed item's files now; false when it would not.
    fn download(&mut self, id: u64) -> bool;
    /// Every subscribed item with its state.
    fn subscribed(&self) -> Vec<(u64, ItemState)>;
    fn progress(&self, id: u64) -> Progress;
    fn vote(&mut self, id: u64) -> Result<Vote, Error>;
    fn set_vote(&mut self, id: u64, up: bool) -> Result<(), Error>;
    fn set_favorite(&mut self, id: u64, on: bool) -> Result<(), Error>;
    /// Who a Steam user is: None until Steam knows.
    fn persona(&mut self, id: u64) -> Option<Persona>;
}

/// Who we are to Steam. Called once, before any thread is started: the
/// environment is not to be changed while another thread may read it.
pub fn init_env() {
    unsafe {
        std::env::set_var("SteamAppId", APP.to_string());
        std::env::set_var("SteamGameId", APP.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_is_finished_once_installed_with_nothing_coming() {
        let installed = ItemState::SUBSCRIBED.with(ItemState::INSTALLED);
        assert!(installed.finished());
        assert!(!installed.with(ItemState::DOWNLOADING).finished());
        assert!(!installed.with(ItemState::DOWNLOAD_PENDING).finished());
        assert!(!ItemState::SUBSCRIBED.finished());
        // A bit a later Steam adds changes nothing of what we read.
        assert!(ItemState(installed.0 | 1 << 20).finished());
        assert!(!ItemState::default().has(ItemState::default()));
    }

    #[test]
    fn a_refusal_is_said_in_words() {
        assert_eq!(Error::Refused(sdk::result::NOT_LOGGED_ON).to_string(), "Steam is not signed in");
        assert_eq!(Error::Refused(1234).to_string(), "the workshop answered with error 1234");
        assert_eq!(Error::Gone.to_string(), "Steam was closed");
    }
}
