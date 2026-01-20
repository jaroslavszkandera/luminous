slint::include_modules!();

pub mod config;
pub mod fs_scan;
mod image_loader;

use fs_scan::ScanResult;
use image_loader::ImageLoader;

use log::{debug, error};
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::error::Error;
use std::rc::Rc;

pub fn run(scan: &ScanResult, worker_count: usize) -> Result<(), Box<dyn Error>> {
    let main_window = MainWindow::new().unwrap();
    let loader = Rc::new(ImageLoader::new(scan.paths.clone(), worker_count));

    let mut grid_data = Vec::new();
    for (i, _) in scan.paths.iter().enumerate() {
        grid_data.push(GridItem {
            image: slint::Image::default(),
            index: i as i32,
        });
    }
    let grid_model = Rc::new(VecModel::from(grid_data));
    main_window.set_grid_model(grid_model.clone().into());

    main_window.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });

    // Grid View
    let loader_grid = loader.clone();
    let window_weak = main_window.as_weak();
    let scan_len = scan.paths.len();

    main_window.on_request_grid_data(move |start_index, count| {
        let start = start_index as usize;
        let end = start + count as usize;
        debug!("on_request_grid_data: start={}, end={}", start, end);

        // TODO: prune images

        for index in start..end {
            if index < scan_len {
                if let Some(loader) = loader_grid.clone().into() {
                    let on_loaded = move |ui: MainWindow, idx: usize, img: slint::Image| {
                        let model = ui.get_grid_model();
                        if let Some(mut item) = model.row_data(idx) {
                            item.image = img;
                            model.set_row_data(idx, item);
                        }
                    };

                    let cached = loader.load_grid_thumb(index, window_weak.clone(), on_loaded);

                    if let Some(buffer) = cached {
                        let window_weak_defer = window_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = window_weak_defer.upgrade() {
                                let model = ui.get_grid_model();
                                if let Some(mut item) = model.row_data(index) {
                                    item.image = Image::from_rgba8(buffer);
                                    model.set_row_data(index, item);
                                }
                            }
                        });
                    }
                }
            }
        }
    });

    // Full View
    let loader_full = loader.clone();
    let paths_len = scan.paths.len();

    let update_full_view = move |ui: MainWindow, index: usize| {
        let window_weak_cb = ui.as_weak();

        let display_img =
            loader_full.load_full_progressive(index, window_weak_cb, move |ui, final_img| {
                ui.set_full_view_image(final_img);
            });

        ui.set_full_view_image(display_img);
        ui.set_curr_image_index(index as i32);

        loader_full.update_sliding_window(index);
    };

    // Callback: Selection from Grid
    let update_fn = update_full_view.clone();
    let window_weak_select = main_window.as_weak();
    main_window.on_image_selected(move |index| {
        if let Some(ui) = window_weak_select.upgrade() {
            update_fn(ui, index as usize);
        }
    });

    // Callback: Next
    let update_fn = update_full_view.clone();
    let window_weak_next = main_window.as_weak();
    main_window.on_request_next_image(move || {
        if let Some(ui) = window_weak_next.upgrade() {
            let mut idx = ui.get_curr_image_index() as usize;
            idx += 1;
            if idx >= paths_len {
                idx = 0;
            }
            update_fn(ui, idx);
        }
    });

    // Callback: Prev
    let update_fn = update_full_view.clone();
    let window_weak_prev = main_window.as_weak();
    main_window.on_request_prev_image(move || {
        if let Some(ui) = window_weak_prev.upgrade() {
            let mut idx = ui.get_curr_image_index() as isize;
            idx -= 1;
            if idx < 0 {
                idx = (paths_len - 1) as isize;
            }
            update_fn(ui, idx as usize);
        }
    });

    // Callback: Rotate +90
    // TODO: Redo it better
    let update_fn = update_full_view.clone();
    let window_weak = main_window.as_weak();
    let loader_full = loader.clone();
    main_window.on_rotate_plus_90(move || {
        let loader = loader_full.clone();

        if let Some(curr_buffer) = loader.get_curr_active_image() {
            let width = curr_buffer.width();
            let height = curr_buffer.height();

            let raw: Vec<u8> = curr_buffer
                .as_slice()
                .iter()
                .flat_map(|pixel| [pixel.r, pixel.g, pixel.b, pixel.a])
                .collect();

            if let Some(image_buffer) = image::RgbaImage::from_raw(width, height, raw) {
                let rot = image::imageops::rotate90(&image_buffer);
                let new_buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                    rot.as_raw(),
                    rot.width(),
                    rot.height(),
                );
                loader.set_curr_active_image(new_buf.clone());

                if let Some(ui) = window_weak.upgrade() {
                    let idx = ui.get_curr_image_index() as usize;
                    update_fn(ui, idx);
                } else {
                    error!("Failed to create image buffer for rotation");
                }
            }
        } else {
            error!("Image probably not loaded");
        }
    });

    // Callback: Rotate -90
    // TODO: Redo it better
    let update_fn = update_full_view.clone();
    let window_weak = main_window.as_weak();
    let loader_full = loader.clone();
    main_window.on_rotate_minus_90(move || {
        let loader = loader_full.clone();

        if let Some(curr_buffer) = loader.get_curr_active_image() {
            let width = curr_buffer.width();
            let height = curr_buffer.height();

            let raw: Vec<u8> = curr_buffer
                .as_slice()
                .iter()
                .flat_map(|pixel| [pixel.r, pixel.g, pixel.b, pixel.a])
                .collect();

            if let Some(image_buffer) = image::RgbaImage::from_raw(width, height, raw) {
                let rot = image::imageops::rotate270(&image_buffer);
                let new_buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                    rot.as_raw(),
                    rot.width(),
                    rot.height(),
                );
                loader.set_curr_active_image(new_buf);

                if let Some(ui) = window_weak.upgrade() {
                    let idx = ui.get_curr_image_index() as usize;
                    update_fn(ui, idx);
                } else {
                    error!("Failed to create image buffer for rotation");
                }
            }
        } else {
            error!("Image probably not loaded");
        }
    });

    // Init
    if !scan.paths.is_empty() {
        debug!("Initializing Full View at index {}", scan.start_index);
        let handle = main_window.as_weak().upgrade().unwrap();
        update_full_view(handle, scan.start_index);
    }

    main_window.run()?;
    Ok(())
}
