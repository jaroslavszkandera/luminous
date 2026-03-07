slint::include_modules!();

pub mod config;
pub mod fs_scan;
pub mod image_loader;
pub mod image_processing;
pub mod plugins;

use config::Config;
use fs_scan::ScanResult;
use image_loader::ImageLoader;
use image_processing::save_image;
use plugins::PluginManager;

use log::{debug, info};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::cmp;
use std::collections::HashSet;
use std::error::Error;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::image_processing::batch_save_images;

struct AppController {
    loader: Arc<ImageLoader>,
    scan: Rc<ScanResult>,
    active_grid_indices: HashSet<usize>,
    filtered_indices: Vec<usize>,
    window_weak: slint::Weak<MainWindow>,
}

impl AppController {
    fn new(
        plugin_manager: PluginManager,
        scan: Rc<ScanResult>,
        config: &Config,
        window: &MainWindow,
    ) -> Self {
        let total = scan.paths.len();
        Self {
            loader: Arc::new(ImageLoader::new(
                scan.paths.clone(),
                config.threads,
                config.window_size,
                plugin_manager,
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

        const MARGIN: usize = 30;
        let keep_start = start.saturating_sub(MARGIN);
        let keep_end = start + count + MARGIN;
        for row in 0..total {
            if row < keep_start || row > keep_end {
                if let Some(mut item) = model.row_data(row) {
                    if item.image != Image::default() {
                        item.image = Image::default();
                        model.set_row_data(row, item);
                    }
                }
            }
        }
        self.loader.prune_grid_thumbs(start, count);

        let visible: Vec<GridItem> = (start..end).filter_map(|i| model.row_data(i)).collect();
        ui.set_visible_grid_model(ModelRc::from(Rc::from(VecModel::from(visible))));

        let visible_range = keep_start..=keep_end;
        self.active_grid_indices
            .retain(|&idx| visible_range.contains(&idx));

        let mut cached_updates: Vec<(usize, SharedPixelBuffer<Rgba8Pixel>)> = Vec::new();

        for row in start..end {
            if self.active_grid_indices.contains(&row) {
                continue;
            }
            self.active_grid_indices.insert(row);

            let abs_idx = self.filtered_indices[row];
            let weak = self.window_weak.clone();

            match self
                .loader
                .load_grid_thumb(abs_idx, weak, move |ui, _, img| {
                    let m = ui.get_grid_model();
                    if let Some(mut item) = m.row_data(row) {
                        item.image = img.clone();
                        m.set_row_data(row, item);
                    }
                    let vm = ui.get_visible_grid_model();
                    for i in 0..vm.row_count() {
                        if let Some(mut v) = vm.row_data(i) {
                            if v.index == row as i32 {
                                v.image = img;
                                vm.set_row_data(i, v);
                                break;
                            }
                        }
                    }
                }) {
                Some(buf) => cached_updates.push((row, buf)),
                None => {}
            }
        }

        if cached_updates.is_empty() {
            return;
        }
        let vm = ui.get_visible_grid_model();
        for (row, buf) in cached_updates {
            let img = Image::from_rgba8(buf);
            if let Some(mut item) = model.row_data(row) {
                item.image = img.clone();
                model.set_row_data(row, item);
            }
            for i in 0..vm.row_count() {
                if let Some(mut v) = vm.row_data(i) {
                    if v.index == row as i32 {
                        v.image = img;
                        vm.set_row_data(i, v);
                        break;
                    }
                }
            }
        }
    }

    fn handle_full_view_load(&self, index: usize) {
        let weak = self.window_weak.clone();
        let loader = self.loader.clone();

        let loader_cb = loader.clone();
        let on_loaded = move |ui: MainWindow, img: Image| {
            ui.set_full_view_image(img);
            ui.set_mask_overlay(Image::default());
            Self::notify_interactive_plugin(&loader_cb);
        };

        let display_img = loader.load_full_progressive(index, weak.clone(), on_loaded);

        if let Some(ui) = weak.upgrade() {
            ui.set_full_view_image(display_img);
            ui.set_mask_overlay(Image::default());
            ui.set_curr_image_index(index as i32);
            if let Some(name) = loader.get_file_name(index) {
                ui.set_curr_image_name(name.into());
            }
            if loader.full_cache_contains(index) {
                Self::notify_interactive_plugin(&loader);
            }
        }

        let window_indices = self.build_window_indices(index);
        loader.update_sliding_window(index, window_indices);
    }

    fn handle_navigate(&self, delta: isize) {
        let ui = match self.window_weak.upgrade() {
            Some(ui) => ui,
            None => return,
        };
        let total = self.filtered_indices.len();
        if total == 0 {
            return;
        }
        let curr = ui.get_curr_image_index() as usize;
        let curr_pos = self
            .filtered_indices
            .iter()
            .position(|&i| i == curr)
            .unwrap_or(0);
        let next_pos = (curr_pos as isize + delta).rem_euclid(total as isize) as usize;
        if let Some(&next_abs) = self.filtered_indices.get(next_pos) {
            self.handle_full_view_load(next_abs);
        }
    }

    fn handle_rotate(&self, degrees: i32) {
        let Some(buffer) = self.loader.get_curr_active_buffer() else {
            return;
        };
        let loader = self.loader.clone();
        let weak = self.window_weak.clone();

        self.loader.pool.spawn(move || {
            let img = {
                let bytes: &[u8] = bytemuck::cast_slice(buffer.as_slice());
                image::RgbaImage::from_raw(buffer.width(), buffer.height(), bytes.to_vec())
            };

            let Some(img) = img else { return };

            let rotated = match degrees {
                90 => image::imageops::rotate90(&img),
                -90 | 270 => image::imageops::rotate270(&img),
                180 => image::imageops::rotate180(&img),
                _ => img,
            };

            let new_buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                rotated.as_raw(),
                rotated.width(),
                rotated.height(),
            );
            let active_idx = loader.active_idx.load(Ordering::Relaxed);
            loader.cache_buffer(active_idx, new_buf.clone());

            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_full_view_image(Image::from_rgba8(new_buf));
            });
        });
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

        debug!("query=\"{query}\" filtered={}", self.filtered_indices.len());

        let Some(ui) = self.window_weak.upgrade() else {
            return;
        };

        let items: Vec<GridItem> = self
            .filtered_indices
            .iter()
            .enumerate()
            .map(|(row, _)| GridItem {
                image: Image::default(),
                index: row as i32,
                selected: false,
            })
            .collect();

        ui.set_selected_count(0);
        ui.set_grid_model(Rc::new(VecModel::from(items)).into());

        if let Some(&first) = self.filtered_indices.first() {
            self.handle_full_view_load(first);
        }
        self.handle_grid_request(0, 50);
    }

