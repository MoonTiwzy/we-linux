//! The Steam workshop of Wallpaper Engine through the Steamworks flat API of a
//! libsteam_api.so, speaking to the running Steam client as app 431960: the
//! same queries, tags and subscriptions the Windows client uses. Everything
//! here runs on one thread ([`spawn`]): Steamworks wants its calls and its
//! callbacks pumped from the same place.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

pub const APP: u32 = 431960;
const RTLD_NOW: i32 = 2;
/// `SteamUGCQueryCompleted_t`.
const QUERY_COMPLETED: i32 = 3401;
/// `k_EUGCMatchingUGCType_Items_ReadyToUse`.
const READY_TO_USE: i32 = 2;
pub const PAGE_SIZE: u32 = 50;

unsafe extern "C" {
    fn dlopen(file: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// How the workshop sorts: an `EUGCQuery` over everything, or a list of the
/// user's own (`EUserUGCList`).
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

impl Sort {
    fn code(self) -> i32 {
        match self {
            Sort::Vote => 0,
            Sort::Newest => 1,
            Sort::Trend => 3,
            Sort::FriendsFavorites => 4,
            Sort::FriendsCreated => 5,
            Sort::FollowedCreated => 7,
            Sort::UpVotes => 10,
            Sort::Subscriptions => 12,
            Sort::Updated => 19,
            Sort::Favorited => 2,
            Sort::VotedUp => 4,
        }
    }

    /// One of the user's own lists, asked for by account.
    fn personal(self) -> bool {
        matches!(self, Sort::Favorited | Sort::VotedUp)
    }
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug, Default)]
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

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub total: u32,
}

/// `EItemState` bits.
pub const SUBSCRIBED: u32 = 1;
pub const INSTALLED: u32 = 4;
pub const DOWNLOADING: u32 = 16;
pub const PENDING: u32 = 32;

#[repr(C)]
struct StringArray {
    strings: *const *const c_char,
    count: i32,
}

type Ugc = *mut c_void;

pub struct Steam {
    run_callbacks: unsafe extern "C" fn(),
    shutdown: unsafe extern "C" fn(),
    ugc: Ugc,
    utils: *mut c_void,
    subscribe: unsafe extern "C" fn(Ugc, u64) -> u64,
    unsubscribe: unsafe extern "C" fn(Ugc, u64) -> u64,
    download: unsafe extern "C" fn(Ugc, u64, bool) -> bool,
    state: unsafe extern "C" fn(Ugc, u64) -> u32,
    count: unsafe extern "C" fn(Ugc, bool) -> u32,
    items: unsafe extern "C" fn(Ugc, *mut u64, u32, bool) -> u32,
    download_info: unsafe extern "C" fn(Ugc, u64, *mut u64, *mut u64) -> bool,
    completed: unsafe extern "C" fn(*mut c_void, u64, *mut bool) -> bool,
    call_result: unsafe extern "C" fn(*mut c_void, u64, *mut c_void, i32, i32, *mut bool) -> bool,
    create_query: unsafe extern "C" fn(Ugc, i32, i32, u32, u32, u32) -> u64,
    required_tag: unsafe extern "C" fn(Ugc, u64, *const c_char) -> bool,
    excluded_tag: unsafe extern "C" fn(Ugc, u64, *const c_char) -> bool,
    tag_group: unsafe extern "C" fn(Ugc, u64, *const StringArray) -> bool,
    search_text: unsafe extern "C" fn(Ugc, u64, *const c_char) -> bool,
    trend_days: unsafe extern "C" fn(Ugc, u64, u32) -> bool,
    send_query: unsafe extern "C" fn(Ugc, u64) -> u64,
    query_result: unsafe extern "C" fn(Ugc, u64, u32, *mut c_void) -> bool,
    preview_url: unsafe extern "C" fn(Ugc, u64, u32, *mut c_char, u32) -> bool,
    release_query: unsafe extern "C" fn(Ugc, u64) -> bool,
    user_query: unsafe extern "C" fn(Ugc, u32, i32, i32, i32, u32, u32, u32) -> u64,
    set_vote: unsafe extern "C" fn(Ugc, u64, bool) -> u64,
    get_vote: unsafe extern "C" fn(Ugc, u64) -> u64,
    add_favorite: unsafe extern "C" fn(Ugc, u32, u64) -> u64,
    remove_favorite: unsafe extern "C" fn(Ugc, u32, u64) -> u64,
    details_query: unsafe extern "C" fn(Ugc, *const u64, u32) -> u64,
    /// The user's account, for the lists of their own (0 when not known).
    account: u32,
    friends: *mut c_void,
    request_user: Option<unsafe extern "C" fn(*mut c_void, u64, bool) -> bool>,
    persona_name: Option<unsafe extern "C" fn(*mut c_void, u64) -> *const c_char>,
    small_avatar: Option<unsafe extern "C" fn(*mut c_void, u64) -> i32>,
    image_size: unsafe extern "C" fn(*mut c_void, i32, *mut u32, *mut u32) -> bool,
    image_rgba: unsafe extern "C" fn(*mut c_void, i32, *mut u8, i32) -> bool,
}

