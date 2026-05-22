pub mod gpu_proc;
pub mod types;

use image::{DynamicImage, ImageBuffer, Rgba};
use log::debug;
use types::*;

pub struct StepFactory;

impl Default for StepFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl StepFactory {
    pub fn new() -> Self {
        Self
    }

    pub fn apply(&self, img: DynamicImage, filter: &FilterKind) -> DynamicImage {
        debug!("Applying filter: {:?}", filter);
        match filter {
            FilterKind::Rotate(angle) => apply_rotate(img, angle),
            FilterKind::GaussianBlur { sigma } => img.blur(*sigma),
            FilterKind::Brighten { value } => img.brighten(*value),
            FilterKind::Resize { w, h } => {
                img.resize_exact(*w, *h, image::imageops::FilterType::Triangle)
            }
            FilterKind::Flip(dir) => apply_flip(img, dir),
            FilterKind::ExtractChannel(channel) => apply_extract_channel(img, channel),
            FilterKind::Contrast { value } => img.adjust_contrast(*value),
            FilterKind::Saturation { value } => apply_saturation(img, *value),
            FilterKind::Crop {
                x,
                y,
                width,
                height,
            } => img.crop_imm(*x, *y, *width, *height),
            FilterKind::Grayscale => DynamicImage::ImageLuma8(img.to_luma8()),
            FilterKind::Noise { intensity } => apply_noise(img, *intensity),
            FilterKind::Sharpness { amount } => apply_sharpness(img, *amount),
        }
    }

    pub fn apply_pipeline(&self, img: DynamicImage, filters: &[FilterKind]) -> DynamicImage {
        filters
            .iter()
            .fold(img, |acc, filter| self.apply(acc, filter))
    }
}

fn resolve_random_angle() -> RotateAngle {
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

fn apply_rotate(img: DynamicImage, angle: &RotateAngle) -> DynamicImage {
    let concrete = if *angle == RotateAngle::Random {
        resolve_random_angle()
    } else {
        angle.clone()
    };
    let rgba = img.to_rgba8();
    DynamicImage::ImageRgba8(match concrete {
        RotateAngle::R90 => image::imageops::rotate90(&rgba),
        RotateAngle::R180 => image::imageops::rotate180(&rgba),
        RotateAngle::R270 => image::imageops::rotate270(&rgba),
        RotateAngle::Random => unreachable!(),
    })
}

fn apply_flip(img: DynamicImage, dir: &FlipDirection) -> DynamicImage {
    let rgba = img.to_rgba8();
    DynamicImage::ImageRgba8(match dir {
        FlipDirection::Horizontal => image::imageops::flip_horizontal(&rgba),
        FlipDirection::Vertical => image::imageops::flip_vertical(&rgba),
    })
}

fn apply_extract_channel(img: DynamicImage, channel: &Channel) -> DynamicImage {
    match channel {
        Channel::Gray => DynamicImage::ImageLumaA8(img.to_luma_alpha8()),
        Channel::Red | Channel::Green | Channel::Blue => {
            let rgba = img.to_rgba8();
            let idx = match channel {
                Channel::Red => 0,
                Channel::Green => 1,
                Channel::Blue => 2,
                _ => 0,
            };
            DynamicImage::ImageLumaA8(ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
                let p = rgba.get_pixel(x, y);
                image::LumaA([p[idx], p[3]])
            }))
        }
        Channel::Hue | Channel::Saturation | Channel::Value => {
            let rgba = img.to_rgba8();
            let mode = channel.clone();
            DynamicImage::ImageLumaA8(ImageBuffer::from_fn(
                rgba.width(),
                rgba.height(),
                move |x, y| {
                    let p = rgba.get_pixel(x, y);
                    let srgb = palette::Srgb::new(
                        p[0] as f32 / 255.0,
                        p[1] as f32 / 255.0,
                        p[2] as f32 / 255.0,
                    );
                    let hsv: palette::Hsv = palette::IntoColor::into_color(srgb);
                    let val = match mode {
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
                },
            ))
        }
    }
}

fn apply_saturation(img: DynamicImage, factor: f32) -> DynamicImage {
    let rgba = img.to_rgba8();
    DynamicImage::ImageRgba8(ImageBuffer::from_fn(
        rgba.width(),
        rgba.height(),
        move |x, y| {
            let p = rgba.get_pixel(x, y);
            let srgb = palette::Srgb::new(
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            );
            let mut hsv: palette::Hsv = palette::IntoColor::into_color(srgb);
            hsv.saturation = (hsv.saturation * factor).clamp(0.0, 1.0);
            let out: palette::Srgb = palette::IntoColor::into_color(hsv);
            Rgba([
                (out.red * 255.0).clamp(0.0, 255.0) as u8,
                (out.green * 255.0).clamp(0.0, 255.0) as u8,
                (out.blue * 255.0).clamp(0.0, 255.0) as u8,
                p[3],
            ])
        },
    ))
}

fn apply_noise(img: DynamicImage, intensity: f32) -> DynamicImage {
    use rand::Rng;
    let rgba = img.to_rgba8();
    if intensity <= 0.0 {
        return DynamicImage::ImageRgba8(rgba);
    }
    let range = (intensity * 255.0) as i32;
    let mut rng = rand::thread_rng();
    let count = (rgba.width() * rgba.height() * 3) as usize;
    let deltas: Vec<i32> = (0..count).map(|_| rng.gen_range(-range..=range)).collect();
    let (w, h) = rgba.dimensions();
    DynamicImage::ImageRgba8(ImageBuffer::from_fn(w, h, |x, y| {
        let p = rgba.get_pixel(x, y);
        let base = ((y * w + x) * 3) as usize;
        Rgba([
            (p[0] as i32 + deltas[base]).clamp(0, 255) as u8,
            (p[1] as i32 + deltas[base + 1]).clamp(0, 255) as u8,
            (p[2] as i32 + deltas[base + 2]).clamp(0, 255) as u8,
            p[3],
        ])
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_random_angle_is_one_of_three() {
        for _ in 0..32 {
            let a = resolve_random_angle();
            assert!(matches!(
                a,
                RotateAngle::R90 | RotateAngle::R180 | RotateAngle::R270
            ));
        }
    }

    #[test]
    fn step_factory_default_constructs() {
        let _ = StepFactory::default();
        let _ = StepFactory::new();
    }
}

fn apply_sharpness(img: DynamicImage, amount: f32) -> DynamicImage {
    if amount <= 0.0 {
        return img;
    }
    let sigma = (amount * 0.5).max(0.5);
    let blurred = img.blur(sigma).to_rgba8();
    let original = img.to_rgba8();
    let (w, h) = original.dimensions();
    DynamicImage::ImageRgba8(ImageBuffer::from_fn(w, h, |x, y| {
        let o = original.get_pixel(x, y);
        let b = blurred.get_pixel(x, y);
        let s = |ov: u8, bv: u8| -> u8 {
            (ov as f32 + amount * (ov as f32 - bv as f32)).clamp(0.0, 255.0) as u8
        };
        Rgba([s(o[0], b[0]), s(o[1], b[1]), s(o[2], b[2]), o[3]])
    }))
}
