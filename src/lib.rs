slint::include_modules!();

mod app_state_cache;
pub mod config;
pub mod image_processing;
pub mod pipeline;
mod ui;

use config::Config;
use luminous_fs_scan::{ImageId, ScanResult};
use luminous_image_loader::{GridViewIdxs, ImageLoader, View};
use luminous_plugins::PluginManager;
use pipeline::GpuStepFactory;

use log::{debug, error, info, trace, warn};
use slint::{Image, Model, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) struct AppController {
    pub(crate) loader: Arc<ImageLoader>,
    pub(crate) scan: ScanResult,
    pub(crate) catalog_view: Vec<ImageId>,
    pub(crate) grid_view_idxs: Option<GridViewIdxs>,
    pub(crate) selected: HashSet<ImageId>,
    pub(crate) visible_model: Rc<VecModel<GridItem>>,
    pub(crate) query: Query,
    pub(crate) window_weak: slint::Weak<MainWindow>,
}

pub enum SortType {
    Name,
    Modified,
    Size,
    // Format,
}

pub struct Query {
    pub text: String,
    pub sort: SortType,
    pub asc: bool,
}

impl AppController {
    fn new(
        plugin_manager: PluginManager,
        scan: ScanResult,
        config: &Config,
        window: &MainWindow,
    ) -> Self {
        let window_weak = window.as_weak();
        let plugin_manager = Arc::new(plugin_manager);
        let mut loader = ImageLoader::new(
            config.threads,
            config.window_size,
            scan.is_dir,
            Arc::clone(&plugin_manager),
        );

        let weak_thumb = window_weak.clone();
        loader.on_thumb_ready(move |index, buffer| {
            let t_start = std::time::Instant::now();
            let _ = weak_thumb.upgrade_in_event_loop(move |ui| {
                let gv = ui.global::<GridViewState>();
                let img = Image::from_rgba8(buffer);
                let m = gv.get_visible_model();

                for row in 0..m.row_count() {
                    if let Some(mut item) = m.row_data(row) {
                        if item.abs_index == index.0 as i32 {
                            item.image = img.clone();
                            m.set_row_data(row, item);
                            break; // Found it
                        }
                    }
                }
                trace!(
                    "on_thumb_ready (id: {:?}): {:.3}ms",
                    index,
                    t_start.elapsed().as_secs_f64() * 1000.0
                );
            });
        });

        let weak_full = window_weak.clone();
        // let pm = Arc::clone(&plugin_manager);
        loader.on_full_ready(move |id, buf| {
            let _ = weak_full.upgrade_in_event_loop(move |ui| {
                let fv = ui.global::<FullViewState>();
                if id.0 == fv.get_curr_image_index() as usize {
                    fv.set_curr_image(Image::from_rgba8(buf));
                    fv.set_mask_overlay(Image::default());

                    // TODO: Auto set image in GUI
                    // for plugin in pm.get_interactive_plugins() {
                    //     let p = Arc::clone(plugin);
                    //     let buf = buffer.clone();
                    //     std::thread::spawn(move || {
                    //         p.set_interactive_image(&buf);
                    //     });
                    // }
                }
            });
        });

        Self {
            loader: Arc::new(loader),
            scan,
            catalog_view: vec![], //scan.image_entries.iter().map(|c| c.id).collect(),
            grid_view_idxs: None,
            selected: HashSet::new(),
            visible_model: Rc::new(VecModel::default()),
            query: Query {
                text: String::new(),
                sort: SortType::Name,
                asc: true,
            },
            window_weak: window.as_weak(), // Why weak?
        }
    }

    fn rebuild_view(&mut self) {
        debug!("rebuid_view");
        let q = self.query.text.to_lowercase();

        let mut ids: Vec<ImageId> = self
            .scan
            .image_entries
            .iter()
            .filter(|e| {
                q.is_empty()
                    || e.path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|s| s.to_lowercase().contains(&q))
                        .unwrap_or(false)
            })
            .map(|e| e.id)
            .collect();

