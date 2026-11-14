//! The real [`Workshop`]: the Steamworks flat API of a libsteam_api.so,
//! loaded at run time.
//!
//! Steam itself has no libsteam_api.so; every native game brings Valve's
//! own, and one is borrowed from the games installed (a copy is kept, so
//! removing the game changes nothing). It is not linked: the program starts,
//! and the library and the wallpapers work, with no Steam at all.
//!
//! A library is only loaded when it is one this build knows: a 64-bit ELF
//! of this machine that exports every function below and the interface
//! accessors of the versions in sdk.rs. Those accessors carry the interface
//! version in their name, and with it the layouts Steam fills, so a library
//! of another SDK is refused with a message that says so, and nothing in it
//! is called. What Steam hands back is read into structs whose sizes are
//! checked as this builds, and Steam is told those sizes with each call
//! result, so it refuses a mismatch itself instead of writing past them.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::sdk::{
    self, CallResult, FavoriteChanged, GetVoteResult, MatchingUgcType, Plain, SetVoteResult, StringArray,
    SubscribeResult, UgcDetails, UgcQuery, UgcQueryCompleted, UnsubscribeResult, UserUgcList,
    UserUgcListSortOrder,
};
use super::{APP, Entry, Error, ItemState, Page, Persona, Progress, Query, Sort, Vote, Workshop};
use crate::lang::t;
use crate::tf;

/// `RTLD_NOW` of <dlfcn.h>: every symbol bound as it loads, so a missing one
/// fails here and not in the middle of a call.
const RTLD_NOW: i32 = 2;
/// How long a call result is waited for.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);
/// How often, while waiting, Steam is looked for.
const RUNNING_CHECK: Duration = Duration::from_secs(1);

