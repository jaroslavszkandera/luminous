use directories::ProjectDirs;
use log::{debug, error, warn};
use sha2::{Digest, Sha256};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, Weak};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use threadpool::ThreadPool;

use crate::MainWindow;
use crate::plugins::PluginManager;

fn get_placeholder() -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::<Rgba8Pixel>::new(1, 1)
}

pub struct ImageLoader {
    thumb_cache: Arc<Mutex<HashMap<usize, SharedPixelBuffer<Rgba8Pixel>>>>,
    full_cache: Arc<Mutex<HashMap<usize, SharedPixelBuffer<Rgba8Pixel>>>>,
    pub paths: Vec<PathBuf>,
    pub pool: ThreadPool,
    pub active_idx: Arc<AtomicUsize>,
    pub active_window: Arc<Mutex<HashSet<usize>>>,
    full_load_generation: Arc<AtomicUsize>,
    pub window_size: usize,
    cache_dir: Option<PathBuf>,
    bucket_resolution: AtomicU32,
    pub plugin_manager: Arc<PluginManager>,
}

impl ImageLoader {
    pub fn new(
        paths: Vec<PathBuf>,
        workers: usize,
        window_size: usize,
        plugin_manager: PluginManager,
    ) -> Self {
        let cache_dir = if let Some(proj_dirs) = ProjectDirs::from("", "", "luminous") {
            let dir = proj_dirs.cache_dir().join("thumbnails");
            if let Err(e) = fs::create_dir_all(&dir) {
                warn!("Failed to create cache directory: {}", e);
                None
            } else {
                Some(dir)
            }
        } else {
            None
        };
        ImageLoader {
            thumb_cache: Arc::new(Mutex::new(HashMap::new())),
            full_cache: Arc::new(Mutex::new(HashMap::new())),
            paths,
            pool: ThreadPool::new(workers),
            active_idx: Arc::new(AtomicUsize::new(0)),
            active_window: Arc::new(Mutex::new(HashSet::new())),
            full_load_generation: Arc::new(AtomicUsize::new(0)),
            window_size: window_size,
            cache_dir,
            bucket_resolution: AtomicU32::new(0),
            plugin_manager: Arc::new(plugin_manager),
        }
    }

    pub fn set_bucket_resolution(&self, resolution: u32) {
        self.bucket_resolution.store(resolution, Ordering::Relaxed);
        self.thumb_cache.lock().unwrap().clear();
    }

    // TODO: detect if cache is corrupted
    fn get_cache_path(
        cache_dir: Option<&PathBuf>,
        original_path: &Path,
        resolution: u32,
    ) -> Option<PathBuf> {
        let metadata = fs::metadata(original_path).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();

        let mut hasher = Sha256::new();
        hasher.update(original_path.to_string_lossy().as_bytes());
        hasher.update(modified.to_be_bytes());
        let hash = hasher.finalize();
        // Is it better to convert it to an uniform format or not?
        let format = "png";

        Some(cache_dir?.join(format!(
            "{}_{}.{}",
            hex::encode(hash),
            resolution,
            format, // original_path.extension()?.to_str()?
        )))
    }

