use crate::AppController;
use crate::FullViewState;
use crate::MainWindow;
use crate::image_processing::save_image;
use cocotools::coco::object_detection::{
    Annotation, Bbox, Dataset, Image as CocoImage, Rle, Segmentation,
};
use log::{debug, error};
use luminous_plugins::{PluginCapability, manifest::InteractiveCapability};
use slint::{ComponentHandle, SharedString, StandardListViewItem, VecModel};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::Ordering;

pub fn register(window: &MainWindow, app_controller: Rc<RefCell<AppController>>) {
    debug!("Registering full view presenter");
    let fv = window.global::<FullViewState>();

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
        acc.borrow_mut().handle_edit_op(op);
    });

    let acc = app_controller.clone();
    fv.on_save_with_format(move |format| {
        let (img, path, weak_ui, plugin_manager) = {
            let c_ref = acc.borrow();
            let idx = c_ref.loader.active_idx.load(Ordering::Relaxed);
            (
                c_ref.loader.get_curr_active_buffer(),
                c_ref.loader.get_path(idx),
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
    fv.on_request_segmentation(move |plugin_id, x1, y1, x2, y2, txt| {
        acc.borrow().handle_segmentation(
            plugin_id.to_string(),
            x1 as i32,
            y1 as i32,
            x2 as i32,
            y2 as i32,
            std::string::String::from(txt),
        );
        let weak_ui = acc.borrow().window_weak.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                ui.invoke_return_focus();
            }
        })
        .unwrap();
    });

    let window_weak = window.as_weak();
    fv.on_clear_curr_mask_overlay(move || {
        let _ = window_weak.upgrade_in_event_loop(move |ui| {
            ui.global::<FullViewState>()
                .set_mask_overlay(Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::new(
                    0, 0,
                )));
        });
    });
    let window_weak = window.as_weak();
    let acc = app_controller.clone();
    fv.on_save_curr_mask_overlay(move || {
        let image_path = acc
            .borrow()
            .loader
            .get_curr_img_path()
            .map(|p| p.to_path_buf());

        let _ = window_weak.upgrade_in_event_loop(move |ui| {
            let fv = ui.global::<FullViewState>();
            let mask_image = fv.get_mask_overlay();

            if let Some(buffer) = mask_image.to_rgba8() {
                if let Some(path) = image_path {
                    let file_name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if let Some(parent_dir) = path.parent() {
                        let annotation_path = parent_dir.join("annotations.json");
                        save_mask(buffer, &annotation_path, &file_name);
                    }
                }
            }
        });
    });

    refresh_interactive_plugins(&app_controller.clone());
}

fn refresh_interactive_plugins(app_controller: &Rc<RefCell<AppController>>) {
    let c_ref = app_controller.borrow();
    let weak = c_ref.window_weak.clone();
    let pm = c_ref.loader.plugin_manager.clone();
    let plugins_vec: Vec<crate::Plugin> = pm
        .get_interactive_plugins()
        .map(|p| {
            let i_caps = p.manifest.capabilities.iter().find_map(|cap| {
                if let PluginCapability::Interactive(inner) = cap {
                    Some(inner)
                } else {
                    None
                }
            });
            crate::Plugin {
                id: p.id.clone().into(),
                click_capability_support: i_caps
                    .map_or(false, |c| c.contains(&InteractiveCapability::Click)),
                select_capability_support: i_caps
                    .map_or(false, |c| c.contains(&InteractiveCapability::Select)),
                text_capability_support: i_caps
                    .map_or(false, |c| c.contains(&InteractiveCapability::Text)),
                ..Default::default()
            }
        })
        .collect();

    if plugins_vec.is_empty() {
        debug!("No interactive plugins found.");
    }
    if let Some(ui) = weak.upgrade() {
        let model = std::rc::Rc::new(slint::VecModel::from(plugins_vec));
        ui.global::<FullViewState>()
            .set_interactive_plugins(model.into());
    }
}

fn save_mask(mask_buffer: SharedPixelBuffer<Rgba8Pixel>, path: &Path, file_name: &str) -> bool {
    let width = mask_buffer.width() as usize;
    let height = mask_buffer.height() as usize;

    if width == 0 || height == 0 {
        debug!("No mask to save (width == 0 || height == 0)");
        return false;
    }

    let mut mask_bits = vec![0u8; width * height];
    let mut area = 0.0;
    let (mut min_x, mut min_y) = (width as f64, height as f64);
    let (mut max_x, mut max_y) = (0.0, 0.0);

    let slice = mask_buffer.as_slice();

    for x in 0..width {
        for y in 0..height {
            let pixel = &slice[y * width + x];
            if pixel.a > 0 {
                mask_bits[x * height + y] = 1;
                area += 1.0;
                let (fx, fy) = (x as f64, y as f64);
                if fx < min_x {
                    min_x = fx;
                }
                if fx > max_x {
                    max_x = fx;
                }
                if fy < min_y {
                    min_y = fy;
                }
                if fy > max_y {
                    max_y = fy;
                }
            }
        }
    }

    if area == 0.0 {
        debug!("No make to save (area == 0.0)");
        return false;
    }

    let mut counts = Vec::new();
    let mut current_val = 0u8;
    let mut current_run = 0u32;
    for bit in mask_bits {
        if bit == current_val {
            current_run += 1;
        } else {
            counts.push(current_run);
            current_run = 1;
            current_val = bit;
        }
    }
    counts.push(current_run);

    let mut dataset = if path.exists() {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<Dataset>(&content).ok())
            .unwrap_or_default()
    } else {
        Dataset::default()
    };

    let image_id = dataset
        .images
        .iter()
        .find(|img| img.file_name == file_name)
        .map(|img| img.id)
        .unwrap_or_else(|| {
            let new_id = (dataset.images.len() + 1) as u64;
            dataset.images.push(CocoImage {
                id: new_id,
                width: width as u32,
                height: height as u32,
                file_name: file_name.to_string(),
                ..Default::default()
            });
            new_id
        });

    let ann_id = (dataset.annotations.len() + 1) as u64;
    dataset.annotations.push(Annotation {
        id: ann_id,
        image_id,
        category_id: 1,
        segmentation: Segmentation::Rle(Rle {
            size: vec![height as u32, width as u32],
            counts,
        }),
        area,
        bbox: Bbox {
            left: min_x,
            top: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        },
        iscrowd: 0,
    });

    serde_json::to_string_pretty(&dataset)
        .ok()
        .and_then(|json| std::fs::write(path, json).ok())
        .map(|_| {
            debug!("Dataset updated at {path:?}");
            true
        })
        .unwrap_or(false)
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