unsafe extern "C" {
    fn dlopen(file: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

type Iface = *mut c_void;

// Every function we call, with the flat API's name and signature
// (steam_api_flat.h of SDK 1.64). C `bool` results and out-parameters are
// bytes: a C library may hand back any byte there, and a Rust bool that is
// not 0 or 1 is undefined behaviour.
macro_rules! api {
    ($($field:ident: $name:expr => fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        struct Api {
            $($field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        /// What a library must export to be loaded at all.
        const REQUIRED: &[&str] = &[$($name),*];

        impl Api {
            /// Every function, or the names of the ones the library lacks.
            fn load(lib: *mut c_void) -> Result<Self, Vec<&'static str>> {
                let mut missing = Vec::new();
                $(
                    let $field = symbol(lib, $name);
                    if $field.is_null() {
                        missing.push($name);
                    }
                )*
                if !missing.is_empty() {
                    return Err(missing);
                }
                // SAFETY: each symbol is the flat API function of that name,
                // whose signature is the one written beside it.
                Ok(unsafe {
                    Self {
                        $($field: std::mem::transmute::<
                            *mut c_void,
                            unsafe extern "C" fn($($arg),*) $(-> $ret)?,
                        >($field),)*
                    }
                })
            }
        }
    };
}

api! {
    init: "SteamAPI_InitFlat" => fn(*mut c_char) -> i32;
    run_callbacks: "SteamAPI_RunCallbacks" => fn();
    shutdown: "SteamAPI_Shutdown" => fn();
    is_running: "SteamAPI_IsSteamRunning" => fn() -> u8;
    ugc_accessor: sdk::UGC_ACCESSOR => fn() -> Iface;
    utils_accessor: sdk::UTILS_ACCESSOR => fn() -> Iface;

    subscribe: "SteamAPI_ISteamUGC_SubscribeItem" => fn(Iface, u64) -> u64;
    unsubscribe: "SteamAPI_ISteamUGC_UnsubscribeItem" => fn(Iface, u64) -> u64;
    download: "SteamAPI_ISteamUGC_DownloadItem" => fn(Iface, u64, bool) -> u8;
    state: "SteamAPI_ISteamUGC_GetItemState" => fn(Iface, u64) -> u32;
    count: "SteamAPI_ISteamUGC_GetNumSubscribedItems" => fn(Iface, bool) -> u32;
    items: "SteamAPI_ISteamUGC_GetSubscribedItems" => fn(Iface, *mut u64, u32, bool) -> u32;
    download_info: "SteamAPI_ISteamUGC_GetItemDownloadInfo" => fn(Iface, u64, *mut u64, *mut u64) -> u8;
    create_query: "SteamAPI_ISteamUGC_CreateQueryAllUGCRequestPage" => fn(Iface, i32, i32, u32, u32, u32) -> u64;
    user_query: "SteamAPI_ISteamUGC_CreateQueryUserUGCRequest" => fn(Iface, u32, i32, i32, i32, u32, u32, u32) -> u64;
    details_query: "SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest" => fn(Iface, *mut u64, u32) -> u64;
    required_tag: "SteamAPI_ISteamUGC_AddRequiredTag" => fn(Iface, u64, *const c_char) -> u8;
    excluded_tag: "SteamAPI_ISteamUGC_AddExcludedTag" => fn(Iface, u64, *const c_char) -> u8;
    tag_group: "SteamAPI_ISteamUGC_AddRequiredTagGroup" => fn(Iface, u64, *const StringArray) -> u8;
    search_text: "SteamAPI_ISteamUGC_SetSearchText" => fn(Iface, u64, *const c_char) -> u8;
    trend_days: "SteamAPI_ISteamUGC_SetRankedByTrendDays" => fn(Iface, u64, u32) -> u8;
    send_query: "SteamAPI_ISteamUGC_SendQueryUGCRequest" => fn(Iface, u64) -> u64;
    query_result: "SteamAPI_ISteamUGC_GetQueryUGCResult" => fn(Iface, u64, u32, *mut UgcDetails) -> u8;
    preview_url: "SteamAPI_ISteamUGC_GetQueryUGCPreviewURL" => fn(Iface, u64, u32, *mut c_char, u32) -> u8;
    release_query: "SteamAPI_ISteamUGC_ReleaseQueryUGCRequest" => fn(Iface, u64) -> u8;
    set_vote: "SteamAPI_ISteamUGC_SetUserItemVote" => fn(Iface, u64, bool) -> u64;
    get_vote: "SteamAPI_ISteamUGC_GetUserItemVote" => fn(Iface, u64) -> u64;
    add_favorite: "SteamAPI_ISteamUGC_AddItemToFavorites" => fn(Iface, u32, u64) -> u64;
    remove_favorite: "SteamAPI_ISteamUGC_RemoveItemFromFavorites" => fn(Iface, u32, u64) -> u64;

    completed: "SteamAPI_ISteamUtils_IsAPICallCompleted" => fn(Iface, u64, *mut u8) -> u8;
    failure_reason: "SteamAPI_ISteamUtils_GetAPICallFailureReason" => fn(Iface, u64) -> i32;
    call_result: "SteamAPI_ISteamUtils_GetAPICallResult" => fn(Iface, u64, *mut c_void, i32, i32, *mut u8) -> u8;
    image_size: "SteamAPI_ISteamUtils_GetImageSize" => fn(Iface, i32, *mut u32, *mut u32) -> u8;
    image_rgba: "SteamAPI_ISteamUtils_GetImageRGBA" => fn(Iface, i32, *mut u8, i32) -> u8;
}

/// The user and their friends: what the lists of one's own and the author
/// line need. A library without them still browses.
struct Friends {
    iface: Iface,
    request_user: unsafe extern "C" fn(Iface, u64, bool) -> u8,
    persona_name: unsafe extern "C" fn(Iface, u64) -> *const c_char,
    small_avatar: unsafe extern "C" fn(Iface, u64) -> i32,
}

pub struct Steam {
    api: Api,
    ugc: Iface,
    utils: Iface,
    /// The user's account, for the lists of their own (0 when not known).
    account: u32,
    friends: Option<Friends>,
}

fn symbol(lib: *mut c_void, name: &str) -> *mut c_void {
    let Ok(c) = CString::new(name) else { return std::ptr::null_mut() };
    unsafe { dlsym(lib, c.as_ptr()) }
}

fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
        .join("we-linux")
}

/// `e_machine` of this build's ELF files (elf.h: EM_X86_64, EM_AARCH64).
const fn own_machine() -> u16 {
    if cfg!(target_arch = "aarch64") { 183 } else { 62 }
}

/// Why a libsteam_api.so will not do, from its bytes alone: nothing of it is
/// loaded to find out.
fn inspect(bytes: &[u8]) -> Result<(), String> {
    // e_ident: the magic, EI_CLASS 2 (ELFCLASS64); e_machine at 18.
    if bytes.len() < 20 || bytes[..4] != *b"\x7fELF" || bytes[4] != 2 {
        return Err(t("not a 64-bit library", "это не 64-битная библиотека").into());
    }
    if u16::from_le_bytes([bytes[18], bytes[19]]) != own_machine() {
        return Err(t("built for another processor", "собрана под другой процессор").into());
    }
    // Names in the dynamic string table stand between NULs: "\0name\0" is
    // that name and not one it begins.
    let exports = |name: &str| {
        let needle = [b"\0", name.as_bytes(), b"\0"].concat();
        bytes.windows(needle.len()).any(|w| w == needle)
    };
    if !bytes.windows(b"SteamAPI_".len()).any(|w| w == b"SteamAPI_") {
        return Err(t("it is not Steam's library", "это не библиотека Steam").into());
    }
    if !exports(sdk::UGC_ACCESSOR) {
        let prefix = b"SteamAPI_SteamUGC_v";
        let other = bytes
            .windows(prefix.len() + 3)
            .find(|w| w.starts_with(prefix))
            .map(|w| String::from_utf8_lossy(&w[prefix.len()..]).into_owned());
        let ours = &sdk::UGC_ACCESSOR[prefix.len()..];
        return Err(match other {
            Some(theirs) => tf!(
                "it speaks the workshop interface v{theirs}, this WE Linux knows v{ours}",
                "в ней интерфейс мастерской v{theirs}, а этот WE Linux знает v{ours}"
            ),
            None => tf!(
                "it is older than the workshop interface v{ours}",
                "она старше интерфейса мастерской v{ours}"
            ),
        });
    }
    let missing: Vec<&str> = REQUIRED.iter().copied().filter(|name| !exports(name)).collect();
    if !missing.is_empty() {
        return Err(tf!("it lacks {}", "в ней нет {}", missing.join(", ")));
    }
    Ok(())
}

fn inspect_file(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    inspect(&bytes)
}

/// The libsteam_api.so files of the installed games, a few folders deep.
fn in_games() -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut folders = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == "libsteam_api.so") {
                out.push(path);
            } else if depth > 0 && path.is_dir() {
                folders.push(path);
            }
        }
        for folder in folders {
            walk(&folder, depth - 1, out);
        }
    }
    let mut out = Vec::new();
    for library in crate::library::libraries() {
        walk(&library.join("steamapps/common"), 5, &mut out);
    }
    out
}

