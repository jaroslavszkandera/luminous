use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use image::DynamicImage;
use std::sync::Arc;
use std::time::Duration;

use luminous::pipeline::StepFactory;
use luminous::pipeline::gpu_proc::GpuProcessor;
use luminous::{Channel, FlipDirection, PipelineStep, PipelineStepKind, RotateAngle};

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

fn extract_channel_step(channel: Channel) -> PipelineStep {
    PipelineStep {
        kind: PipelineStepKind::ExtractChannel,
        extract_channel: channel,
        ..Default::default()
    }
}

fn rotate_step(angle: RotateAngle) -> PipelineStep {
    PipelineStep {
        kind: PipelineStepKind::Rotate,
        rotate_angle: angle,
        ..Default::default()
    }
}

fn flip_step(dir: FlipDirection) -> PipelineStep {
    PipelineStep {
        kind: PipelineStepKind::Flip,
        flip_direction: dir,
        ..Default::default()
    }
}

fn brighten_step(value: i32) -> PipelineStep {
    PipelineStep {
        kind: PipelineStepKind::Brighten,
        brighten_value: value,
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

    let mut group = c.benchmark_group(format!("blur (sigma={sigma}) {w}x{h}"));
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
    let (w_resize, h_resize) = (384, 384);
    let step = resize_step(w_resize, h_resize);

    let cpu = Arc::new(StepFactory::new(false));
    let gpu = Arc::new(StepFactory::new(true));
    let has_gpu = pollster::block_on(GpuProcessor::new()).is_some();

    let mut group = c.benchmark_group(format!("resize {w}x{h} -> {w_resize}x{h_resize}"));
    group.throughput(Throughput::Elements(1));

    group.bench_function("CPU", |b| b.iter(|| cpu.apply(img.clone(), &step)));

    if has_gpu {
        group.bench_function("GPU", |b| b.iter(|| gpu.apply(img.clone(), &step)));
    }

    group.finish();
}

fn bench_full_pipeline(c: &mut Criterion) {
    let (w, h) = (1920, 1080);
    let img = test_image(w, h);

    let steps = vec![
        extract_channel_step(Channel::Gray),
        resize_step(384, 384),
        blur_step(3.0),
        rotate_step(RotateAngle::R90),
        flip_step(FlipDirection::Horizontal),
        brighten_step(10),
    ];

    let cpu = Arc::new(StepFactory::new(false));
    let gpu = Arc::new(StepFactory::new(true));
    let has_gpu = pollster::block_on(GpuProcessor::new()).is_some();

    let mut group = c.benchmark_group("full_pipeline");
    group.throughput(Throughput::Elements(1));
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("CPU", |b| {
        b.iter(|| cpu.apply_pipeline(img.clone(), &steps))
    });

    if has_gpu {
        group.bench_function("GPU", |b| {
            b.iter(|| gpu.apply_pipeline(img.clone(), &steps))
        });
    }

    group.finish();
}

// TODO: benchmark the read and write functions of the GPU
// NOTE: At what point is GPU faster than CPU? What operations in general?
criterion_group!(benches, bench_blur, bench_resize, bench_full_pipeline);
criterion_main!(benches);