        // TODO: plugin search next...

        let image_entries = |id: ImageId| &self.scan.image_entries[id.0 as usize];
        match self.query.sort {
            SortType::Name => {
                ids.sort_by(|&a, &b| image_entries(a).path.cmp(&image_entries(b).path))
            }
            SortType::Modified => ids.sort_by_key(|&id| image_entries(id).mtime),
            SortType::Size => ids.sort_by_key(|&id| image_entries(id).size),
        }
        if !self.query.asc {
            ids.reverse();
        }

        self.catalog_view = ids.clone();
        self.loader.set_catalog_view(ids);
        if let Some(grid_view_idxs) = self.grid_view_idxs {
            // FIX: catalog and grid_view_idxs must be updated before load, otherwise
            // race condition
            self.loader.load_grid_thumbs(&GridViewIdxs {
                start: 0,
                focus: 0,
                end: (grid_view_idxs.end - grid_view_idxs.start).min(self.catalog_view.len() - 1),
            });
            self.grid_view_idxs = None;
        }

        let weak_ui = self.window_weak.clone();
        let selected_count = self.selected.len() as i32;
        let total_images = self.catalog_view.len() as i32;
        let sort_asc = self.query.asc;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                let gv = ui.global::<GridViewState>();
                gv.set_selected_count(selected_count);
                gv.set_sort_asc(sort_asc);
                ui.set_total_images(total_images);
                ui.invoke_refresh_grid();
            }
        });

        if !self.catalog_view.is_empty() {
            self.handle_full_view_load(self.scan.start_index);
        }
    }

    // tmp
    fn set_sort_asceding(&mut self, ascending: bool) {
        self.query.asc = ascending;
    }

    fn set_search_text(&mut self, text: &str) {
        self.query.text = text.to_string();
    }

    fn handle_grid_request(&mut self, first_row: usize, visible_rows: usize, num_cols: usize) {
        let t_start = std::time::Instant::now();

        let buffer_rows = visible_rows.max(1);
        let start_row = first_row.saturating_sub(buffer_rows);
        let end_row = first_row + visible_rows + buffer_rows;

        let total = self.catalog_view.len();
        if self.catalog_view.is_empty() {
            return;
        }
        let gv_idxs = GridViewIdxs {
            start: (start_row * num_cols).min(total),
            focus: ((first_row + visible_rows / 2) * num_cols).min(total - 1),
            end: (end_row * num_cols).min(total),
        };
        if self.grid_view_idxs == Some(gv_idxs) {
            return;
        } else {
            self.grid_view_idxs = Some(gv_idxs);
            debug!(
                "Handle grid rows (st-en): {}-{} idxs {}",
                start_row, end_row, gv_idxs
            );
        }

        // only queries thumbs for load
        self.loader.load_grid_thumbs(&gv_idxs);

        let grid_vec: Vec<GridItem> = self.catalog_view[gv_idxs.start..gv_idxs.end]
            .iter()
            .enumerate()
            .map(|(i, &id)| GridItem {
                // TODO: rename the idxs to make sense
                abs_index: id.0 as i32,
                index: (gv_idxs.start + i) as i32,
                image: self
                    .loader
                    .get_grid_thumb(id)
                    .map(Image::from_rgba8)
                    .unwrap_or_default(),
                selected: self.selected.contains(&id),
            })
            .collect();
        self.visible_model.set_vec(grid_vec);

        let Some(ui) = self.window_weak.upgrade() else {
            return;
        };
        let gv = ui.global::<GridViewState>();
        gv.set_visible_model(self.visible_model.clone().into());

        trace!(
            "visible_model: {:?} catalog_view: {:?}",
            self.visible_model
                .iter()
                .map(|id| id.abs_index)
                .collect::<Vec<i32>>(),
            self.catalog_view
        );

        debug!(
            "Handle grid req: {:.3}ms",
            t_start.elapsed().as_secs_f64() * 1000.0
        );
    }

    fn handle_full_view_load(&self, index: usize) {
        let weak = self.window_weak.clone();
        let loader = self.loader.clone();

        let display_img = loader.load_full_progressive(index, false);

        if let Some(ui) = weak.upgrade() {
            let fv = ui.global::<FullViewState>();
            fv.set_curr_image(display_img);
            fv.set_mask_overlay(Image::default());
            fv.set_curr_image_index(index as i32);
            // TODO:
            // if let Some(name) = self.scan.image_entries.get(index).unwrap().path {
            //     fv.set_curr_image_name(name.into());
            // }
            // let row = self
            //     .filtered_indices
            //     .iter()
            //     .position(|&i| i == index)
            //     .unwrap_or(0);
            // ui.global::<GridViewState>().set_curr_grid_row(row as i32);
        }
    }

    // NOTE: serves next and prev images
    fn handle_navigate(&self, delta: isize) {
        let ui = match self.window_weak.upgrade() {
            Some(ui) => ui,
            None => return,
        };

        let total = self.catalog_view.len();
        if total == 0 {
            return;
        }

        let curr = ui.global::<FullViewState>().get_curr_image_index() as usize;
        let next_pos = (curr as isize + delta).rem_euclid(total as isize) as usize;
        self.handle_full_view_load(next_pos);
    }

    fn handle_edit_op(&mut self, _op: EditOp) {
        todo!();
    }

    fn handle_bucket_resolution(&mut self, resolution: u32) {
        self.loader.set_bucket_resolution(resolution);
    }

    fn handle_segmentation(
        &self,
        plugin_id: String,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        txt: String,
    ) {
        let weak = self.window_weak.clone();
        let loader = self.loader.clone();
        let before_idx = loader.active_idx.load(Ordering::Relaxed);

        std::thread::Builder::new()
            .name("segm".to_string())
            .spawn(move || {
                if let Some(plugin) = loader.plugin_manager.get_plugin_by_id(&plugin_id) {
                    if txt.len() > 0 {
                        if let Some(mask) = plugin.text_to_mask(txt) {
                            if before_idx == loader.active_idx.load(Ordering::Relaxed) {
                                let _ = weak.upgrade_in_event_loop(move |ui| {
                                    ui.global::<FullViewState>()
                                        .set_mask_overlay(Image::from_rgba8(mask));
                                });
                            } else {
                                debug!("Index has moved, not applying mask");
                            }
                        } else {
                            warn!("Text to mask failed");
                        }
                    } else if x2 < 0 || y2 < 0 {
                        if let Some(mask) = plugin.interactive_click(x1 as u32, y1 as u32) {
                            if before_idx == loader.active_idx.load(Ordering::Relaxed) {
                                let _ = weak.upgrade_in_event_loop(move |ui| {
                                    ui.global::<FullViewState>()
                                        .set_mask_overlay(Image::from_rgba8(mask));
                                });
                            } else {
                                debug!("Index has moved, not applying mask");
                            }
                        } else {
                            warn!("Interactive click failed");
                        }
                    } else if let Some(mask) =
                        plugin.interactive_rect_select(x1 as u32, y1 as u32, x2 as u32, y2 as u32)
                    {
                        if before_idx == loader.active_idx.load(Ordering::Relaxed) {
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                ui.global::<FullViewState>()
                                    .set_mask_overlay(Image::from_rgba8(mask));
                            });
                        } else {
                            debug!("Index has moved, not applying mask");
                        }
                    } else {
                        warn!("Interactive select failed");
                    }
                }
            })
            .expect("Failed to spawn segmentation thread");
    }

    // fn notify_interactive_plugin(plugin_id: String, loader: &Arc<ImageLoader>) {
    //     let loader = loader.clone();
    //     let plugin_manager = loader.plugin_manager.clone();
    //     let curr_active_path = loader.get_curr_img_path();
    //     let curr_active_buffer = loader.get_curr_active_buffer();
    //     loader.pool.spawn(move || {
    //         if let Some(plugin) = plugin_manager.get_plugin_by_id(&plugin_id) {
    //             if let Some(buf) = curr_active_buffer
    //                 && let Some(path) = curr_active_path
    //             {
    //                 plugin.set_interactive_image(&buf, &path);
    //             }
    //         }
    //     });
    // }

    fn toggle_select_all(&mut self, select: bool) {
        if select {
            self.scan
                .image_entries
                .iter()
                .enumerate()
                .for_each(|(i, _)| {
                    self.selected.insert(ImageId(i));
                });
        } else {
            self.selected.clear();
        }
    }

    fn toggle_select_range(&mut self, start_idx: usize, end_idx: usize) {
        let lo = start_idx.min(end_idx);
        let hi = start_idx.max(end_idx);
        let target = self.selected.contains(&self.catalog_view[start_idx]);

        for row in lo..=hi {
            let id = self.catalog_view[row];
            if target {
                self.selected.insert(id);
            } else {
                self.selected.remove(&id);
            }
        }
    }

    fn toggle_select_single(&mut self, index: i32) {
        debug!("handle_toggle_selection {}", index);
        let Some(ui) = self.window_weak.upgrade() else {
            return;
        };
        let gv = ui.global::<GridViewState>();
        let model = gv.get_visible_model();

        let mut found = false;
        let mut is_selected = false;

        for i in 0..model.row_count() {
            if let Some(mut item) = model.row_data(i) {
                if item.index == index {
                    item.selected = !item.selected;
                    is_selected = item.selected;
                    model.set_row_data(i, item.clone());
                    found = true;
                    break;
                }
            }
        }

        if !found {
            error!("handle_toggle_selection item not present");
            return;
        }

        let id = self.catalog_view[index as usize];
        if is_selected {
            self.selected.insert(id);
        } else {
            self.selected.remove(&id);
        }
        gv.set_selected_count(self.selected.len() as i32);
    }

    fn is_selected(&self, abs_index: i32) -> bool {
        self.selected.contains(&ImageId(abs_index as usize))
    }

    fn collect_selected_paths(&self) -> Vec<std::path::PathBuf> {
        self.selected
            .iter()
            .filter_map(|&image_id| {
                self.scan
                    .image_entries
                    .get(image_id.0 as usize)
                    .map(|item| item.path.clone())
            })
            .collect()
    }

    fn handle_open_images(app_controller: Rc<RefCell<Self>>) {
        let extra_exts = app_controller
            .borrow()
            .loader
            .plugin_manager
            .get_supported_extensions();

        if let Some(path) = rfd::FileDialog::new()
            .pick_folder()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
        {
            let scan = luminous_fs_scan::scan(&path, &extra_exts);
            if scan.image_entries.is_empty() {
                return;
            }

            AppController::set_scan(&app_controller, scan);
        }
    }

    fn set_scan(app_controller: &Rc<RefCell<AppController>>, scan: ScanResult) {
        let mut acc = app_controller.borrow_mut();
        let scan_len = scan.image_entries.len().try_into().unwrap_or(0);
        let is_dir = scan.is_dir;
        acc.scan = scan.clone();
        acc.catalog_view = vec![];
        acc.loader.set_catalog(scan.image_entries);
        acc.loader.set_catalog_view(vec![]);
        acc.rebuild_view();

        if let Some(ui) = acc.window_weak.upgrade() {
            let gv = ui.global::<GridViewState>();
            gv.set_selected_count(0);
            acc.selected.clear();
            ui.set_view_mode(if is_dir {
                ViewMode::Grid
            } else {
                ViewMode::Full
            });
            ui.set_total_images(scan_len);
        }

        drop(acc); // set exif borrows a ref
        ui::full_view_presenter::set_exif(app_controller);
    }
}