/// The libsteam_api.so to load: named by WE_LINUX_STEAM_API, next to the
/// binary, our own copy, else the first usable one of an installed game,
/// copied to our data directory. Each that will not do is said, with why.
fn find_library() -> Result<PathBuf, String> {
    let mut refused: Vec<String> = Vec::new();
    let mut fixed = Vec::new();
    if let Some(p) = std::env::var_os("WE_LINUX_STEAM_API") {
        fixed.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        fixed.push(exe.with_file_name("libsteam_api.so"));
    }
    let own = data_dir().join("libsteam_api.so");
    fixed.push(own.clone());
    for path in fixed.into_iter().filter(|p| p.is_file()) {
        match inspect_file(&path) {
            Ok(()) => return Ok(path),
            Err(why) => refused.push(format!("{}: {why}", path.display())),
        }
    }
    for path in in_games() {
        match inspect_file(&path) {
            Ok(()) => return Ok(keep_copy(&path, &own).unwrap_or(path)),
            Err(why) => refused.push(format!("{}: {why}", path.display())),
        }
    }
    let hint = t(
        "install any game with a native Linux version in Steam (a free one will do)",
        "установи в Steam любую игру с нативной версией под Linux (подойдёт бесплатная)",
    );
    Err(if refused.is_empty() {
        tf!("libsteam_api.so was not found: {hint}", "не найден libsteam_api.so: {hint}")
    } else {
        tf!(
            "no libsteam_api.so this WE Linux can use ({}): {hint}",
            "нет libsteam_api.so, с которым этот WE Linux умеет работать ({}): {hint}",
            refused.join("; ")
        )
    })
}

/// Our own copy of a game's library. Written beside and renamed over: an old
/// copy may be mapped in this process, and writing into it would pull its
/// pages from under the code.
fn keep_copy(found: &Path, own: &Path) -> Option<PathBuf> {
    let dir = own.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    let partial = dir.join("libsteam_api.so.part");
    std::fs::copy(found, &partial).ok()?;
    std::fs::rename(&partial, own).ok()?;
    Some(own.to_path_buf())
}

fn text(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().map(|&c| c as u8).take_while(|&b| b != 0).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// Which of Steam's queries a [`Query`] is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Plan {
    All(UgcQuery),
    User { account: u32, list: UserUgcList, order: UserUgcListSortOrder },
}

