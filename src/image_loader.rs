use log::{debug, error};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, Weak};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
    pool: ThreadPool,
}

impl ImageLoader {
    pub fn new(paths: Vec<PathBuf>, workers: usize) -> Self {
        ImageLoader {
            thumb_cache: Arc::new(Mutex::new(HashMap::new())),
            full_cache: Arc::new(Mutex::new(HashMap::new())),
            paths,
            pool: ThreadPool::new(workers),
        }
    }

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
                let start = Instant::now();
                let buffer = match image::open(&path) {
                    Ok(dyn_img) => {
                        let dyn_img = dyn_img.thumbnail(200, 200);
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

                debug!(
                    "Thumb loaded: {:?} in {:.2}ms",
                    path.file_name().unwrap_or_default(),
                    start.elapsed().as_secs_f64() * 1000.0
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

    /// Load for Full View.
    /// 1. Return Full Res from Cache (if exists).
    /// 2. Else Return Thumbnail from Cache (if exists) AND spawn Full Res load.
    /// 3. Else Return Placeholder AND spawn Full Res load.
    pub fn load_full_progressive<F>(
        &self,
        index: usize,
        ui_handle: Weak<MainWindow>,
        on_loaded_full: F,
    ) -> Image
    where
        F: Fn(MainWindow, Image) + Send + 'static,
    {
        {
            let full_handle = self.full_cache.lock().unwrap();
            if let Some(buffer) = full_handle.get(&index) {
                debug!("Full cache hit: {}", index);
                return Image::from_rgba8(buffer.clone());
            }
        }

        let backup_image = {
            let thumb_handle = self.thumb_cache.lock().unwrap();
            if let Some(buffer) = thumb_handle.get(&index) {
                debug!("Full cache miss, using thumb: {}", index);
                Image::from_rgba8(buffer.clone())
            } else {
                debug!("Full & Thumb cache miss, using placeholder: {}", index);
                Image::from_rgba8(get_placeholder())
            }
        };

        if let Some(path) = self.paths.get(index) {
            let path = path.clone();
            let cache_clone = self.full_cache.clone();

            self.pool.execute(move || {
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
                        debug!("Diff curr index, not showing");
                    }
                });
            });
        }

        backup_image
    }

    /// Sliding window cache
    pub fn update_sliding_window(&self, center_idx: usize) {
        let len = self.paths.len();
        if len == 0 {
            return;
        }

        let window_radius = 1;

        let mut keep_indices = HashSet::new();
        keep_indices.insert(center_idx);

        for i in 1..=window_radius {
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

            self.pool.execute(move || {
                // Try without checking this
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
                    debug!("Preloaded: {}", index);
                }
            });
        }
    }
}
