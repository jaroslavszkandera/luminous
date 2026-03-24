use crate::AppController;
use crate::FullViewState;
use crate::MainWindow;
use crate::image_processing::save_image;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;

pub fn register(window: &MainWindow, app_controller: Rc<RefCell<AppController>>) {
    log::debug!("Registering full view presenter");
    let fv = window.global::<FullViewState>();
    let acc = app_controller.clone();
    fv.on_request_next_image(move || acc.borrow().handle_navigate(1));

    let acc = app_controller.clone();
    fv.on_request_prev_image(move || acc.borrow().handle_navigate(-1));

    let acc = app_controller.clone();
    fv.on_apply_edit(move |op| {
        acc.borrow().handle_edit_op(op);
    });

    let acc = app_controller.clone();
    fv.on_save_with_format(move |format| {
        let (img, path, weak_ui) = {
            let c_ref = acc.borrow();
            let idx = c_ref.loader.active_idx.load(Ordering::Relaxed);
            (
                c_ref.loader.get_curr_active_buffer(),
                c_ref.loader.paths.get(idx).cloned(),
                c_ref.window_weak.clone(),
            )
        };
        save_image(img, path, format);
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                ui.invoke_return_focus();
            }
        })
        .unwrap();
    });

    let acc = app_controller.clone();
    fv.on_request_segmentation(move |x1, y1, x2, y2| {
        acc.borrow()
            .handle_segmentation(x1 as i32, y1 as i32, x2 as i32, y2 as i32);
    });
}
