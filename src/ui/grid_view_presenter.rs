use crate::AppController;
use crate::MainWindow;
use crate::image_processing::batch_save_images;
use log::info;
use slint::Model;
use std::cell::RefCell;
use std::rc::Rc;

pub fn register(window: &MainWindow, c: Rc<RefCell<AppController>>) {
    let c1 = c.clone();
    window.on_request_grid_data(move |start, count| {
        c1.borrow_mut()
            .handle_grid_request(start as usize, count as usize);
    });

    let c2 = c.clone();
    window.on_bucket_resolution_changed(move |res| {
        c2.borrow_mut().handle_bucket_resolution(res as u32);
    });

    let c3 = c.clone();
    window.on_search_submitted(move |query| {
        c3.borrow_mut().handle_search(query.to_string());
    });

    let c4 = c.clone();
    window.on_image_selected(move |index| {
        let c_ref = c4.borrow();
        if let Some(&abs) = c_ref.filtered_indices.get(index as usize) {
            c_ref.handle_full_view_load(abs);
        }
    });

    let c5 = c.clone();
    window.on_toggle_select_all(move |select| {
        let Some(ui) = c5.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_grid_model();
        let vm = ui.get_visible_grid_model();
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

    let c6 = c.clone();
    window.on_toggle_selection(move |index| {
        c6.borrow().handle_toggle_selection(index);
    });

    let c7 = c.clone();
    window.on_request_range_select(move |start_idx, end_idx| {
        let Some(ui) = c7.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_grid_model();
        let vm = ui.get_visible_grid_model();
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
        ui.set_selected_count(total_selected);
    });

    let c8 = c.clone();
    window.on_print_selected_paths(move || {
        let paths = c8.borrow().collect_selected_paths();
        if paths.is_empty() {
            info!("No files selected");
        } else {
            info!("Selected ({}):", paths.len());
            for p in &paths {
                info!("  {}", p.display());
            }
        }
    });

    let c9 = c.clone();
    window.on_batch_save_with_format(move |format| {
        let (paths, weak_ui) = {
            let c_ref = c9.borrow();
            let paths = c_ref.collect_selected_paths();
            let weak = c_ref.window_weak.clone();
            (paths, weak)
        };
        if paths.is_empty() {
            info!("No files selected");
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
