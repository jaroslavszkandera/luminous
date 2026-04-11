use crate::AppController;
use crate::FullViewState;
use crate::MainWindow;
use crate::image_processing::save_image;
use log::{debug, error};
use slint::{ComponentHandle, SharedString, StandardListViewItem, VecModel};
use std::cell::RefCell;
use std::fs::File;
use std::io::BufReader;
use std::rc::Rc;
use std::sync::atomic::Ordering;

pub fn register(window: &MainWindow, app_controller: Rc<RefCell<AppController>>) {
    debug!("Registering full view presenter");
    let fv = window.global::<FullViewState>();

    let encoder_extensions = app_controller
        .borrow()
        .scan
        .clone()
        .image_formats
        .get_all_encoding_exts();
    let mut sorted_exts: Vec<slint::SharedString> = encoder_extensions
        .into_iter()
        .map(slint::SharedString::from)
        .collect();
    sorted_exts.sort();
    let model = std::rc::Rc::new(slint::VecModel::from(sorted_exts));
    fv.set_encoder_extensions(model.into());

    let acc = app_controller.clone();
    fv.on_request_next_image(move || {
        acc.borrow().handle_navigate(1);
        set_exif(acc.clone());
    });

    let acc = app_controller.clone();
    fv.on_request_prev_image(move || {
        acc.borrow().handle_navigate(-1);
        set_exif(acc.clone());
    });

    let acc = app_controller.clone();
    fv.on_apply_edit(move |op| {
        acc.borrow().handle_edit_op(op);
    });

    let acc = app_controller.clone();
    fv.on_save_with_format(move |format| {
        let (img, path, weak_ui, plugin_manager) = {
            let c_ref = acc.borrow();
            let idx = c_ref.loader.active_idx.load(Ordering::Relaxed);
            (
                c_ref.loader.get_curr_active_buffer(),
                c_ref.loader.paths.get(idx).cloned(),
                c_ref.window_weak.clone(),
                c_ref.loader.plugin_manager.clone(),
            )
        };
        save_image(img, path, format.as_str().into(), plugin_manager);
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

fn set_empty_exif() -> slint::ModelRc<slint::ModelRc<StandardListViewItem>> {
    slint::ModelRc::new(VecModel::from(vec![slint::ModelRc::new(VecModel::from(
        vec![
            StandardListViewItem::from(SharedString::from("No EXIF data")),
            StandardListViewItem::from(SharedString::from("")),
        ],
    ))]))
}

fn set_exif_rows(
    app_controller: &Rc<RefCell<AppController>>,
    rows: slint::ModelRc<slint::ModelRc<StandardListViewItem>>,
) {
    if let Some(ui) = app_controller.borrow().window_weak.upgrade() {
        ui.global::<FullViewState>().set_exif_rows(rows);
    }
}

pub fn set_exif(app_controller: Rc<RefCell<AppController>>) {
    let c_ref = app_controller.borrow();
    let img_path = {
        match c_ref.loader.get_curr_img_path() {
            Some(p) => p,
            None => {
                error!("No image path for curr idx");
                return set_exif_rows(&app_controller, set_empty_exif());
            }
        }
    };

    let file = match File::open(&img_path) {
        Ok(f) => f,
        Err(e) => {
            error!("Cannot open file {:?}: {}", img_path, e);
            return set_exif_rows(&app_controller, set_empty_exif());
        }
    };

    let exif = match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(e) => e,
        Err(_e) => {
            // debug!("Cannot read EXIF from {:?}: {}", img_path, _e);
            return set_exif_rows(&app_controller, set_empty_exif());
        }
    };

    let rows: Vec<slint::ModelRc<StandardListViewItem>> = exif
        .fields()
        .filter_map(|f| {
            let tag = f.tag.to_string();
            let value = f.display_value().with_unit(&exif).to_string();

            Some(slint::ModelRc::new(VecModel::from(vec![
                StandardListViewItem::from(SharedString::from(tag.as_str())),
                StandardListViewItem::from(SharedString::from(value.as_str())),
            ])))
        })
        .collect();

    let model = if rows.is_empty() {
        set_empty_exif()
    } else {
        slint::ModelRc::new(VecModel::from(rows))
    };

    set_exif_rows(&app_controller, model);
}
