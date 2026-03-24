slint::include_modules!();

pub mod config;
pub mod fs_scan;
pub mod image_loader;
pub mod image_processing;
pub mod pipeline;
pub mod plugins;
mod ui;

use config::Config;
use fs_scan::ScanResult;
use image_loader::ImageLoader;
use pipeline::StepFactory;
use plugins::PluginManager;

#[allow(unused_imports)]
use log::{debug, error, info, warn};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::cmp;
use std::collections::HashSet;
use std::error::Error;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) struct AppController {
    pub(crate) loader: Arc<ImageLoader>,
    pub(crate) scan: Rc<ScanResult>,
    pub(crate) active_grid_indices: HashSet<usize>,
    pub(crate) filtered_indices: Vec<usize>,
    pub(crate) window_weak: slint::Weak<MainWindow>,
}

impl AppController {
    fn new(
        plugin_manager: PluginManager,
        scan: Rc<ScanResult>,
        config: &Config,
        window: &MainWindow,
    ) -> Self {
        let window_weak = window.as_weak();
        let plugin_manager = Arc::new(plugin_manager);
        let mut loader = ImageLoader::new(
            scan.paths.clone(),
            config.threads,
            config.window_size,
            Arc::clone(&plugin_manager),
        );

        let weak_thumb = window_weak.clone();
        loader.on_thumb_ready(move |index, buffer| {
            let _ = weak_thumb.upgrade_in_event_loop(move |ui| {
                let img = Image::from_rgba8(buffer);
                let m = ui.get_grid_model();
                if let Some(mut item) = m.row_data(index) {
                    item.image = img.clone();
                    m.set_row_data(index, item);
                }
                let vm = ui.get_visible_grid_model();
                for i in 0..vm.row_count() {
                    if let Some(mut v) = vm.row_data(i) {
                        if v.index == index as i32 {
                            v.image = img;
                            vm.set_row_data(i, v);
                            break;
                        }
                    }
                }
            });
        });

        let weak_full = window_weak.clone();
        let pm = Arc::clone(&plugin_manager);
        loader.on_full_ready(move |index, buffer| {
            if let Some(plugin) = pm.get_interactive_plugin() {
                let buf = buffer.clone();
                std::thread::spawn(move || {
                    plugin.set_interactive_image(&buf);
                });
            }
            let _ = weak_full.upgrade_in_event_loop(move |ui| {
                let img = Image::from_rgba8(buffer);
                let fv = ui.global::<FullViewState>();
                if index == fv.get_curr_image_index() as usize {
                    fv.set_curr_image(img);
                    ui.set_mask_overlay(Image::default());
                }
            });
        });

        let total = scan.paths.len();
        Self {
            loader: Arc::new(loader),
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
            if let Some(buf) = self.loader.load_grid_thumb(abs_idx) {
                cached_updates.push((row, buf));
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

        let display_img = loader.load_full_progressive(index);

        if let Some(ui) = weak.upgrade() {
            let fv = ui.global::<FullViewState>();
            fv.set_curr_image(display_img);
            ui.set_mask_overlay(Image::default());
            fv.set_curr_image_index(index as i32);
            if let Some(name) = loader.get_file_name(index) {
                fv.set_curr_image_name(name.into());
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
        let curr = ui.global::<FullViewState>().get_curr_image_index() as usize;
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

    fn handle_edit_op(&self, op: EditOp) {
        let Some(buffer) = self.loader.get_curr_active_buffer() else {
            return;
        };
        let loader = self.loader.clone();
        let weak = self.window_weak.clone();

        self.loader.pool.spawn(move || {
            let bytes: &[u8] = bytemuck::cast_slice(buffer.as_slice());
            let Some(rgba) =
                image::RgbaImage::from_raw(buffer.width(), buffer.height(), bytes.to_vec())
            else {
                return;
            };

            let img = image::DynamicImage::ImageRgba8(rgba);

            let mut save_to_cache = false;
            let result = match op.kind {
                EditOpKind::RotateCW => {
                    save_to_cache = true;
                    image::DynamicImage::ImageRgba8(image::imageops::rotate90(&img.to_rgba8()))
                }
                EditOpKind::RotateCCW => {
                    save_to_cache = true;
                    image::DynamicImage::ImageRgba8(image::imageops::rotate270(&img.to_rgba8()))
                }
                EditOpKind::FlipH => {
                    save_to_cache = true;
                    image::DynamicImage::ImageRgba8(image::imageops::flip_horizontal(
                        &img.to_rgba8(),
                    ))
                }
                EditOpKind::FlipV => {
                    save_to_cache = true;
                    image::DynamicImage::ImageRgba8(image::imageops::flip_vertical(&img.to_rgba8()))
                }
                EditOpKind::Brighten => img.brighten(op.int_val),
                EditOpKind::Contrast => img.adjust_contrast(op.float_val),
            };

            let new_buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                result.to_rgba8().as_raw(),
                result.width(),
                result.height(),
            );

            // FIX: If next/prev image is requested quickly, it can be saved into a different
            // cache idx.
            if save_to_cache {
                let active_idx = loader.active_idx.load(Ordering::Relaxed);
                loader.cache_buffer(active_idx, new_buf.clone());
            }

            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.global::<FullViewState>()
                    .set_curr_image(Image::from_rgba8(new_buf));
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

    fn handle_segmentation(&self, x1: i32, y1: i32, x2: i32, y2: i32) {
        let weak = self.window_weak.clone();
        let loader = self.loader.clone();

        if let Some(plugin) = loader.plugin_manager.get_interactive_plugin() {
            if x2 < 0 || y2 < 0 {
                if let Some(mask) = plugin.interactive_click(x1 as u32, y1 as u32) {
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.set_mask_overlay(Image::from_rgba8(mask));
                    });
                } else {
                    warn!("Interactive click failed");
                }
            } else if let Some(mask) =
                plugin.interactive_rect_select(x1 as u32, y1 as u32, x2 as u32, y2 as u32)
            {
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_mask_overlay(Image::from_rgba8(mask));
                });
            } else {
                warn!("Interactive select failed");
            }
        }
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

    pub(crate) fn collect_selected_paths(&self) -> Vec<std::path::PathBuf> {
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
    info!("Starting Luminous");
    let init_start = std::time::Instant::now();
    let mut plugin_manager = plugins::PluginManager::new();

    if !config.safe_mode {
        plugin_manager.discover(Path::new("plugins"));
    } else {
        info!("Starting in safe mode");
    }

    let extra_exts = plugin_manager.get_supported_extensions();
    let scan = fs_scan::scan(&config.path, &extra_exts);
    if scan.paths.is_empty() {
        error!("No supported images found in {}", config.path);
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
    let app_controller = Rc::new(RefCell::new(AppController::new(
        plugin_manager,
        scan_rc.clone(),
        &config,
        &main_window,
    )));

    let factory = Arc::new(StepFactory::new());

    ui::grid_view_presenter::register(&main_window, app_controller.clone());
    ui::full_view_presenter::register(&main_window, app_controller.clone());
    ui::pipeline_presenter::register(&main_window, app_controller.clone(), factory);
    ui::bindings::setup(&main_window, &config);

    main_window.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });

    main_window.set_app_background(config.background);
    main_window.set_view_mode(if scan_rc.is_dir {
        ViewMode::Grid
    } else {
        ViewMode::Full
    });

    if !scan_rc.paths.is_empty() {
        app_controller
            .borrow()
            .handle_full_view_load(scan_rc.start_index);
    }

    debug!(
        "Init in {:.1} ms",
        init_start.elapsed().as_secs_f64() * 1000.0
    );
    main_window.run()?;
    Ok(())
}
