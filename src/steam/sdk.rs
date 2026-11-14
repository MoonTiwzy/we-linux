//! What the Steamworks SDK says, by its own names: the interface versions we
//! were written against, the ids of the call results we read, the enums we
//! pass and receive, and the layout of every struct Steam fills for us.
//!
//! All of it is from the headers of Steamworks SDK 1.64 (`isteamugc.h`,
//! `isteamutils.h`, `isteamremotestorage.h`, `steamclientpublic.h`,
//! `steam_api.h`, `steam_api_flat.h`). The layouts are checked when this file
//! compiles, against the sizes and offsets the SDK's own headers give (as
//! bindgen measured them in steamworks-sys 0.13, which is built from the same
//! headers): a field moved by hand here does not build.
//!
//! Valve versions an interface as a whole: a method or a struct it hands back
//! changes, and the interface gets a new version, with a new flat accessor
//! (`SteamAPI_SteamUGC_v022`). A library that exports the accessor of the
//! version below has the very functions and layouts this file describes;
//! one that does not is refused before anything is called (see flat.rs).

use std::ffi::c_char;
use std::mem::{offset_of, size_of};

/// The accessors of the interfaces we use, as the flat API names them. Each
/// is the interface version of SDK 1.64 (`STEAMUGC_INTERFACE_VERSION021`,
/// `SteamUtils010`, `SteamUser023`, `SteamFriends018`).
pub const UGC_ACCESSOR: &str = "SteamAPI_SteamUGC_v021";
pub const UTILS_ACCESSOR: &str = "SteamAPI_SteamUtils_v010";
pub const USER_ACCESSOR: &str = "SteamAPI_SteamUser_v023";
/// Friends 017 (SDK 1.57 and before) has the same three calls we make of it.
pub const FRIENDS_ACCESSORS: [&str; 2] = ["SteamAPI_SteamFriends_v018", "SteamAPI_SteamFriends_v017"];

/// `ESteamAPIInitResult`, what `SteamAPI_InitFlat` returns (steam_api.h).
pub mod init_result {
    pub const OK: i32 = 0;
    pub const NO_STEAM_CLIENT: i32 = 2;
    pub const VERSION_MISMATCH: i32 = 3;
}

/// `SteamErrMsg`: the buffer `SteamAPI_InitFlat` writes why into (steam_api.h).
pub const ERR_MSG_LEN: usize = 1024;

/// `k_UGCQueryHandleInvalid` (isteamugc.h).
pub const QUERY_HANDLE_INVALID: u64 = u64::MAX;
/// `k_uAPICallInvalid` (steamclientpublic.h).
pub const API_CALL_INVALID: u64 = 0;
/// `k_cchPublishedDocumentTitleMax` and friends (isteamremotestorage.h).
pub const TITLE_MAX: usize = 128 + 1;
pub const DESCRIPTION_MAX: usize = 8000;
pub const TAG_LIST_MAX: usize = 1024 + 1;
pub const FILENAME_MAX: usize = 260;
pub const URL_MAX: usize = 256;
/// The buffer `GetQueryUGCPreviewURL` is given: a preview URL is shorter by far.
pub const PREVIEW_URL_BUFFER: usize = 1024;

/// `EUGCQuery` (isteamugc.h): the orders of a query over all items. Only
/// passed to Steam, never read back, so an enum is safe here.
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UgcQuery {
    RankedByVote = 0,
    RankedByPublicationDate = 1,
    RankedByTrend = 3,
    FavoritedByFriendsRankedByPublicationDate = 4,
    CreatedByFriendsRankedByPublicationDate = 5,
    CreatedByFollowedUsersRankedByPublicationDate = 7,
    RankedByVotesUp = 10,
    RankedByTotalUniqueSubscriptions = 12,
    RankedByLastUpdatedDate = 19,
}

/// `EUserUGCList` (isteamugc.h): one user's lists.
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserUgcList {
    Published = 0,
    VotedUp = 2,
    Favorited = 5,
}

/// `EUserUGCListSortOrder` (isteamugc.h).
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserUgcListSortOrder {
    CreationOrderDesc = 0,
    SubscriptionDateDesc = 4,
}

/// `EUGCMatchingUGCType` (isteamugc.h): wallpapers are ready-to-use items.
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MatchingUgcType {
    ItemsReadyToUse = 2,
}

/// `EItemState` (isteamugc.h), bits. Read from Steam, so a plain number:
/// a bit added by a later Steam is carried along, not undefined behaviour.
pub mod item_state {
    pub const SUBSCRIBED: u32 = 1;
    pub const INSTALLED: u32 = 4;
    pub const DOWNLOADING: u32 = 16;
    pub const DOWNLOAD_PENDING: u32 = 32;
}

