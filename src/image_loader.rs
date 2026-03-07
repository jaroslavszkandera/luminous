use dashmap::DashMap;
use directories::ProjectDirs;
use log::{debug, error, warn};
use rayon::ThreadPool;
use sha2::{Digest, Sha256};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, Weak};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::MainWindow;
use crate::plugins::PluginManager;

fn get_placeholder() -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::<Rgba8Pixel>::new(1, 1)
}

pub type ReadyHook = Option<Arc<dyn Fn(usize) + Send + Sync>>;

pub struct ImageLoader {
    thumb_cache: Arc<DashMap<usize, SharedPixelBuffer<Rgba8Pixel>>>,
    full_cache: Arc<DashMap<usize, SharedPixelBuffer<Rgba8Pixel>>>,
    pub paths: Vec<PathBuf>,
    pub pool: Arc<ThreadPool>,
    pub active_idx: Arc<AtomicUsize>,
    active_window: Arc<Mutex<HashSet<usize>>>,
    next_full_token: Arc<AtomicUsize>,
    pub window_size: usize,
    cache_dir: Option<PathBuf>,
    bucket_resolution: AtomicU32,
    pub plugin_manager: Arc<PluginManager>,
    on_thumb_ready: ReadyHook,
    on_full_ready: ReadyHook,
}

impl ImageLoader {
    pub fn new(
        paths: Vec<PathBuf>,
        workers: usize,
        window_size: usize,
        plugin_manager: PluginManager,
    ) -> Self {
        let cache_dir = ProjectDirs::from("", "", "luminous").and_then(|proj| {
            let dir = proj.cache_dir().join("thumbnails");
            match fs::create_dir_all(&dir) {
                Ok(_) => Some(dir),
                Err(e) => {
                    warn!("Failed to create cache directory: {e}");
                    None
                }
            }
        });

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .expect("Failed to build rayon thread pool");

        ImageLoader {
            thumb_cache: Arc::new(DashMap::new()),
            full_cache: Arc::new(DashMap::new()),
            paths,
            pool: Arc::new(pool),
            active_idx: Arc::new(AtomicUsize::new(0)),
            active_window: Arc::new(Mutex::new(HashSet::new())),
            next_full_token: Arc::new(AtomicUsize::new(0)),
            window_size,
            cache_dir,
            bucket_resolution: AtomicU32::new(0),
            plugin_manager: Arc::new(plugin_manager),
            on_thumb_ready: None,
            on_full_ready: None,
        }
    }

    pub fn set_ready_hooks(
        &mut self,
        on_thumb_ready: impl Fn(usize) + Send + Sync + 'static,
        on_full_ready: impl Fn(usize) + Send + Sync + 'static,
    ) {
        self.on_thumb_ready = Some(Arc::new(on_thumb_ready));
        self.on_full_ready = Some(Arc::new(on_full_ready));
    }

    pub fn set_bucket_resolution(&self, resolution: u32) {
        self.bucket_resolution.store(resolution, Ordering::Relaxed);
        self.thumb_cache.clear();
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

        // TODO: consider JPEG/WebP for faster encode/decode
        Some(cache_dir?.join(format!("{}_{}.png", hex::encode(hash), resolution)))
    }

    pub fn clear_thumbs(&self) {
        self.thumb_cache.clear();
    }

    pub fn prune_grid_thumbs(&self, start: usize, count: usize) {
        const MARGIN: usize = 30;
        self.thumb_cache
            .retain(|&idx, _| idx >= start.saturating_sub(MARGIN) && idx <= start + count + MARGIN);
    }

    pub fn evict_all(&self) {
        self.active_window.lock().unwrap().clear();
        self.full_cache.clear();
    }