pub(super) fn plan(q: &Query, account: u32) -> Result<Plan, Error> {
    if q.author != 0 {
        return Ok(Plan::User {
            account: q.author,
            list: UserUgcList::Published,
            order: UserUgcListSortOrder::CreationOrderDesc,
        });
    }
    let own = |list| {
        if account == 0 {
            Err(Error::NoAccount)
        } else {
            Ok(Plan::User { account, list, order: UserUgcListSortOrder::SubscriptionDateDesc })
        }
    };
    Ok(Plan::All(match q.sort {
        Sort::Favorited => return own(UserUgcList::Favorited),
        Sort::VotedUp => return own(UserUgcList::VotedUp),
        Sort::Vote => UgcQuery::RankedByVote,
        Sort::Newest => UgcQuery::RankedByPublicationDate,
        Sort::Trend => UgcQuery::RankedByTrend,
        Sort::FriendsFavorites => UgcQuery::FavoritedByFriendsRankedByPublicationDate,
        Sort::FriendsCreated => UgcQuery::CreatedByFriendsRankedByPublicationDate,
        Sort::FollowedCreated => UgcQuery::CreatedByFollowedUsersRankedByPublicationDate,
        Sort::UpVotes => UgcQuery::RankedByVotesUp,
        Sort::Subscriptions => UgcQuery::RankedByTotalUniqueSubscriptions,
        Sort::Updated => UgcQuery::RankedByLastUpdatedDate,
    }))
}

/// An item of a page as the window has it.
fn entry_of(d: &UgcDetails, preview_url: String) -> Entry {
    // Copied out of the packed struct before use: a reference into it may
    // not be aligned.
    let (file_size, total_files_size) = (d.file_size, d.total_files_size);
    let tags = text(&{ d.tags });
    Entry {
        id: d.published_file_id,
        title: text(&{ d.title }),
        tags: tags.split(',').filter(|t| !t.is_empty()).map(str::to_owned).collect(),
        // The size of all its files; legacy items have the one file's only.
        size: if total_files_size > 0 { total_files_size } else { file_size.max(0) as u64 },
        up: d.votes_up,
        down: d.votes_down,
        preview_url,
        updated: d.time_updated,
        owner: d.steam_id_owner,
    }
}

/// The account of a Steam id: its low 32 bits (`CSteamID`, steamclientpublic.h).
fn account_of(steam_id: u64) -> u32 {
    (steam_id & u64::from(u32::MAX)) as u32
}