    // source: https://github.com/slint-ui/slint/discussions/5140
    pub fn load_grid_thumb<F>(
        &self,
        index: usize,
        ui_handle: Weak<MainWindow>,
        on_loaded: F,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>>
    where
        F: Fn(MainWindow, usize, Image) + Send + 'static,
    {
        // FIX: Should be handled by slint
        if self.bucket_resolution.load(Ordering::Relaxed) == 0 {
            // error!("Bucket resolution is 0");
            return Some(get_placeholder());
        }
        {
            let cache_handle = self.thumb_cache.lock().unwrap();
            if let Some(buffer) = cache_handle.get(&index) {
                return Some(buffer.clone());
            }
        }

        if let Some(path) = self.paths.get(index) {
            let path = path.clone();
            let cache_clone = self.thumb_cache.clone();
            let cache_dir = self.cache_dir.clone();
            let res = self.bucket_resolution.load(Ordering::Relaxed);
            let cache_path = Self::get_cache_path(cache_dir.as_ref(), &path, res);
            let plugin_manager = self.plugin_manager.clone();

            self.pool.execute(move || {
                let _start = Instant::now();

                let buffer = if let Some(ref cp) = cache_path.as_ref().filter(|p| p.exists()) {
                    match image::open(cp) {
                        Ok(cached_img) => {
                            let rgba = cached_img.to_rgba8();
                            SharedPixelBuffer::clone_from_slice(
                                rgba.as_raw(),
                                rgba.width(),
                                rgba.height(),
                            )
                        }
                        Err(_) => {
                            error!(
                                "Cache failed to open ({} px) for {:?} (plugin)",
                                res,
                                path.file_name()
                            );
                            get_placeholder()
                        }
                    }
                } else {
                    debug!("Cache ({} px) not found for {:?}", res, path.file_name());
                    let full_buffer = Self::fetch_buffer(&path, &plugin_manager);
                    let (w, h) = (full_buffer.width(), full_buffer.height());
                    let img_view = image::RgbaImage::from_raw(
                        w,
                        h,
                        full_buffer
                            .as_slice()
                            .iter()
                            .flat_map(|p| [p.r, p.g, p.b, p.a])
                            .collect(),
                    )
                    .unwrap();
                    let resized = image::DynamicImage::ImageRgba8(img_view).thumbnail(res, res);
                    if let Some(ref cp) = cache_path {
                        if let Err(e) = resized.save(cp) {
                            error!("Failed to save thumbnail cache to {:?}: {}", cp, e);
                        }
                    }
                    let rgba = resized.to_rgba8();
                    SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height())
                };

                debug!(
                    "Thumb loaded ({} px): {:?} in {:.2}ms",
                    res,
                    path.file_name().unwrap_or_default(),
                    _start.elapsed().as_secs_f64() * 1000.0
                );

                cache_clone.lock().unwrap().insert(index, buffer.clone());

                let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                    let img = Image::from_rgba8(buffer);
                    on_loaded(ui, index, img);
                });
            });
        }
        None
    }

    // NOTE: Maybe retain would be a better name?
    pub fn prune_grid_thumbs(&self, start: usize, count: usize) {
        let margin = 30;
        let mut cache = self.thumb_cache.lock().unwrap();
        cache.retain(|&idx, _| {
            idx >= start.saturating_sub(margin) && idx <= (start + count + margin)
        });
    }

    pub fn clear_thumbs(&self) {
        self.thumb_cache.lock().unwrap().clear();
    }

    pub fn load_full_progressive<F>(
        &self,
        index: usize,
        ui_handle: Weak<MainWindow>,
        on_loaded_full: F,
    ) -> Image
    where
        F: Fn(MainWindow, Image) + Send + 'static,
    {
        let job_generation = self.full_load_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.active_idx.store(index, Ordering::Relaxed);

        // Check Cache
        {
            let full_handle = self.full_cache.lock().unwrap();
            if let Some(buffer) = full_handle.get(&index) {
                debug!("Full cache hit: {}", index);
                return Image::from_rgba8(buffer.clone());
            }
        }

        // Prepare Backup (Thumbnail else Placeholder)
        let backup_image = {
            let thumb_handle = self.thumb_cache.lock().unwrap();
            if let Some(buffer) = thumb_handle.get(&index) {
                Image::from_rgba8(buffer.clone())
            } else {
                Image::from_rgba8(get_placeholder())
            }
        };

        // Spawn Job
        if let Some(path) = self.paths.get(index) {
            let path = path.clone();
            let cache_clone = self.full_cache.clone();
            let full_load_generation = self.full_load_generation.clone();
            let plugin_manager = self.plugin_manager.clone();
            // let active_idx = self.active_idx.clone();

            self.pool.execute(move || {
                let current_generation = full_load_generation.load(Ordering::Relaxed);
                if job_generation < current_generation {
                    debug!(
                        "Skipping obsolete job: {} (Generation {} < Current {})",
                        index, job_generation, current_generation
                    );
                    return;
                }

                let start = Instant::now();
                let buffer = Self::fetch_buffer(&path, &plugin_manager);

                debug!(
                    "Full loaded: {:?} in {:.2}ms",
                    path.file_name().unwrap_or_default(),
                    start.elapsed().as_secs_f64() * 1000.0
                );

                cache_clone.lock().unwrap().insert(index, buffer.clone());

                let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                    if index == ui.get_curr_image_index() as usize {
                        let img = Image::from_rgba8(buffer);
                        on_loaded_full(ui, img);
                    } else {
                        debug!(
                            "Obsolete job (index {}), not showing (current index {})",
                            index,
                            ui.get_curr_image_index()
                        );
                    }
                });
            });
        }

        backup_image
    }

    pub fn update_sliding_window(&self, center_idx: usize, window_indices: Vec<usize>) {
        if self.paths.is_empty() {
            return;
        }

        {
            let mut active_set = self.active_window.lock().unwrap();
            active_set.clear();
            active_set.insert(center_idx);
            for &idx in &window_indices {
                active_set.insert(idx);
            }
        }

        for &idx in &window_indices {
            self.preload_background(idx);
        }

        // Eviction Policy
        let mut cache = self.full_cache.lock().unwrap();
        let active_set = self.active_window.lock().unwrap();
        let keys_to_remove: Vec<usize> = cache
            .keys()
            .filter(|k| !active_set.contains(k))
            .cloned()
            .collect();

        for k in keys_to_remove {
            cache.remove(&k);
            debug!("Evicted full image: {}", k);
        }
    }

    fn preload_background(&self, index: usize) {
        if self.full_cache.lock().unwrap().contains_key(&index) {
            return;
        }

        if let Some(path) = self.paths.get(index) {
            let path = path.clone();
            let cache_clone = self.full_cache.clone();
            let active_window = self.active_window.clone();

            self.pool.execute(move || {
                if !active_window.lock().unwrap().contains(&index) {
                    return;
                }
                if cache_clone.lock().unwrap().contains_key(&index) {
                    return;
                }

                if let Ok(dyn_img) = image::open(&path) {
                    let rgba = dyn_img.to_rgba8();
                    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        rgba.as_raw(),
                        rgba.width(),
                        rgba.height(),
                    );
                    cache_clone.lock().unwrap().insert(index, buffer);
                }
            });
        }
    }

    pub fn get_curr_active_buffer(&self) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let active_idx = self.active_idx.load(Ordering::Relaxed);
        let full_handle = self.full_cache.lock().unwrap();
        if let Some(buffer) = full_handle.get(&active_idx) {
            return Some(buffer.clone());
        }
        // NOTE: Failsafe
        error!("Current image not loaded (index: {})", active_idx);
        None
    }

    pub fn cache_buffer(&self, idx: usize, buf: SharedPixelBuffer<Rgba8Pixel>) {
        self.full_cache.lock().unwrap().insert(idx, buf.clone());
        // TODO: resize to thumbnail
        self.thumb_cache.lock().unwrap().insert(idx, buf);
    }

    // TODO: Handle long image file names based on window width
    pub fn get_curr_image_file_name(&self, idx: usize) -> &str {
        self.paths[idx]
            .file_name()
            .unwrap()
            .to_str()
            .expect("Image file name should be present")
    }

    fn fetch_buffer(path: &Path, plugin_manager: &PluginManager) -> SharedPixelBuffer<Rgba8Pixel> {
        let buffer = if let Some(buffer) = plugin_manager.decode(path) {
            buffer
        } else {
            image::open(path)
                .map(|dyn_img| {
                    let rgba = dyn_img.to_rgba8();
                    SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        rgba.as_raw(),
                        rgba.width(),
                        rgba.height(),
                    )
                })
                .unwrap_or_else(|e| {
                    error!("Image load failed for {:?}: {}", path, e);
                    get_placeholder()
                })
        };

        buffer
    }
}
