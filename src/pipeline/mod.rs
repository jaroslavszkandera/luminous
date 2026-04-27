pub mod gpu_proc;

use image::DynamicImage;
use log::{debug, error, trace};
use luminous_plugins::PluginManager;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::{Channel, FlipDirection, PipelineStep, PipelineStepKind, RotateAngle};
use gpu_proc::GpuProcessor;

pub trait ProcessingStep: Send + Sync {
    fn apply(&self, img: DynamicImage, params: &PipelineStep) -> DynamicImage;
    fn name(&self) -> &'static str;
}

pub struct StepFactory {
    steps: HashMap<u32, Box<dyn ProcessingStep>>,
    gpu: Option<Arc<GpuProcessor>>,
}

impl StepFactory {
    pub fn new(use_gpu: bool) -> Self {
        let mut f = Self {
            steps: HashMap::new(),
            gpu: if use_gpu {
                pollster::block_on(GpuProcessor::new()).map(Arc::new)
            } else {
                None
            },
        };
        f.register(PipelineStepKind::Rotate, RotateStep);
        f.register(PipelineStepKind::GaussianBlur, GaussianBlurStep);
        f.register(PipelineStepKind::Brighten, BrightenStep);
        f.register(PipelineStepKind::Resize, ResizeStep);
        f.register(PipelineStepKind::ExtractChannel, ExtractChannelStep);
        f.register(PipelineStepKind::Flip, FlipStep);
        f
    }

    fn register(&mut self, kind: PipelineStepKind, step: impl ProcessingStep + 'static) {
        self.steps.insert(kind as u32, Box::new(step));
    }

    pub fn apply(&self, img: DynamicImage, step: &PipelineStep) -> DynamicImage {
        match self.steps.get(&(step.kind as u32)) {
            Some(handler) => handler.apply(img, step),
            None => img,
        }
    }

    pub fn apply_pipeline(&self, img: DynamicImage, steps: &[PipelineStep]) -> DynamicImage {
        if steps.is_empty() {
            return img;
        }

        if let Some(gpu) = &self.gpu {
            debug!("Pipeline: Running on GPU");
            let mut gpu_tex = gpu.upload(&img);

            for step in steps {
                gpu_tex = match step.kind {
                    PipelineStepKind::GaussianBlur => {
                        gpu.blur_gpu(&gpu_tex, step.blur_sigma.max(0.1))
                    }
                    PipelineStepKind::Resize => gpu.resize_gpu(
                        &gpu_tex,
                        step.resize_width as u32,
                        step.resize_height as u32,
                    ),
                    PipelineStepKind::Rotate => {
                        gpu.rotate_gpu(&gpu_tex, resolve_random_angle(step.rotate_angle))
                    }
                    PipelineStepKind::Brighten => gpu.brighten_gpu(&gpu_tex, step.brighten_value),
                    PipelineStepKind::Flip => gpu.flip_gpu(&gpu_tex, step.flip_direction),
                    PipelineStepKind::ExtractChannel => {
                        gpu.extract_channel_gpu(&gpu_tex, step.extract_channel)
                    }
                };
            }

            return gpu.download(&gpu_tex);
        }

        steps
            .iter()
            .fold(img, |acc, step| match self.steps.get(&(step.kind as u32)) {
                Some(handler) => {
                    debug!("Pipeline applying step (CPU): {}", handler.name());
                    handler.apply(acc, step)
                }
                None => {
                    error!(
                        "Pipeline: no handler registered for step kind {:?}",
                        step.kind as u32
                    );
                    acc
                }
            })
    }
}

struct RotateStep;
impl ProcessingStep for RotateStep {
    fn name(&self) -> &'static str {
        "Rotate"
    }
    fn apply(&self, img: DynamicImage, params: &PipelineStep) -> DynamicImage {
        let angle = resolve_random_angle(params.rotate_angle);
        let rgba = img.to_rgba8();
        let rotated = match angle {
            RotateAngle::R90 => image::imageops::rotate90(&rgba),
            RotateAngle::R180 => image::imageops::rotate180(&rgba),
            RotateAngle::R270 => image::imageops::rotate270(&rgba),
            RotateAngle::Random => unreachable!(),
        };
        DynamicImage::ImageRgba8(rotated)
    }
}

fn resolve_random_angle(angle: RotateAngle) -> RotateAngle {
    if angle != RotateAngle::Random {
        return angle;
    }
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut h = DefaultHasher::new();
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        .hash(&mut h);
    std::thread::current().id().hash(&mut h);

    match h.finish() % 3 {
        0 => RotateAngle::R90,
        1 => RotateAngle::R180,
        _ => RotateAngle::R270,
    }
}

struct GaussianBlurStep;
impl ProcessingStep for GaussianBlurStep {
    fn name(&self) -> &'static str {
        "GaussianBlur"
    }
    fn apply(&self, img: DynamicImage, params: &PipelineStep) -> DynamicImage {
        img.blur(params.blur_sigma.max(0.1))
    }
}

struct BrightenStep;
impl ProcessingStep for BrightenStep {
    fn name(&self) -> &'static str {
        "Brighten"
    }
    fn apply(&self, img: DynamicImage, params: &PipelineStep) -> DynamicImage {
        img.brighten(params.brighten_value)
    }
}

struct ResizeStep;
impl ProcessingStep for ResizeStep {
    fn name(&self) -> &'static str {
        "Resize"
    }
    fn apply(&self, img: DynamicImage, params: &PipelineStep) -> DynamicImage {
        img.resize_exact(
            params.resize_width as u32,
            params.resize_height as u32,
            image::imageops::FilterType::Triangle,
        )
    }
}

