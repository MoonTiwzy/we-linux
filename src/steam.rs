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

/// `EUGCQuery`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Trend,
    Vote,
    Newest,
    Subscriptions,
    Updated,
}

impl Sort {
    fn code(self) -> i32 {
        match self {
            Sort::Vote => 0,
            Sort::Newest => 1,
            Sort::Trend => 3,
            Sort::Subscriptions => 12,
            Sort::Updated => 19,
        }
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
}

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub size: u64,
    pub up: u32,
    pub down: u32,
    pub subscriptions: u64,
    pub preview_url: String,
    pub updated: u32,
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
    long_description: unsafe extern "C" fn(Ugc, u64, bool) -> bool,
    send_query: unsafe extern "C" fn(Ugc, u64) -> u64,
    query_result: unsafe extern "C" fn(Ugc, u64, u32, *mut c_void) -> bool,
    preview_url: unsafe extern "C" fn(Ugc, u64, u32, *mut c_char, u32) -> bool,
    statistic: unsafe extern "C" fn(Ugc, u64, u32, i32, *mut u64) -> bool,
    release_query: unsafe extern "C" fn(Ugc, u64) -> bool,
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

impl Steam {
    pub fn open() -> Result<Self, String> {
        unsafe {
            std::env::set_var("SteamAppId", APP.to_string());
            std::env::set_var("SteamGameId", APP.to_string());
        }
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
                long_description: f!("SteamAPI_ISteamUGC_SetReturnLongDescription"),
                send_query: f!("SteamAPI_ISteamUGC_SendQueryUGCRequest"),
                query_result: f!("SteamAPI_ISteamUGC_GetQueryUGCResult"),
                preview_url: f!("SteamAPI_ISteamUGC_GetQueryUGCPreviewURL"),
                statistic: f!("SteamAPI_ISteamUGC_GetQueryUGCStatistic"),
                release_query: f!("SteamAPI_ISteamUGC_ReleaseQueryUGCRequest"),
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
            let handle = (self.create_query)(self.ugc, q.sort.code(), READY_TO_USE, APP, APP, q.page.max(1));
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
            (self.long_description)(self.ugc, handle, true);
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
            // 153[8000], updated 8168, tags 8183[1025], size 9484, votes up
            // 9748 and down 9752.
            let mut details = vec![0u8; 16384];
            for index in 0..returned {
                details.fill(0);
                if !(self.query_result)(self.ugc, handle, index, details.as_mut_ptr() as *mut c_void) {
                    continue;
                }
                let mut url = [0 as c_char; 1024];
                (self.preview_url)(self.ugc, handle, index, url.as_mut_ptr(), url.len() as u32);
                let mut subscriptions = 0u64;
                (self.statistic)(self.ugc, handle, index, 0, &mut subscriptions);
                entries.push(Entry {
                    id: u64::from_le_bytes(details[0..8].try_into().unwrap()),
                    title: text(&details[24..153]),
                    description: text(&details[153..8153]),
                    tags: text(&details[8183..9208]).split(',').filter(|t| !t.is_empty()).map(str::to_owned).collect(),
                    size: u32_at(&details, 9484) as u64,
                    up: u32_at(&details, 9748),
                    down: u32_at(&details, 9752),
                    subscriptions,
                    preview_url: CStr::from_ptr(url.as_ptr()).to_string_lossy().into_owned(),
                    updated: u32_at(&details, 8168),
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
    Subscribe(u64),
    Unsubscribe(u64),
    Quit,
}

pub enum Reply {
    /// Steam is there (or why not).
    Session(Result<(), String>),
    Page { generation: u64, result: Result<Page, String> },
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
                let command = match pending.pop() {
                    Some(c) if steam.is_some() => Some(c),
                    Some(c) => {
                        pending.push(c);
                        None
                    }
                    None => None,
                };
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
                if !watched.is_empty() && polled.elapsed() >= Duration::from_millis(500) {
                    polled = Instant::now();
                    let mut landed = false;
                    watched.retain(|&(id, since)| {
                        let (state, done, total) = s.progress(id);
                        reply(Reply::Progress { id, state, done, total });
                        let finished = state & INSTALLED != 0 && state & (DOWNLOADING | PENDING) == 0;
                        landed |= finished;
                        // Steam takes a moment to mark a new subscription: an
                        // item without the mark is watched for a while before
                        // it counts as unsubscribed again.
                        let gone = state & SUBSCRIBED == 0 && since.elapsed() > Duration::from_secs(20);
                        !finished && !gone
                    });
                    if landed {
                        reply(Reply::Subscriptions(s.subscribed()));
                    }
                }
            }
        })
        .expect("steam thread");
    tx
}