    pub fn cache_buffer(&self, idx: usize, buf: SharedPixelBuffer<Rgba8Pixel>) {
        self.full_cache.insert(idx, buf.clone());
        // TODO: downscale to thumbnail resolution before inserting
        self.thumb_cache.insert(idx, buf);
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
        // FIX: resolution of 0 means the grid hasn't measured its cells yet;
        // return a 1×1 placeholder so Slint gets a valid image synchronously.
        if self.bucket_resolution.load(Ordering::Relaxed) == 0 {
            return Some(get_placeholder());
        }

        if let Some(buf) = self.thumb_cache.get(&index) {
            return Some(buf.clone());
        }

        let Some(path) = self.paths.get(index) else {
            return None;
        };
        let path = path.clone();
        let cache_clone = self.thumb_cache.clone();
        let res = self.bucket_resolution.load(Ordering::Relaxed);
        let cache_path = Self::get_cache_path(self.cache_dir.as_ref(), &path, res);
        let plugin_manager = self.plugin_manager.clone();
        let on_ready = self.on_thumb_ready.clone();

        self.pool.spawn(move || {
            let start = Instant::now();

            let buffer = Self::decode_thumb(&path, &plugin_manager, &cache_path, res);

            debug!(
                "Thumb loaded ({res} px): {:?} in {:.2}ms",
                path.file_name().unwrap_or_default(),
                start.elapsed().as_secs_f64() * 1000.0
            );

            cache_clone.insert(index, buffer.clone());

            // Signal benchmarks/tests before attempting Slint dispatch.
            if let Some(hook) = &on_ready {
                hook(index);
            }

            let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                on_loaded(ui, index, Image::from_rgba8(buffer));
            });
        });

        None
    }

    fn decode_thumb(
        path: &Path,
        plugin_manager: &PluginManager,
        cache_path: &Option<PathBuf>,
        res: u32,
    ) -> SharedPixelBuffer<Rgba8Pixel> {
        if let Some(cp) = cache_path.as_ref().filter(|p| p.exists()) {
            match image::open(cp) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    return SharedPixelBuffer::clone_from_slice(
                        rgba.as_raw(),
                        rgba.width(),
                        rgba.height(),
                    );
                }
                Err(_) => error!("Corrupt disk cache at {:?}, re-generating", cp),
            }
        }

        // Decode full image, resize, persist to disk cache.
        debug!("Cache ({res} px) miss for {:?}", path.file_name());
        let full = Self::fetch_buffer(path, plugin_manager);
        let (w, h) = (full.width(), full.height());

        let raw_bytes: Vec<u8> = full
            .as_slice()
            .iter()
            .flat_map(|p| [p.r, p.g, p.b, p.a])
            .collect();

        let resized = image::RgbaImage::from_raw(w, h, raw_bytes)
            .map(image::DynamicImage::ImageRgba8)
            .map(|img| img.thumbnail(res, res))
            .unwrap_or_else(|| {
                error!("Failed to construct RgbaImage for {:?}", path.file_name());
                image::DynamicImage::ImageRgba8(image::RgbaImage::new(1, 1))
            });

        if let Some(cp) = cache_path {
            if let Err(e) = resized.save(cp) {
                error!("Failed to save thumbnail cache to {cp:?}: {e}");
            }
        }

        let rgba = resized.to_rgba8();
        SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height())
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
        let my_token = self.next_full_token.fetch_add(1, Ordering::Relaxed);
        self.active_idx.store(index, Ordering::Relaxed);

        if let Some(buf) = self.full_cache.get(&index) {
            debug!("Full cache hit: {index}");
            return Image::from_rgba8(buf.clone());
        }

        // Show thumbnail (or placeholder) while the full image loads.
        let backup = self
            .thumb_cache
            .get(&index)
            .map(|buf| Image::from_rgba8(buf.clone()))
            .unwrap_or_else(|| Image::from_rgba8(get_placeholder()));

        let Some(path) = self.paths.get(index) else {
            return backup;
        };
        let path = path.clone();
        let cache_clone = self.full_cache.clone();
        let token_counter = self.next_full_token.clone();
        let plugin_manager = self.plugin_manager.clone();
        let on_ready = self.on_full_ready.clone();

        self.pool.spawn(move || {
            let start = Instant::now();
            let buffer = Self::fetch_buffer(&path, &plugin_manager);

            debug!(
                "Full loaded: {:?} in {:.2}ms",
                path.file_name().unwrap_or_default(),
                start.elapsed().as_secs_f64() * 1000.0
            );

            cache_clone.insert(index, buffer.clone());

            if let Some(hook) = &on_ready {
                hook(index);
            }

            let latest_token = token_counter.load(Ordering::Relaxed);
            if my_token + 1 < latest_token {
                debug!(
                    "Skipping stale UI update for index {index} \
                     (token {my_token}, latest {latest_token})"
                );
                return;
            }

            let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                if index == ui.get_curr_image_index() as usize {
                    on_loaded_full(ui, Image::from_rgba8(buffer));
                } else {
                    debug!(
                        "Stale job for index {index}, current UI index {}",
                        ui.get_curr_image_index()
                    );
                }
            });
        });

        backup
    }

    pub fn update_sliding_window(&self, center_idx: usize, window_indices: Vec<usize>) {
        if self.paths.is_empty() {
            return;
        }

        // Update the active set first
        {
            let mut active = self.active_window.lock().unwrap();
            active.clear();
            active.insert(center_idx);
            active.extend(window_indices.iter().copied());
        }

        for &idx in &window_indices {
            self.preload_background(idx);
        }

        // Evict entries that fell out of the window.
        let active = self.active_window.lock().unwrap();
        self.full_cache.retain(|k, _| {
            let keep = active.contains(k);
            if !keep {
                debug!("Evicted full image: {k}");
            }
            keep
        });
    }

    fn preload_background(&self, index: usize) {
        if self.full_cache.contains_key(&index) {
            return;
        }
        let Some(path) = self.paths.get(index) else {
            return;
        };
        let path = path.clone();
        let cache_clone = self.full_cache.clone();
        let active_window = self.active_window.clone();
        let plugin_manager = self.plugin_manager.clone();

        self.pool.spawn(move || {
            if !active_window.lock().unwrap().contains(&index) {
                return;
            }
            if cache_clone.contains_key(&index) {
                return;
            }

            let buffer = Self::fetch_buffer(&path, &plugin_manager);
            cache_clone.insert(index, buffer);
        });
    }

    pub fn get_curr_active_buffer(&self) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let idx = self.active_idx.load(Ordering::Relaxed);
        if let Some(buf) = self.full_cache.get(&idx) {
            return Some(buf.clone());
        }
        error!("Current image not in cache (index: {idx})");
        None
    }

    // TODO: Handle long file names based on window width
    pub fn get_curr_image_file_name(&self, idx: usize) -> &str {
        self.paths[idx]
            .file_name()
            .unwrap()
            .to_str()
            .expect("image file name should be valid UTF-8")
    }

    fn fetch_buffer(path: &Path, plugin_manager: &PluginManager) -> SharedPixelBuffer<Rgba8Pixel> {
        if let Some(buf) = plugin_manager.decode(path) {
            return buf;
        }

        image::open(path)
            .map(|img| {
                let rgba = img.to_rgba8();
                SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                )
            })
            .unwrap_or_else(|e| {
                error!("Image load failed for {path:?}: {e}");
                get_placeholder()
            })
    }
}
