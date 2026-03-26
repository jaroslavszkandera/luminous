use dashmap::DashMap;
use directories::ProjectDirs;
use image::imageops::FilterType;
use log::{debug, error, warn};
use rayon::ThreadPool;
use sha2::{Digest, Sha256};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::plugins::PluginManager;

const THUMB_FILTER: FilterType = FilterType::Nearest;

pub type ImageReadyFn = Arc<dyn Fn(usize, SharedPixelBuffer<Rgba8Pixel>) + Send + Sync>;
pub type ImageReadyHook = Option<ImageReadyFn>;

fn placeholder() -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::<Rgba8Pixel>::new(1, 1)
}

fn to_pixel_buffer(img: image::DynamicImage) -> SharedPixelBuffer<Rgba8Pixel> {
    let rgba = img.into_rgba8();
    SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height())
}

pub struct ImageLoader {
    thumb_cache: Arc<DashMap<usize, SharedPixelBuffer<Rgba8Pixel>>>,
    full_cache: Arc<DashMap<usize, SharedPixelBuffer<Rgba8Pixel>>>,

    pub paths: Vec<PathBuf>,
    pub pool: Arc<ThreadPool>,
    pub active_idx: Arc<AtomicUsize>,
    pub window_size: usize,
    pub plugin_manager: Arc<PluginManager>,

    // TODO: load from the closest requested token for better results
    active_window: Arc<Mutex<HashSet<usize>>>,
    thumb_epoch: Arc<AtomicUsize>,
    next_full_token: Arc<AtomicUsize>,
    window_epoch: Arc<AtomicUsize>,

    cache_dir: Option<PathBuf>,
    bucket_resolution: AtomicU32,

    on_thumb_ready: ImageReadyHook,
    on_full_ready: ImageReadyHook,
}

impl ImageLoader {
    pub fn new(
        paths: Vec<PathBuf>,
        workers: usize,
        window_size: usize,
        plugin_manager: Arc<PluginManager>,
    ) -> Self {
        let cache_dir = ProjectDirs::from("", "", "luminous").and_then(|proj| {
            let dir = proj.cache_dir().join("thumbnails");
            fs::create_dir_all(&dir)
                .map(|_| dir)
                .map_err(|e| warn!("Failed to create thumbnail cache dir: {e}"))
                .ok()
        });

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .expect("Failed to build rayon thread pool");

        Self {
            thumb_cache: Arc::new(DashMap::new()),
            full_cache: Arc::new(DashMap::new()),
            paths,
            pool: Arc::new(pool),
            active_idx: Arc::new(AtomicUsize::new(0)),
            active_window: Arc::new(Mutex::new(HashSet::new())),
            thumb_epoch: Arc::new(AtomicUsize::new(0)),
            next_full_token: Arc::new(AtomicUsize::new(0)),
            window_epoch: Arc::new(AtomicUsize::new(0)),
            window_size,
            cache_dir,
            bucket_resolution: AtomicU32::new(0),
            plugin_manager: plugin_manager,
            on_thumb_ready: None,
            on_full_ready: None,
        }
    }

    pub fn on_thumb_ready<F>(&mut self, f: F)
    where
        F: Fn(usize, SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static,
    {
        self.on_thumb_ready = Some(Arc::new(f));
    }

    pub fn on_full_ready<F>(&mut self, f: F)
    where
        F: Fn(usize, SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static,
    {
        self.on_full_ready = Some(Arc::new(f));
    }

    pub fn set_bucket_resolution(&self, resolution: u32) {
        self.bucket_resolution.store(resolution, Ordering::Relaxed);
        self.thumb_epoch.fetch_add(1, Ordering::Relaxed);
        self.thumb_cache.clear();
    }

    pub fn clear_thumbs(&self) {
        self.thumb_epoch.fetch_add(1, Ordering::Relaxed);
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
        self.thumb_cache.insert(idx, buf);
    }

    pub fn full_cache_contains(&self, idx: usize) -> bool {
        self.full_cache.contains_key(&idx)
    }

    pub fn get_curr_active_buffer(&self) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let idx = self.active_idx.load(Ordering::Relaxed);
        self.full_cache.get(&idx).map(|r| r.clone()).or_else(|| {
            error!("Active image not in cache (index: {idx})");
            None
        })
    }

    pub fn get_file_name(&self, idx: usize) -> Option<&str> {
        self.paths.get(idx)?.file_name()?.to_str()
    }

    pub fn get_curr_img_path(&self) -> Option<&PathBuf> {
        self.paths.get(self.active_idx.load(Ordering::Relaxed))
    }

    // source: https://github.com/slint-ui/slint/discussions/5140
    pub fn load_grid_thumb(&self, index: usize) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let res = self.bucket_resolution.load(Ordering::Relaxed);
        if res == 0 {
            return Some(placeholder());
        }

        if let Some(buf) = self.thumb_cache.get(&index) {
            return Some(buf.clone());
        }

        let path = self.paths.get(index)?.clone();
        let cache_clone = self.thumb_cache.clone();
        let cache_path = Self::disk_cache_path(self.cache_dir.as_ref(), &path, res);
        let plugin_manager = self.plugin_manager.clone();
        let on_ready = self.on_thumb_ready.clone();

        let my_epoch = self.thumb_epoch.load(Ordering::Relaxed);
        let epoch_counter = self.thumb_epoch.clone();

        self.pool.spawn(move || {
            if epoch_counter.load(Ordering::Relaxed) != my_epoch {
                debug!("Thumb job cancelled (epoch mismatch) index={index}");
                return;
            }

            let t = Instant::now();
            let buffer = Self::decode_thumb(&path, &plugin_manager, &cache_path, res);

            if epoch_counter.load(Ordering::Relaxed) != my_epoch {
                debug!("Thumb job discarded after decode (epoch mismatch) index={index}");
                return;
            }

            debug!(
                "Thumb ({res}px) {:?} {:.1}ms",
                path.file_name().unwrap_or_default(),
                t.elapsed().as_secs_f64() * 1000.0
            );

            cache_clone.insert(index, buffer.clone());
            if let Some(h) = &on_ready {
                h(index, buffer);
            }
        });

        None
    }

