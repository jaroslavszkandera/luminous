use image::DynamicImage;
use log::{debug, error};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use crate::{PipelineStep, PipelineStepKind, RotateAngle};

pub trait ProcessingStep: Send + Sync {
    fn apply(&self, img: DynamicImage, params: &PipelineStep) -> DynamicImage;
    fn name(&self) -> &'static str;
}

pub struct StepFactory {
    steps: HashMap<u32, Box<dyn ProcessingStep>>,
}

impl StepFactory {
    pub fn new() -> Self {
        let mut f = Self {
            steps: HashMap::new(),
        };
        f.register(PipelineStepKind::Rotate, RotateStep);
        f.register(PipelineStepKind::GaussianBlur, GaussianBlurStep);
        f.register(PipelineStepKind::Brighten, BrightenStep);
        f
    }

    fn register(&mut self, kind: PipelineStepKind, step: impl ProcessingStep + 'static) {
        self.steps.insert(kind as u32, Box::new(step));
    }

    pub fn apply(&self, img: DynamicImage, step: &PipelineStep) -> DynamicImage {
        match self.steps.get(&(step.kind as u32)) {
            Some(handler) => {
                debug!("Pipeline applying step: {}", handler.name());
                handler.apply(img, step)
            }
            None => {
                error!(
                    "Pipeline: no handler registered for step kind {:?}",
                    step.kind as u32
                );
                img
            }
        }
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

pub fn run_pipeline_on_selection(
    paths: Vec<PathBuf>,
    steps: Vec<PipelineStep>,
    factory: std::sync::Arc<StepFactory>,
) {
    if paths.is_empty() {
        debug!("Pipeline: no images selected");
        return;
    }
    if steps.is_empty() {
        debug!("Pipeline: no steps defined");
        return;
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

            let result = steps.iter().fold(img, |acc, step| factory.apply(acc, step));

            let file_name = path.file_name().unwrap_or_default();
            let dst_file = dst_dir.join(file_name);

            if let Err(e) = save_result(result, &dst_file) {
                error!("Pipeline: failed to save {:?}: {}", dst_file, e);
                return;
            }

            debug!(
                "Pipeline: {:?} → {:?} in {:.2}ms",
                file_name,
                dst_file,
                start.elapsed().as_secs_f64() * 1000.0
            );
        });
    });
}

// TODO: Format conversion
fn save_result(img: DynamicImage, dst: &PathBuf) -> Result<(), image::ImageError> {
    img.save(dst)
}
