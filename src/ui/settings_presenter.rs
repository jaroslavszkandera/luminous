use crate::AppController;
use crate::MainWindow;
use crate::SettingsState;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

pub fn register(window: &MainWindow, app_controller: Rc<RefCell<AppController>>) {
    log::debug!("Registering settings presenter");
    let sg = window.global::<SettingsState>();
    let acc = app_controller.clone();
    sg.on_clear_image_cache(move || {
        acc.borrow().loader.clear_disk_cache();
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

    let acc = app_controller.clone();
    sg.on_settings_opened(move || {
        let plugins_raw = acc.borrow().loader.plugin_manager.get_all_plugins();
        let weak_ui = acc.borrow().window_weak.clone();

        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                let plugins_vec: Vec<crate::Plugin> = plugins_raw
                    .iter()
                    .map(|p| crate::Plugin {
                        name: p.id.clone().into(),
                        // TODO: Placeholders
                        enable: true,
                        auto_start: true,
                    })
                    .collect();

                let names_vec: Vec<slint::StandardListViewItem> = plugins_raw
                    .into_iter()
                    .map(|p| {
                        slint::StandardListViewItem::from(slint::SharedString::from(p.id.clone()))
                    })
                    .collect();

                let plugins_model = std::rc::Rc::new(slint::VecModel::from(plugins_vec));
                let names_model = std::rc::Rc::new(slint::VecModel::from(names_vec));

                let state = ui.global::<SettingsState>();
                state.set_plugins(plugins_model.into());
                state.set_plugin_names(names_model.into());
            }
        })
        .unwrap();
    });
}