/// `ESteamAPICallFailure` (isteamutils.h): why a call result did not come.
pub mod call_failure {
    pub const STEAM_GONE: i32 = 0;
    pub const NETWORK_FAILURE: i32 = 1;
    pub const MISMATCHED_CALLBACK: i32 = 3;
}

/// `EResult` (steamclientpublic.h): the values we explain in words. Any
/// other is shown by its number.
pub mod result {
    pub const OK: i32 = 1;
    pub const NO_CONNECTION: i32 = 3;
    pub const INVALID_PARAM: i32 = 8;
    pub const FILE_NOT_FOUND: i32 = 9;
    pub const BUSY: i32 = 10;
    pub const ACCESS_DENIED: i32 = 15;
    pub const TIMEOUT: i32 = 16;
    pub const SERVICE_UNAVAILABLE: i32 = 20;
    pub const NOT_LOGGED_ON: i32 = 21;
    pub const LIMIT_EXCEEDED: i32 = 25;
    pub const DUPLICATE_REQUEST: i32 = 29;
    pub const NO_MATCH: i32 = 42;
    pub const RATE_LIMIT_EXCEEDED: i32 = 84;
}

/// The callback groups of steam_api_internal.h: a call result's id is its
/// group plus its place in it.
const STEAM_REMOTE_STORAGE_CALLBACKS: i32 = 1300;
const STEAM_UGC_CALLBACKS: i32 = 3400;

/// Plain data Steam writes into: integers, bytes and arrays of them, so any
/// bit pattern is a value and a zeroed one is a valid start. Bools are bytes
/// here for that reason: Steam writing a 2 into a Rust `bool` would be
/// undefined behaviour.
///
/// # Safety
/// Only for `repr(C)` structs of integer fields.
pub unsafe trait Plain: Copy {
    fn zeroed() -> Self {
        // SAFETY: the implementor promises every bit pattern, zero among them, is valid.
        unsafe { std::mem::zeroed() }
    }
}

/// A call result: the struct `GetAPICallResult` fills, and its id.
pub trait CallResult: Plain {
    /// `k_iCallback` of the struct.
    const ID: i32;
    /// Its `m_eResult`.
    fn result(&self) -> i32;
}

/// `SteamParamStringArray_t` (steamclientpublic.h).
#[repr(C, packed(4))]
pub struct StringArray {
    pub strings: *const *const c_char,
    pub count: i32,
}

/// `SteamUGCDetails_t` (isteamugc.h): one item of a query's page.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct UgcDetails {
    pub published_file_id: u64,
    pub result: i32,
    pub file_type: i32,
    pub creator_app_id: u32,
    pub consumer_app_id: u32,
    pub title: [c_char; TITLE_MAX],
    pub description: [c_char; DESCRIPTION_MAX],
    pub steam_id_owner: u64,
    pub time_created: u32,
    pub time_updated: u32,
    pub time_added_to_user_list: u32,
    pub visibility: i32,
    pub banned: u8,
    pub accepted_for_use: u8,
    pub tags_truncated: u8,
    pub tags: [c_char; TAG_LIST_MAX],
    pub file: u64,
    pub preview_file: u64,
    pub file_name: [c_char; FILENAME_MAX],
    pub file_size: i32,
    pub preview_file_size: i32,
    pub url: [c_char; URL_MAX],
    pub votes_up: u32,
    pub votes_down: u32,
    pub score: f32,
    pub num_children: u32,
    pub total_files_size: u64,
}
unsafe impl Plain for UgcDetails {}

/// `SteamUGCQueryCompleted_t` (isteamugc.h).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct UgcQueryCompleted {
    pub handle: u64,
    pub result: i32,
    pub returned: u32,
    pub total: u32,
    pub cached: u8,
    pub next_cursor: [c_char; URL_MAX],
}
unsafe impl Plain for UgcQueryCompleted {}
impl CallResult for UgcQueryCompleted {
    const ID: i32 = STEAM_UGC_CALLBACKS + 1;
    fn result(&self) -> i32 {
        self.result
    }
}

/// `RemoteStorageSubscribePublishedFileResult_t` (isteamremotestorage.h):
/// what `SubscribeItem` answers.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct SubscribeResult {
    pub result: i32,
    pub published_file_id: u64,
}
unsafe impl Plain for SubscribeResult {}
impl CallResult for SubscribeResult {
    const ID: i32 = STEAM_REMOTE_STORAGE_CALLBACKS + 13;
    fn result(&self) -> i32 {
        self.result
    }
}

/// `RemoteStorageUnsubscribePublishedFileResult_t` (isteamremotestorage.h).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct UnsubscribeResult {
    pub result: i32,
    pub published_file_id: u64,
}
unsafe impl Plain for UnsubscribeResult {}
impl CallResult for UnsubscribeResult {
    const ID: i32 = STEAM_REMOTE_STORAGE_CALLBACKS + 15;
    fn result(&self) -> i32 {
        self.result
    }
}

