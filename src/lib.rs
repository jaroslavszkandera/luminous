slint::include_modules!();

pub mod config;
pub mod fs_scan;
mod image_loader;
pub mod plugins;

use config::Config;
use fs_scan::ScanResult;
use image_loader::ImageLoader;

use log::debug;
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
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

        let model = ui.get_grid_model();
        let total = model.row_count();
        let end = cmp::min(start + count, total);

        let mut visible_items = Vec::new();
        for i in start..end {
            if let Some(item) = model.row_data(i) {
                visible_items.push(item);
            }
        }
        ui.set_visible_grid_model(ModelRc::from(Rc::from(VecModel::from(visible_items))));

        let margin = 30;
        let visible_range = start.saturating_sub(margin)..(start + count + margin);
        self.loader.prune_grid_thumbs(start, count);
        self.active_grid_indices
            .retain(|&idx| visible_range.contains(&idx));
        let mut cached_updates = Vec::new();

        for row in start..end {
            if self.active_grid_indices.contains(&row) {
                continue;
            }

            self.active_grid_indices.insert(row);
            let abs_idx = self.filtered_indices[row];
            let weak = self.window_weak.clone();

            if let Some(buffer) = self
                .loader
                .load_grid_thumb(abs_idx, weak, move |ui, _, img| {
                    let m = ui.get_grid_model();
                    if let Some(mut it) = m.row_data(row) {
                        it.image = img.clone();
                        m.set_row_data(row, it.clone());

                        let vm = ui.get_visible_grid_model();
                        for i in 0..vm.row_count() {
                            if let Some(mut v_it) = vm.row_data(i) {
                                if v_it.index == it.index {
                                    v_it.image = img;
                                    vm.set_row_data(i, v_it);
                                    break;
                                }
                            }
                        }
                    }
                })
            {
                cached_updates.push((row, buffer));
            }
        }

        if !cached_updates.is_empty() {
            let vm = ui.get_visible_grid_model();
            for (row, buf) in cached_updates {
                let img = Image::from_rgba8(buf);
                if let Some(mut item) = model.row_data(row) {
                    item.image = img.clone();
                    model.set_row_data(row, item.clone());

                    for i in 0..vm.row_count() {
                        if let Some(mut v_it) = vm.row_data(i) {
                            if v_it.index == item.index {
                                v_it.image = img;
                                vm.set_row_data(i, v_it);
                                break;
                            }
                        }
                    }
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

        let mut window_indices = Vec::new();
        let len = self.filtered_indices.len();

        if len > 0 {
            let curr_pos = self
                .filtered_indices
                .iter()
                .position(|&x| x == index)
                .unwrap_or(0);
            for i in 1..=loader.window_size {
                let prev_pos = (curr_pos as isize - i as isize).rem_euclid(len as isize) as usize;
                let next_pos = (curr_pos + i).rem_euclid(len);
                window_indices.push(self.filtered_indices[prev_pos]);
                window_indices.push(self.filtered_indices[next_pos]);
            }
        }

        self.loader.update_sliding_window(index, window_indices);
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

        self.active_grid_indices.clear();
        self.loader.clear_thumbs();

        debug!(
            "query: \"{}\"\n filtered_indices: {:?}",
            query, self.filtered_indices
        );

        if let Some(ui) = self.window_weak.upgrade() {
            let items: Vec<GridItem> = self
                .filtered_indices
                .iter()
                .enumerate()
                .map(|(row_idx, &_abs_idx)| GridItem {
                    image: Image::default(),
                    index: row_idx as i32,
                    selected: false,
                })
                .collect();

            ui.set_selected_count(0);
            ui.set_grid_model(Rc::new(VecModel::from(items)).into());

            if let Some(&first_abs_idx) = self.filtered_indices.first() {
                self.loader
                    .active_idx
                    .store(first_abs_idx, Ordering::Relaxed);
                self.handle_full_view_load(first_abs_idx);
            }

            self.handle_grid_request(0, 50);
        }
    }

    fn handle_toggle_selection(&self, index: i32) {
        if let Some(ui) = self.window_weak.upgrade() {
            let model = ui.get_grid_model();
            let row_idx = index as usize;

            if let Some(mut item) = model.row_data(row_idx) {
                item.selected = !item.selected;
                model.set_row_data(row_idx, item.clone());

                let current_count = ui.get_selected_count();
                ui.set_selected_count(if item.selected {
                    current_count + 1
                } else {
                    current_count - 1
                });

                let vm = ui.get_visible_grid_model();
                for i in 0..vm.row_count() {
                    if let Some(mut v_it) = vm.row_data(i) {
                        if v_it.index == item.index {
                            v_it.selected = item.selected;
                            vm.set_row_data(i, v_it);
                            break;
                        }
                    }
                }
            }
        }
    }
}

pub fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let scan = fs_scan::scan(&config.path, &[]);

    if scan.paths.is_empty() {
        // TODO: File manager pop-up
        log::error!("No supported images found in {}", config.path);
        return Err(format!("No images in {}", config.path).into());
    }

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
        let c_ref = c.borrow();
        let row_idx = index as usize;
        if let Some(&abs_idx) = c_ref.filtered_indices.get(row_idx) {
            c_ref.handle_full_view_load(abs_idx);
        }
    });

    let c = controller.clone();
    main_window.on_toggle_select_all(move |select| {
        if let Some(ui) = c.borrow().window_weak.upgrade() {
            let model = ui.get_grid_model();
            let visible_model = ui.get_visible_grid_model();

            for i in 0..model.row_count() {
                if let Some(mut item) = model.row_data(i) {
                    if item.selected != select {
                        item.selected = select;
                        model.set_row_data(i, item.clone());

                        for j in 0..visible_model.row_count() {
                            if let Some(mut v_item) = visible_model.row_data(j) {
                                if v_item.index == item.index {
                                    v_item.selected = select;
                                    visible_model.set_row_data(j, v_item);
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let c = controller.clone();
    main_window.on_toggle_selection(move |index| {
        c.borrow().handle_toggle_selection(index);
    });

    let c = controller.clone();
    main_window.on_request_range_select(move |start_idx, end_idx| {
        let ui = c.borrow().window_weak.upgrade().unwrap();
        let model = ui.get_grid_model();
        let visible_model = ui.get_visible_grid_model();

        let target_state = model
            .row_data(start_idx as usize)
            .map(|item| item.selected)
            .unwrap_or(true);
        let (min, max) = (
            start_idx.min(end_idx) as usize,
            start_idx.max(end_idx) as usize,
        );
        let mut total_selected = 0;

        for i in 0..model.row_count() {
            if let Some(mut item) = model.row_data(i) {
                let should_be_selected = (i >= min && i <= max) && target_state;
                if item.selected != should_be_selected {
                    item.selected = should_be_selected;
                    model.set_row_data(i, item.clone());
                }
                for j in 0..visible_model.row_count() {
                    if let Some(mut v_item) = visible_model.row_data(j) {
                        if v_item.index == item.index {
                            v_item.selected = should_be_selected;
                            visible_model.set_row_data(j, v_item);
                        }
                    }
                }
                if should_be_selected {
                    total_selected += 1;
                }
            }
        }

        ui.set_selected_count(total_selected);
    });

    let c = controller.clone();
    main_window.on_print_selected_paths(move || {
        let c_ref = c.borrow();
        let ui = c_ref.window_weak.upgrade().unwrap();
        let model = ui.get_grid_model();
        let paths = c_ref.scan.paths.clone();
        let filtered = &c_ref.filtered_indices;

        let mut selected_paths = Vec::new();

        for i in 0..model.row_count() {
            if let Some(item) = model.row_data(i) {
                if item.selected {
                    let row_idx = item.index as usize;
                    if let Some(&abs_idx) = filtered.get(row_idx) {
                        if let Some(path) = paths.get(abs_idx) {
                            selected_paths.push(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        if selected_paths.is_empty() {
            log::info!("No files selected");
            return;
        }
        log::info!(
            "Files selected ({:}): {:?}",
            selected_paths.len(),
            selected_paths
        );
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