impl Steam {
    /// A session with the running Steam client.
    pub fn open() -> Result<Self, Error> {
        let path = find_library().map_err(Error::Unavailable)?;
        let c =
            CString::new(path.to_string_lossy().as_bytes()).map_err(|e| Error::Unavailable(e.to_string()))?;
        let lib = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            let why = unsafe { dlerror() };
            let why = if why.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(why) }.to_string_lossy().into_owned()
            };
            return Err(Error::Unavailable(tf!(
                "{} did not load: {why}",
                "{} не загрузился: {why}",
                path.display()
            )));
        }
        let api = Api::load(lib).map_err(|missing| {
            Error::Unavailable(tf!("{} lacks {}", "в {} нет {}", path.display(), missing.join(", ")))
        })?;
        let mut message = [0 as c_char; sdk::ERR_MSG_LEN];
        let started = unsafe { (api.init)(message.as_mut_ptr()) };
        if started != sdk::init_result::OK {
            let why = text(&message);
            return Err(Error::Unavailable(match started {
                sdk::init_result::NO_STEAM_CLIENT => t("Steam is not running", "Steam не запущен").to_owned(),
                sdk::init_result::VERSION_MISMATCH => tf!(
                    "the Steam client is older than {}: update Steam",
                    "клиент Steam старше, чем {}: обнови Steam",
                    path.display()
                ),
                sdk::init_result::FAILED_GENERIC => {
                    tf!("Steam does not answer: {why}", "Steam не отвечает: {why}")
                }
                _ => tf!("Steam does not answer: {why}", "Steam не отвечает: {why}"),
            }));
        }
        let (ugc, utils) = unsafe { ((api.ugc_accessor)(), (api.utils_accessor)()) };
        if ugc.is_null() || utils.is_null() {
            unsafe { (api.shutdown)() };
            return Err(Error::Unavailable(
                t(
                    "Steam did not give the workshop's interface: update Steam",
                    "Steam не дал интерфейс мастерской: обнови Steam",
                )
                .into(),
            ));
        }
        let account = unsafe { own_account(lib) };
        let friends = unsafe { friends(lib) };
        Ok(Self { api, ugc, utils, account, friends })
    }

    /// Pump callbacks until `call` is answered.
    fn wait(&self, call: u64) -> Result<(), Error> {
        if call == sdk::API_CALL_INVALID {
            return Err(Error::Refused(sdk::result::INVALID_PARAM));
        }
        let start = Instant::now();
        let mut looked = start;
        while start.elapsed() < ANSWER_TIMEOUT {
            let mut failed = 0u8;
            unsafe { (self.api.run_callbacks)() };
            if unsafe { (self.api.completed)(self.utils, call, &mut failed) } != 0 {
                return if failed != 0 { Err(self.failure(call)) } else { Ok(()) };
            }
            if looked.elapsed() >= RUNNING_CHECK {
                looked = Instant::now();
                if !self.running() {
                    return Err(Error::Gone);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(Error::NoAnswer)
    }

    fn failure(&self, call: u64) -> Error {
        match unsafe { (self.api.failure_reason)(self.utils, call) } {
            sdk::call_failure::STEAM_GONE => Error::Gone,
            sdk::call_failure::NETWORK_FAILURE => Error::Refused(sdk::result::NO_CONNECTION),
            sdk::call_failure::MISMATCHED_CALLBACK => Error::Mismatch,
            sdk::call_failure::NONE | sdk::call_failure::INVALID_HANDLE => Error::NoAnswer,
            _ => Error::NoAnswer,
        }
    }

    /// The answer to `call`, read as `T`. Steam is told the size we have
    /// for it and refuses one it does not match.
    fn answer<T: CallResult>(&self, call: u64) -> Result<T, Error> {
        self.wait(call)?;
        let mut out = T::zeroed();
        let mut failed = 0u8;
        let ok = unsafe {
            (self.api.call_result)(
                self.utils,
                call,
                (&mut out as *mut T).cast(),
                size_of::<T>() as i32,
                T::ID,
                &mut failed,
            )
        };
        if ok == 0 || failed != 0 {
            return Err(self.failure(call));
        }
        match out.result() {
            sdk::result::OK => Ok(out),
            code => Err(Error::Refused(code)),
        }
    }

    /// Send a query made ready and read its page; the query is let go of either way.
    fn run(&self, handle: u64) -> Result<Page, Error> {
        let result = self.read_page(handle);
        unsafe { (self.api.release_query)(self.ugc, handle) };
        result
    }

    fn read_page(&self, handle: u64) -> Result<Page, Error> {
        let call = unsafe { (self.api.send_query)(self.ugc, handle) };
        let done: UgcQueryCompleted = self.answer(call)?;
        let (returned, total) = (done.returned, done.total);
        let mut entries = Vec::with_capacity(returned as usize);
        for index in 0..returned {
            let mut details = UgcDetails::zeroed();
            if unsafe { (self.api.query_result)(self.ugc, handle, index, &mut details) } == 0 {
                continue;
            }
            let mut url = [0 as c_char; sdk::PREVIEW_URL_BUFFER];
            unsafe { (self.api.preview_url)(self.ugc, handle, index, url.as_mut_ptr(), url.len() as u32) };
            entries.push(entry_of(&details, text(&url)));
        }
        Ok(Page { entries, total })
    }
}

/// The signed-in user's account, when the library can say.
unsafe fn own_account(lib: *mut c_void) -> u32 {
    let (user, id) = (symbol(lib, sdk::USER_ACCESSOR), symbol(lib, "SteamAPI_ISteamUser_GetSteamID"));
    if user.is_null() || id.is_null() {
        return 0;
    }
    unsafe {
        let user = std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> Iface>(user)();
        let id = std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Iface) -> u64>(id);
        if user.is_null() { 0 } else { account_of(id(user)) }
    }
}

unsafe fn friends(lib: *mut c_void) -> Option<Friends> {
    let accessor = sdk::FRIENDS_ACCESSORS.iter().map(|name| symbol(lib, name)).find(|p| !p.is_null())?;
    let request = symbol(lib, "SteamAPI_ISteamFriends_RequestUserInformation");
    let name = symbol(lib, "SteamAPI_ISteamFriends_GetFriendPersonaName");
    let avatar = symbol(lib, "SteamAPI_ISteamFriends_GetSmallFriendAvatar");
    if request.is_null() || name.is_null() || avatar.is_null() {
        return None;
    }
    unsafe {
        let iface = std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> Iface>(accessor)();
        (!iface.is_null()).then(|| Friends {
            iface,
            request_user: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Iface, u64, bool) -> u8>(
                request,
            ),
            persona_name: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Iface, u64) -> *const c_char>(
                name,
            ),
            small_avatar: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Iface, u64) -> i32>(avatar),
        })
    }
}

