use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use image::DynamicImage;
use std::sync::Arc;
use std::time::Duration;

use luminous::pipeline::StepFactory;
use luminous::pipeline::gpu_proc::GpuProcessor;
use luminous::{PipelineStep, PipelineStepKind};

fn test_image(w: u32, h: u32) -> DynamicImage {
    let buf = image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([
            ((x * 3) % 256) as u8,
            ((y * 3) % 256) as u8,
            ((x + y) % 256) as u8,
            255,
        ])
    });
    DynamicImage::ImageRgba8(buf)
}

// --- Step helper functions
fn blur_step(sigma: f32) -> PipelineStep {
    PipelineStep {
        kind: PipelineStepKind::GaussianBlur,
        blur_sigma: sigma,
        ..Default::default()
    }
}

fn resize_step(w: i32, h: i32) -> PipelineStep {
    PipelineStep {
        kind: PipelineStepKind::Resize,
        resize_width: w,
        resize_height: h,
        ..Default::default()
    }
}

fn bench_blur(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);
    let sigma = 3.0;
    let step = blur_step(sigma);

    let cpu = Arc::new(StepFactory::new(false));
    let gpu = Arc::new(StepFactory::new(true));
    let has_gpu = pollster::block_on(GpuProcessor::new()).is_some();

    let mut group = c.benchmark_group(format!("Image Blur (sigma={sigma}) {w}x{h}"));
    group.measurement_time(Duration::from_secs(15));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| b.iter(|| cpu.apply(img.clone(), &step)));

    if has_gpu {
        group.bench_function("GPU", |b| b.iter(|| gpu.apply(img.clone(), &step)));
    }

    group.finish();
}

fn bench_resize(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);
    let (w_resize, h_resize) = (224, 224);
    let step = resize_step(w_resize, h_resize);

    let cpu = Arc::new(StepFactory::new(false));
    let gpu = Arc::new(StepFactory::new(true));
    let has_gpu = pollster::block_on(GpuProcessor::new()).is_some();

    let mut group = c.benchmark_group(format!("Image Resize {w}x{h} -> {w_resize}x{h_resize}"));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| b.iter(|| cpu.apply(img.clone(), &step)));

    if has_gpu {
        group.bench_function("GPU", |b| b.iter(|| gpu.apply(img.clone(), &step)));
    }

    group.finish();
}

// TODO: bench the
fn bench_resize_then_blur(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);

    let sigma = 3.0;
    let (w_resize, h_resize) = (224, 224);
    let steps = vec![resize_step(w_resize, h_resize), blur_step(sigma)];

    let cpu = Arc::new(StepFactory::new(false));
    let gpu = Arc::new(StepFactory::new(true));
    let has_gpu = pollster::block_on(GpuProcessor::new()).is_some();

    let mut group = c.benchmark_group(format!(
        "Pipeline: Resize {w}x{h} -> {w_resize}x{h_resize} then Blur sigma={sigma}"
    ));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| {
        b.iter(|| {
            steps
                .iter()
                .fold(img.clone(), |acc, step| cpu.apply(acc, step))
        })
    });

    if has_gpu {
        group.bench_function("GPU", |b| {
            b.iter(|| {
                steps
                    .iter()
                    .fold(img.clone(), |acc, step| gpu.apply(acc, step))
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_blur, bench_resize, bench_resize_then_blur);
criterion_main!(benches);
