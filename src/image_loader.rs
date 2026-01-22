use log::{debug, error};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, Weak};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use threadpool::ThreadPool;

use crate::MainWindow;

fn get_placeholder() -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::<Rgba8Pixel>::new(1, 1)
}

pub struct ImageLoader {
    thumb_cache: Arc<Mutex<HashMap<usize, SharedPixelBuffer<Rgba8Pixel>>>>,
    full_cache: Arc<Mutex<HashMap<usize, SharedPixelBuffer<Rgba8Pixel>>>>,
    paths: Vec<PathBuf>,
    pub pool: ThreadPool,
    pub active_idx: Arc<AtomicUsize>,
    full_load_generation: Arc<AtomicUsize>,
    window_size: usize,
}

impl ImageLoader {
    pub fn new(paths: Vec<PathBuf>, workers: usize) -> Self {
        ImageLoader {
            thumb_cache: Arc::new(Mutex::new(HashMap::new())),
            full_cache: Arc::new(Mutex::new(HashMap::new())),
            paths,
            pool: ThreadPool::new(workers),
            active_idx: Arc::new(AtomicUsize::new(0)),
            full_load_generation: Arc::new(AtomicUsize::new(0)),
            window_size: 3,
        }
    }

    fn is_job_relevant(
        target_idx: usize,
        job_idx: usize,
        total_len: usize,
        window_size: usize,
    ) -> bool {
        let dist = (target_idx as isize - job_idx as isize).abs() as usize;
        let wrap_dist = total_len - dist;
        let actual_dist = dist.min(wrap_dist);
        actual_dist <= window_size
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
        let cache_handle = self.thumb_cache.lock().unwrap();
        if let Some(buffer) = cache_handle.get(&index) {
            return Some(buffer.clone());
        }
        drop(cache_handle);

        if let Some(path) = self.paths.get(index) {
            let path = path.clone();
            let cache_clone = self.thumb_cache.clone();

            self.pool.execute(move || {
                let _start = Instant::now();
                let buffer = match image::open(&path) {
                    Ok(dyn_img) => {
                        let dyn_img = dyn_img.thumbnail(500, 500);
                        let rgba = dyn_img.to_rgba8();
                        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                            rgba.as_raw(),
                            rgba.width(),
                            rgba.height(),
                        )
                    }
                    Err(e) => {
                        error!("Thumb load fail {}: {}", path.display(), e);
                        get_placeholder()
                    }
                };
                // debug!(
                //     "Thumb loaded: {:?} in {:.2}ms",
                //     path.file_name().unwrap_or_default(),
                //     _start.elapsed().as_secs_f64() * 1000.0
                // );

                cache_clone.lock().unwrap().insert(index, buffer.clone());

                let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                    let img = Image::from_rgba8(buffer);
                    on_loaded(ui, index, img);
                });
            });
        }
        None
    }

    pub fn prune_grid_thumbs(&self, indices: &[usize]) {
        if indices.is_empty() {
            return;
        }
        let mut cache = self.thumb_cache.lock().unwrap();
        for idx in indices {
            cache.remove(idx);
        }
        debug!("Batch pruned {} images", indices.len());
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
                let buffer = match image::open(&path) {
                    Ok(dyn_img) => {
                        let rgba = dyn_img.to_rgba8();
                        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                            rgba.as_raw(),
                            rgba.width(),
                            rgba.height(),
                        )
                    }
                    Err(e) => {
                        error!("Full load fail {}: {}", path.display(), e);
                        get_placeholder()
                    }
                };

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

    pub fn update_sliding_window(&self, center_idx: usize) {
        let len = self.paths.len();
        if len == 0 {
            return;
        }

        let mut keep_indices = HashSet::new();
        keep_indices.insert(center_idx);

        for i in 1..=self.window_size {
            let prev = (center_idx as isize - i as isize).rem_euclid(len as isize) as usize;
            keep_indices.insert(prev);
            self.preload_background(prev);

            let next = (center_idx + i).rem_euclid(len);
            keep_indices.insert(next);
            self.preload_background(next);
        }

        // Eviction Policy
        let mut cache = self.full_cache.lock().unwrap();
        let keys_to_remove: Vec<usize> = cache
            .keys()
            .filter(|k| !keep_indices.contains(k))
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
            let active_idx = self.active_idx.clone();
            let total_len = self.paths.len();
            let window_size = self.window_size;

            self.pool.execute(move || {
                let current_focus = active_idx.load(Ordering::Relaxed);
                if !Self::is_job_relevant(current_focus, index, total_len, window_size) {
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
}
