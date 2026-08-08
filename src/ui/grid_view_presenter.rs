use crate::AppController;
use crate::GridViewState;
use crate::MainWindow;
use crate::image_processing::batch_save_images;
use log::{debug, error, info, trace, warn};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::rc::Rc;

pub fn register(window: &MainWindow, app_controller: Rc<RefCell<AppController>>) {
    let acc = app_controller.clone();
    let gv = window.global::<GridViewState>();
    gv.on_request_grid_data(move |first_row, visible_rows, num_cols| {
        trace!(
            "on_request_grid_data, first_row {}, visible_rows {}, num_cols {}",
            first_row, visible_rows, num_cols
        );
        if let Ok(mut app) = acc.try_borrow_mut() {
            app.handle_grid_request(first_row as usize, visible_rows as usize, num_cols as usize);
        } else {
            error!("on_request_grid_data borrow not successful");
        }
    });

    let acc = app_controller.clone();
    gv.on_bucket_resolution_changed(move |res| {
        acc.borrow_mut().handle_bucket_resolution(res as u32);
    });

    let acc = app_controller.clone();
    gv.on_search_submitted(move |text| {
        debug!("on_search_submitted {}", text);
        acc.borrow_mut().set_search_text(&text);
        acc.borrow_mut().rebuild_view();

        let weak = acc.borrow().window_weak.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_return_focus();
            }
        })
        .unwrap();
    });

    let acc = app_controller.clone();
    gv.on_request_sort(move |ascending| {
        debug!("on_request_sort {}", ascending);
        acc.borrow_mut().set_sort_asceding(ascending);
        acc.borrow_mut().rebuild_view();
    });

    let acc = app_controller.clone();
    gv.on_image_selected(move |index| {
        let c_ref = acc.borrow();
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        ui.set_view_mode(crate::ViewMode::Full);
        c_ref.handle_full_view_load(index as usize);
    });

    let acc = app_controller.clone();
    gv.on_toggle_select_all(move |select| {
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        acc.borrow_mut().toggle_select_all(select);
        let gv = ui.global::<GridViewState>();
        let m = gv.get_visible_model();
        for i in 0..m.row_count() {
            if let Some(mut item) = m.row_data(i) {
                if item.selected != select {
                    item.selected = select;
                    m.set_row_data(i, item.clone());
                }
            }
        }
    });

    // TODO: implement better behavior
    let acc = app_controller.clone();
    gv.on_request_range_select(move |start_idx, end_idx| {
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };

        acc.borrow_mut()
            .toggle_select_range(start_idx as usize, end_idx as usize);

        let c_ref = acc.borrow();
        let gv = ui.global::<GridViewState>();
        let vm = gv.get_visible_model();
        for i in 0..vm.row_count() {
            if let Some(mut item) = vm.row_data(i) {
                let should = c_ref.is_selected(item.abs_index);
                if item.selected != should {
                    item.selected = should;
                    vm.set_row_data(i, item.clone());
                }
            }
        }
        gv.set_selected_count(c_ref.selected.len() as i32);
    });

    let acc = app_controller.clone();
    gv.on_toggle_selection(move |index| {
        acc.borrow_mut().toggle_select_single(index);
    });

    let acc = app_controller.clone();
    gv.on_print_selected_paths(move || {
        let paths = acc.borrow().collect_selected_paths();
        if paths.is_empty() {
            warn!("No files selected");
        } else {
            info!("Selected ({}): {:#?}", paths.len(), paths);
        }
    });

    let acc = app_controller.clone();
    window.on_batch_save_with_format(move |format| {
        let (paths, weak_ui) = {
            let c_ref = acc.borrow();
            let paths = c_ref.collect_selected_paths();
            let weak = c_ref.window_weak.clone();
            (paths, weak)
        };
        if paths.is_empty() {
            warn!("No files selected");
            return;
        }
        batch_save_images(paths, format);
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                ui.invoke_return_focus();
            }
        })
        .unwrap();
    });
}
