pub mod mapping;

pub use luminous_pipeline::StepFactory;
pub use luminous_pipeline::gpu_proc::GpuProcessor;

use image::DynamicImage;
use log::{debug, error, trace};
use luminous_pipeline::types::{FilterKind, RotateAngle};
use luminous_plugins::PluginManager;
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub struct GpuStepFactory {
    cpu: StepFactory,
    gpu: Option<Arc<GpuProcessor>>,
}

impl GpuStepFactory {
    pub fn new(use_gpu: bool) -> Self {
        Self {
            cpu: StepFactory::new(),
            gpu: if use_gpu {
                pollster::block_on(GpuProcessor::new()).map(Arc::new)
            } else {
                None
            },
        }
    }

    pub fn apply_pipeline(&self, img: DynamicImage, filters: &[FilterKind]) -> DynamicImage {
        if filters.is_empty() {
            return img;
        }

        if let Some(gpu) = &self.gpu {
            debug!("Pipeline: Running on GPU");
            let mut gpu_tex = gpu.upload(&img);

            for filter in filters {
                gpu_tex = match filter {
                    FilterKind::GaussianBlur { sigma } => gpu.blur_gpu(&gpu_tex, sigma.max(0.1)),
                    FilterKind::Resize { w, h } => gpu.resize_gpu(&gpu_tex, *w, *h),
                    FilterKind::Rotate(angle) => {
                        gpu.rotate_gpu(&gpu_tex, resolve_random_angle(angle))
                    }
                    FilterKind::Brighten { value } => gpu.brighten_gpu(&gpu_tex, *value),
                    FilterKind::Flip(dir) => gpu.flip_gpu(&gpu_tex, dir.clone()),
                    FilterKind::ExtractChannel(ch) => gpu.extract_channel_gpu(&gpu_tex, ch.clone()),
                    other => {
                        let img_cpu = gpu.download(&gpu_tex);
                        let result = self.cpu.apply(img_cpu, other);
                        gpu.upload(&result)
                    }
                };
            }

            return gpu.download(&gpu_tex);
        }

        debug!("Pipeline: Running on CPU");
        self.cpu.apply_pipeline(img, filters)
    }
}

fn resolve_random_angle(angle: &RotateAngle) -> RotateAngle {
    if *angle != RotateAngle::Random {
        return angle.clone();
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

pub fn run_pipeline_on_selection(
    paths: Vec<PathBuf>,
    filters: Vec<FilterKind>,
    factory: Arc<GpuStepFactory>,
    encode_extension: String,
    plugin_manager: Arc<PluginManager>,
    weak_ui: slint::Weak<crate::MainWindow>,
) {
    if paths.is_empty() {
        debug!("Pipeline: no images selected");
        return;
    }
    if filters.is_empty() {
        debug!("Pipeline: no steps defined (only conversion)");
    }

    let mut dialog = rfd::FileDialog::new();
    if let Some(parent) = paths[0].parent() {
        dialog = dialog.set_directory(parent);
    }
    let Some(dst_dir) = dialog.pick_folder() else {
        debug!("Pipeline: user cancelled folder picker");
        return;
    };

    let weak_ui_clone = weak_ui.clone();
    std::thread::spawn(move || {
        let total = paths.len();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        paths.par_iter().for_each(|path| {
            let start = Instant::now();

            let img = match image::open(path) {
                Ok(i) => i,
                Err(e) => {
                    error!("Pipeline: failed to open {:?}: {}", path, e);
                    return;
                }
            };

            let result = factory.apply_pipeline(img, &filters);

            let file_name = path.file_name().unwrap_or_default();
            let dst_file = dst_dir.join(file_name);

            if save_result(result, &dst_file, &encode_extension, plugin_manager.clone()).is_err() {
                return;
            }

            debug!(
                "Pipeline: {:?} -> {:?} in {:.2}ms",
                file_name,
                dst_file,
                start.elapsed().as_secs_f64() * 1000.0
            );
            let completed = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let progress = completed as f32 / total as f32;
            let ui = weak_ui_clone.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    debug!("pipeline progress: {progress:.1}%");
                    ui.set_pipeline_progress(progress);
                }
            })
            .ok();
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
