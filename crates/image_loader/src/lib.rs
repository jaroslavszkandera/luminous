use dashmap::DashMap;
use directories::ProjectDirs;
use log::{debug, error, trace};
use sha2::{Digest, Sha256};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use luminous_fs_scan::{ImageEntry, ImageId};
use luminous_plugins::PluginManager;
mod threadpool;
pub use threadpool::{GridViewIdxs, View};
use threadpool::{Job, ThreadPool};

pub type ImageReadyFn = Arc<dyn Fn(ImageId, SharedPixelBuffer<Rgba8Pixel>) + Send + Sync>;
pub type ImageReadyHook = Option<ImageReadyFn>;

fn placeholder() -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::<Rgba8Pixel>::new(1, 1)
}

pub fn to_pixel_buffer(img: image::DynamicImage) -> SharedPixelBuffer<Rgba8Pixel> {
    let rgba = img.into_rgba8();
    SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height())
}

pub fn to_slint_image(buf: SharedPixelBuffer<Rgba8Pixel>) -> Image {
    Image::from_rgba8(buf)
}

pub struct ImageLoader {
    pub plugin_manager: Arc<PluginManager>,
    catalog: Arc<RwLock<Vec<ImageEntry>>>,
    catalog_view: Arc<RwLock<Vec<ImageId>>>,
    pool: Arc<ThreadPool>,

    // Full cache
    pub active_idx: Arc<AtomicUsize>,
    pub window_size: usize,
    full_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>>,
    on_full_ready: Arc<RwLock<ImageReadyHook>>,

    // Thumbnail cache
    thumb_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>>,
    on_thumb_ready: Arc<RwLock<ImageReadyHook>>,
    cache_dir: Option<PathBuf>,
    thumb_res: Arc<AtomicU32>,
    grid_view_idxs: RwLock<GridViewIdxs>,
}

