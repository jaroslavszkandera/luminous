use dashmap::DashMap;
use log::error;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Hash)]
pub enum ThumbRes {
    Small = 256,
    Medium = 512,
    Large = 1024,
}

impl fmt::Display for ThumbRes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let val = match self {
            ThumbRes::Small => "256",
            ThumbRes::Medium => "512",
            ThumbRes::Large => "1024",
        };
        write!(f, "{}", val)
    }
}

impl TryFrom<u32> for ThumbRes {
    type Error = ();

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            256 => Ok(ThumbRes::Small),
            512 => Ok(ThumbRes::Medium),
            1024 => Ok(ThumbRes::Large),
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
    Thumb { id: ImageId, res: ThumbRes },
    Image { id: ImageId }, // Full
}

struct ThreadPoolState {
    catalog_view: RwLock<Vec<ImageId>>,

    // Grid view
    grid_idxs: RwLock<GridViewIdxs>,
    thumb_loading: Mutex<HashMap<ImageId, ThumbRes>>,
    thumb_cache: Arc<DashMap<ImageId, (ThumbRes, SharedPixelBuffer<Rgba8Pixel>)>>,
    thumb_res: Arc<RwLock<ThumbRes>>,

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
                error!("ID: {id:?} next best job alredy taken");
                None
            }
        } else {
            None
        }
    }

    fn get_next_thumb_job(&self, catalog_view: &[ImageId]) -> Option<Job> {
        let grid_idxs = self.grid_idxs.read().unwrap();
        let mut thumb_loading = self.thumb_loading.lock().unwrap();
        let curr_thumb_res = *self.thumb_res.read().unwrap();

        let next_to_load = (grid_idxs.start..grid_idxs.end)
            // .filter(|&i| i < catalog_view.len())
            .filter(|&i| {
                let id = &catalog_view[i];
                let loading = thumb_loading
                    .get(id)
                    .is_some_and(|&res| curr_thumb_res <= res);
                let cached = match self.thumb_cache.get(id) {
                    Some(thumb) => thumb.0 >= curr_thumb_res,
                    None => false,
                };
                !loading && !cached
            })
            .min_by_key(|&i| (i as isize - grid_idxs.focus as isize).abs());
        if let Some(i) = next_to_load {
            let id = catalog_view[i];
            let res = curr_thumb_res;
            match thumb_loading.get(&id) {
                Some(&existing) if existing >= res => {
                    error!("ID: {id:?} next best job already taken");
                    None
                }
                _ => {
                    thumb_loading.insert(id, res);
                    Some(Job::Thumb { id, res })
                }
            }
        } else {
            None
        }
    }

    fn mark_done(&self, job: &Job) {
        match *job {
            Job::Thumb { id, res } => {
                let mut thumb_loading = self.thumb_loading.lock().unwrap();
                // Only drop the entry if no higher-res decode replaced it.
                if thumb_loading.get(&id) == Some(&res) {
                    thumb_loading.remove(&id);
                }
            }
            Job::Image { id } => {
                self.full_loading.lock().unwrap().remove(&id);
            }
        }
    }
}

struct JobDone<'a> {
    state: &'a ThreadPoolState,
    job: Job,
}

impl Drop for JobDone<'_> {
    fn drop(&mut self) {
        self.state.mark_done(&self.job);
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
        thumb_cache: Arc<DashMap<ImageId, (ThumbRes, SharedPixelBuffer<Rgba8Pixel>)>>,
        thumb_res: Arc<RwLock<ThumbRes>>,
        full_cache: Arc<DashMap<ImageId, SharedPixelBuffer<Rgba8Pixel>>>,
        window_size: usize,
        grid_view_active: bool,
        handler: H,
    ) -> Self
    where
        H: Fn(Job) + Send + Sync + 'static,
    {
        log::info!("Starting image loader threadpool with {workers_cnt} threads");
        let lock = Arc::new(Mutex::new(()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let cv = Arc::new(Condvar::new());
        let state = Arc::new(ThreadPoolState {
            catalog_view: RwLock::new(Vec::new()),

            grid_idxs: RwLock::new(GridViewIdxs::default()),
            thumb_loading: Mutex::new(HashMap::new()),
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
                    move || loop {
                        if shutdown.load(Ordering::Relaxed) {
                            break;
                        }

                        let job = {
                            let mut job = state.get_next_best_job();
                            if job.is_none() {
                                let mut guard = lock.lock().unwrap();
                                job = state.get_next_best_job();
                                while job.is_none() && !shutdown.load(Ordering::Relaxed) {
                                    guard = cv.wait(guard).unwrap_or_else(|e| e.into_inner());
                                    job = state.get_next_best_job();
                                }
                            }
                            job
                        };

                        let Some(job) = job else {
                            continue;
                        };

                        let done = JobDone {
                            state: &state,
                            job: job.clone(),
                        };
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(job)))
                            .is_err()
                        {
                            error!("Image loader worker panicked");
                        }
                        drop(done);
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