impl Workshop for Steam {
    fn pump(&mut self) {
        unsafe { (self.api.run_callbacks)() }
    }

    fn running(&self) -> bool {
        unsafe { (self.api.is_running)() != 0 }
    }

    fn query(&mut self, q: &Query) -> Result<Page, Error> {
        let page = q.page.max(1);
        let (ready, api) = (MatchingUgcType::ItemsReadyToUse as i32, &self.api);
        let handle = unsafe {
            match plan(q, self.account)? {
                Plan::All(order) => (api.create_query)(self.ugc, order as i32, ready, APP, APP, page),
                Plan::User { account, list, order } => {
                    (api.user_query)(self.ugc, account, list as i32, ready, order as i32, APP, APP, page)
                }
            }
        };
        if handle == sdk::QUERY_HANDLE_INVALID {
            return Err(Error::Refused(sdk::result::INVALID_PARAM));
        }
        unsafe {
            for tag in &q.required {
                (api.required_tag)(self.ugc, handle, cstr(tag).as_ptr());
            }
            for tag in &q.excluded {
                (api.excluded_tag)(self.ugc, handle, cstr(tag).as_ptr());
            }
            for group in &q.groups {
                let owned: Vec<CString> = group.iter().map(|t| cstr(t)).collect();
                let pointers: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
                let array = StringArray { strings: pointers.as_ptr(), count: pointers.len() as i32 };
                (api.tag_group)(self.ugc, handle, &array);
            }
            if !q.text.trim().is_empty() {
                (api.search_text)(self.ugc, handle, cstr(q.text.trim()).as_ptr());
            }
            if q.sort == Sort::Trend {
                (api.trend_days)(self.ugc, handle, q.trend_days.clamp(1, 365));
            }
        }
        self.run(handle)
    }

    fn details(&mut self, ids: &[u64]) -> Result<Page, Error> {
        let mut ids = ids.to_vec();
        let handle = unsafe { (self.api.details_query)(self.ugc, ids.as_mut_ptr(), ids.len() as u32) };
        if handle == sdk::QUERY_HANDLE_INVALID {
            return Err(Error::Refused(sdk::result::INVALID_PARAM));
        }
        self.run(handle)
    }

    fn subscribe(&mut self, id: u64) -> Result<(), Error> {
        let call = unsafe { (self.api.subscribe)(self.ugc, id) };
        self.answer::<SubscribeResult>(call).map(drop)
    }

    fn unsubscribe(&mut self, id: u64) -> Result<(), Error> {
        let call = unsafe { (self.api.unsubscribe)(self.ugc, id) };
        self.answer::<UnsubscribeResult>(call).map(drop)
    }

    fn download(&mut self, id: u64) -> bool {
        unsafe { (self.api.download)(self.ugc, id, true) != 0 }
    }

    fn subscribed(&self) -> Vec<(u64, ItemState)> {
        unsafe {
            let n = (self.api.count)(self.ugc, false);
            let mut ids = vec![0u64; n as usize];
            let got = (self.api.items)(self.ugc, ids.as_mut_ptr(), n, false) as usize;
            ids.truncate(got);
            ids.into_iter().map(|id| (id, ItemState((self.api.state)(self.ugc, id)))).collect()
        }
    }

    fn progress(&self, id: u64) -> Progress {
        let (mut done, mut total) = (0u64, 0u64);
        unsafe {
            let state = ItemState((self.api.state)(self.ugc, id));
            if (self.api.download_info)(self.ugc, id, &mut done, &mut total) == 0 {
                (done, total) = (0, 0);
            }
            Progress { state, done, total }
        }
    }

    fn vote(&mut self, id: u64) -> Result<Vote, Error> {
        let call = unsafe { (self.api.get_vote)(self.ugc, id) };
        let answer: GetVoteResult = self.answer(call)?;
        Ok(if answer.voted_up != 0 {
            Vote::Up
        } else if answer.voted_down != 0 {
            Vote::Down
        } else {
            Vote::None
        })
    }

    fn set_vote(&mut self, id: u64, up: bool) -> Result<(), Error> {
        let call = unsafe { (self.api.set_vote)(self.ugc, id, up) };
        self.answer::<SetVoteResult>(call).map(drop)
    }

