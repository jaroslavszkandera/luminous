use crate::AppController;
use crate::GridViewState;
use crate::MainWindow;
use crate::image_processing::batch_save_images;
use log::{info, warn};
use slint::ComponentHandle;
use slint::Model;
use std::cell::RefCell;
use std::rc::Rc;

pub fn register(window: &MainWindow, app_controller: Rc<RefCell<AppController>>) {
    let acc = app_controller.clone();
    let gv = window.global::<GridViewState>();
    gv.on_request_grid_data(move |start, count| {
        acc.borrow_mut()
            .handle_grid_request(start as usize, count as usize);
    });

    let acc = app_controller.clone();
    gv.on_bucket_resolution_changed(move |res| {
        acc.borrow_mut().handle_bucket_resolution(res as u32);
    });

    let acc = app_controller.clone();
    gv.on_search_submitted(move |query| {
        acc.borrow_mut().handle_search(query.to_string());
    });

    let acc = app_controller.clone();
    gv.on_image_selected(move |index| {
        let c_ref = acc.borrow();
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        if let Some(&abs) = c_ref.filtered_indices.get(index as usize) {
            ui.set_view_mode(crate::ViewMode::Full);
            c_ref.handle_full_view_load(abs);
        }
    });

    let acc = app_controller.clone();
    gv.on_toggle_select_all(move |select| {
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        let gv = ui.global::<GridViewState>();
        let model = gv.get_model();
        let vm = gv.get_visible_model();
        for i in 0..model.row_count() {
            if let Some(mut item) = model.row_data(i) {
                if item.selected != select {
                    item.selected = select;
                    model.set_row_data(i, item.clone());
                    for j in 0..vm.row_count() {
                        if let Some(mut v) = vm.row_data(j) {
                            if v.index == item.index {
                                v.selected = select;
                                vm.set_row_data(j, v);
                            }
                        }
                    }
                }
            }
        }
    });

    let acc = app_controller.clone();
    gv.on_toggle_selection(move |index| {
        acc.borrow().handle_toggle_selection(index);
    });

    let acc = app_controller.clone();
    gv.on_request_range_select(move |start_idx, end_idx| {
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        let gv = ui.global::<GridViewState>();
        let model = gv.get_model();
        let vm = gv.get_visible_model();
        let target = model
            .row_data(start_idx as usize)
            .map(|i| i.selected)
            .unwrap_or(true);
        let (lo, hi) = (
            (start_idx.min(end_idx)) as usize,
            (start_idx.max(end_idx)) as usize,
        );
        let mut total_selected = 0i32;
        for i in 0..model.row_count() {
            if let Some(mut item) = model.row_data(i) {
                let should = (i >= lo && i <= hi) && target;
                if item.selected != should {
                    item.selected = should;
                    model.set_row_data(i, item.clone());
                }
                for j in 0..vm.row_count() {
                    if let Some(mut v) = vm.row_data(j) {
                        if v.index == item.index {
                            v.selected = should;
                            vm.set_row_data(j, v);
                        }
                    }
                }
                if should {
                    total_selected += 1;
                }
            }
        }
        gv.set_selected_count(total_selected);
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
