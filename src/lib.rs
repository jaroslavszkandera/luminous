slint::include_modules!();

pub mod config;
pub mod fs_scan;
pub mod image_processing;
pub mod pipeline;
mod ui;

use config::Config;
use fs_scan::ScanResult;
use luminous_image_loader::ImageLoader;
use luminous_plugins::PluginManager;
use pipeline::StepFactory;

#[allow(unused_imports)]
use log::{debug, error, info, warn};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::cmp;
use std::collections::HashSet;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) struct AppController {
    pub(crate) loader: Arc<ImageLoader>,
    pub(crate) scan: Arc<ScanResult>,
    pub(crate) active_grid_indices: HashSet<usize>,
    pub(crate) filtered_indices: Vec<usize>,
    pub(crate) window_weak: slint::Weak<MainWindow>,
}

impl AppController {
    fn new(
        plugin_manager: PluginManager,
        scan: Arc<ScanResult>,
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
                let gv = ui.global::<GridViewState>();
                let img = Image::from_rgba8(buffer);
                let m = gv.get_model();

                for row in 0..m.row_count() {
                    if let Some(mut item) = m.row_data(row) {
                        if item.abs_index == index as i32 {
                            item.image = img.clone();
                            m.set_row_data(row, item);
                            break; // Found it
                        }
                    }
                }

                let vm = gv.get_visible_model();
                for i in 0..vm.row_count() {
                    if let Some(mut v) = vm.row_data(i) {
                        if v.abs_index == index as i32 {
                            v.image = img;
                            vm.set_row_data(i, v);
                            break;
                        }
                    }
                }
            });
        });

        let weak_full = window_weak.clone();
        // let pm = Arc::clone(&plugin_manager);
        loader.on_full_ready(move |index, buffer| {
            // NOTE: Why is it here?
            // TODO: Auto set image in GUI
            // for plugin in pm.get_interactive_plugins() {
            //     let p = Arc::clone(plugin);
            //     let buf = buffer.clone();
            //     std::thread::spawn(move || {
            //         p.set_interactive_image(&buf);
            //     });
            // }
            let _ = weak_full.upgrade_in_event_loop(move |ui| {
                let img = Image::from_rgba8(buffer);
                let fv = ui.global::<FullViewState>();
                if index == fv.get_curr_image_index() as usize {
                    fv.set_curr_image(img);
                    fv.set_mask_overlay(Image::default());
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
        // debug!("handle grid request");
        let Some(ui) = self.window_weak.upgrade() else {
            return;
        };
        let gv = ui.global::<GridViewState>();

        let model = gv.get_model();
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
        gv.set_visible_model(ModelRc::from(Rc::from(VecModel::from(visible))));

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
                self.active_grid_indices.insert(row);
                cached_updates.push((row, buf));
            }
        }

        if cached_updates.is_empty() {
            return;
        }
        let vm = gv.get_visible_model();
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
        let pm = self.loader.plugin_manager.clone();

        let display_img = loader.load_full_progressive(index, false);

        if let Some(ui) = weak.upgrade() {
            let fv = ui.global::<FullViewState>();
            fv.set_curr_image(display_img);
            fv.set_mask_overlay(Image::default());
            fv.set_curr_image_index(index as i32);
            if let Some(name) = loader.get_file_name(index) {
                fv.set_curr_image_name(name.into());
            }
            if loader.full_cache_contains(index) {
                for plugin in pm.get_interactive_plugins() {
                    // TODO: auto send image in GUI
                    Self::notify_interactive_plugin(plugin.id.clone(), &loader);
                }
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

    // TODO: How to not reload images from disk and keep the cache consistent?
    fn handle_edit_op(&mut self, op: EditOp) {
        let Some(buffer) = self.loader.get_curr_active_buffer() else {
            return;
        };

        let loader = self.loader.clone();
        let before_idx = loader.active_idx.load(Ordering::Relaxed);
        if let EditOpKind::Delete = op.kind {
            if let Some(p) = loader.get_path(before_idx) {
                let _ = trash::delete(&p);
            }
            loader.rm_img(before_idx);

            let pos = self.filtered_indices.iter().position(|&i| i == before_idx);
            if let Some(p) = pos {
                self.filtered_indices.remove(p);
            }
            self.filtered_indices.iter_mut().for_each(|idx| {
                if *idx > before_idx {
                    *idx -= 1;
                }
            });

            self.active_grid_indices.clear();
            loader.clear_thumbs();

            if let Some(ui) = self.window_weak.upgrade() {
                let filtered_items: Vec<GridItem> = self
                    .filtered_indices
                    .iter()
                    .enumerate()
                    .map(|(r, &idx)| GridItem {
                        image: Image::default(),
                        index: r as i32,
                        abs_index: idx as i32,
                        selected: false,
                    })
                    .collect();

                let gv = ui.global::<GridViewState>();
                gv.set_model(Rc::new(VecModel::from(filtered_items)).into());

                if self.filtered_indices.is_empty() {
                    let fv = ui.global::<FullViewState>();
                    fv.set_curr_image(Image::default());
                    fv.set_curr_image_name("No images".into());
                } else {
                    let next_pos = pos
                        .unwrap_or(0)
                        .min(self.filtered_indices.len().saturating_sub(1));
                    let next_abs = self.filtered_indices[next_pos];
                    self.handle_full_view_load(next_abs);
                }
                self.handle_grid_request(0, 50);
            }
            return;
        }

        let weak = self.window_weak.clone();
        let selection = weak
            .upgrade()
            .map(|window| window.global::<FullViewState>().get_selection())
            .unwrap_or_default();

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
                EditOpKind::Crop => {
                    save_to_cache = true;
                    img.crop_imm(
                        selection.x as u32,
                        selection.y as u32,
                        selection.w as u32,
                        selection.h as u32,
                    )
                }
                EditOpKind::ColorSpace => match op.string_val.as_str() {
                    "RGB" => {
                        loader.load_full_progressive(before_idx, true);
                        return;
                    }
                    "HSV" => {
                        let rgba = img.to_rgba8();
                        let hsv_img =
                            image::ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
                                let p = rgba.get_pixel(x, y);
                                let srgb = palette::Srgb::new(
                                    p[0] as f32 / 255.0,
                                    p[1] as f32 / 255.0,
                                    p[2] as f32 / 255.0,
                                );
                                let hsv: palette::Hsv = palette::IntoColor::into_color(srgb);

                                let h = hsv.hue.into_positive_degrees();
                                let h_u8 = if h.is_nan() {
                                    0
                                } else {
                                    (h / 360.0 * 255.0).round() as u8
                                };
                                let s_u8 = (hsv.saturation * 255.0).round() as u8;
                                let v_u8 = (hsv.value * 255.0).round() as u8;

                                image::Rgba([h_u8, s_u8, v_u8, p[3]])
                            });
                        image::DynamicImage::ImageRgba8(hsv_img)
                    }
                    "Gray" => image::DynamicImage::ImageLumaA8(img.to_luma_alpha8()),
                    "Red" | "Green" | "Blue" => {
                        let rgba = img.to_rgba8();
                        let channel_idx = match op.string_val.as_str() {
                            "Red" => 0,
                            "Green" => 1,
                            "Blue" => 2,
                            _ => 0,
                        };
                        let luma_a =
                            image::ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
                                let p = rgba.get_pixel(x, y);
                                image::LumaA([p[channel_idx], p[3]])
                            });
                        image::DynamicImage::ImageLumaA8(luma_a)
                    }
                    "Hue" | "Saturation" | "Value" => {
                        let rgba = img.to_rgba8();
                        let mode = op.string_val.clone();
                        let luma_a =
                            image::ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
                                let p = rgba.get_pixel(x, y);
                                let srgb = palette::Srgb::new(
                                    p[0] as f32 / 255.0,
                                    p[1] as f32 / 255.0,
                                    p[2] as f32 / 255.0,
                                );
                                let hsv: palette::Hsv = palette::IntoColor::into_color(srgb);
                                let val = match mode.as_str() {
                                    "Hue" => {
                                        let h = hsv.hue.into_positive_degrees();
                                        if h.is_nan() {
                                            0
                                        } else {
                                            (h / 360.0 * 255.0).round() as u8
                                        }
                                    }
                                    "Saturation" => (hsv.saturation * 255.0).round() as u8,
                                    "Value" => (hsv.value * 255.0).round() as u8,
                                    _ => 0,
                                };
                                image::LumaA([val, p[3]])
                            });
                        image::DynamicImage::ImageLumaA8(luma_a)
                    }
                    _ => img,
                },
                EditOpKind::Reset => {
                    loader.load_full_progressive(before_idx, true);
                    return;
                }
                EditOpKind::Copy => {
                    match arboard::Clipboard::new() {
                        Ok(mut clipboard) => {
                            let image_data = arboard::ImageData {
                                width: buffer.width() as usize,
                                height: buffer.height() as usize,
                                bytes: std::borrow::Cow::Borrowed(bytemuck::cast_slice(
                                    buffer.as_slice(),
                                )),
                            };
                            if let Err(e) = clipboard.set_image(image_data) {
                                error!("Clipboard copy failed: {e}");
                            } else {
                                debug!(
                                    "Clipboard copy of {:?} successful",
                                    loader.get_file_name(before_idx)
                                );
                            }
                        }
                        Err(e) => error!("Could not initialize clipboard: {e}"),
                    }
                    return;
                }
                EditOpKind::Delete => {
                    unreachable!("Delete should have been handled already");
                }
            };

            let new_buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                result.to_rgba8().as_raw(),
                result.width(),
                result.height(),
            );

            let active_idx = loader.active_idx.load(Ordering::Relaxed);
            if before_idx == active_idx {
                if save_to_cache {
                    loader.cache_buffer(active_idx, new_buf.clone());
                }

                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<FullViewState>()
                        .set_curr_image(Image::from_rgba8(new_buf));
                    ui.invoke_return_focus();
                });
            }
        });
    }

    fn handle_bucket_resolution(&mut self, resolution: u32) {
        self.loader.set_bucket_resolution(resolution);
        self.active_grid_indices.clear();
    }

    fn handle_search(&mut self, query: String) {
        let start = std::time::Instant::now();
        let query = query.to_lowercase();

        // First pass by file name
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

        // Second pass with plugins
        if !query.is_empty() {
            // TODO: set deadline for plugin(s) search
            for search_plugin in self.loader.plugin_manager.get_search_plugins() {
                debug!("Available search plugin: {}", search_plugin.id);
                if !search_plugin.is_running() {
                    warn!(
                        "Search plugin {} is registered but not running.",
                        search_plugin.id
                    );
                } else {
                    if let Some(semantic_search_paths) =
                        search_plugin.semantic_image_search(&self.scan.paths, &query)
                    {
                        debug!("semantic image search paths: {:?}", semantic_search_paths);
                        let semantic_indices: Vec<usize> = semantic_search_paths
                            .iter()
                            .filter_map(|p| self.scan.paths.iter().position(|sp| sp == p))
                            .collect();

                        let mut combined = self.filtered_indices.clone();
                        for idx in semantic_indices {
                            if !combined.contains(&idx) {
                                combined.push(idx);
                            }
                        }
                        self.filtered_indices = combined;
                    }
                }
            }
        }

        self.active_grid_indices.clear();
        self.loader.clear_thumbs();

        debug!("query=\"{query}\" filtered={}", self.filtered_indices.len());

        let Some(ui) = self.window_weak.upgrade() else {
            return;
        };

        let filtered_items: Vec<GridItem> = self
            .filtered_indices
            .iter()
            .enumerate()
            .map(|(row, _)| GridItem {
                image: Image::default(),
                index: row as i32,
                abs_index: self.filtered_indices[row] as i32,
                selected: false,
            })
            .collect();

        let gv = ui.global::<GridViewState>();
        gv.set_selected_count(0);
        gv.set_model(Rc::new(VecModel::from(filtered_items)).into());

        if let Some(&first) = self.filtered_indices.first() {
            self.handle_full_view_load(first);
        }
        self.handle_grid_request(0, 50);
        let weak_ui = self.window_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                ui.invoke_return_focus();
            }
        });
        debug!("Search in {}ms", start.elapsed().as_secs_f64() * 1000.0);
    }

    fn handle_toggle_selection(&self, index: i32) {
        let Some(ui) = self.window_weak.upgrade() else {
            return;
        };
        let gv = ui.global::<GridViewState>();
        let model = gv.get_model();
        let row = index as usize;

        let Some(mut item) = model.row_data(row) else {
            return;
        };
        item.selected = !item.selected;
        model.set_row_data(row, item.clone());

        gv.set_selected_count(gv.get_selected_count() + if item.selected { 1 } else { -1 });

        let vm = gv.get_visible_model();
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

    fn notify_interactive_plugin(plugin_id: String, loader: &Arc<ImageLoader>) {
        let loader = loader.clone();
        let plugin_manager = loader.plugin_manager.clone();
        let curr_active_path = loader.get_curr_img_path();
        let curr_active_buffer = loader.get_curr_active_buffer();
        loader.pool.spawn(move || {
            if let Some(plugin) = plugin_manager.get_plugin_by_id(&plugin_id) {
                if let Some(buf) = curr_active_buffer
                    && let Some(path) = curr_active_path
                {
                    plugin.set_interactive_image(&buf, &path);
                }
            }
        });
    }

    pub(crate) fn collect_selected_paths(&self) -> Vec<std::path::PathBuf> {
        let Some(ui) = self.window_weak.upgrade() else {
            return Vec::new();
        };
        let model = ui.global::<GridViewState>().get_model();
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

    fn handle_open_images(controller_rc: Rc<RefCell<Self>>) {
        let extra_exts = controller_rc
            .borrow()
            .loader
            .plugin_manager
            .get_supported_extensions();

        if let Some(path) = rfd::FileDialog::new()
            .pick_folder()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
        {
            let scan = Arc::new(fs_scan::scan(&path, &extra_exts));
            if scan.paths.is_empty() {
                return;
            }

            controller_rc.borrow_mut().replace_scan(scan);
        }
    }

    fn replace_scan(&mut self, scan: Arc<ScanResult>) {
        self.scan = scan.clone();
        self.loader.update_paths(scan.paths.clone());
        self.filtered_indices = (0..scan.paths.len()).collect();
        self.active_grid_indices.clear();

        if let Some(ui) = self.window_weak.upgrade() {
            let grid_data: Vec<GridItem> = scan
                .paths
                .iter()
                .enumerate()
                .map(|(i, _)| GridItem {
                    image: Image::default(),
                    index: i as i32,
                    abs_index: i as i32,
                    selected: false,
                })
                .collect();

            let gv = ui.global::<GridViewState>();
            gv.set_model(Rc::new(VecModel::from(grid_data)).into());
            gv.set_selected_count(0);

            ui.set_view_mode(if scan.is_dir {
                ViewMode::Grid
            } else {
                ViewMode::Full
            });

            if !scan.paths.is_empty() {
                self.handle_full_view_load(scan.start_index);
            }

            self.handle_grid_request(0, 50);
        }
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
    let scan = fs_scan::scan(&config.path, &extra_exts);

    let main_window = MainWindow::new()?;

    let grid_data: Vec<GridItem> = scan
        .paths
        .iter()
        .enumerate()
        .map(|(i, _)| GridItem {
            image: Image::default(),
            index: i as i32,
            abs_index: i as i32,
            selected: false,
        })
        .collect();
    main_window
        .global::<GridViewState>()
        .set_model(Rc::new(VecModel::from(grid_data)).into());

    let scan = Arc::new(scan);
    let app_controller = Rc::new(RefCell::new(AppController::new(
        plugin_manager,
        scan.clone(),
        &config,
        &main_window,
    )));

    let factory = Arc::new(StepFactory::new(false));

    ui::grid_view_presenter::register(&main_window, app_controller.clone());
    ui::full_view_presenter::register(&main_window, app_controller.clone());
    ui::pipeline_presenter::register(&main_window, app_controller.clone(), factory);
    ui::settings_presenter::register(&main_window, app_controller.clone());
    ui::bindings::setup(&main_window, &config);

    let acc = app_controller.clone();
    main_window.on_open_images(move || {
        AppController::handle_open_images(acc.clone());
    });
    main_window.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });

    main_window.set_app_background(config.background);
    main_window.set_view_mode(if scan.is_dir {
        ViewMode::Grid
    } else {
        ViewMode::Full
    });

    if !scan.paths.is_empty() {
        app_controller
            .borrow()
            .handle_full_view_load(scan.start_index);
        ui::full_view_presenter::set_exif(app_controller);
    }

    debug!(
        "Init in {:.1} ms",
        init_start.elapsed().as_secs_f64() * 1000.0
    );
    main_window.run()?;
    Ok(())
}
