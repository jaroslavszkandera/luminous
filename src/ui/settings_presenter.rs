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
}
