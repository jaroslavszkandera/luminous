use image;
use log::{debug, error, warn};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Instant;

struct CacheJob {
    path_idx: usize,
}

struct CacheData {
    image_map: HashMap<usize, SharedPixelBuffer<Rgba8Pixel>>,
    paths_index: usize,
}

pub struct ImageRingCache {
    data: Arc<Mutex<CacheData>>,
    paths: Arc<Vec<PathBuf>>,
    job_sender: mpsc::Sender<CacheJob>,
    half_cache_size: usize,
}

impl ImageRingCache {
    pub fn new(
        paths: Rc<Vec<PathBuf>>,
        half_cache_size: usize,
        start_index: usize,
    ) -> ImageRingCache {
        let cache_size = half_cache_size * 2 + 1;
        let paths_len = paths.len();

        let mut initial_map = HashMap::with_capacity(cache_size);

        for i in -(half_cache_size as isize)..=(half_cache_size as isize) {
            let path_idx = wrap_index(start_index, i, paths_len);

            let buffer =
                Self::load_buffer(&paths[path_idx]).unwrap_or_else(|_| create_placeholder_buffer());

            initial_map.insert(path_idx, buffer);
        }

        let initial_data = CacheData {
            image_map: initial_map,
            paths_index: start_index,
        };

        let shared_data = Arc::new(Mutex::new(initial_data));
        let worker_data = shared_data.clone();
        let (job_sender, job_receiver) = mpsc::channel::<CacheJob>();

        let paths_arc = Arc::new(paths.to_vec());
        let worker_paths = paths_arc.clone();

        thread::spawn(move || {
            debug!("Cache worker thread started.");
            for job in job_receiver {
                debug!("Worker received job to load path index {}", job.path_idx);

                let buffer_result = Self::load_buffer(&worker_paths[job.path_idx]);

                match worker_data.lock() {
                    Ok(mut data) => {
                        let buffer = buffer_result.unwrap_or_else(|e| {
                            error!("Worker failed to load image: {}", e);
                            create_placeholder_buffer()
                        });

                        data.image_map.insert(job.path_idx, buffer);

                        if data.image_map.len() > cache_size {
                            let current_index = data.paths_index;
                            let paths_len = worker_paths.len();

                            let max_circular_dist = half_cache_size as isize;

                            let keys_to_remove: Vec<usize> = data
                                .image_map
                                .keys()
                                .filter(|&&k| {
                                    let k_isize = k as isize;
                                    let current_isize = current_index as isize;
                                    let len_isize = paths_len as isize;

                                    let dist_forward =
                                        (k_isize - current_isize + len_isize) % len_isize;
                                    let dist_backward =
                                        (current_isize - k_isize + len_isize) % len_isize;

                                    let shortest_dist = dist_forward.min(dist_backward);
                                    shortest_dist > max_circular_dist
                                })
                                .copied()
                                .collect();

                            for k in keys_to_remove {
                                data.image_map.remove(&k);
                                debug!("Evicted path index {} from cache.", k);
                            }
                        }
                    }
                    Err(poison) => {
                        error!("Mutex poisoned in worker thread: {}", poison);
                        break;
                    }
                }
            }
            debug!("Cache worker thread exiting.");
        });

        ImageRingCache {
            data: shared_data,
            paths: paths_arc,
            job_sender,
            half_cache_size,
        }
    }

    fn load_and_cache(
        &self,
        path_idx: usize,
    ) -> Result<SharedPixelBuffer<Rgba8Pixel>, Box<dyn Error>> {
        let path = &self.paths[path_idx];
        let buffer = Self::load_buffer(path)?;

        if let Ok(mut data) = self.data.lock() {
            data.image_map.insert(path_idx, buffer.clone());
        }
        Ok(buffer)
    }

    fn queue_preloads(&self, current_paths_index: usize, paths_len: usize) {
        for i in -(self.half_cache_size as isize)..=(self.half_cache_size as isize) {
            let path_to_load_idx = wrap_index(current_paths_index, i, paths_len);

            if self
                .data
                .lock()
                .unwrap()
                .image_map
                .contains_key(&path_to_load_idx)
            {
                continue;
            }

            let job = CacheJob {
                path_idx: path_to_load_idx,
            };
            if self.job_sender.send(job).is_err() {
                error!("Failed to send pre-load job for path {}.", path_to_load_idx);
            }
        }
    }

    pub fn get_next(&mut self) -> Result<Image, Box<dyn Error>> {
        let paths_len = self.paths.len();
        let new_path_idx = {
            let mut data = self.data.lock().unwrap();
            data.paths_index = wrap_index(data.paths_index, 1, paths_len);
            data.paths_index
        };

        let buffer_to_show = {
            let data = self.data.lock().unwrap();
            if let Some(buffer) = data.image_map.get(&new_path_idx) {
                buffer.clone()
            } else {
                drop(data);
                warn!(
                    "Cache miss on get_next. Synchronous load for path {}.",
                    new_path_idx
                );
                self.load_and_cache(new_path_idx)?
            }
        };

        self.queue_preloads(new_path_idx, paths_len);

        Ok(Image::from_rgba8_premultiplied(buffer_to_show))
    }

    pub fn get_prev(&mut self) -> Result<Image, Box<dyn Error>> {
        let paths_len = self.paths.len();
        let new_path_idx = {
            let mut data = self.data.lock().unwrap();
            data.paths_index = wrap_index(data.paths_index, -1, paths_len);
            data.paths_index
        };

        let buffer_to_show = {
            let data = self.data.lock().unwrap();
            if let Some(buffer) = data.image_map.get(&new_path_idx) {
                buffer.clone()
            } else {
                drop(data);
                warn!(
                    "Cache miss on get_prev. Synchronous load for path {}.",
                    new_path_idx
                );
                self.load_and_cache(new_path_idx)?
            }
        };

        self.queue_preloads(new_path_idx, paths_len);

        Ok(Image::from_rgba8_premultiplied(buffer_to_show))
    }

    fn load_buffer(path: &Path) -> Result<SharedPixelBuffer<Rgba8Pixel>, Box<dyn Error>> {
        let img_name = path.file_name().unwrap_or_default();
        let load_start = Instant::now();
        let dyn_img = image::open(path)?;
        let rgba_img = dyn_img.to_rgba8();
        let (width, height) = rgba_img.dimensions();

        let buffer =
            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba_img.as_raw(), width, height);

        debug!(
            "Image loaded in {:.2} ms ({})",
            load_start.elapsed().as_millis(),
            img_name.to_string_lossy()
        );
        Ok(buffer)
    }
}

fn create_placeholder_buffer() -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::<Rgba8Pixel>::new(200, 200)
}

fn wrap_index(current_index: usize, offset: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }

    let len = len as isize;
    let current = current_index as isize;
    let target = current + offset;
    let wrapped = (target % len + len) % len;
    wrapped as usize
}