    pub fn load_full_progressive(&self, index: usize) -> Image {
        let my_token = self.next_full_token.fetch_add(1, Ordering::Relaxed);
        self.active_idx.store(index, Ordering::Relaxed);

        if let Some(buf) = self.full_cache.get(&index) {
            debug!("Full cache hit: {index}");
            return Image::from_rgba8(buf.clone());
        }

        let backup = self
            .thumb_cache
            .get(&index)
            .map(|buf| Image::from_rgba8(buf.clone()))
            .unwrap_or_default();

        let path = match self.paths.get(index) {
            Some(p) => p.clone(),
            None => return backup,
        };

        let cache_clone = self.full_cache.clone();
        let token_counter = self.next_full_token.clone();
        let plugin_manager = self.plugin_manager.clone();
        let on_ready = self.on_full_ready.clone();

        self.pool.spawn(move || {
            let latest = token_counter.load(Ordering::Relaxed);
            if my_token + 1 < latest {
                debug!(
                    "Full job skipped before decode index={index} token={my_token} latest={latest}"
                );
                return;
            }

            let t = Instant::now();
            let buffer = Self::decode_full(&path, &plugin_manager);

            debug!(
                "Full {:?} {:.1}ms",
                path.file_name().unwrap_or_default(),
                t.elapsed().as_secs_f64() * 1000.0
            );

            cache_clone.insert(index, buffer.clone());

            let latest = token_counter.load(Ordering::Relaxed);
            if my_token + 1 < latest {
                debug!("Full UI update skipped index={index} token={my_token} latest={latest}");
                return;
            }

            if let Some(h) = &on_ready {
                h(index, buffer);
            }
        });

        backup
    }

    pub fn update_sliding_window(&self, center_idx: usize, window_indices: Vec<usize>) {
        if self.paths.is_empty() {
            return;
        }

        self.window_epoch.fetch_add(1, Ordering::Relaxed);

        {
            let mut active = self.active_window.lock().unwrap();
            active.clear();
            active.insert(center_idx);
            active.extend(&window_indices);
        }

        for &idx in &window_indices {
            self.preload_background(idx);
        }

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
        let path = match self.paths.get(index) {
            Some(p) => p.clone(),
            None => return,
        };
        let cache_clone = self.full_cache.clone();
        let active_window = self.active_window.clone();
        let plugin_manager = self.plugin_manager.clone();

        let my_epoch = self.window_epoch.load(Ordering::Relaxed);
        let window_epoch = self.window_epoch.clone();

        self.pool.spawn(move || {
            if window_epoch.load(Ordering::Relaxed) != my_epoch {
                return;
            }
            if !active_window.lock().unwrap().contains(&index) {
                return;
            }
            if cache_clone.contains_key(&index) {
                return;
            }
            cache_clone.insert(index, Self::decode_full(&path, &plugin_manager));
        });
    }

    fn disk_cache_path(cache_dir: Option<&PathBuf>, path: &Path, res: u32) -> Option<PathBuf> {
        let meta = fs::metadata(path).ok()?;
        let mtime = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();

        let mut h = Sha256::new();
        h.update(path.to_string_lossy().as_bytes());
        h.update(mtime.to_be_bytes());

        // TODO: switch to JPEG/WebP for faster encode/decode
        Some(cache_dir?.join(format!("{}_{res}.png", hex::encode(h.finalize()))))
    }

    fn decode_thumb(
        path: &Path,
        plugin_manager: &PluginManager,
        cache_path: &Option<PathBuf>,
        res: u32,
    ) -> SharedPixelBuffer<Rgba8Pixel> {
        if let Some(cp) = cache_path.as_ref().filter(|p| p.exists()) {
            match image::open(cp) {
                Ok(img) => return to_pixel_buffer(img),
                Err(_) => error!("Corrupt disk cache {cp:?}, regenerating"),
            }
        }

        let dynamic = plugin_manager.decode_dynamic(path).or_else(|| {
            image::open(path)
                .map_err(|e| error!("Load failed {path:?}: {e}"))
                .ok()
        });

        let Some(img) = dynamic else {
            return placeholder();
        };

        let (w, h) = (img.width(), img.height());
        let scale = (res as f64 / w.max(h) as f64).min(1.0);
        let resized = img.resize(
            (w as f64 * scale).round() as u32,
            (h as f64 * scale).round() as u32,
            THUMB_FILTER,
        );

        if let Some(cp) = cache_path {
            if let Err(e) = resized.save(cp) {
                error!("Failed to save thumb cache {cp:?}: {e}");
            }
        }

        to_pixel_buffer(resized)
    }

    // TODO: encode_full for all formats in context menu
    fn decode_full(path: &Path, plugin_manager: &PluginManager) -> SharedPixelBuffer<Rgba8Pixel> {
        if let Some(buf) = plugin_manager.decode(path) {
            return buf;
        }
        match image::open(path) {
            Ok(img) => to_pixel_buffer(img),
            Err(e) => {
                error!("Image load failed {path:?}: {e}");
                placeholder()
            }
        }
    }
}
