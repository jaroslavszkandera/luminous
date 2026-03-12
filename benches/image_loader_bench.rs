use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use tempfile::TempDir;

use luminous::image_loader::ImageLoader;
use luminous::plugins::PluginManager;

// Settings
const WORKERS: usize = 8;
const WINDOW_SIZE: usize = 8;
const ITER_TIMEOUT: Duration = Duration::from_secs(60);
const IMAGE_COUNT: usize = 100;

// Helpers
struct Preset {
    width: u32,
    height: u32,
}

const PRESETS: &[Preset] = &[
    Preset {
        width: 1920,
        height: 1080,
    },
    Preset {
        width: 3840,
        height: 2160,
    },
];

static IMAGES: OnceLock<(TempDir, Vec<PathBuf>)> = OnceLock::new();

fn images() -> &'static Vec<PathBuf> {
    &IMAGES
        .get_or_init(|| {
            let dir = TempDir::new().unwrap();
            let paths = (0..IMAGE_COUNT)
                .map(|i| {
                    let p = &PRESETS[i % PRESETS.len()];
                    let img = RgbImage::from_fn(p.width, p.height, |x, y| {
                        Rgb([
                            (x * 255 / p.width) as u8,
                            (y * 255 / p.height) as u8,
                            ((x + y) * 127 / (p.width + p.height)) as u8,
                        ])
                    });
                    let path = dir.path().join(format!("{i:04}.jpg"));
                    DynamicImage::ImageRgb8(img)
                        .save_with_format(&path, ImageFormat::Jpeg)
                        .unwrap();
                    path
                })
                .collect();
            (dir, paths)
        })
        .1
}

fn make_loader() -> ImageLoader {
    ImageLoader::new(images().clone(), WORKERS, WINDOW_SIZE, PluginManager::new())
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

// Is it only testing the DashMap?
fn bench_warm_cache_decode(c: &mut Criterion) {
    let mut loader = make_loader();
    let flag = FlagLatch::new(0);
    loader.on_full_ready(flag.hook());

    for idx in 0..IMAGE_COUNT {
        let f = FlagLatch::new(idx);
        loader.on_full_ready(f.hook());
        loader.load_full_progressive(idx);
        assert!(f.wait(ITER_TIMEOUT), "Warm-up timed out at idx={idx}");
    }

    let mut group = c.benchmark_group("full_load");
    group.throughput(Throughput::Elements(1));
    group.bench_function("warm_cache", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for i in 0..iters {
                let idx = (i as usize) % IMAGE_COUNT;
                let start = Instant::now();
                let img = loader.load_full_progressive(idx);
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
                let mut loader = make_loader();

                let start = Instant::now();
                for step in 0..BROWSE_COUNT {
                    let idx = (start_idx + step) % IMAGE_COUNT;
                    let flag = FlagLatch::new(idx);
                    loader.on_full_ready(flag.hook());
                    loader.load_full_progressive(idx);
                    assert!(flag.wait(ITER_TIMEOUT), "Timed out at idx={idx}");
                }
                total += start.elapsed();
            }
            total
        });
    });
    group.finish();
}

criterion_group!(benches, bench_warm_cache_decode, bench_sequential_browse,);
criterion_main!(benches);