impl ImageLoader {
    pub fn new(
        workers: usize,
        window_size: usize,
        grid_view_active: bool,
        plugin_manager: Arc<PluginManager>,
    ) -> Self {
        let cache_dir = ProjectDirs::from("", "", "luminous").and_then(|proj| {
            let dir = proj.cache_dir().join("thumbnails");
            fs::create_dir_all(&dir)
                .map(|_| dir)
                .map_err(|e| error!("Failed to create thumbnail cache dir: {e}"))
                .ok()
        });

        let catalog: Arc<RwLock<Vec<ImageEntry>>> = Arc::new(RwLock::new(Vec::new()));
        let thumb_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>> =
            Arc::new(DashMap::new());
        let full_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>> =
            Arc::new(DashMap::new());
        let on_full_ready: Arc<RwLock<ImageReadyHook>> = Arc::new(RwLock::new(None));
        let on_thumb_ready: Arc<RwLock<ImageReadyHook>> = Arc::new(RwLock::new(None));
        let thumb_res = Arc::new(AtomicU32::new(256));

        let handler = {
            let catalog = Arc::clone(&catalog);
            let thumb_cache = Arc::clone(&thumb_cache);
            let full_cache = Arc::clone(&full_cache);
            let on_thumb_ready = Arc::clone(&on_thumb_ready);
            let on_full_ready = Arc::clone(&on_full_ready);
            let plugin_manager = Arc::clone(&plugin_manager);
            let cache_dir = cache_dir.clone();

            move |job: Job| match job {
                Job::Thumb { id, res } => {
                    let Some(path) = catalog
                        .read()
                        .unwrap()
                        .get(id.0 as usize)
                        .map(|e| e.path.clone())
                    else {
                        return;
                    };
                    let t = Instant::now();
                    let buffer = Self::decode_thumb(&path, &plugin_manager, &cache_dir, res);
                    trace!(
                        "Thumb ({res:?}px) id={id:?} {:.1}ms",
                        t.elapsed().as_secs_f64() * 1000.0
                    );
                    thumb_cache.insert(id, buffer.clone());
                    if let Some(h) = on_thumb_ready.read().unwrap().as_ref() {
                        h(id, buffer);
                    }
                }
                Job::Image { id } => {
                    let Some(path) = catalog
                        .read()
                        .unwrap()
                        .get(id.0 as usize)
                        .map(|e| e.path.clone())
                    else {
                        return;
                    };
                    let t = Instant::now();
                    let buffer = Self::decode_full(&path, &plugin_manager);
                    trace!("Full id={id:?} {:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
                    full_cache.insert(id, buffer.clone());
                    if let Some(h) = on_full_ready.read().unwrap().as_ref() {
                        h(id, buffer);
                    }
                }
            }
        };

        let pool = ThreadPool::new(
            workers,
            thumb_cache.clone(),
            thumb_res.clone(),
            full_cache.clone(),
            window_size,
            grid_view_active,
            handler,
        );

        Self {
            window_size,
            cache_dir,
            plugin_manager,
            pool: Arc::new(pool),
            catalog,
            catalog_view: Arc::new(RwLock::new(Vec::new())),
            thumb_cache,
            full_cache,
            grid_view_idxs: RwLock::new(GridViewIdxs {
                start: 0,
                focus: 0,
                end: 0,
            }),
            active_idx: Arc::new(AtomicUsize::new(0)),
            thumb_res,
            on_thumb_ready,
            on_full_ready,
        }
    }

    pub fn on_thumb_ready<F>(&mut self, f: F)
    where
        F: Fn(ImageId, SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static,
    {
        *self.on_thumb_ready.write().unwrap() = Some(Arc::new(f));
    }

    pub fn on_full_ready<F>(&mut self, f: F)
    where
        F: Fn(ImageId, SharedPixelBuffer<Rgba8Pixel>) + Send + Sync + 'static,
    {
        *self.on_full_ready.write().unwrap() = Some(Arc::new(f));
    }

    pub fn set_bucket_resolution(&self, resolution: u32) {
        self.thumb_res.store(resolution, Ordering::Relaxed);
    }

    pub fn set_active_view(&self, active_view: View) {
        self.pool.set_active_view(active_view);
    }

    pub fn set_catalog(&self, catalog: Vec<ImageEntry>) {
        *self.catalog.write().unwrap() = catalog;
        self.full_cache.clear();
        self.thumb_cache.clear();
    }

    pub fn set_catalog_view(&self, catalog_view: Vec<ImageId>) {
        self.pool.set_catalog_view(catalog_view.clone());
        *self.catalog_view.write().unwrap() = catalog_view;
    }

    // Queries to load thumbs
    pub fn load_grid_thumbs(&self, grid_view_idxs: &GridViewIdxs) {
        *self.grid_view_idxs.write().unwrap() = grid_view_idxs.clone();
        debug!("load_grid_thumbs {}", grid_view_idxs);
        let catalog_view = self.catalog_view.read().unwrap();
        self.pool.set_grid_view(*grid_view_idxs);

        // Clear thumbs outside window
        let start = grid_view_idxs.start.min(catalog_view.len());
        let end = grid_view_idxs.end.min(catalog_view.len());
        let visible_view = &self.catalog_view.read().unwrap()[start..end];
        self.thumb_cache.retain(|k, _| visible_view.contains(k));
        trace!(
            "Retained thumbs: {:?}",
            self.thumb_cache
                .iter()
                .map(|r| r.key().clone())
                .collect::<Vec<_>>()
        );
    }

    pub fn get_grid_thumb(&self, id: ImageId) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        if let Some(buf) = self.thumb_cache.get(&id) {
            Some(buf.clone())
        } else {
            None
        }
    }

    pub fn get_curr_img_path(&self) -> Option<PathBuf> {
        let idx = self.active_idx.load(Ordering::Relaxed);
        let image_id = *self.catalog_view.read().ok()?.get(idx)?;
        self.catalog
            .read()
            .ok()?
            .get(image_id.0 as usize)
            .map(|img| img.path.clone())
    }

    pub fn get_curr_buffer(&self) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let idx = self.active_idx.load(Ordering::Relaxed);
        let image_id = *self.catalog_view.read().ok()?.get(idx)?;
        self.full_cache.get(&image_id).map(|r| r.clone())
    }

    pub fn load_full_progressive(&self, index: usize, _force_disk_reload: bool) -> Image {
        self.active_idx.store(index, Ordering::Relaxed);
        self.pool.set_active_idx(index);

        let catalog_view = self.catalog_view.read().unwrap();
        let len = catalog_view.len();

        if len == 0 {
            error!("catalog_view empty not clearing slinding window");
            return Image::default();
        }

        let window_size = self.window_size as isize;
        let active_idx = index as isize;

        let valid_ids: Vec<ImageId> = (-window_size..=window_size)
            .map(|offset| catalog_view[(active_idx + offset).rem_euclid(len as isize) as usize])
            .collect();

        self.full_cache.retain(|k, _| valid_ids.contains(k));
        trace!(
            "Retained full: {:?}",
            self.full_cache
                .iter()
                .map(|r| r.key().clone())
                .collect::<Vec<_>>()
        );

        let target_id = catalog_view[index];

        if let Some(buf) = self.full_cache.get(&target_id) {
            trace!("Full cache hit: {index}");
            return Image::from_rgba8(buf.clone());
        }

        self.thumb_cache
            .get(&target_id)
            .map(|buf| Image::from_rgba8(buf.clone()))
            .unwrap_or_default()
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

        Some(cache_dir?.join(format!("{}_{res}.webp", hex::encode(h.finalize()))))
    }

