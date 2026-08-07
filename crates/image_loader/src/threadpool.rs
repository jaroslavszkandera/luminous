use dashmap::DashMap;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashSet;
use std::fmt;
use std::{
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::{self, JoinHandle},
};

use luminous_fs_scan::ImageId;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GridViewIdxs {
    pub start: usize,
    pub focus: usize,
    pub end: usize,
}

impl fmt::Display for GridViewIdxs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Start: {}, Focus: {}, End: {}",
            self.start, self.focus, self.end,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ThumbRes {
    Small = 256,
    Medium = 512,
    Large = 1024,
    NotResized,
}

impl TryFrom<u32> for ThumbRes {
    type Error = ();

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            256 => Ok(ThumbRes::Small),
            512 => Ok(ThumbRes::Medium),
            1024 => Ok(ThumbRes::Large),
            u32::MAX => Ok(ThumbRes::NotResized),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Full,
    Grid,
}

#[derive(Debug, Clone)]
pub enum Job {
    Thumb { id: ImageId, res: u32 },
    Image { id: ImageId }, // Full
}

struct ThreadPoolState {
    catalog_view: RwLock<Vec<ImageId>>,

    // Grid view
    grid_idxs: RwLock<GridViewIdxs>,
    thumb_loading: Mutex<HashSet<(ImageId, u32)>>,
    thumb_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>>,
    thumb_res: Arc<AtomicU32>,

    // Full view
    full_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>>,
    full_loading: Mutex<HashSet<ImageId>>,
    active_idx: AtomicU32,
    window_size: usize,
    active_view: RwLock<View>,
}

impl ThreadPoolState {
    fn get_next_best_job(&self) -> Option<Job> {
        let catalog_view = self.catalog_view.read().unwrap();

        match *self.active_view.read().unwrap() {
            View::Grid => self
                .get_next_thumb_job(&catalog_view)
                .or_else(|| self.get_next_full_job(&catalog_view)),
            View::Full => self
                .get_next_full_job(&catalog_view)
                .or_else(|| self.get_next_thumb_job(&catalog_view)),
        }
    }

    fn get_next_full_job(&self, catalog_view: &[ImageId]) -> Option<Job> {
        let len = catalog_view.len() as isize;
        if len == 0 {
            return None;
        }
        let mut full_loading = self.full_loading.lock().unwrap();
        let active_idx = self.active_idx.load(Ordering::Relaxed) as isize;
        let window_size = self.window_size as isize;

        let next_to_load = (-window_size..=window_size)
            .map(|offset| (active_idx + offset).rem_euclid(len) as usize)
            .filter(|&i| {
                let id = &catalog_view[i];
                let loading = full_loading.contains(&id);
                let cached = self.full_cache.contains_key(&id);
                !loading && !cached
            })
            .min_by_key(|&i| (i as isize - active_idx as isize).abs());
        if let Some(i) = next_to_load {
            let id = catalog_view[i];
            if full_loading.insert(id) {
                return Some(Job::Image { id });
            } else {
                log::error!("ID: {id:?} next best job alredy taken");
                None
            }
        } else {
            None
        }
    }

    fn get_next_thumb_job(&self, catalog_view: &[ImageId]) -> Option<Job> {
        let grid_idxs = self.grid_idxs.read().unwrap();
        let mut thumb_loading = self.thumb_loading.lock().unwrap();
        let curr_thumb_res = self.thumb_res.load(Ordering::Relaxed);

        let next_to_load = (grid_idxs.start..grid_idxs.end)
            // .filter(|&i| i < catalog_view.len())
            .filter(|&i| {
                let id = &catalog_view[i];
                let loading = thumb_loading
                    .iter()
                    .any(|(loading_id, res)| *loading_id == *id && curr_thumb_res <= *res);
                let cached = match self.thumb_cache.get(id) {
                    Some(buf) => match ThumbRes::try_from(curr_thumb_res) {
                        Ok(ThumbRes::NotResized) => true,
                        Ok(_) => buf.width().max(buf.height()) >= curr_thumb_res,
                        Err(_) => buf.width().max(buf.height()) >= curr_thumb_res,
                    },
                    None => false,
                };
                !loading && !cached
            })
            .min_by_key(|&i| (i as isize - grid_idxs.focus as isize).abs());
        if let Some(i) = next_to_load {
            let id = catalog_view[i];
            let res = curr_thumb_res;
            if thumb_loading.insert((id, res)) {
                return Some(Job::Thumb { id, res });
            } else {
                log::error!("ID: {id:?} next best job alredy taken");
                None
            }
        } else {
            None
        }
    }

