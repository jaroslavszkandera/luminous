use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use image::DynamicImage;
use luminous_pipeline::StepFactory;
use luminous_pipeline::gpu_proc::GpuProcessor;
use luminous_pipeline::types::*;
use std::time::Duration;

fn test_image(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([
            ((x * 3) % 256) as u8,
            ((y * 3) % 256) as u8,
            ((x + y) % 256) as u8,
            255,
        ])
    }))
}

/// One upload one download
fn gpu_pipeline_batch(
    g: &GpuProcessor,
    img: &DynamicImage,
    filters: &[FilterKind],
) -> DynamicImage {
    let cpu = StepFactory::new();
    let mut tex = g.upload(img);
    for f in filters {
        tex = match f {
            FilterKind::GaussianBlur { sigma } => g.blur_gpu(&tex, sigma.max(0.1)),
            FilterKind::Resize { w, h } => g.resize_gpu(&tex, *w, *h),
            FilterKind::Rotate(a) => g.rotate_gpu(&tex, a.clone()),
            FilterKind::Brighten { value } => g.brighten_gpu(&tex, *value),
            FilterKind::Flip(d) => g.flip_gpu(&tex, d.clone()),
            FilterKind::ExtractChannel(ch) => g.extract_channel_gpu(&tex, ch.clone()),
            other => {
                // CPU fall-back for ops without a GPU shader
                let img_cpu = g.download(&tex);
                g.upload(&cpu.apply(img_cpu, other))
            }
        };
    }
    g.download(&tex)
}

/// Naive sequential GPU path
fn gpu_pipeline_sequential(
    g: &GpuProcessor,
    cpu: &StepFactory,
    img: &DynamicImage,
    filters: &[FilterKind],
) -> DynamicImage {
    let mut current = img.clone();
    for f in filters {
        current = match f {
            FilterKind::GaussianBlur { sigma } => {
                let tex = g.upload(&current);
                g.download(&g.blur_gpu(&tex, sigma.max(0.1)))
            }
            FilterKind::Resize { w, h } => {
                let tex = g.upload(&current);
                g.download(&g.resize_gpu(&tex, *w, *h))
            }
            FilterKind::Rotate(a) => {
                let tex = g.upload(&current);
                g.download(&g.rotate_gpu(&tex, a.clone()))
            }
            FilterKind::Brighten { value } => {
                let tex = g.upload(&current);
                g.download(&g.brighten_gpu(&tex, *value))
            }
            FilterKind::Flip(d) => {
                let tex = g.upload(&current);
                g.download(&g.flip_gpu(&tex, d.clone()))
            }
            FilterKind::ExtractChannel(ch) => {
                let tex = g.upload(&current);
                g.download(&g.extract_channel_gpu(&tex, ch.clone()))
            }
            other => cpu.apply(current, other),
        };
    }
    current
}

fn bench_blur(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);
    let sigma = 3.0_f32;
    let filter = FilterKind::GaussianBlur { sigma };

    let cpu = StepFactory::new();
    let gpu = pollster::block_on(GpuProcessor::new());

    let mut group = c.benchmark_group(format!("blur (sigma={sigma}) {w}x{h}"));
    group.measurement_time(Duration::from_secs(15));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| b.iter(|| cpu.apply(img.clone(), &filter)));

    if let Some(ref g) = gpu {
        group.bench_function("GPU", |b| {
            b.iter(|| {
                let tex = g.upload(&img);
                let out = g.blur_gpu(&tex, sigma.max(0.1));
                g.download(&out)
            })
        });
    }

    group.finish();
}

fn bench_resize(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);
    let (dst_w, dst_h) = (384u32, 384u32);
    let filter = FilterKind::Resize { w: dst_w, h: dst_h };

    let cpu = StepFactory::new();
    let gpu = pollster::block_on(GpuProcessor::new());

    let mut group = c.benchmark_group(format!("resize {w}x{h} -> {dst_w}x{dst_h}"));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| b.iter(|| cpu.apply(img.clone(), &filter)));

    if let Some(ref g) = gpu {
        group.bench_function("GPU", |b| {
            b.iter(|| {
                let tex = g.upload(&img);
                let out = g.resize_gpu(&tex, dst_w, dst_h);
                g.download(&out)
            })
        });
    }

    group.finish();
}