pub fn run(config: Config) -> Result<(), Box<dyn Error>> {
    info!("Starting Luminous");
    let init_start = std::time::Instant::now();
    let mut plugin_manager = luminous_plugins::PluginManager::new();

    let mut settings = ui::settings_presenter::read_settings()
        .unwrap_or_else(|| ui::settings_presenter::Settings { plugins: vec![] });

    if config.safe_mode {
        info!("Starting in safe mode");
    } else {
        let auto_start_ids: Vec<String> = settings
            .plugins
            .iter()
            .filter(|p| p.auto_start)
            .map(|p| p.id.clone())
            .collect();
        let discovered_ids = plugin_manager.discover(&auto_start_ids);
        settings.sync_plugins(discovered_ids);
        if let Err(e) = ui::settings_presenter::write_settings(&settings) {
            error!("Failed to save plugins settings: {}", e);
        }
    }

    let extra_exts = plugin_manager.get_supported_extensions();
    let scan = luminous_fs_scan::scan(&config.path, &extra_exts);
    let encoder_extensions = scan.image_formats.get_all_encoding_exts();

    let main_window = MainWindow::new()?;

    let cached_state = app_state_cache::load_app_state();
    {
        let win = main_window.window();
        if cached_state.width > 0 && cached_state.height > 0 {
            win.set_size(slint::PhysicalSize::new(
                cached_state.width,
                cached_state.height,
            ));
            win.set_position(slint::PhysicalPosition::new(cached_state.x, cached_state.y));
        }
        win.set_fullscreen(cached_state.fullscreen);

        let fv = main_window.global::<FullViewState>();
        fv.set_footer_visible(cached_state.full_view_footer_visible);
        fv.set_side_panel_visible(cached_state.full_view_side_panel_visible);

        let gv = main_window.global::<GridViewState>();
        gv.set_side_panel_visible(cached_state.grid_view_side_panel_visible);
        gv.set_grid_cols(cached_state.grid_view_cols as i32);
    }

    let is_scan_empty = scan.image_entries.is_empty();
    let app_controller = Rc::new(RefCell::new(AppController::new(
        plugin_manager,
        scan.clone(), // TODO: refactor, delete
        &config,
        &main_window,
    )));

    let factory = Arc::new(GpuStepFactory::new(false));

    ui::grid_view_presenter::register(&main_window, app_controller.clone());
    ui::full_view_presenter::register(&main_window, app_controller.clone());
    ui::pipeline_presenter::register(&main_window, app_controller.clone(), factory);
    ui::settings_presenter::register(&main_window, app_controller.clone());
    ui::bindings::setup(&main_window, &config);

    let acc = app_controller.clone();
    main_window.on_open_images(move || {
        AppController::handle_open_images(acc.clone());
    });

    let win_weak = main_window.as_weak();
    main_window.on_quit_app(move || {
        if let Some(mw) = win_weak.upgrade() {
            app_state_cache::save_app_state(&mw);
        }
        let _ = slint::quit_event_loop();
    });

    main_window.set_app_background(config.background);

    let acc = app_controller.clone();
    main_window.on_view_changed(move |mode| {
        let active_view = match mode {
            ViewMode::Grid => View::Grid,
            ViewMode::Full => View::Full,
        };
        acc.borrow().loader.set_active_view(active_view);
    });

    let mut sorted_exts: Vec<slint::SharedString> = encoder_extensions
        .into_iter()
        .map(slint::SharedString::from)
        .collect();
    sorted_exts.sort();
    let exts_model = std::rc::Rc::new(slint::VecModel::from(sorted_exts));
    main_window.set_encoder_extensions(exts_model.into());

    if !is_scan_empty {
        AppController::set_scan(&app_controller, scan);
    }

    debug!(
        "Init in {:.1} ms",
        init_start.elapsed().as_secs_f64() * 1000.0
    );
    main_window.run()?;
    Ok(())
}