    fn mark_done(&self, job: &Job) {
        match *job {
            Job::Thumb { id, res } => {
                self.thumb_loading.lock().unwrap().remove(&(id, res));
            }
            Job::Image { id } => {
                self.full_loading.lock().unwrap().remove(&id);
            }
        }
    }
}

pub struct ThreadPool {
    workers: Vec<JoinHandle<()>>,
    cv: Arc<Condvar>,
    lock: Arc<Mutex<()>>,
    shutdown: Arc<AtomicBool>,

    state: Arc<ThreadPoolState>,
}

impl ThreadPool {
    pub fn new<H>(
        workers_cnt: usize,
        thumb_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>>,
        thumb_res: Arc<AtomicU32>,
        full_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>>,
        window_size: usize,
        grid_view_active: bool,
        handler: H,
    ) -> Self
    where
        H: Fn(Job) + Send + Sync + 'static,
    {
        log::info!("Starting image loader with {workers_cnt} threads");
        let lock = Arc::new(Mutex::new(()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let cv = Arc::new(Condvar::new());
        let state = Arc::new(ThreadPoolState {
            catalog_view: RwLock::new(Vec::new()),

            grid_idxs: RwLock::new(GridViewIdxs::default()),
            thumb_loading: Mutex::new(HashSet::new()),
            thumb_cache,
            thumb_res,

            full_cache,
            full_loading: Mutex::new(HashSet::new()),
            active_idx: AtomicU32::new(0),
            window_size,
            active_view: RwLock::new(match grid_view_active {
                true => View::Grid,
                false => View::Full,
            }),
        });
        let handler = Arc::new(handler);

        let workers = (0..workers_cnt)
            .map(|_| {
                let shutdown = Arc::clone(&shutdown);
                let lock = Arc::clone(&lock);
                let cv = Arc::clone(&cv);
                let state = Arc::clone(&state);

                thread::spawn({
                    let handler = Arc::clone(&handler);
                    move || {
                        loop {
                            if shutdown.load(Ordering::Relaxed) {
                                break;
                            }
                            match state.get_next_best_job() {
                                Some(job) => {
                                    handler(job.clone());
                                    state.mark_done(&job);
                                }
                                None => {
                                    let guard = lock.lock().unwrap();
                                    let _guard = cv.wait(guard);
                                }
                            }
                        }
                    }
                })
            })
            .collect();
        Self {
            workers,
            cv,
            lock,
            shutdown,
            state,
        }
    }

    pub fn set_catalog_view(&self, catalog_view: Vec<ImageId>) {
        *self.state.catalog_view.write().unwrap() = catalog_view;
    }

    pub fn set_grid_view(&self, idxs: GridViewIdxs) {
        *self.state.grid_idxs.write().unwrap() = idxs;
        self.wake();
    }

    pub fn set_active_idx(&self, index: usize) {
        self.state.active_idx.store(index as u32, Ordering::Relaxed);
        self.wake();
    }

    pub fn set_active_view(&self, active_view: View) {
        *self.state.active_view.write().unwrap() = active_view;
    }

    fn wake(&self) {
        let _guard = self.lock.lock().unwrap();
        self.cv.notify_all();
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        {
            let _guard = self.lock.lock().unwrap();
            self.cv.notify_all();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}