    fn handle_toggle_selection(&self, index: i32) {
        let Some(ui) = self.window_weak.upgrade() else {
            return;
        };
        let model = ui.get_grid_model();
        let row = index as usize;

        let Some(mut item) = model.row_data(row) else {
            return;
        };
        item.selected = !item.selected;
        model.set_row_data(row, item.clone());

        ui.set_selected_count(ui.get_selected_count() + if item.selected { 1 } else { -1 });

        let vm = ui.get_visible_grid_model();
        for i in 0..vm.row_count() {
            if let Some(mut v) = vm.row_data(i) {
                if v.index == item.index {
                    v.selected = item.selected;
                    vm.set_row_data(i, v);
                    break;
                }
            }
        }
    }

    fn handle_segmentation(&self, x: u32, y: u32) {
        let weak = self.window_weak.clone();
        let loader = self.loader.clone();

        self.loader.pool.spawn(move || {
            if let Some(plugin) = loader.plugin_manager.get_interactive_plugin() {
                if let Some(mask) = plugin.interactive_click(x, y) {
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.set_mask_overlay(Image::from_rgba8(mask));
                    });
                }
            }
        });
    }

    fn build_window_indices(&self, center: usize) -> Vec<usize> {
        let len = self.filtered_indices.len();
        if len == 0 {
            return Vec::new();
        }
        let pos = self
            .filtered_indices
            .iter()
            .position(|&x| x == center)
            .unwrap_or(0);

        (1..=self.loader.window_size)
            .flat_map(|i| {
                let prev = (pos as isize - i as isize).rem_euclid(len as isize) as usize;
                let next = (pos + i).rem_euclid(len);
                [self.filtered_indices[prev], self.filtered_indices[next]]
            })
            .collect()
    }

    fn notify_interactive_plugin(loader: &Arc<ImageLoader>) {
        let loader = loader.clone();
        let plugin_manager = loader.plugin_manager.clone();
        let curr_active_buffer = loader.get_curr_active_buffer();
        loader.pool.spawn(move || {
            if let Some(plugin) = plugin_manager.get_interactive_plugin() {
                if let Some(buf) = curr_active_buffer {
                    plugin.set_interactive_image(&buf);
                }
            }
        });
    }

    fn collect_selected_paths(&self) -> Vec<std::path::PathBuf> {
        let Some(ui) = self.window_weak.upgrade() else {
            return Vec::new();
        };
        let model = ui.get_grid_model();
        (0..model.row_count())
            .filter_map(|i| {
                let item = model.row_data(i)?;
                if !item.selected {
                    return None;
                }
                let abs = *self.filtered_indices.get(item.index as usize)?;
                self.scan.paths.get(abs).cloned()
            })
            .collect()
    }
}

