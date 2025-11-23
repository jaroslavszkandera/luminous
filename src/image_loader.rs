use log::{debug, error};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, Weak};
use std::collections::HashMap;
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

    pub fn load_lazy<F>(
        &self,
        index: usize,
        ui_handle: Weak<MainWindow>,
        on_loaded: F,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>>
    where
        F: Fn(MainWindow, usize, Image) + Send + 'static,
    {
        // Check Cache
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
                        // Force resize for grid
                        let dyn_img = dyn_img.thumbnail(200, 200);
                        let rgba = dyn_img.to_rgba8();
                        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                            rgba.as_raw(),
                            rgba.width(),
                            rgba.height(),
                        )
                    }
                    Err(e) => {
                        error!("Failed to load thumbnail {}: {}", path.display(), e);
                        get_placeholder()
                    }
                };

                let duration = start.elapsed();
                debug!(
                    "Thumbnail loaded: {:?} in {:.2}ms",
                    path.file_name().unwrap_or_default(),
                    duration.as_secs_f64() * 1000.0
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

    pub fn preload(&self, index: usize) {
        // Check Full Cache
        if self.full_cache.lock().unwrap().contains_key(&index) {
            return;
        }

        if let Some(path) = self.paths.get(index) {
            let path = path.clone();
            let cache_clone = self.full_cache.clone();

            self.pool.execute(move || {
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

    // TODO(JS): Make async
    pub fn load_full(&self, index: usize) -> Image {
        {
            let cache_handle = self.full_cache.lock().unwrap();
            if let Some(buffer) = cache_handle.get(&index) {
                debug!("Full image cache hit: index {}", index);
                return Image::from_rgba8(buffer.clone());
            }
        }

        if let Some(path) = self.paths.get(index) {
            let start = Instant::now();
            match image::open(path) {
                Ok(dyn_img) => {
                    let rgba = dyn_img.to_rgba8();
                    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        rgba.as_raw(),
                        rgba.width(),
                        rgba.height(),
                    );

                    let duration = start.elapsed();
                    debug!(
                        "Full image loaded: {:?} in {:.2}ms",
                        path.file_name().unwrap_or_default(),
                        duration.as_secs_f64() * 1000.0
                    );

                    self.full_cache
                        .lock()
                        .unwrap()
                        .insert(index, buffer.clone());
                    return Image::from_rgba8(buffer);
                }
                Err(e) => {
                    error!("Error loading full image: {}", e);
                    return Image::from_rgba8(get_placeholder());
                }
            }
        }
        Image::from_rgba8(get_placeholder())
    }
}
