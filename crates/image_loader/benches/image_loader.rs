use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use luminous_image_loader::{ImageLoader, to_pixel_buffer, to_slint_image};
use luminous_plugins::PluginManager;
use tempfile::TempDir;

const WINDOW_SIZE: usize = 8;
const ITER_TIMEOUT: Duration = Duration::from_secs(120);
const IMAGE_COUNT: usize = 100;
const DEFAULT_RESOLUTION: u32 = 256;

const THREAD_COUNTS: &[usize] = &[1, 2, 3, 4, 5, 6, 7, 8];
const COLD_BATCH: usize = 100;

const POLL_INTERVAL: Duration = Duration::from_millis(2);
const PROGRESS_LOG_EVERY: Duration = Duration::from_secs(5);

fn debug_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        std::env::var("BENCH_LATCH_DEBUG")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    })
}

macro_rules! ldbg {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!("[latch] {}", format_args!($($arg)*));
        }
    };
}

struct Preset {
    width: u32,
    height: u32,
}

const PRESETS: &[Preset] = &[
    Preset {
        width: 3840,
        height: 2160,
    },
    Preset {
        width: 1920,
        height: 1080,
    },
];

struct ImageSource {
    _temp_dir: Option<TempDir>,
    paths: Vec<PathBuf>,
    label: String,
}

static IMAGES: OnceLock<ImageSource> = OnceLock::new();

fn get_image_dir() -> Option<PathBuf> {
    std::env::var("BENCH_IMAGE_DIR").ok().map(PathBuf::from)
}

fn scan_images(dir: &PathBuf) -> Vec<PathBuf> {
    let extensions = ["jpg", "jpeg", "png", "webp", "tiff", "gif"];
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if extensions.contains(&ext.to_string_lossy().to_lowercase().as_str()) {
                        paths.push(path);
                    }
                }
            }
        }
    }
    paths.sort();
    paths
}

fn init_images() -> &'static ImageSource {
    IMAGES.get_or_init(|| {
        if let Some(dir) = get_image_dir() {
            if dir.exists() {
                let paths = scan_images(&dir);
                if !paths.is_empty() {
                    let label = format!(
                        "BENCH_IMAGE_DIR acknowledged: {} images from {:?}",
                        paths.len(),
                        dir
                    );
                    return ImageSource { _temp_dir: None, paths, label };
                }
                eprintln!(
                    "BENCH_IMAGE_DIR={:?} exists but has no supported images, falling back to tempdir",
                    dir
                );
            } else {
                eprintln!("BENCH_IMAGE_DIR={:?} does not exist, falling back to tempdir", dir);
            }
        } else {
            eprintln!("BENCH_IMAGE_DIR not set, generating images in tempdir");
        }

        let temp_dir = TempDir::new().unwrap();
        let generated: Vec<PathBuf> = (0..IMAGE_COUNT)
            .map(|i| {
                let p = &PRESETS[i % PRESETS.len()];
                let img = RgbImage::from_fn(p.width, p.height, |x, y| {
                    Rgb([
                        (x * 255 / p.width) as u8,
                        (y * 255 / p.height) as u8,
                        ((x + y) * 127 / (p.width + p.height)) as u8,
                    ])
                });
                let path = temp_dir.path().join(format!("{i:04}.jpg"));
                DynamicImage::ImageRgb8(img)
                    .save_with_format(&path, ImageFormat::Jpeg)
                    .unwrap();
                path
            })
            .collect();

        let label = format!(
            "Generated {} synthetic images in tempdir {:?}",
            generated.len(),
            temp_dir.path()
        );
        ImageSource { _temp_dir: Some(temp_dir), paths: generated, label }
    })
}

fn images() -> &'static Vec<PathBuf> {
    &init_images().paths
}

fn print_source_banner() {
    static PRINTED: OnceLock<()> = OnceLock::new();
    PRINTED.get_or_init(|| {
        let src = init_images();
        eprintln!("{}", src.label);
        eprintln!("images available: {}", src.paths.len());
        eprintln!("thread counts under test: {:?}", THREAD_COUNTS);
        eprintln!("batch size per cold iter: {}", COLD_BATCH);
    });
}

fn make_loader(workers: usize, clear_disk_cache: bool) -> ImageLoader {
    let loader = ImageLoader::new(
        images().clone(),
        workers,
        WINDOW_SIZE,
        PluginManager::new().into(),
    );
    if clear_disk_cache {
        loader.clear_disk_cache();
    }
    loader
}

struct PollLatch<F: Fn() -> (usize, usize)> {
    label: &'static str,
    probe: F,
}

impl<F: Fn() -> (usize, usize)> PollLatch<F> {
    fn new(label: &'static str, probe: F) -> Self {
        Self { label, probe }
    }

