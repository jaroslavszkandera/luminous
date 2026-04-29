use crate::{FullViewState, GridViewState, MainWindow};
use directories::ProjectDirs;
use log::{debug, error};
use serde::{Deserialize, Serialize};
use slint::ComponentHandle;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct AppState {
    pub fullscreen: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub full_view_footer_visible: bool,
    pub full_view_side_panel_visible: bool,
    pub grid_view_side_panel_visible: bool,
}

pub fn load_app_state() -> AppState {
    ProjectDirs::from("", "", "luminous")
        .and_then(|dirs| {
            let path = dirs.cache_dir().join("state.toml");
            debug!("Checking for state file at: {:?}", path);
            std::fs::read_to_string(path).ok()
        })
        .and_then(|s| toml::from_str(&s).ok())
        .inspect(|state| debug!("Successfully loaded state: {:?}", state))
        .unwrap_or_else(|| {
            debug!("No cache found or failed to parse; using defaults");
            AppState::default()
        })
}

pub fn save_app_state(window: &MainWindow) {
    if let Some(dirs) = ProjectDirs::from("", "", "luminous") {
        let cache_dir = dirs.cache_dir();
        if let Err(e) = std::fs::create_dir_all(cache_dir) {
            error!("Could not create cache directory: {}", e);
            return;
        }

        let win = window.window();
        let fv = window.global::<FullViewState>();
        let gv = window.global::<GridViewState>();

        let state = AppState {
            fullscreen: win.is_fullscreen(),
            x: win.position().x,
            y: win.position().y,
            width: win.size().width,
            height: win.size().height,
            full_view_footer_visible: fv.get_footer_visible(),
            full_view_side_panel_visible: fv.get_side_panel_visible(),
            grid_view_side_panel_visible: gv.get_side_panel_visible(),
        };

        match toml::to_string(&state) {
            Ok(toml_str) => {
                let path = cache_dir.join("state.toml");
                if let Err(e) = std::fs::write(&path, toml_str) {
                    error!("Failed to write state to {:?}: {}", path, e);
                } else {
                    debug!("App state {:?} saved to {:?}", state, path);
                }
            }
            Err(e) => error!("Failed to serialize app state: {}", e),
        }
    }
}