/// A Steam user as the details panel shows them.
pub struct Persona {
    pub name: String,
    /// 32x32 RGBA, when Steam has it.
    pub avatar: Option<(u32, u32, Vec<u8>)>,
}

fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
        .join("we-linux")
}

/// A 64-bit libsteam_api.so with the interface we call.
fn usable(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else { return false };
    let needle = b"SteamAPI_SteamUGC_v021";
    bytes.len() > 5 && bytes[..4] == *b"\x7fELF" && bytes[4] == 2 && bytes.windows(needle.len()).any(|w| w == needle)
}

/// Steam itself has no libsteam_api.so, every native game brings its own:
/// look through the installed ones (a few folders deep) for a usable one.
fn find_in_games() -> Option<PathBuf> {
    fn walk(dir: &Path, depth: u32) -> Option<PathBuf> {
        let mut folders = Vec::new();
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == "libsteam_api.so") && usable(&path) {
                return Some(path);
            }
            if depth > 0 && path.is_dir() {
                folders.push(path);
            }
        }
        folders.into_iter().find_map(|f| walk(&f, depth - 1))
    }
    crate::library::libraries().into_iter().find_map(|l| walk(&l.join("steamapps/common"), 5))
}

/// The libsteam_api.so to load: named by WE_LINUX_STEAM_API, next to the
/// binary, our own copy, else one found in an installed game (copied to our
/// data directory, so removing the game later changes nothing).
fn steam_api() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("WE_LINUX_STEAM_API") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe.with_file_name("libsteam_api.so"));
    }
    let own = data_dir().join("libsteam_api.so");
    candidates.push(own.clone());
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(found);
    }
    let found = find_in_games()?;
    let copied = std::fs::create_dir_all(data_dir()).and_then(|()| std::fs::copy(&found, &own));
    Some(if copied.is_ok() { own } else { found })
}

fn symbol(lib: *mut c_void, name: &str) -> Result<*mut c_void, String> {
    let c = CString::new(name).map_err(|e| e.to_string())?;
    let p = unsafe { dlsym(lib, c.as_ptr()) };
    if p.is_null() { Err(crate::tf!("libsteam_api.so has no {name}", "в libsteam_api.so нет {name}")) } else { Ok(p) }
}

fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Who we are to Steam. Called once, before any thread is started: the
/// environment is not to be changed while another thread may read it.
pub fn init_env() {
    unsafe {
        std::env::set_var("SteamAppId", APP.to_string());
        std::env::set_var("SteamGameId", APP.to_string());
    }
}

