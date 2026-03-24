use crate::MainWindow;
use crate::config::Config;

pub fn setup(window: &MainWindow, config: &Config) {
    let get_key = |action: &str| {
        Config::get_slint_key_string(
            config
                .bindings
                .get(action)
                .unwrap_or_else(|| panic!("Binding '{action}' not in config")),
        )
    };
    window.set_bind_quit(get_key("quit"));
    window.set_bind_fullscreen(get_key("toggle_fullscreen"));
    window.set_bind_switch_view_mode(get_key("switch_view_mode"));
    window.set_bind_reset_zoom(get_key("reset_zoom"));
    window.set_bind_grid_pg_dn(get_key("grid_page_down"));
    window.set_bind_grid_pg_up(get_key("grid_page_up"));
    window.set_bind_toggle_side_panel(get_key("toggle_side_panel"));
}
