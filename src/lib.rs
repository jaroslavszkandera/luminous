slint::include_modules!();

pub mod config;
pub mod fs_scan;
mod image_loader;

use config::Config;
use fs_scan::ScanResult;
use image_loader::ImageLoader;

use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::cmp;
use std::collections::HashSet;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;

struct AppController {
    loader: Arc<ImageLoader>,
    scan: Rc<ScanResult>,
    active_grid_indices: HashSet<usize>,
    window_weak: slint::Weak<MainWindow>,
}

impl AppController {
    fn new(scan: Rc<ScanResult>, config: &Config, window: &MainWindow) -> Self {
        Self {
            loader: Arc::new(ImageLoader::new(
                scan.paths.clone(),
                config.threads,
                config.window_size,
            )),
            scan,
            active_grid_indices: HashSet::new(),
            window_weak: window.as_weak(),
        }
    }

    fn handle_grid_request(&mut self, start: usize, count: usize) {
        let _timer = std::time::Instant::now();
        let total = self.scan.paths.len();
        let end = cmp::min(start + count, total);

        let buffer = 50;
        let keep_start = start.saturating_sub(buffer);
        let keep_end = end + buffer;

        let to_remove: Vec<usize> = self
            .active_grid_indices
            .iter()
            .cloned()
            .filter(|&idx| idx < keep_start || idx >= keep_end)
            .collect();

        for idx in &to_remove {
            self.active_grid_indices.remove(idx);
        }
        self.loader.prune_grid_thumbs(&to_remove);

        if !to_remove.is_empty() {
            let weak = self.window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let model = ui.get_grid_model();
                    for idx in to_remove {
                        if let Some(mut item) = model.row_data(idx) {
                            if item.image.size().width > 0 {
                                item.image = Image::default();
                                model.set_row_data(idx, item);
                            }
                        }
                    }
                }
            });
        }

        let mut cached_updates = Vec::new();
        for index in start..end {
            if self.active_grid_indices.contains(&index) {
                continue;
            }
            self.active_grid_indices.insert(index);

            let weak = self.window_weak.clone();
            let on_loaded = move |ui: MainWindow, idx: usize, img: Image| {
                let model = ui.get_grid_model();
                if let Some(mut item) = model.row_data(idx) {
                    item.image = img;
                    model.set_row_data(idx, item);
                }
            };

            if let Some(buffer) = self.loader.load_grid_thumb(index, weak, on_loaded) {
                cached_updates.push((index, buffer));
            }
        }

        if !cached_updates.is_empty() {
            let weak = self.window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    let model = ui.get_grid_model();
                    for (idx, buf) in cached_updates {
                        if let Some(mut item) = model.row_data(idx) {
                            item.image = Image::from_rgba8(buf);
                            model.set_row_data(idx, item);
                        }
                    }
                }
            });
        }
    }

    fn handle_full_view_load(&self, index: usize) {
        let weak = self.window_weak.clone();
        let loader = self.loader.clone();

        let on_loaded = move |ui: MainWindow, img: Image| {
            ui.set_full_view_image(img);
        };

        let display_img = loader.load_full_progressive(index, weak.clone(), on_loaded);

        if let Some(ui) = weak.upgrade() {
            ui.set_full_view_image(display_img);
            ui.set_curr_image_index(index as i32);
            ui.set_curr_image_name(loader.get_curr_image_file_name(index).into());
        }

        self.loader.update_sliding_window(index);
    }

    fn handle_navigate(&self, delta: isize) {
        if let Some(ui) = self.window_weak.upgrade() {
            let len = self.scan.paths.len() as isize;
            let current = ui.get_curr_image_index() as isize;
            let next = (current + delta).rem_euclid(len) as usize;
            self.handle_full_view_load(next);
        }
    }

    fn handle_rotate(&self, degrees: i32) {
        if let Some(buffer) = self.loader.get_curr_active_buffer() {
            let loader = self.loader.clone();
            let weak = self.window_weak.clone();

            self.loader.pool.execute(move || {
                let width = buffer.width();
                let height = buffer.height();
                let raw: Vec<u8> = buffer
                    .as_slice()
                    .iter()
                    .flat_map(|p| [p.r, p.g, p.b, p.a])
                    .collect();

                if let Some(img_buf) = image::RgbaImage::from_raw(width, height, raw) {
                    let rotated = match degrees {
                        90 => image::imageops::rotate90(&img_buf),
                        -90 | 270 => image::imageops::rotate270(&img_buf),
                        180 => image::imageops::rotate180(&img_buf),
                        _ => img_buf,
                    };

                    let new_buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        rotated.as_raw(),
                        rotated.width(),
                        rotated.height(),
                    );

                    loader.cache_buffer(
                        loader.active_idx.load(std::sync::atomic::Ordering::Relaxed),
                        new_buf.clone(),
                    );

                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.set_full_view_image(Image::from_rgba8(new_buf));
                    });
                }
            });
        }
    }

    fn handle_bucket_resolution(&mut self, resolution: u32) {
        self.loader.set_bucket_resolution(resolution);
        self.active_grid_indices.clear();
    }
}

pub fn run(scan: ScanResult, config: Config) -> Result<(), Box<dyn Error>> {
    let main_window = MainWindow::new()?;

    let grid_data: Vec<GridItem> = scan
        .paths
        .iter()
        .enumerate()
        .map(|(i, _)| GridItem {
            image: Image::default(),
            index: i as i32,
        })
        .collect();
    main_window.set_grid_model(Rc::new(VecModel::from(grid_data)).into());

    let scan_rc = Rc::new(scan);
    let controller = Rc::new(RefCell::new(AppController::new(
        scan_rc.clone(),
        &config,
        &main_window,
    )));

    // Callbacks

    let c = controller.clone();
    main_window.on_request_grid_data(move |start, count| {
        c.borrow_mut()
            .handle_grid_request(start as usize, count as usize);
    });

    let c = controller.clone();
    main_window.on_bucket_resolution_changed(move |bucket_resolution| {
        dbg!(bucket_resolution);
        c.borrow_mut()
            .handle_bucket_resolution(bucket_resolution as u32);
    });

    let c = controller.clone();
    main_window.on_image_selected(move |index| {
        c.borrow().handle_full_view_load(index as usize);
    });

    let c = controller.clone();
    main_window.on_request_next_image(move || c.borrow().handle_navigate(1));
    let c = controller.clone();
    main_window.on_request_prev_image(move || c.borrow().handle_navigate(-1));

    let c = controller.clone();
    main_window.on_rotate_plus_90(move || c.borrow().handle_rotate(90));
    let c = controller.clone();
    main_window.on_rotate_minus_90(move || c.borrow().handle_rotate(-90));

    main_window.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });

    // Initial Load
    if !scan_rc.paths.is_empty() {
        controller
            .borrow()
            .handle_full_view_load(scan_rc.start_index);
    }

    main_window.set_app_background(config.background);

    main_window.run()?;
    Ok(())
}