    fn wait(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        let deadline = start + timeout;
        let mut last_log = start;
        let (initial_done, total) = (self.probe)();
        ldbg!(
            "{}: start, {}/{} already done",
            self.label,
            initial_done,
            total
        );

        loop {
            let (done, total) = (self.probe)();
            if done >= total {
                ldbg!(
                    "{}: ok {}/{} in {:.2}s",
                    self.label,
                    done,
                    total,
                    start.elapsed().as_secs_f64()
                );
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                eprintln!(
                    "[latch] {}: TIMEOUT at {}/{} after {:.2}s",
                    self.label,
                    done,
                    total,
                    start.elapsed().as_secs_f64()
                );
                return false;
            }
            if debug_enabled() && now.duration_since(last_log) >= PROGRESS_LOG_EVERY {
                ldbg!(
                    "{}: progress {}/{} at {:.2}s",
                    self.label,
                    done,
                    total,
                    start.elapsed().as_secs_f64()
                );
                last_log = now;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

#[derive(Clone)]
struct SignalLatch {
    label: &'static str,
    target: usize,
    fired: Arc<AtomicBool>,
    cv: Arc<(Mutex<()>, Condvar)>,
    done_count: Arc<Mutex<usize>>,
}

impl SignalLatch {
    fn new(label: &'static str, target: usize) -> Self {
        Self {
            label,
            target,
            fired: Arc::new(AtomicBool::new(false)),
            cv: Arc::new((Mutex::new(()), Condvar::new())),
            done_count: Arc::new(Mutex::new(0)),
        }
    }

    fn signal(&self, idx: usize) {
        let mut done = self.done_count.lock().unwrap();
        *done += 1;
        ldbg!(
            "{}: signal idx={} {}/{}",
            self.label,
            idx,
            *done,
            self.target
        );
        if *done >= self.target {
            self.fired.store(true, Ordering::Release);
            let (lock, cvar) = &*self.cv;
            let _g = lock.lock().unwrap();
            cvar.notify_all();
        }
    }

    fn done(&self) -> usize {
        *self.done_count.lock().unwrap()
    }

    fn wait(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        let deadline = start + timeout;
        let mut last_log = start;
        let (lock, cvar) = &*self.cv;
        let mut g = lock.lock().unwrap();
        ldbg!("{}: wait start, target={}", self.label, self.target);
        while !self.fired.load(Ordering::Acquire) {
            let now = Instant::now();
            if now >= deadline {
                eprintln!(
                    "[latch] {}: TIMEOUT at {}/{} after {:.2}s",
                    self.label,
                    self.done(),
                    self.target,
                    start.elapsed().as_secs_f64()
                );
                return false;
            }
            let wait_for = (deadline - now).min(PROGRESS_LOG_EVERY);
            let (g2, _) = cvar.wait_timeout(g, wait_for).unwrap();
            g = g2;
            if debug_enabled() && Instant::now().duration_since(last_log) >= PROGRESS_LOG_EVERY {
                ldbg!(
                    "{}: progress {}/{} at {:.2}s",
                    self.label,
                    self.done(),
                    self.target,
                    start.elapsed().as_secs_f64()
                );
                last_log = Instant::now();
            }
        }
        ldbg!(
            "{}: ok {}/{} in {:.2}s",
            self.label,
            self.done(),
            self.target,
            start.elapsed().as_secs_f64()
        );
        true
    }

    fn hook(
        &self,
    ) -> impl Fn(usize, slint::SharedPixelBuffer<slint::Rgba8Pixel>) + Send + Sync + 'static {
        let me = self.clone();
        move |idx, _buf| me.signal(idx)
    }
}

fn full_cache_done(loader: &ImageLoader, indices: &[usize]) -> usize {
    indices
        .iter()
        .filter(|&&i| loader.full_cache_contains(i))
        .count()
}

fn bench_cold_full_throughput(c: &mut Criterion) {
    print_source_banner();
    let paths = images();
    if paths.is_empty() {
        return;
    }

    let batch = COLD_BATCH.min(paths.len());
    let mut group = c.benchmark_group("cold_full_throughput");
    group.throughput(Throughput::Elements(batch as u64));
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(100));

    for &workers in THREAD_COUNTS {
        group.bench_with_input(
            BenchmarkId::from_parameter(workers),
            &workers,
            |b, &workers| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for i in 0..iters {
                        let loader = make_loader(workers, false);
                        loader.evict_all();

                        let offset = (i as usize * batch) % paths.len();
                        let indices: Vec<usize> =
                            (0..batch).map(|s| (offset + s) % paths.len()).collect();
                        let center = indices[0];

                        let probe_indices = indices.clone();
                        let latch = PollLatch::new("cold_full", || {
                            (full_cache_done(&loader, &probe_indices), batch)
                        });

                        let start = Instant::now();
                        loader.update_sliding_window(center, indices.clone());
                        if !latch.wait(ITER_TIMEOUT) {
                            panic!("Timeout in cold_full workers={workers} batch={batch}");
                        }
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

fn bench_cold_thumb_throughput(c: &mut Criterion) {
    print_source_banner();
    let paths = images();
    if paths.is_empty() {
        return;
    }

    let batch = COLD_BATCH.min(paths.len());
    let mut group = c.benchmark_group("cold_thumb_throughput");
    group.throughput(Throughput::Elements(batch as u64));
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(100));

    for &workers in THREAD_COUNTS {
        group.bench_with_input(
            BenchmarkId::from_parameter(workers),
            &workers,
            |b, &workers| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for i in 0..iters {
                        let mut loader = make_loader(workers, true);
                        loader.set_bucket_resolution(DEFAULT_RESOLUTION);

                        let latch = SignalLatch::new("cold_thumb", batch);
                        loader.on_thumb_ready(latch.hook());

                        let offset = (i as usize * batch) % paths.len();
                        let indices: Vec<usize> =
                            (0..batch).map(|s| (offset + s) % paths.len()).collect();

                        let start = Instant::now();
                        for &idx in &indices {
                            loader.load_grid_thumb(idx);
                        }
                        if !latch.wait(ITER_TIMEOUT) {
                            panic!("Timeout in cold_thumb workers={workers} batch={batch}");
                        }
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

fn bench_warm_cache_decode(c: &mut Criterion) {
    print_source_banner();
    let paths = images();
    if paths.is_empty() {
        return;
    }

    let loader = make_loader(8, false);
    let warmup = paths.len().min(IMAGE_COUNT);
    let indices: Vec<usize> = (0..warmup).collect();

    let probe = indices.clone();
    let latch = PollLatch::new("warm_warmup", || (full_cache_done(&loader, &probe), warmup));
    loader.update_sliding_window(0, indices);
    assert!(latch.wait(ITER_TIMEOUT), "Warm-up timed out");

    let mut group = c.benchmark_group("warm");
    group.throughput(Throughput::Elements(1));
    group.bench_function("dashmap_lookup", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for i in 0..iters {
                let idx = (i as usize) % warmup;
                let start = Instant::now();
                let img = loader.load_full_progressive(idx, false);
                std::hint::black_box(img);
                total += start.elapsed();
            }
            total
        });
    });
    group.finish();
}

fn bench_sequential_browse(c: &mut Criterion) {
    print_source_banner();
    let paths = images();
    if paths.is_empty() {
        return;
    }
    const BROWSE_COUNT: usize = 50;
    let browse = BROWSE_COUNT.min(paths.len());

    let mut group = c.benchmark_group("full_load");
    group.throughput(Throughput::Elements(browse as u64));
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(25));

    group.bench_function(format!("sequential_browse_{browse}"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for iter in 0..iters {
                let start_idx = (iter as usize * browse) % paths.len();
                let loader = make_loader(8, false);

                let start = Instant::now();
                for step in 0..browse {
                    let idx = (start_idx + step) % paths.len();
                    loader.load_full_progressive(idx, false);
                    let one = vec![idx];
                    let latch =
                        PollLatch::new("seq_browse_one", || (full_cache_done(&loader, &one), 1));
                    if !latch.wait(ITER_TIMEOUT) {
                        panic!("Timed out at idx={idx}");
                    }
                }
                total += start.elapsed();
            }
            total
        });
    });
    group.finish();
}

fn bench_dynamic_to_shared(c: &mut Criterion) {
    print_source_banner();
    let paths = images();
    if paths.is_empty() {
        return;
    }
    let img = image::open(&paths[0]).unwrap();

    c.bench_function("dynamic_to_shared", |b| {
        b.iter_batched(
            || img.clone(),
            |i| to_pixel_buffer(i),
            BatchSize::SmallInput,
        )
    });
}

fn bench_shared_to_image(c: &mut Criterion) {
    print_source_banner();
    let paths = images();
    if paths.is_empty() {
        return;
    }
    let img = image::open(&paths[0]).unwrap();
    let buf = to_pixel_buffer(img);

    c.bench_function("shared_to_image", |b| {
        b.iter(|| to_slint_image(buf.clone()))
    });
}

criterion_group!(
    benches,
    bench_cold_full_throughput,
    bench_cold_thumb_throughput,
    bench_warm_cache_decode,
    bench_sequential_browse,
    bench_dynamic_to_shared,
    bench_shared_to_image,
);
criterion_main!(benches);
