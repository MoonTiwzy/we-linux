//! Every callback of the window, and what it calls. Each is one line: what a
//! callback does is the part's own (see `mod.rs` for which part is where).

use slint::ComponentHandle;
use we_linux_ui::AppWindow;

use super::slot;
use crate::selection;

pub(super) fn wire(ui: &AppWindow) {
    // A callback runs on the app, which it has for that call only. A macro,
    // since the callbacks take arguments of every number and kind.
    macro_rules! on {
        ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {
            ui.$setter(move |$($arg),*| slot::with(|$app| {
                $body;
            }))
        };
    }
    on!(on_tab_changed, |app, tab| app.tab_picked(tab));
    on!(on_search_typed, |app, text| app.search_typed(text.to_string()));
    on!(on_search_submitted, |app, text| app.search(text.to_string()));
    on!(on_sort_changed, |app, index| app.sort_picked(index));
    on!(on_sort_direction_changed, |app| app.sort_direction_changed());
    on!(on_page_clicked, |app, page| app.page_picked(page));
    on!(on_notice_closed, |app| app.notice_closed());
    on!(on_reload, |app| app.reload());

    on!(on_filter_toggled, |app, group, option| app.filter_toggled(group, option));
    on!(on_filter_group_all, |app, group, checked| app.filter_group_all(group, checked));
    on!(on_filter_section_all, |app, group, section, checked| app
        .filter_section_all(group, section, checked));
    on!(on_group_toggled, |app, group| app.group_toggled(group));
    on!(on_group_shown, |app, group| app.group_shown(group));
    on!(on_content_preferences, |app| app.content_preferences());
    on!(on_anime_changed, |app, shown| app.anime_changed(shown));
    on!(on_resolution_changed, |app, recommended, other| app.resolution_changed(recommended, other));
    on!(on_filters_reset, |app| app.filters_reset());

    on!(on_visible_changed, |app, first, last| app
        .visible_changed(first.max(0) as usize, last.max(0) as usize));
    on!(on_item_selected, |app, index, ctrl, shift| app.click(index.max(0) as usize, ctrl, shift));
    on!(on_item_activated, |app, index| app.activate(index.max(0) as usize));
    on!(on_band_picked, |app, x, y, width, height, cols, step, row_step, side, ctrl, shift| {
        let grid = selection::Grid { cols: cols.max(1) as usize, step, row_step, side };
        app.band(&grid, (x, y, width, height), ctrl, shift)
    });
    on!(on_select_all, |app| app.select_all());
    on!(on_navigate, |app, step| app.navigate(step));
    on!(on_delete_pressed, |app| app.delete_pressed());
    on!(on_item_context, |app, index| app.context_menu(index.max(0) as usize));
    on!(on_context_activated, |app, id| app.context_activated(&id));

    on!(on_subscribe, |app| app.subscribe());
    on!(on_unsubscribe, |app| app.unsubscribe());
    on!(on_unsubscribe_answered, |app, ok| app.unsubscribe_answered(ok));
    on!(on_apply, |app| app.apply());
    on!(on_favorite, |app| app.toggle_favorite());
    on!(on_vote, |app, up| app.vote(up));
    on!(on_browse_author, |app| app.browse_selected_author());
    on!(on_open_author, |app| app.open_author());
    on!(on_open_steam, |app| app.open_steam());
    on!(on_open_folder, |app| app.open_folder());
    on!(on_copy_link, |app| app.copy_link());
    on!(on_report, |app| app.report_selected());

    on!(on_prop_bool, |app, row, value| app.prop_bool(row, value));
    on!(on_prop_number, |app, row, value| app.prop_number(row, value));
    on!(on_prop_text, |app, row, text| app.prop_text(row, &text));
    on!(on_prop_index, |app, row, index| app.prop_index(row, index));
    on!(on_props_reset, |app| app.reset_properties());
    on!(on_preset_save, |app, name| app.preset_save(&name));
    on!(on_preset_load, |app, row| app.preset_load(row.max(0) as usize));
    on!(on_preset_apply_all, |app| app.preset_apply_all());
    on!(on_preset_share, |app| app.preset_share());

    on!(on_playlist_add, |app| app.playlist_add());
    on!(on_playlist_remove, |app, row| app.playlist_remove(row.max(0) as usize));
    on!(on_playlist_save, |app| app.playlist_save());
    on!(on_playlist_load, |app, row| app.playlist_load(row));
    on!(on_playlist_start, |app| app.playlist_run(true));
    on!(on_playlist_stop, |app| app.playlist_run(false));
    on!(on_playlist_clear, |app| app.playlist_clear());
    on!(on_playlist_toggle_selected, |app| app.playlist_toggle_selected());
    on!(on_playlist_select, |app, row| app.playlist_select(row));

    on!(on_open_monitors, |app| app.open_monitors());
    on!(on_monitor_picked, |app, index| app.pick_monitor(index));
    on!(on_monitor_cleared, |app, index| app.clear_monitor(index));
    on!(on_monitors_on_start_changed, |app, on| app.monitors_on_start_changed(on));
    on!(on_monitor_profiles, |app, save| app.monitor_profiles(save));
    on!(on_profile_save, |app, name| app.profile_save(&name));
    on!(on_profile_load, |app, row| app.profile_load(row));
    on!(on_open_file, |app, kind| app.open_file(kind == "static"));
    on!(on_open_editor, |app| app.open_editor());

    on!(on_settings_shown, |app| app.show_settings());
    on!(on_settings_save, |app| app.save_settings());
    on!(on_settings_reset, |app| app.reset_settings());
    on!(on_cache_clear, |app| app.clear_cache());
    on!(on_language_picked, |app, index| app.language_picked(index));

    // The window itself, which needs no app.
    {
        let weak = ui.as_weak();
        ui.on_window_close(move || {
            if let Some(ui) = weak.upgrade() {
                let _ = ui.hide();
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_window_minimize(move || {
            if let Some(ui) = weak.upgrade() {
                ui.window().set_minimized(true);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_window_maximize(move || {
            if let Some(ui) = weak.upgrade() {
                let window = ui.window();
                window.set_maximized(!window.is_maximized());
            }
        });
    }
    {
        // The small window: the card alone.
        let weak = ui.as_weak();
        ui.on_mini_toggled(move || {
            if let Some(ui) = weak.upgrade() {
                let mini = !ui.get_mini();
                ui.set_mini(mini);
                let height = ui.window().size().to_logical(ui.window().scale_factor()).height;
                ui.window().set_size(slint::LogicalSize::new(if mini { 340.0 } else { 1656.0 }, height));
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_accept(move || {
            if let Some(ui) = weak.upgrade() {
                let _ = ui.hide();
            }
        });
    }
    {
        // "Отмена": what the desktop showed when the window opened, then the window goes.
        let weak = ui.as_weak();
        ui.on_cancel(move || {
            slot::with(|app| app.revert());
            if let Some(ui) = weak.upgrade() {
                let _ = ui.hide();
            }
        });
    }
}