/// `UserFavoriteItemsListChanged_t` (isteamugc.h).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct FavoriteChanged {
    pub published_file_id: u64,
    pub result: i32,
    pub was_add_request: u8,
}
unsafe impl Plain for FavoriteChanged {}
impl CallResult for FavoriteChanged {
    const ID: i32 = STEAM_UGC_CALLBACKS + 7;
    fn result(&self) -> i32 {
        self.result
    }
}

/// `SetUserItemVoteResult_t` (isteamugc.h).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct SetVoteResult {
    pub published_file_id: u64,
    pub result: i32,
    pub vote_up: u8,
}
unsafe impl Plain for SetVoteResult {}
impl CallResult for SetVoteResult {
    const ID: i32 = STEAM_UGC_CALLBACKS + 8;
    fn result(&self) -> i32 {
        self.result
    }
}

/// `GetUserItemVoteResult_t` (isteamugc.h).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct GetVoteResult {
    pub published_file_id: u64,
    pub result: i32,
    pub voted_up: u8,
    pub voted_down: u8,
    pub vote_skipped: u8,
}
unsafe impl Plain for GetVoteResult {}
impl CallResult for GetVoteResult {
    const ID: i32 = STEAM_UGC_CALLBACKS + 9;
    fn result(&self) -> i32 {
        self.result
    }
}

// The SDK's sizes and offsets (callback structs are `#pragma pack(4)` on
// Linux, VALVE_CALLBACK_PACK_SMALL in steamclientpublic.h).
const _: () = {
    assert!(size_of::<StringArray>() == 12);
    assert!(offset_of!(StringArray, count) == 8);

    assert!(size_of::<UgcDetails>() == 9772);
    assert!(offset_of!(UgcDetails, result) == 8);
    assert!(offset_of!(UgcDetails, title) == 24);
    assert!(offset_of!(UgcDetails, description) == 153);
    assert!(offset_of!(UgcDetails, steam_id_owner) == 8156);
    assert!(offset_of!(UgcDetails, time_updated) == 8168);
    assert!(offset_of!(UgcDetails, visibility) == 8176);
    assert!(offset_of!(UgcDetails, tags) == 8183);
    assert!(offset_of!(UgcDetails, file) == 9208);
    assert!(offset_of!(UgcDetails, file_size) == 9484);
    assert!(offset_of!(UgcDetails, url) == 9492);
    assert!(offset_of!(UgcDetails, votes_up) == 9748);
    assert!(offset_of!(UgcDetails, votes_down) == 9752);
    assert!(offset_of!(UgcDetails, total_files_size) == 9764);

    assert!(size_of::<UgcQueryCompleted>() == 280);
    assert!(offset_of!(UgcQueryCompleted, result) == 8);
    assert!(offset_of!(UgcQueryCompleted, returned) == 12);
    assert!(offset_of!(UgcQueryCompleted, total) == 16);
    assert!(offset_of!(UgcQueryCompleted, next_cursor) == 21);

    assert!(size_of::<SubscribeResult>() == 12);
    assert!(offset_of!(SubscribeResult, published_file_id) == 4);
    assert!(size_of::<UnsubscribeResult>() == 12);
    assert!(offset_of!(UnsubscribeResult, published_file_id) == 4);

    assert!(size_of::<FavoriteChanged>() == 16);
    assert!(offset_of!(FavoriteChanged, was_add_request) == 12);
    assert!(size_of::<SetVoteResult>() == 16);
    assert!(offset_of!(SetVoteResult, vote_up) == 12);
    assert!(size_of::<GetVoteResult>() == 16);
    assert!(offset_of!(GetVoteResult, voted_up) == 12);
    assert!(offset_of!(GetVoteResult, voted_down) == 13);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_result_ids_are_the_sdks() {
        // The ids as the SDK's headers spell them out, k_iSteamUGCCallbacks + n.
        assert_eq!(UgcQueryCompleted::ID, 3401);
        assert_eq!(FavoriteChanged::ID, 3407);
        assert_eq!(SetVoteResult::ID, 3408);
        assert_eq!(GetVoteResult::ID, 3409);
        assert_eq!(SubscribeResult::ID, 1313);
        assert_eq!(UnsubscribeResult::ID, 1315);
    }

    #[test]
    fn a_users_lists_are_the_ones_asked_for() {
        // k_EUserUGCList_* counted from Published = 0 in isteamugc.h: the
        // favourites are the sixth, what was voted up the third.
        assert_eq!(UserUgcList::Favorited as i32, 5);
        assert_eq!(UserUgcList::VotedUp as i32, 2);
        assert_eq!(UserUgcListSortOrder::SubscriptionDateDesc as i32, 4);
    }
}