    fn set_favorite(&mut self, id: u64, on: bool) -> Result<(), Error> {
        let call = unsafe {
            if on {
                (self.api.add_favorite)(self.ugc, APP, id)
            } else {
                (self.api.remove_favorite)(self.ugc, APP, id)
            }
        };
        self.answer::<FavoriteChanged>(call).map(drop)
    }

    fn persona(&mut self, id: u64) -> Option<Persona> {
        let friends = self.friends.as_ref().filter(|_| id != 0)?;
        unsafe {
            // True: Steam has to fetch it first.
            if (friends.request_user)(friends.iface, id, false) != 0 {
                return None;
            }
            let name = (friends.persona_name)(friends.iface, id);
            if name.is_null() {
                return None;
            }
            let name = CStr::from_ptr(name).to_string_lossy().into_owned();
            if name.is_empty() || name == "[unknown]" {
                return None;
            }
            let image = (friends.small_avatar)(friends.iface, id);
            let (mut w, mut h) = (0u32, 0u32);
            // A handle of 0 is no avatar (isteamfriends.h); a sane one is 32x32.
            let avatar = (image > 0
                && (self.api.image_size)(self.utils, image, &mut w, &mut h) != 0
                && (1..=256).contains(&w)
                && (1..=256).contains(&h))
            .then(|| {
                let mut rgba = vec![0u8; (w * h * 4) as usize];
                ((self.api.image_rgba)(self.utils, image, rgba.as_mut_ptr(), rgba.len() as i32) != 0)
                    .then_some((w, h, rgba))
            })
            .flatten();
            Some(Persona { name, avatar })
        }
    }
}

