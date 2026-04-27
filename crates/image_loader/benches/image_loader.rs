use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use luminous_image_loader::{ImageLoader, to_pixel_buffer, to_slint_image};
use luminous_plugins::PluginManager;
use tempfile::TempDir;

// Settings
const WORKERS: usize = 8;
const WINDOW_SIZE: usize = 8;
const ITER_TIMEOUT: Duration = Duration::from_secs(60);
const IMAGE_COUNT: usize = 100;
const DEFAULT_RESOLUTION: u32 = 256;

// Helpers
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
}

static IMAGES: OnceLock<ImageSource> = OnceLock::new();

fn get_image_dir() -> PathBuf {
    std::env::var("BENCH_IMAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            directories::UserDirs::new()
                .and_then(|dirs| dirs.picture_dir().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."))
        })
}

fn scan_images(dir: &PathBuf) -> Vec<PathBuf> {
    let extensions = [
        "jpg", "jpeg", "png", "webp", "tiff", "bmp", "gif", "heic", "raw", "arw", "cr2", "dng",
    ];
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

fn images() -> &'static Vec<PathBuf> {
    &IMAGES
        .get_or_init(|| {
            let dir = get_image_dir();
            let paths = if dir.exists() && dir.to_string_lossy() != "." {
                scan_images(&dir)
            } else {
                Vec::new()
            };

            if paths.is_empty() {
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
                ImageSource {
                    _temp_dir: Some(temp_dir),
                    paths: generated,
                }
            } else {
                eprintln!("Using {} real images from {:?}", paths.len(), dir);
                ImageSource {
                    _temp_dir: None,
                    paths,
                }
            }
        })
        .paths
}

fn make_loader(clear_disk_cache: bool) -> ImageLoader {
    let loader = ImageLoader::new(
        images().clone(),
        WORKERS,
        WINDOW_SIZE,
        PluginManager::new().into(),
    );
    if clear_disk_cache {
        loader.clear_disk_cache();
    }
    loader
}

#[derive(Clone)]
struct FlagLatch {
    target: usize,
    fired: Arc<(Mutex<bool>, Condvar)>,
}

impl FlagLatch {
    fn new(target: usize) -> Self {
        Self {
            target,
            fired: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn wait(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.fired;
        let deadline = Instant::now() + timeout;
        let mut fired = lock.lock().unwrap();
        while !*fired {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (f, _) = cvar.wait_timeout(fired, remaining).unwrap();
            fired = f;
        }
        true
    }

    fn hook(
        &self,
    ) -> impl Fn(usize, slint::SharedPixelBuffer<slint::Rgba8Pixel>) + Send + Sync + 'static {
        let target = self.target;
        let fired = self.fired.clone();
        move |idx, _buf| {
            if idx == target {
                *fired.0.lock().unwrap() = true;
                fired.1.notify_all();
            }
        }
    }
}

// Benchmarks

// Cold full image load (disk I/O + decode)
fn bench_cold_full_load(c: &mut Criterion) {
    let paths = images();
    if paths.is_empty() {
        return;
    }

    let mut group = c.benchmark_group("cold_full");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    group.bench_function("cold_single_full", |b| {
        b.iter_custom(|iters| {
            let mut total_duration = Duration::ZERO;
            for i in 0..iters {
                let idx = (i as usize) % paths.len();
                let mut loader = make_loader(false);
                let flag = FlagLatch::new(idx);
                loader.on_full_ready(flag.hook());

                let start = Instant::now();
                loader.load_full_progressive(idx, false);
                assert!(flag.wait(ITER_TIMEOUT), "Timeout for full at idx {}", idx);
                total_duration += start.elapsed();
            }
            total_duration
        });
    });
    group.finish();
}

// Cold thumbnail load (disk I/O + decode + resize)
fn bench_cold_thumb_load(c: &mut Criterion) {
    let paths = images();
    if paths.is_empty() {
        return;
    }

    let mut group = c.benchmark_group("cold_thumb");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    group.bench_function("cold_single_thumb", |b| {
        b.iter_custom(|iters| {
            let mut total_duration = Duration::ZERO;
            for i in 0..iters {
                let idx = (i as usize) % paths.len();
                let mut loader = make_loader(true);
                loader.set_bucket_resolution(DEFAULT_RESOLUTION);
                let flag = FlagLatch::new(idx);
                loader.on_thumb_ready(flag.hook());

                let start = Instant::now();
                loader.load_grid_thumb(idx);
                assert!(flag.wait(ITER_TIMEOUT), "Timeout for thumb at idx {}", idx);
                total_duration += start.elapsed();
            }
            total_duration
        });
    });
    group.finish();
}

// Warm cache - DashMap lookup only
fn bench_warm_cache_decode(c: &mut Criterion) {
    let mut loader = make_loader(false);
    let flag = FlagLatch::new(0);
    loader.on_full_ready(flag.hook());

    for idx in 0..IMAGE_COUNT {
        let f = FlagLatch::new(idx);
        loader.on_full_ready(f.hook());
        loader.load_full_progressive(idx, false);
        assert!(f.wait(ITER_TIMEOUT), "Warm-up timed out at idx={idx}");
    }

    let mut group = c.benchmark_group("warm");
    group.throughput(Throughput::Elements(1));
    group.bench_function("dashmap_lookup", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for i in 0..iters {
                let idx = (i as usize) % IMAGE_COUNT;
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
    const BROWSE_COUNT: usize = 50;

    let mut group = c.benchmark_group("full_load");
    group.throughput(Throughput::Elements(BROWSE_COUNT as u64));
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(25));

    group.bench_function(format!("sequential_browse_{BROWSE_COUNT}"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for iter in 0..iters {
                let start_idx = (iter as usize * BROWSE_COUNT) % IMAGE_COUNT;
                let mut loader = make_loader(false);

                let start = Instant::now();
                for step in 0..BROWSE_COUNT {
                    let idx = (start_idx + step) % IMAGE_COUNT;
                    let flag = FlagLatch::new(idx);
                    loader.on_full_ready(flag.hook());
                    loader.load_full_progressive(idx, false);
                    assert!(flag.wait(ITER_TIMEOUT), "Timed out at idx={idx}");
                }
                total += start.elapsed();
            }
            total
        });
    });
    group.finish();
}

fn bench_dynamic_to_shared(c: &mut Criterion) {
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
    bench_cold_full_load,
    bench_cold_thumb_load,
    bench_warm_cache_decode,
    bench_sequential_browse,
    bench_dynamic_to_shared,
    bench_shared_to_image,
);
criterion_main!(benches);