struct FlipStep;
impl ProcessingStep for FlipStep {
    fn name(&self) -> &'static str {
        "Flip"
    }
    fn apply(&self, img: DynamicImage, params: &PipelineStep) -> DynamicImage {
        match params.flip_direction {
            FlipDirection::Horizontal => img.fliph(),
            FlipDirection::Vertical => img.flipv(),
        }
    }
}

struct ExtractChannelStep;
impl ProcessingStep for ExtractChannelStep {
    fn name(&self) -> &'static str {
        "Extract Channel"
    }
    fn apply(&self, img: DynamicImage, params: &PipelineStep) -> DynamicImage {
        let rgba = img.to_rgba8();
        match params.extract_channel {
            Channel::Gray => DynamicImage::ImageLumaA8(img.to_luma_alpha8()),
            Channel::Red | Channel::Green | Channel::Blue => {
                let idx = match params.extract_channel {
                    Channel::Red => 0,
                    Channel::Green => 1,
                    Channel::Blue => 2,
                    _ => 0,
                };
                let luma_a = image::ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
                    let p = rgba.get_pixel(x, y);
                    image::LumaA([p[idx], p[3]])
                });
                DynamicImage::ImageLumaA8(luma_a)
            }
            Channel::Hue | Channel::Saturation | Channel::Value => {
                let luma_a = image::ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
                    let p = rgba.get_pixel(x, y);
                    let srgb = palette::Srgb::new(
                        p[0] as f32 / 255.0,
                        p[1] as f32 / 255.0,
                        p[2] as f32 / 255.0,
                    );
                    let hsv: palette::Hsv = palette::IntoColor::into_color(srgb);
                    let val = match params.extract_channel {
                        Channel::Hue => {
                            let h = hsv.hue.into_positive_degrees();
                            if h.is_nan() {
                                0
                            } else {
                                (h / 360.0 * 255.0).round() as u8
                            }
                        }
                        Channel::Saturation => (hsv.saturation * 255.0).round() as u8,
                        Channel::Value => (hsv.value * 255.0).round() as u8,
                        _ => 0,
                    };
                    image::LumaA([val, p[3]])
                });
                DynamicImage::ImageLumaA8(luma_a)
            }
        }
    }
}

pub fn run_pipeline_on_selection(
    paths: Vec<PathBuf>,
    steps: Vec<PipelineStep>,
    factory: Arc<StepFactory>,
    encode_extension: String,
    plugin_manager: Arc<PluginManager>,
) {
    if paths.is_empty() {
        debug!("Pipeline: no images selected");
        return;
    }
    if steps.is_empty() {
        debug!("Pipeline: no steps defined (only conversion)");
        // return;
    }

    let mut dialog = rfd::FileDialog::new();
    if let Some(parent) = paths[0].parent() {
        dialog = dialog.set_directory(parent);
    }
    let Some(dst_dir) = dialog.pick_folder() else {
        debug!("Pipeline: user cancelled folder picker");
        return;
    };

    std::thread::spawn(move || {
        paths.par_iter().for_each(|path| {
            let start = Instant::now();

            let img = match image::open(path) {
                Ok(i) => i,
                Err(e) => {
                    error!("Pipeline: failed to open {:?}: {}", path, e);
                    return;
                }
            };

            let result = factory.apply_pipeline(img, &steps);

            let file_name = path.file_name().unwrap_or_default();
            let dst_file = dst_dir.join(file_name);

            if let Err(_) =
                save_result(result, &dst_file, &encode_extension, plugin_manager.clone())
            {
                // Should we continue or not on error?
                return;
            }

            debug!(
                "Pipeline: {:?} -> {:?} in {:.2}ms",
                file_name,
                dst_file,
                start.elapsed().as_secs_f64() * 1000.0
            );
        });
    });
}

fn save_result(
    img: DynamicImage,
    dst: &PathBuf,
    format: &str,
    plugin_manager: Arc<PluginManager>,
) -> Result<(), image::ImageError> {
    let mut dst = dst.with_extension(format);
    let res = if let Some(native_format) = image::ImageFormat::from_extension(format) {
        let fmt_lower = format.to_lowercase();
        if fmt_lower == "jpg" || fmt_lower == "jpeg" {
            let out = std::fs::File::create(&dst).map_err(image::ImageError::IoError)?;
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(out, 90);
            img.write_with_encoder(encoder)
        } else {
            img.save_with_format(&dst, native_format)
        }
    } else {
        // TODO: Save to collections like HDF5 and WebDataset with a flag collections
        // FIX: Incorrect WebDataset formatting for .json
        dst = dst
            .parent()
            .expect("The path should be valid")
            .join(PathBuf::from("dataset").with_extension(format));
        if plugin_manager.encode(&dst, &img) {
            Ok(())
        } else {
            Err(image::ImageError::Unsupported(
                image::error::UnsupportedError::from_format_and_kind(
                    image::error::ImageFormatHint::Name(format.to_string()),
                    image::error::UnsupportedErrorKind::Format(
                        image::error::ImageFormatHint::Name(format.to_string()),
                    ),
                ),
            ))
        }
    };

    match &res {
        Ok(_) => trace!("Successfully saved image to {:?}", &dst),
        Err(e) => error!("Failed to save image to {:?}: {}", dst, e),
    }

    res
}