impl Drop for Steam {
    fn drop(&mut self) {
        unsafe { (self.api.shutdown)() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(sort: Sort) -> Query {
        Query {
            sort,
            text: String::new(),
            page: 1,
            groups: Vec::new(),
            required: Vec::new(),
            excluded: Vec::new(),
            trend_days: 7,
            author: 0,
        }
    }

    /// The bytes of a small library: an ELF header of this machine and a
    /// string table with `names`.
    fn library(names: &[&str]) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[18..20].copy_from_slice(&own_machine().to_le_bytes());
        for name in names {
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn a_library_with_every_function_is_taken() {
        assert_eq!(inspect(&library(REQUIRED)), Ok(()));
    }

    #[test]
    fn a_library_of_another_workshop_interface_is_refused_by_name() {
        let names: Vec<&str> = REQUIRED
            .iter()
            .map(|n| if *n == sdk::UGC_ACCESSOR { "SteamAPI_SteamUGC_v022" } else { n })
            .collect();
        let why = inspect(&library(&names)).unwrap_err();
        assert!(why.contains("v022") && why.contains("v021"), "{why}");
    }

    #[test]
    fn a_library_without_a_function_says_which() {
        let names: Vec<&str> =
            REQUIRED.iter().copied().filter(|n| *n != "SteamAPI_ISteamUGC_AddRequiredTagGroup").collect();
        let why = inspect(&library(&names)).unwrap_err();
        assert!(why.contains("SteamAPI_ISteamUGC_AddRequiredTagGroup"), "{why}");
    }

    #[test]
    fn a_name_that_only_begins_like_ours_is_not_ours() {
        let names: Vec<&str> = REQUIRED
            .iter()
            .map(|n| if *n == sdk::UGC_ACCESSOR { "SteamAPI_SteamUGC_v0210" } else { n })
            .collect();
        assert!(inspect(&library(&names)).is_err());
    }

    #[test]
    fn a_library_for_another_processor_or_word_size_is_refused() {
        let mut other = library(REQUIRED);
        other[18..20].copy_from_slice(&(own_machine() + 1).to_le_bytes());
        assert!(inspect(&other).is_err());
        let mut narrow = library(REQUIRED);
        narrow[4] = 1;
        assert!(inspect(&narrow).is_err());
        assert!(inspect(b"#!/bin/sh").is_err());
    }

    #[test]
    fn another_library_is_not_taken_for_steams() {
        let why = inspect(&library(&["malloc", "free"])).unwrap_err();
        assert!(why.contains("not Steam's") || why.contains("не библиотека Steam"), "{why}");
    }

    #[test]
    fn the_users_lists_are_asked_for_by_their_own_names() {
        let own =
            |list| Ok(Plan::User { account: 7, list, order: UserUgcListSortOrder::SubscriptionDateDesc });
        assert_eq!(plan(&query(Sort::Favorited), 7), own(UserUgcList::Favorited));
        assert_eq!(plan(&query(Sort::VotedUp), 7), own(UserUgcList::VotedUp));
        assert_eq!(plan(&query(Sort::Favorited), 0), Err(Error::NoAccount));
    }

    #[test]
    fn an_authors_wallpapers_are_their_published_list_newest_first() {
        let q = Query { author: 42, ..query(Sort::Trend) };
        assert_eq!(
            plan(&q, 7),
            Ok(Plan::User {
                account: 42,
                list: UserUgcList::Published,
                order: UserUgcListSortOrder::CreationOrderDesc
            })
        );
        assert_eq!(plan(&query(Sort::Trend), 7), Ok(Plan::All(UgcQuery::RankedByTrend)));
        assert_eq!(plan(&query(Sort::Updated), 0), Ok(Plan::All(UgcQuery::RankedByLastUpdatedDate)));
    }

    #[test]
    fn an_item_is_read_from_steams_struct() {
        let mut d = UgcDetails::zeroed();
        d.published_file_id = 123;
        d.title[..3].copy_from_slice(&[b'S' as c_char, b'e' as c_char, b'a' as c_char]);
        let tags = b"Scene,Anime,,Everyone";
        for (i, b) in tags.iter().enumerate() {
            d.tags[i] = *b as c_char;
        }
        d.steam_id_owner = 76561197960287930;
        d.votes_up = 5;
        d.votes_down = 1;
        d.file_size = 1000;
        let legacy = entry_of(&d, "https://preview".into());
        assert_eq!(legacy.id, 123);
        assert_eq!(legacy.title, "Sea");
        assert_eq!(legacy.tags, ["Scene", "Anime", "Everyone"]);
        assert_eq!(legacy.size, 1000);
        assert_eq!((legacy.up, legacy.down), (5, 1));
        assert_eq!(legacy.owner, 76561197960287930);
        // An item of several files is as big as all of them, past 4 GB too.
        d.total_files_size = 5 << 30;
        assert_eq!(entry_of(&d, String::new()).size, 5 << 30);
    }

    #[test]
    fn an_account_is_the_low_half_of_a_steam_id() {
        assert_eq!(account_of(76561197960287930), 22202);
    }

    /// Against the Steam running here, reading only: nothing is subscribed
    /// to, voted on or downloaded. `cargo test live -- --ignored --test-threads=1`
    #[test]
    #[ignore = "needs Steam running and signed in"]
    fn live_steam_is_read_as_this_binding_expects() {
        super::super::init_env();
        let mut steam = Steam::open().expect("a session");
        assert!(steam.running());
        let mut q = query(Sort::Vote);
        q.groups = vec![vec!["Scene".into()], vec!["Everyone".into()]];
        let first = steam.query(&q).expect("a page");
        assert_eq!(first.entries.len(), super::super::PAGE_SIZE as usize);
        assert!(first.total > 1000, "{}", first.total);
        let e = &first.entries[0];
        assert!(e.id > 0 && !e.title.is_empty() && e.tags.iter().any(|t| t == "Scene"), "{e:?}");
        assert!(e.preview_url.starts_with("https://"), "{}", e.preview_url);
        assert!(e.up > e.down && e.owner >> 32 != 0 && e.size > 0, "{e:?}");
        q.page = 2;
        let second = steam.query(&q).expect("page 2");
        assert!(second.entries.iter().all(|s| first.entries.iter().all(|f| f.id != s.id)));
        let details = steam.details(&[e.id]).expect("details");
        assert_eq!(details.entries.first().map(|d| (d.id, d.title.as_str())), Some((e.id, e.title.as_str())));
        let _ = steam.vote(e.id).expect("the user's vote on it");
        let subscribed = steam.subscribed();
        if let Some(&(id, state)) = subscribed.first() {
            assert!(state.subscribed());
            assert_eq!(steam.progress(id).state, state);
        }
        let own: Vec<u32> = [Sort::Favorited, Sort::VotedUp]
            .into_iter()
            .map(|sort| steam.query(&query(sort)).expect("one's own list").total)
            .collect();
        let mut persona = None;
        for _ in 0..100 {
            steam.pump();
            persona = steam.persona(e.owner);
            if persona.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let persona = persona.expect("the author's name");
        assert!(!persona.name.is_empty());
        eprintln!(
            "live: {} items, first {} \"{}\" by {}, {} subscribed, {} favourites, {} voted up",
            first.total,
            e.id,
            e.title,
            persona.name,
            subscribed.len(),
            own[0],
            own[1]
        );
    }
}