impl Steam {
    // Each function's type is the field it is put in: the transmutes need no
    // other annotation.
    #[allow(clippy::missing_transmute_annotations)]
    pub fn open() -> Result<Self, String> {
        let path = steam_api().ok_or(crate::lang::t(
            "libsteam_api.so was not found: install any game with a native Linux version in Steam (a free one will do)",
            "не найден libsteam_api.so: установи в Steam любую игру с нативной версией под Linux (подойдёт бесплатная)",
        ))?;
        let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let lib = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err(crate::tf!("{} did not load", "не загрузился {}", path.display()));
        }
        macro_rules! f {
            ($name:literal) => {
                std::mem::transmute(symbol(lib, $name)?)
            };
        }
        unsafe {
            let init: unsafe extern "C" fn(*mut c_char) -> i32 = f!("SteamAPI_InitFlat");
            let mut message = [0 as c_char; 1024];
            if init(message.as_mut_ptr()) != 0 {
                let why = CStr::from_ptr(message.as_ptr()).to_string_lossy().into_owned();
                return Err(crate::tf!("Steam does not answer: {why}", "Steam не отвечает: {why}"));
            }
            let ugc: unsafe extern "C" fn() -> *mut c_void = f!("SteamAPI_SteamUGC_v021");
            let utils: unsafe extern "C" fn() -> *mut c_void = f!("SteamAPI_SteamUtils_v010");
            // The user and their friends: what the lists of one's own and the
            // author line need. A library without them still browses.
            let optional = |name: &str| symbol(lib, name).ok();
            let account = match (optional("SteamAPI_SteamUser_v023"), optional("SteamAPI_ISteamUser_GetSteamID")) {
                (Some(user), Some(id)) => {
                    let user: unsafe extern "C" fn() -> *mut c_void = std::mem::transmute(user);
                    let id: unsafe extern "C" fn(*mut c_void) -> u64 = std::mem::transmute(id);
                    id(user()) as u32
                }
                _ => 0,
            };
            let friends = optional("SteamAPI_SteamFriends_v018")
                .or_else(|| optional("SteamAPI_SteamFriends_v017"))
                .map(|f| std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> *mut c_void>(f)())
                .unwrap_or(std::ptr::null_mut());
            Ok(Self {
                run_callbacks: f!("SteamAPI_RunCallbacks"),
                shutdown: f!("SteamAPI_Shutdown"),
                ugc: ugc(),
                utils: utils(),
                subscribe: f!("SteamAPI_ISteamUGC_SubscribeItem"),
                unsubscribe: f!("SteamAPI_ISteamUGC_UnsubscribeItem"),
                download: f!("SteamAPI_ISteamUGC_DownloadItem"),
                state: f!("SteamAPI_ISteamUGC_GetItemState"),
                count: f!("SteamAPI_ISteamUGC_GetNumSubscribedItems"),
                items: f!("SteamAPI_ISteamUGC_GetSubscribedItems"),
                download_info: f!("SteamAPI_ISteamUGC_GetItemDownloadInfo"),
                completed: f!("SteamAPI_ISteamUtils_IsAPICallCompleted"),
                call_result: f!("SteamAPI_ISteamUtils_GetAPICallResult"),
                create_query: f!("SteamAPI_ISteamUGC_CreateQueryAllUGCRequestPage"),
                required_tag: f!("SteamAPI_ISteamUGC_AddRequiredTag"),
                excluded_tag: f!("SteamAPI_ISteamUGC_AddExcludedTag"),
                tag_group: f!("SteamAPI_ISteamUGC_AddRequiredTagGroup"),
                search_text: f!("SteamAPI_ISteamUGC_SetSearchText"),
                trend_days: f!("SteamAPI_ISteamUGC_SetRankedByTrendDays"),
                send_query: f!("SteamAPI_ISteamUGC_SendQueryUGCRequest"),
                query_result: f!("SteamAPI_ISteamUGC_GetQueryUGCResult"),
                preview_url: f!("SteamAPI_ISteamUGC_GetQueryUGCPreviewURL"),
                release_query: f!("SteamAPI_ISteamUGC_ReleaseQueryUGCRequest"),
                user_query: f!("SteamAPI_ISteamUGC_CreateQueryUserUGCRequest"),
                set_vote: f!("SteamAPI_ISteamUGC_SetUserItemVote"),
                get_vote: f!("SteamAPI_ISteamUGC_GetUserItemVote"),
                add_favorite: f!("SteamAPI_ISteamUGC_AddItemToFavorites"),
                remove_favorite: f!("SteamAPI_ISteamUGC_RemoveItemFromFavorites"),
                details_query: f!("SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest"),
                account,
                friends,
                request_user: optional("SteamAPI_ISteamFriends_RequestUserInformation").map(|f| std::mem::transmute(f)),
                persona_name: optional("SteamAPI_ISteamFriends_GetFriendPersonaName").map(|f| std::mem::transmute(f)),
                small_avatar: optional("SteamAPI_ISteamFriends_GetSmallFriendAvatar").map(|f| std::mem::transmute(f)),
                image_size: f!("SteamAPI_ISteamUtils_GetImageSize"),
                image_rgba: f!("SteamAPI_ISteamUtils_GetImageRGBA"),
            })
        }
    }

    pub fn pump(&self) {
        unsafe { (self.run_callbacks)() }
    }

    /// Pump callbacks until `call` is answered; false on a failure or after 20 s.
    fn wait(&self, call: u64) -> bool {
        if call == 0 {
            return false;
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let mut failed = false;
            self.pump();
            if unsafe { (self.completed)(self.utils, call, &mut failed) } {
                return !failed;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    pub fn query(&self, q: &Query) -> Result<Page, String> {
        let cstr = |s: &str| CString::new(s.replace('\0', "")).unwrap_or_default();
        unsafe {
            let handle = if q.author != 0 {
                // k_EUserUGCList_Published, k_EUserUGCListSortOrder_CreationOrderDesc
                (self.user_query)(self.ugc, q.author, 0, READY_TO_USE, 0, APP, APP, q.page.max(1))
            } else if q.sort.personal() {
                if self.account == 0 {
                    return Err(crate::lang::t("Steam does not say who is signed in", "Steam не говорит, кто вошёл").into());
                }
                // k_EUserUGCListSortOrder_SubscriptionDateDesc
                (self.user_query)(self.ugc, self.account, q.sort.code(), READY_TO_USE, 4, APP, APP, q.page.max(1))
            } else {
                (self.create_query)(self.ugc, q.sort.code(), READY_TO_USE, APP, APP, q.page.max(1))
            };
            if handle == u64::MAX {
                return Err(crate::lang::t("Steam did not take the request", "Steam не принял запрос").into());
            }
            for tag in &q.required {
                (self.required_tag)(self.ugc, handle, cstr(tag).as_ptr());
            }
            for tag in &q.excluded {
                (self.excluded_tag)(self.ugc, handle, cstr(tag).as_ptr());
            }
            for group in &q.groups {
                let owned: Vec<CString> = group.iter().map(|t| cstr(t)).collect();
                let pointers: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
                let array = StringArray { strings: pointers.as_ptr(), count: pointers.len() as i32 };
                (self.tag_group)(self.ugc, handle, &array);
            }
            if !q.text.trim().is_empty() {
                (self.search_text)(self.ugc, handle, cstr(q.text.trim()).as_ptr());
            }
            if q.sort == Sort::Trend {
                (self.trend_days)(self.ugc, handle, q.trend_days.clamp(1, 365));
            }
            self.run(handle)
        }
    }

    /// The workshop's details of items by id: what the installed wallpapers'
    /// panel shows of them (votes, size, author).
    pub fn details(&self, ids: &[u64]) -> Result<Page, String> {
        unsafe {
            let handle = (self.details_query)(self.ugc, ids.as_ptr(), ids.len() as u32);
            if handle == u64::MAX {
                return Err(crate::lang::t("Steam did not take the request", "Steam не принял запрос").into());
            }
            self.run(handle)
        }
    }

    /// The user's vote on an item: 1 up, -1 down, 0 none (or not known).
    pub fn vote(&self, id: u64) -> i32 {
        unsafe {
            let call = (self.get_vote)(self.ugc, id);
            if !self.wait(call) {
                return 0;
            }
            // GetUserItemVoteResult_t (3409): id 0, result 8, voted up 12, voted down 13.
            let mut done = [0u8; 32];
            let mut failed = false;
            let ok = (self.call_result)(self.utils, call, done.as_mut_ptr() as *mut c_void, done.len() as i32, 3409, &mut failed);
            if !ok || failed || u32_at(&done, 8) != 1 {
                return 0;
            }
            if done[12] != 0 { 1 } else if done[13] != 0 { -1 } else { 0 }
        }
    }

    /// Vote an item up or down.
    pub fn set_vote(&self, id: u64, up: bool) -> bool {
        self.wait(unsafe { (self.set_vote)(self.ugc, id, up) })
    }

    /// Add an item to the user's workshop favourites, or take it out.
    pub fn set_favorite(&self, id: u64, on: bool) -> bool {
        self.wait(unsafe { if on { (self.add_favorite)(self.ugc, APP, id) } else { (self.remove_favorite)(self.ugc, APP, id) } })
    }

    /// Who a Steam user is: asked for once, known after a moment (None until).
    pub fn persona(&self, id: u64) -> Option<Persona> {
        let (Some(request), Some(name)) = (self.request_user, self.persona_name) else { return None };
        if self.friends.is_null() || id == 0 {
            return None;
        }
        unsafe {
            // True: Steam has to fetch it first.
            if request(self.friends, id, false) {
                return None;
            }
            let name = CStr::from_ptr(name(self.friends, id)).to_string_lossy().into_owned();
            if name.is_empty() || name == "[unknown]" {
                return None;
            }
            let avatar = self.small_avatar.and_then(|small| {
                let image = small(self.friends, id);
                let (mut w, mut h) = (0u32, 0u32);
                if image <= 0 || !(self.image_size)(self.utils, image, &mut w, &mut h) || w == 0 || h == 0 {
                    return None;
                }
                let mut rgba = vec![0u8; (w * h * 4) as usize];
                (self.image_rgba)(self.utils, image, rgba.as_mut_ptr(), rgba.len() as i32).then_some((w, h, rgba))
            });
            Some(Persona { name, avatar })
        }
    }

    /// Send a query made ready and read its page.
    unsafe fn run(&self, handle: u64) -> Result<Page, String> {
        unsafe {
            let call = (self.send_query)(self.ugc, handle);
            if !self.wait(call) {
                (self.release_query)(self.ugc, handle);
                return Err(crate::lang::t("the workshop did not answer", "мастерская не ответила").into());
            }
            // SteamUGCQueryCompleted_t: handle 0, result 8, returned 12, total 16.
            let mut done = [0u8; 512];
            let mut failed = false;
            let ok = (self.call_result)(
                self.utils,
                call,
                done.as_mut_ptr() as *mut c_void,
                done.len() as i32,
                QUERY_COMPLETED,
                &mut failed,
            );
            if !ok || failed || u32_at(&done, 8) != 1 {
                (self.release_query)(self.ugc, handle);
                return Err(crate::tf!("the workshop answered with error {}", "мастерская ответила ошибкой {}", u32_at(&done, 8)));
            }
            let (returned, total) = (u32_at(&done, 12), u32_at(&done, 16));
            let mut entries = Vec::with_capacity(returned as usize);
            // SteamUGCDetails_t (pack 4): id 0, title 24[129], description
            // 153[8000], owner 8156, updated 8168, tags 8183[1025], size 9484,
            // votes up 9748 and down 9752.
            let mut details = vec![0u8; 16384];
            for index in 0..returned {
                details.fill(0);
                if !(self.query_result)(self.ugc, handle, index, details.as_mut_ptr() as *mut c_void) {
                    continue;
                }
                let mut url = [0 as c_char; 1024];
                (self.preview_url)(self.ugc, handle, index, url.as_mut_ptr(), url.len() as u32);
                entries.push(Entry {
                    id: u64::from_le_bytes(details[0..8].try_into().unwrap()),
                    title: text(&details[24..153]),
                    tags: text(&details[8183..9208]).split(',').filter(|t| !t.is_empty()).map(str::to_owned).collect(),
                    size: u32_at(&details, 9484) as u64,
                    up: u32_at(&details, 9748),
                    down: u32_at(&details, 9752),
                    preview_url: CStr::from_ptr(url.as_ptr()).to_string_lossy().into_owned(),
                    updated: u32_at(&details, 8168),
                    owner: u64::from_le_bytes(details[8156..8164].try_into().unwrap()),
                });
            }
            (self.release_query)(self.ugc, handle);
            Ok(Page { entries, total })
        }
    }

    pub fn subscribe(&self, id: u64) -> bool {
        let ok = self.wait(unsafe { (self.subscribe)(self.ugc, id) });
        if ok {
            unsafe { (self.download)(self.ugc, id, true) };
        }
        ok
    }

    pub fn unsubscribe(&self, id: u64) -> bool {
        self.wait(unsafe { (self.unsubscribe)(self.ugc, id) })
    }

    /// Subscribed ids with their `EItemState`.
    pub fn subscribed(&self) -> Vec<(u64, u32)> {
        unsafe {
            let n = (self.count)(self.ugc, false);
            let mut ids = vec![0u64; n as usize];
            let got = (self.items)(self.ugc, ids.as_mut_ptr(), n, false) as usize;
            ids[..got.min(ids.len())].iter().map(|&id| (id, (self.state)(self.ugc, id))).collect()
        }
    }

    /// State of one item and, while it downloads, bytes done and in all.
    pub fn progress(&self, id: u64) -> (u32, u64, u64) {
        let (mut done, mut total) = (0u64, 0u64);
        unsafe {
            let state = (self.state)(self.ugc, id);
            (self.download_info)(self.ugc, id, &mut done, &mut total);
            (state, done, total)
        }
    }
}

impl Drop for Steam {
    fn drop(&mut self) {
        unsafe { (self.shutdown)() }
    }
}

pub enum Command {
    Query { generation: u64, query: Query },
    /// The workshop's details of installed items.
    Details(Vec<u64>),
    /// Who an author is.
    Persona(u64),
    /// The user's vote on an item, asked or given.
    Vote(u64),
    SetVote { id: u64, up: bool },
    Favorite { id: u64, on: bool },
    Subscribe(u64),
    Unsubscribe(u64),
    Quit,
}

pub enum Reply {
    /// Steam is there (or why not).
    Session(Result<(), String>),
    Page { generation: u64, result: Result<Page, String> },
    Details(Vec<Entry>),
    Persona { id: u64, persona: Persona },
    Vote { id: u64, vote: i32 },
    Subscribed { id: u64, ok: bool },
    Unsubscribed { id: u64, ok: bool },
    Subscriptions(Vec<(u64, u32)>),
    Progress { id: u64, state: u32, done: u64, total: u64 },
}

/// The Steam thread: opens the session (trying again every few seconds while
/// Steam is not running), answers commands and pumps callbacks in between.
pub fn spawn(reply: impl Fn(Reply) + Send + 'static) -> Sender<Command> {
    let (tx, rx): (Sender<Command>, Receiver<Command>) = channel();
    std::thread::Builder::new()
        .name("steam".into())
        .spawn(move || {
            let mut steam: Option<Steam> = None;
            let mut retry = Instant::now();
            // Subscribed here and not on the disk yet, with when.
            let mut watched: Vec<(u64, Instant)> = Vec::new();
            let mut polled = Instant::now();
            let mut pending: Vec<Command> = Vec::new();
            // Authors Steam is fetching, with when they were asked for.
            let mut asked: Vec<(u64, Instant)> = Vec::new();
            loop {
                if steam.is_none() && Instant::now() >= retry {
                    match Steam::open() {
                        Ok(s) => {
                            reply(Reply::Session(Ok(())));
                            reply(Reply::Subscriptions(s.subscribed()));
                            steam = Some(s);
                        }
                        Err(e) => {
                            reply(Reply::Session(Err(e)));
                            retry = Instant::now() + Duration::from_secs(5);
                        }
                    }
                }
                // What waited for Steam, in the order it was asked for.
                let command = if steam.is_some() && !pending.is_empty() { Some(pending.remove(0)) } else { None };
                let command = match command {
                    Some(c) => Some(c),
                    None => match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(c) => Some(c),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    },
                };
                if matches!(command, Some(Command::Quit)) {
                    return;
                }
                let Some(s) = &steam else {
                    // Keep the newest query for when Steam is there.
                    if let Some(c) = command {
                        if matches!(c, Command::Query { .. }) {
                            pending.retain(|p| !matches!(p, Command::Query { .. }));
                        }
                        pending.push(c);
                    }
                    continue;
                };
                match command {
                    Some(Command::Query { generation, query }) => {
                        reply(Reply::Page { generation, result: s.query(&query) });
                    }
                    Some(Command::Details(ids)) => {
                        // A page of the workshop is fifty at most.
                        for chunk in ids.chunks(PAGE_SIZE as usize) {
                            if let Ok(page) = s.details(chunk) {
                                reply(Reply::Details(page.entries));
                            }
                        }
                    }
                    Some(Command::Persona(id)) => {
                        // Steam fetches a stranger first: asked again for a few seconds.
                        if let Some(persona) = s.persona(id) {
                            reply(Reply::Persona { id, persona });
                        } else if !asked.iter().any(|(a, _)| *a == id) {
                            asked.push((id, Instant::now()));
                        }
                    }
                    Some(Command::Vote(id)) => reply(Reply::Vote { id, vote: s.vote(id) }),
                    Some(Command::SetVote { id, up }) => {
                        if s.set_vote(id, up) {
                            reply(Reply::Vote { id, vote: if up { 1 } else { -1 } });
                        }
                    }
                    Some(Command::Favorite { id, on }) => {
                        s.set_favorite(id, on);
                    }
                    Some(Command::Subscribe(id)) => {
                        let ok = s.subscribe(id);
                        reply(Reply::Subscribed { id, ok });
                        if ok && !watched.iter().any(|(w, _)| *w == id) {
                            watched.push((id, Instant::now()));
                        }
                    }
                    Some(Command::Unsubscribe(id)) => {
                        let ok = s.unsubscribe(id);
                        reply(Reply::Unsubscribed { id, ok });
                        reply(Reply::Subscriptions(s.subscribed()));
                    }
                    Some(Command::Quit) | None => {}
                }
                s.pump();
                asked.retain(|&(id, since)| match s.persona(id) {
                    Some(persona) => {
                        reply(Reply::Persona { id, persona });
                        false
                    }
                    None => since.elapsed() < Duration::from_secs(10),
                });
                if !watched.is_empty() && polled.elapsed() >= Duration::from_millis(500) {
                    polled = Instant::now();
                    let mut landed = false;
                    let mut lost = Vec::new();
                    watched.retain(|&(id, since)| {
                        let (state, done, total) = s.progress(id);
                        reply(Reply::Progress { id, state, done, total });
                        let finished = state & INSTALLED != 0 && state & (DOWNLOADING | PENDING) == 0;
                        landed |= finished;
                        // Steam takes a moment to mark a new subscription: an
                        // item without the mark is watched for a while before
                        // it counts as unsubscribed again.
                        let gone = state & SUBSCRIBED == 0 && since.elapsed() > Duration::from_secs(20);
                        if gone && !finished {
                            lost.push(id);
                        }
                        !finished && !gone
                    });
                    // The subscription did not hold: nothing is coming.
                    for id in lost {
                        reply(Reply::Subscribed { id, ok: false });
                    }
                    if landed {
                        reply(Reply::Subscriptions(s.subscribed()));
                    }
                }
            }
        })
        .expect("steam thread");
    tx
}