fn bench_rotate(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);
    let filter = FilterKind::Rotate(RotateAngle::R90);

    let cpu = StepFactory::new();
    let gpu = pollster::block_on(GpuProcessor::new());

    let mut group = c.benchmark_group(format!("rotate_90 {w}x{h}"));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| b.iter(|| cpu.apply(img.clone(), &filter)));

    if let Some(ref g) = gpu {
        group.bench_function("GPU", |b| {
            b.iter(|| {
                let tex = g.upload(&img);
                let out = g.rotate_gpu(&tex, RotateAngle::R90);
                g.download(&out)
            })
        });
    }

    group.finish();
}

fn bench_brighten(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);
    let value = 50_i32;
    let filter = FilterKind::Brighten { value };

    let cpu = StepFactory::new();
    let gpu = pollster::block_on(GpuProcessor::new());

    let mut group = c.benchmark_group(format!("brighten (+{value}) {w}x{h}"));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| b.iter(|| cpu.apply(img.clone(), &filter)));

    if let Some(ref g) = gpu {
        group.bench_function("GPU", |b| {
            b.iter(|| {
                let tex = g.upload(&img);
                let out = g.brighten_gpu(&tex, value);
                g.download(&out)
            })
        });
    }

    group.finish();
}

fn bench_flip(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);
    let filter = FilterKind::Flip(FlipDirection::Horizontal);

    let cpu = StepFactory::new();
    let gpu = pollster::block_on(GpuProcessor::new());

    let mut group = c.benchmark_group(format!("flip_horizontal {w}x{h}"));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| b.iter(|| cpu.apply(img.clone(), &filter)));

    if let Some(ref g) = gpu {
        group.bench_function("GPU", |b| {
            b.iter(|| {
                let tex = g.upload(&img);
                let out = g.flip_gpu(&tex, FlipDirection::Horizontal);
                g.download(&out)
            })
        });
    }

    group.finish();
}

fn bench_full_pipeline_gpu(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);

    let filters = vec![
        FilterKind::ExtractChannel(Channel::Gray),
        FilterKind::Resize { w: 384, h: 384 },
        FilterKind::GaussianBlur { sigma: 3.0 },
        FilterKind::Rotate(RotateAngle::R90),
        FilterKind::Flip(FlipDirection::Horizontal),
        FilterKind::Brighten { value: 10 },
    ];

    let cpu = StepFactory::new();
    let gpu = pollster::block_on(GpuProcessor::new());

    let mut group = c.benchmark_group(format!("full_pipeline {w}x{h}"));
    group.throughput(Throughput::Elements(1));
    group.measurement_time(Duration::from_secs(12));

    group.bench_function("CPU", |b| {
        b.iter(|| cpu.apply_pipeline(img.clone(), &filters))
    });

    if let Some(ref g) = gpu {
        group.bench_function("GPU/batch", |b| {
            b.iter(|| gpu_pipeline_batch(g, &img, &filters))
        });

        group.bench_function("GPU/sequential", |b| {
            b.iter(|| gpu_pipeline_sequential(g, &cpu, &img, &filters))
        });
    }

    group.finish();
}

/// GPU transfer overhead at different image sizes.
fn bench_gpu_transfer(c: &mut Criterion) {
    let gpu = pollster::block_on(GpuProcessor::new());
    let Some(ref g) = gpu else { return };

    let sizes: &[(u32, u32)] = &[(512, 512), (1920, 1080), (3840, 2160)];

    let mut group = c.benchmark_group("gpu_transfer");
    group.measurement_time(Duration::from_secs(10));

    for &(w, h) in sizes {
        let img = test_image(w, h);
        let label = format!("{w}x{h}");

        let src_tex = g.upload(&img);
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("noop_only", &label), &src_tex, |b, tex| {
            b.iter(|| g.brighten_gpu(tex, 0))
        });

        // Image upload + download so * 2
        group.throughput(Throughput::Bytes((w * h * 4 * 2) as u64));
        group.bench_with_input(
            BenchmarkId::new("upload+noop+download", &label),
            &img,
            |b, img| {
                b.iter(|| {
                    let tex = g.upload(img);
                    let out = g.brighten_gpu(&tex, 0);
                    g.download(&out)
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_blur,
    bench_resize,
    bench_rotate,
    bench_brighten,
    bench_flip,
    bench_full_pipeline_gpu,
    bench_gpu_transfer,
);
criterion_main!(benches);
