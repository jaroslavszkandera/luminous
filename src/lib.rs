slint::include_modules!();

pub mod config;
pub mod fs_scan;
mod image_loader;

use config::Config;
use fs_scan::ScanResult;
use image_loader::ImageLoader;

use log::debug;
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::cmp;
use std::collections::HashSet;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

struct AppController {
    // TODO: ImageLoader loades images based on loader.scan.paths on filtered indicies,
    // load it for filtered loader.scan.paths, could be better in future for sorting
    loader: Arc<ImageLoader>,
    scan: Rc<ScanResult>,
    active_grid_indices: HashSet<usize>,
    filtered_indices: Vec<usize>,
    window_weak: slint::Weak<MainWindow>,
}

impl AppController {
    fn new(scan: Rc<ScanResult>, config: &Config, window: &MainWindow) -> Self {
        let total = scan.paths.len();
        Self {
            loader: Arc::new(ImageLoader::new(
                scan.paths.clone(),
                config.threads,
                config.window_size,
            )),
            scan,
            active_grid_indices: HashSet::new(),
            filtered_indices: (0..total).collect(),
            window_weak: window.as_weak(),
        }
    }

    fn handle_grid_request(&mut self, start: usize, count: usize) {
        let ui = match self.window_weak.upgrade() {
            Some(ui) => ui,
            None => return,
        };

        let margin = 30;
        let visible_range = start.saturating_sub(margin)..(start + count + margin);
        self.loader.prune_grid_thumbs(start, count);
        self.active_grid_indices
            .retain(|&idx| visible_range.contains(&idx));

        let model = ui.get_grid_model();
        let end = cmp::min(start + count, model.row_count());
        let mut cached_updates = Vec::new();

        for row in start..end {
            if self.active_grid_indices.contains(&row) {
                continue;
            }

            if let Some(item) = model.row_data(row) {
                self.active_grid_indices.insert(row);
                let abs_idx = item.index as usize;
                let weak = self.window_weak.clone();

                if let Some(buffer) =
                    self.loader
                        .load_grid_thumb(abs_idx, weak, move |ui, _, img| {
                            let m = ui.get_grid_model();
                            if let Some(mut it) = m.row_data(row) {
                                it.image = img;
                                m.set_row_data(row, it);
                            }
                        })
                {
                    cached_updates.push((row, buffer));
                }
            }
        }

        if !cached_updates.is_empty() {
            for (row, buf) in cached_updates {
                if let Some(mut item) = model.row_data(row) {
                    item.image = Image::from_rgba8(buf);
                    model.set_row_data(row, item);
                }
            }
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
            let total_filtered = self.filtered_indices.len();
            if total_filtered == 0 {
                return;
            }
            let curr_abs_idx = ui.get_curr_image_index() as usize;
            let curr_pos = self
                .filtered_indices
                .iter()
                .position(|&idx| idx == curr_abs_idx)
                .unwrap_or(0);
            let next_pos = (curr_pos as isize + delta).rem_euclid(total_filtered as isize) as usize;
            if let Some(&next_abs_idx) = self.filtered_indices.get(next_pos) {
                self.handle_full_view_load(next_abs_idx);
            }
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

    fn handle_search(&mut self, query: String) {
        let query = query.to_lowercase();

        self.filtered_indices = self
            .scan
            .paths
            .iter()
            .enumerate()
            .filter(|(_, path)| {
                query.is_empty()
                    || path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_lowercase().contains(&query))
                        .unwrap_or(false)
            })
            .map(|(idx, _)| idx)
            .collect();
        debug!(
            "query: \"{}\"\n filtered_indices: {:?}",
            query, self.filtered_indices
        );

        self.active_grid_indices.clear();
        self.loader.clear_thumbs();

        if let Some(ui) = self.window_weak.upgrade() {
            let items: Vec<GridItem> = self
                .filtered_indices
                .iter()
                .map(|&abs_idx| GridItem {
                    image: Image::default(),
                    index: abs_idx as i32,
                    selected: false,
                })
                .collect();

            ui.set_grid_model(Rc::new(VecModel::from(items)).into());
        }

        if let Some(&first_abs_idx) = self.filtered_indices.first() {
            self.loader
                .active_idx
                .store(first_abs_idx, Ordering::Relaxed);
            self.handle_full_view_load(first_abs_idx);
        }
        self.handle_grid_request(0, 50);
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
            selected: false,
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
    main_window.on_search_submitted(move |query| {
        c.borrow_mut().handle_search(query.to_string());
    });

    let c = controller.clone();
    main_window.on_image_selected(move |index| {
        c.borrow().handle_full_view_load(index as usize);
    });

    let c = controller.clone();
    main_window.on_toggle_select_all(move |select| {
        if let Some(ui) = c.borrow().window_weak.upgrade() {
            let model = ui.get_grid_model();

            for i in 0..model.row_count() {
                if let Some(mut item) = model.row_data(i) {
                    item.selected = select;
                    model.set_row_data(i, item);
                }
            }
        }
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
    main_window.set_view_mode(if scan_rc.is_dir {
        ViewMode::Grid
    } else {
        ViewMode::Full
    });

    setup_bindings(&main_window, &config);

    main_window.run()?;
    Ok(())
}

fn setup_bindings(main_window: &MainWindow, config: &Config) {
    let get_key = |action: &str| {
        Config::get_slint_key_string(config.bindings.get(action).unwrap_or_else(|| {
            panic!("Binding '{}' should already be populated by config", action)
        }))
    };

    main_window.set_bind_quit(get_key("quit"));
    main_window.set_bind_fullscreen(get_key("toggle_fullscreen"));
    main_window.set_bind_switch_view_mode(get_key("switch_view_mode"));
    main_window.set_bind_switch_mouse_mode(get_key("switch_mouse_mode"));
    main_window.set_bind_reset_zoom(get_key("reset_zoom"));
    main_window.set_bind_grid_pg_dn(get_key("grid_page_down"));
    main_window.set_bind_grid_pg_up(get_key("grid_page_up"));
}