    fn decode_thumb(
        path: &Path,
        plugin_manager: &PluginManager,
        cache_dir: &Option<PathBuf>,
        res: u32,
    ) -> SharedPixelBuffer<Rgba8Pixel> {
        trace!("decode_thumb - path {path:?} res {res:?}");
        let cache_path = Self::disk_cache_path(cache_dir.as_ref(), &path, res);
        if let Some(cp) = cache_path.as_ref().filter(|p| p.exists()) {
            match image::open(cp) {
                Ok(img) => return to_pixel_buffer(img),
                Err(_) => error!("Corrupt disk cache {cp:?}, regenerating"),
            }
        }

        let t = Instant::now();
        let mut buf = [0; 256];
        let known_format = std::fs::File::open(path)
            .and_then(|mut f| std::io::Read::read(&mut f, &mut buf))
            .ok()
            .and_then(|_| image::guess_format(&buf).ok());

        trace!(
            "Detected format for {:?} in {:.3}ms: {:?}",
            path,
            t.elapsed().as_secs_f64() * 1000.0,
            known_format,
        );

        let dynamic = if let Some(fmt) = known_format {
            match std::fs::File::open(path) {
                Ok(f) => image::load(std::io::BufReader::new(f), fmt)
                    .map_err(|e| error!("Load failed {path:?}: {e}"))
                    .ok(),
                Err(e) => {
                    error!("Load failed {path:?}: {e}");
                    None
                }
            }
        } else {
            plugin_manager.decode_dynamic(path)
        };

        let Some(img) = dynamic else {
            error!("Error loading image");
            return placeholder();
        };

        if res >= img.width() || res >= img.height() {
            trace!(
                "Not saving thumb {:?}, smaller than bucket res (res={res}, w={}, h={})",
                path.file_name(),
                img.width(),
                img.height()
            );
            return to_pixel_buffer(img);
        }

        let resized = img.thumbnail(res, res);

        if let Some(cp) = cache_path {
            if let Err(e) = resized.save(&cp) {
                error!("Failed to save thumb cache {cp:?}: {e}");
            }
        }

        to_pixel_buffer(resized)
    }

    fn decode_full(path: &Path, plugin_manager: &PluginManager) -> SharedPixelBuffer<Rgba8Pixel> {
        let t = Instant::now();
        let mut buf = [0; 256];
        let known_format = std::fs::File::open(path)
            .and_then(|mut f| std::io::Read::read(&mut f, &mut buf))
            .ok()
            .and_then(|_| image::guess_format(&buf).ok());

        trace!(
            "Detected format for {:?} in {:.3}ms: {:?}",
            path,
            t.elapsed().as_secs_f64() * 1000.0,
            known_format
        );

        if let Some(fmt) = known_format {
            match std::fs::File::open(path) {
                Ok(f) => match image::load(std::io::BufReader::new(f), fmt) {
                    Ok(img) => to_pixel_buffer(img),
                    Err(e) => {
                        error!("Image load failed {path:?}: {e}");
                        placeholder()
                    }
                },
                Err(e) => {
                    error!("Image load failed {path:?}: {e}");
                    placeholder()
                }
            }
        } else if let Some(buf) = plugin_manager.decode(path) {
            buf
        } else {
            error!("Image load failed {path:?}: Unknown format");
            placeholder()
        }
    }

    pub fn clear_disk_cache(&self) -> bool {
        let Some(ref dir) = self.cache_dir else {
            return false;
        };

        if !dir.exists() {
            error!("Failed to clear disk cache at {dir:?}: No directory");
            return false;
        }

        if let Err(e) = fs::remove_dir_all(dir) {
            error!("Failed to clear disk cache at {dir:?}: {e}");
            return false;
        }

        if let Err(e) = fs::create_dir_all(dir) {
            error!("Failed to recreate cache directory: {e}");
            return false;
        }

        debug!("Disk cache cleared");
        true
    }

    pub fn get_image_disk_cache_count(&self) -> u64 {
        let Some(ref dir) = self.cache_dir else {
            return 0;
        };

        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                    .count() as u64
            })
            .unwrap_or(0)
    }
}
