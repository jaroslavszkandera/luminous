use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use image::DynamicImage;
use luminous_pipeline::gpu_proc::GpuProcessor;
use luminous_pipeline::types::*;
use luminous_pipeline::StepFactory;
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

fn bench_full_pipeline(c: &mut Criterion) {
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

    let mut group = c.benchmark_group("full_pipeline");
    group.throughput(Throughput::Elements(1));
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("CPU", |b| {
        b.iter(|| cpu.apply_pipeline(img.clone(), &filters))
    });

    group.finish();
}

// TODO: benchmark GPU full pipeline once GpuStepFactory is in luminous-pipeline
criterion_group!(benches, bench_blur, bench_resize, bench_full_pipeline);
criterion_main!(benches);