pub fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let mut plugin_manager = plugins::PluginManager::new();

    if !config.safe_mode {
        plugin_manager.discover(Path::new("plugins"));
    } else {
        info!("Starting in safe mode");
    }

    let extra_exts = plugin_manager.get_supported_extensions();
    let scan = fs_scan::scan(&config.path, &extra_exts);
    if scan.paths.is_empty() {
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
        plugin_manager,
        scan_rc.clone(),
        &config,
        &main_window,
    )));

    let c = controller.clone();
    main_window.on_request_grid_data(move |start, count| {
        c.borrow_mut()
            .handle_grid_request(start as usize, count as usize);
    });

    let c = controller.clone();
    main_window.on_bucket_resolution_changed(move |res| {
        c.borrow_mut().handle_bucket_resolution(res as u32);
    });

    let c = controller.clone();
    main_window.on_search_submitted(move |query| {
        c.borrow_mut().handle_search(query.to_string());
    });

    let c = controller.clone();
    main_window.on_image_selected(move |index| {
        let c_ref = c.borrow();
        if let Some(&abs) = c_ref.filtered_indices.get(index as usize) {
            c_ref.handle_full_view_load(abs);
        }
    });

    let c = controller.clone();
    main_window.on_toggle_select_all(move |select| {
        let Some(ui) = c.borrow().window_weak.upgrade() else {
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

    let c = controller.clone();
    main_window.on_toggle_selection(move |index| {
        c.borrow().handle_toggle_selection(index);
    });

    let c = controller.clone();
    main_window.on_request_range_select(move |start_idx, end_idx| {
        let Some(ui) = c.borrow().window_weak.upgrade() else {
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

    let c = controller.clone();
    main_window.on_print_selected_paths(move || {
        let paths = c.borrow().collect_selected_paths();
        if paths.is_empty() {
            log::info!("No files selected");
        } else {
            log::info!("Selected ({}):", paths.len());
            for p in &paths {
                log::info!("  {}", p.display());
            }
        }
    });

    let c = controller.clone();
    main_window.on_batch_save_with_format(move |format| {
        let (paths, weak_ui) = {
            let c_ref = c.borrow();
            let paths = c_ref.collect_selected_paths();
            let weak = c_ref.window_weak.clone();
            (paths, weak)
        };
        if paths.is_empty() {
            log::info!("No files selected");
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

    let c = controller.clone();
    main_window.on_request_next_image(move || c.borrow().handle_navigate(1));
    let c = controller.clone();
    main_window.on_request_prev_image(move || c.borrow().handle_navigate(-1));

    let c = controller.clone();
    main_window.on_rotate_plus_90(move || c.borrow().handle_rotate(90));
    let c = controller.clone();
    main_window.on_rotate_minus_90(move || c.borrow().handle_rotate(-90));

    let c = controller.clone();
    main_window.on_save_with_format(move |format| {
        let (img, path, weak_ui) = {
            let c_ref = c.borrow();
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

    let c = controller.clone();
    main_window.on_request_segmentation(move |x, y| {
        debug!("Segmentation [{x},{y}]");
        c.borrow().handle_segmentation(x as u32, y as u32);
    });

    main_window.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });

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
        Config::get_slint_key_string(
            config
                .bindings
                .get(action)
                .unwrap_or_else(|| panic!("Binding '{action}' not in config")),
        )
    };
    main_window.set_bind_quit(get_key("quit"));
    main_window.set_bind_fullscreen(get_key("toggle_fullscreen"));
    main_window.set_bind_switch_view_mode(get_key("switch_view_mode"));
    main_window.set_bind_switch_mouse_mode(get_key("switch_mouse_mode"));
    main_window.set_bind_reset_zoom(get_key("reset_zoom"));
    main_window.set_bind_grid_pg_dn(get_key("grid_page_down"));
    main_window.set_bind_grid_pg_up(get_key("grid_page_up"));
}
