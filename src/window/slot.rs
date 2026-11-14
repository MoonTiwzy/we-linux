//! Where the app lives while the window is open, and the only ways into it.
//!
//! Slint's window is single-threaded. Its callbacks and timers are `'static`
//! closures, and Steam's answers and decoded previews come from threads of
//! their own, which can hand the event loop only a `Send` closure. Neither
//! can carry the app itself, so it lives here, in a thread-local of the UI
//! thread, and everything reaches it through `with`, `try_with` or `on_ui`.
//! A `RefCell` is the honest guard for one thread: one handler has the app
//! at a time. A handler that reaches for it while another has it is a bug
//! and panics (`with`); the timers that tick on their own (the previews'
//! frames, following the desktop) skip a turn instead (`try_with`): a frame
//! late is nothing to stop the window for.

use std::cell::RefCell;

use super::App;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// The app, from now until the window closes.
pub(super) fn install(app: App) {
    APP.with_borrow_mut(|slot| *slot = Some(app));
}

/// Run `f` on the app, on the UI thread.
pub(super) fn with(f: impl FnOnce(&mut App)) {
    APP.with_borrow_mut(|slot| {
        if let Some(app) = slot.as_mut() {
            f(app);
        }
    });
}

/// Run `f` on the app unless something has it already (then not at all).
pub(super) fn try_with(f: impl FnOnce(&mut App)) {
    APP.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut()
            && let Some(app) = slot.as_mut()
        {
            f(app);
        }
    });
}

/// Run `f` on the app from any thread, through the event loop.
pub(super) fn on_ui(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with(f));
}
