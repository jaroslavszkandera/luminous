use crate::{AppController, Config, MainWindow, SettingsState};
use directories::ProjectDirs;
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::fs::{self, File};
use std::path::PathBuf;
use std::rc::Rc;

pub fn register(window: &MainWindow, app_controller: Rc<RefCell<AppController>>) {
    debug!("Registering settings presenter");
    let sg = window.global::<SettingsState>();

    let acc = app_controller.clone();
    sg.on_clear_image_cache(move || {
        let acc_bor = acc.borrow();
        acc_bor.loader.clear_disk_cache();
        let disk_cache_count = acc_bor.loader.get_image_disk_cache_count();
        let weak_ui = acc_bor.window_weak.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                ui.global::<SettingsState>()
                    .set_image_cache_count(disk_cache_count as i32);
            }
        })
        .unwrap();
    });

    let acc = app_controller.clone();
    sg.on_get_image_cache_count(move || {
        let disk_cache_count = acc.borrow().loader.get_image_disk_cache_count();
        let weak_ui = acc.borrow().window_weak.clone();

        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                ui.global::<SettingsState>()
                    .set_image_cache_count(disk_cache_count as i32);
            }
        })
        .unwrap();
    });

    let plugins = app_controller
        .borrow()
        .loader
        .plugin_manager
        .get_all_plugins();
    for plugin in plugins {
        let id = plugin.id.clone();

        let weak_ui = app_controller.borrow().window_weak.clone();
        plugin.on_state_change(move |state| {
            let id_clone = id.clone();
            let state_str = state.to_str().to_string();
            let is_busy = state_str == "Starting" || state_str == "Stopping";
            let is_enabled = state_str == "Enable" || state_str == "Starting";

            let weak_ui_c = weak_ui.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak_ui_c.upgrade() {
                    let g = ui.global::<SettingsState>();
                    let model = g.get_plugins();

                    for i in 0..model.row_count() {
                        if let Some(mut p) = model.row_data(i) {
                            if p.id == id_clone {
                                p.state = state_str.clone().into();
                                p.is_busy = is_busy;
                                p.enabled = is_enabled;
                                model.set_row_data(i, p);
                                break;
                            }
                        }
                    }
                }
            });
        });
    }

    let acc = app_controller.clone();
    sg.on_settings_opened(move || {
        let plugins_manager = acc.borrow().loader.plugin_manager.clone();
        let plugins = plugins_manager.get_all_plugins();
        let plugin_ids: Vec<String> = plugins.iter().map(|p| p.id.clone()).collect();

        let mut settings = read_settings().unwrap_or(Settings { plugins: vec![] });
        settings.sync_plugins(plugin_ids);

        let _ = write_settings(&settings);

        let config = Config::load();
        let mut bindings_vec = Vec::new();
        for (k, v) in config.bindings.iter() {
            bindings_vec.push(crate::SettingItem {
                id: SharedString::from(k),
                label: SharedString::from(k),
                value: SharedString::from(v),
            });
        }
        bindings_vec.sort_by(|a, b| a.id.cmp(&b.id));

        let weak_ui = acc.borrow().window_weak.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                let state = ui.global::<SettingsState>();
                let plugins_vec: Vec<crate::Plugin> = settings
                    .plugins
                    .into_iter()
                    .filter_map(|p| {
                        let plugin = plugins_manager.get_plugin_by_id(&p.id)?;
                        let state_str = plugin.get_state().to_str();

                        Some(crate::Plugin {
                            id: p.id.into(),
                            enabled: plugin.is_running(),
                            auto_start: p.auto_start,
                            state: state_str.into(),
                            is_busy: state_str == "Starting" || state_str == "Stopping",
                            ..Default::default()
                        })
                    })
                    .collect();

                let names_vec: Vec<slint::StandardListViewItem> = plugins_vec
                    .iter()
                    .map(|p| slint::StandardListViewItem::from(p.id.clone()))
                    .collect();

                state.set_plugins(std::rc::Rc::new(slint::VecModel::from(plugins_vec)).into());
                state.set_plugin_names(std::rc::Rc::new(slint::VecModel::from(names_vec)).into());
                state.set_binding_settings(ModelRc::from(std::rc::Rc::new(VecModel::from(
                    bindings_vec,
                ))));
            }
        })
        .unwrap();
    });

    let acc = app_controller.clone();
    sg.on_toggle_plugin_enable(move |id| {
        let plugins_manager = acc.borrow().loader.plugin_manager.clone();
        if let Some(plugin) = plugins_manager.get_plugin_by_id(&id) {
            if plugin.is_running() {
                plugin.stop(5000, false);
            } else {
                plugin.start();
            }
        } else {
            return;
        };
    });

    let acc = app_controller.clone();
    sg.on_toggle_plugin_auto_start(move |id, idx| {
        let mut settings = read_settings().unwrap_or(Settings { plugins: vec![] });
        if let Some(plugin_settings) = settings.plugins.iter_mut().find(|p| p.id == id.as_str()) {
            plugin_settings.auto_start = !plugin_settings.auto_start;
            let new_auto_start = plugin_settings.auto_start;

            if let Err(e) = write_settings(&settings) {
                error!("Failed to save auto-start preference: {e}");
                return;
            }

            let weak_ui = acc.borrow().window_weak.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak_ui.upgrade() {
                    let state = ui.global::<SettingsState>();
                    let model = state.get_plugins();
                    if let Some(mut p) = model.row_data(idx as usize) {
                        p.auto_start = new_auto_start;
                        model.set_row_data(idx as usize, p);
                    }
                }
            })
            .unwrap();
        }
    });

    // sg.on_update_setting(move |id, val, category| {
    //     let id_str = id.as_str();
    //     let val_str = val.as_str();
    //
    //     // Load, mutate, and save your TOML file here based on category/id
    // });
}

#[derive(Deserialize, Serialize)]
pub struct PluginSettings {
    pub id: String,
    pub auto_start: bool,
}

#[derive(Deserialize, Serialize)]
pub struct Settings {
    pub plugins: Vec<PluginSettings>,
}

impl Settings {
    pub fn sync_plugins(&mut self, active_ids: Vec<String>) {
        self.plugins.retain(|p| active_ids.contains(&p.id));

        for id in active_ids {
            if !self.plugins.iter().any(|p| p.id == id) {
                self.plugins.push(PluginSettings {
                    id,
                    auto_start: true,
                });
            }
        }
    }
}

fn get_settings_path() -> Option<PathBuf> {
    let proj_dirs = ProjectDirs::from("", "", "luminous")?;
    let settings_dir = proj_dirs.config_dir();
    let settings_path = settings_dir.join("settings.toml");

    if let Err(e) = fs::create_dir_all(settings_dir) {
        error!("Failed to create config directory: {e}");
        return None;
    }

    if !settings_path.exists() {
        if let Err(e) = File::create(&settings_path) {
            error!("Failed to create settings file: {e}");
            return None;
        }
        info!("Created new settings file at {:?}", settings_path);
    }

    Some(settings_path)
}

pub fn read_settings() -> Option<Settings> {
    get_settings_path().and_then(|path| {
        let content = std::fs::read_to_string(path).ok()?;
        toml::from_str(&content).ok()
    })
}

pub fn write_settings(settings: &Settings) -> Result<(), Box<dyn std::error::Error>> {
    let path = get_settings_path().ok_or("Could not determine settings path")?;

    let toml_string = toml::to_string_pretty(settings)?;
    std::fs::write(path, toml_string)?;

    Ok(())
}
