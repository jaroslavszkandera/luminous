use image::{DynamicImage, GenericImageView, RgbaImage};
use luminous_pipeline::StepFactory;
use luminous_pipeline::types::*;

fn solid(w: u32, h: u32, r: u8, g: u8, b: u8) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |_, _| image::Rgba([r, g, b, 255])))
}

fn gradient(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([
            ((x * 255) / w.max(1)) as u8,
            ((y * 255) / h.max(1)) as u8,
            128u8,
            255,
        ])
    }))
}

fn mse(a: &DynamicImage, b: &DynamicImage) -> f32 {
    let (w, h) = a.dimensions();
    assert_eq!((w, h), b.dimensions(), "mse: dimension mismatch");
    let ba = a.to_rgba8();
    let bb = b.to_rgba8();
    let sum: f32 = ba
        .pixels()
        .zip(bb.pixels())
        .flat_map(|(p, q)| {
            (0..3).map(move |c| {
                let d = p[c] as f32 - q[c] as f32;
                d * d
            })
        })
        .sum();
    sum / (w * h * 3) as f32
}

fn factory() -> StepFactory {
    StepFactory::new()
}

#[test]
fn rotate_90_swaps_dims() {
    let img = gradient(100, 60);
    let out = factory().apply(img, &FilterKind::Rotate(RotateAngle::R90));
    assert_eq!(out.dimensions(), (60, 100));
}

#[test]
fn rotate_270_swaps_dims() {
    let img = gradient(100, 60);
    let out = factory().apply(img, &FilterKind::Rotate(RotateAngle::R270));
    assert_eq!(out.dimensions(), (60, 100));
}

#[test]
fn rotate_180_keeps_dims() {
    let img = gradient(80, 80);
    let out = factory().apply(img, &FilterKind::Rotate(RotateAngle::R180));
    assert_eq!(out.dimensions(), (80, 80));
}

#[test]
fn rotate_180_is_not_identity() {
    let img = gradient(64, 64);
    let out = factory().apply(img.clone(), &FilterKind::Rotate(RotateAngle::R180));
    assert!(
        mse(&img, &out) > 1.0,
        "180
        ° rotation should cha
       nge the image"
    );
}

#[test]
fn rotate_random_produces_valid_output() {
    let img = gradient(64, 64);
    let out = factory().apply(img, &FilterKind::Rotate(RotateAngle::Random));
    let (w, h) = out.dimensions();
    assert!(
        (w == 64 && h == 64) || (w == 64 && h == 64),
        "random rotate output has unexpected dims {w}x{h}"
    );
    let img2 = gradient(80, 60);
    let out2 = factory().apply(img2, &FilterKind::Rotate(RotateAngle::Random));
    let (ow, oh) = out2.dimensions();
    assert!(
        (ow == 80 && oh == 60) || (ow == 60 && oh == 80),
        "unexpected random rotate dims {ow}x{oh}"
    );
}

#[test]
fn gaussian_blur_keeps_dims() {
    let img = gradient(64, 64);
    let out = factory().apply(img, &FilterKind::GaussianBlur { sigma: 2.0 });
    assert_eq!(out.dimensions(), (64, 64));
}

#[test]
fn gaussian_blur_changes_pixels() {
    let img = gradient(64, 64);
    let out = factory().apply(img.clone(), &FilterKind::GaussianBlur { sigma: 3.0 });
    assert!(mse(&img, &out) > 0.5, "blur should change pixels");
}

#[test]
fn brighten_positive_increases_values() {
    let img = solid(32, 32, 100, 100, 100);
    let out = factory()
        .apply(img, &FilterKind::Brighten { value: 50 })
        .to_rgba8();
    let p = out.get_pixel(0, 0);
    assert!(p[0] > 100, "expected brighter red channel");
}

#[test]
fn brighten_negative_decreases_values() {
    let img = solid(32, 32, 200, 200, 200);
    let out = factory()
        .apply(img, &FilterKind::Brighten { value: -50 })
        .to_rgba8();
    let p = out.get_pixel(0, 0);
    assert!(p[0] < 200, "expected darker red channel");
}

#[test]
fn resize_exact_dimensions() {
    let img = gradient(128, 128);
    let out = factory().apply(img, &FilterKind::Resize { w: 48, h: 32 });
    assert_eq!(out.dimensions(), (48, 32));
}

#[test]
fn resize_upscale() {
    let img = gradient(32, 32);
    let out = factory().apply(img, &FilterKind::Resize { w: 128, h: 128 });
    assert_eq!(out.dimensions(), (128, 128));
}

#[test]
fn flip_horizontal_keeps_dims() {
    let img = gradient(80, 60);
    let out = factory().apply(img, &FilterKind::Flip(FlipDirection::Horizontal));
    assert_eq!(out.dimensions(), (80, 60));
}

#[test]
fn flip_vertical_keeps_dims() {
    let img = gradient(80, 60);
    let out = factory().apply(img, &FilterKind::Flip(FlipDirection::Vertical));
    assert_eq!(out.dimensions(), (80, 60));
}

#[test]
fn flip_horizontal_mirrors_image() {
    let img = gradient(64, 64);
    let out = factory()
        .apply(img.clone(), &FilterKind::Flip(FlipDirection::Horizontal))
        .to_rgba8();
    let orig = img.to_rgba8();
    for y in 0..64 {
        assert_eq!(orig.get_pixel(0, y), out.get_pixel(63, y));
    }
}

#[test]
fn extract_channel_gray_produces_luma_alpha() {
    let img = gradient(32, 32);
    let out = factory().apply(img, &FilterKind::ExtractChannel(Channel::Gray));
    assert_eq!(out.dimensions(), (32, 32));
}

#[test]
fn extract_channel_red_keeps_dims() {
    let img = solid(32, 32, 200, 100, 50);
    let out = factory().apply(img, &FilterKind::ExtractChannel(Channel::Red));
    assert_eq!(out.dimensions(), (32, 32));
}

#[test]
fn extract_channel_hue_keeps_dims() {
    let img = gradient(32, 32);
    let out = factory().apply(img, &FilterKind::ExtractChannel(Channel::Hue));
    assert_eq!(out.dimensions(), (32, 32));
}

#[test]
fn contrast_keeps_dims() {
    let img = gradient(64, 64);
    let out = factory().apply(img, &FilterKind::Contrast { value: 1.5 });
    assert_eq!(out.dimensions(), (64, 64));
}

#[test]
fn contrast_changes_pixels() {
    let img = gradient(64, 64);
    let out = factory().apply(img.clone(), &FilterKind::Contrast { value: 2.0 });
    assert!(mse(&img, &out) > 1.0, "contrast should change pixels");
}

#[test]
fn saturation_keeps_dims() {
    let img = gradient(64, 64);
    let out = factory().apply(img, &FilterKind::Saturation { value: 2.0 });
    assert_eq!(out.dimensions(), (64, 64));
}

#[test]
fn saturation_zero_produces_grayscale_like_result() {
    let img = solid(32, 32, 200, 100, 50);
    let out = factory()
        .apply(img, &FilterKind::Saturation { value: 0.0 })
        .to_rgba8();
    let p = out.get_pixel(0, 0);
    let r = p[0] as i32;
    let g = p[1] as i32;
    let b = p[2] as i32;
    assert!(
        (r - g).abs() <= 2 && (r - b).abs() <= 2,
        "saturation=0 should produce near-gray: ({r},{g},{b})"
    );
}

#[test]
fn crop_produces_correct_dimensions() {
    let img = gradient(128, 128);
    let out = factory().apply(
        img,
        &FilterKind::Crop {
            x: 10,
            y: 10,
            width: 50,
            height: 40,
        },
    );
    assert_eq!(out.dimensions(), (50, 40));
}

#[test]
fn crop_top_left_matches_original() {
    let img = gradient(64, 64);
    let orig = img.to_rgba8();
    let out = factory()
        .apply(
            img,
            &FilterKind::Crop {
                x: 5,
                y: 7,
                width: 20,
                height: 15,
            },
        )
        .to_rgba8();
    assert_eq!(out.get_pixel(0, 0), orig.get_pixel(5, 7));
    assert_eq!(out.get_pixel(1, 0), orig.get_pixel(6, 7));
}

#[test]
fn grayscale_keeps_dims() {
    let img = gradient(64, 64);
    let out = factory().apply(img, &FilterKind::Grayscale);
    assert_eq!(out.dimensions(), (64, 64));
}

#[test]
fn noise_keeps_dims() {
    let img = solid(64, 64, 128, 128, 128);
    let out = factory().apply(img, &FilterKind::Noise { intensity: 0.1 });
    assert_eq!(out.dimensions(), (64, 64));
}

#[test]
fn noise_zero_intensity_is_identity() {
    let img = solid(32, 32, 100, 150, 200);
    let out = factory().apply(img.clone(), &FilterKind::Noise { intensity: 0.0 });
    assert_eq!(
        mse(&img, &out),
        0.0,
        "zero noise should leave image unchanged"
    );
}

#[test]
fn noise_positive_changes_pixels() {
    let img = solid(64, 64, 128, 128, 128);
    let out = factory().apply(img.clone(), &FilterKind::Noise { intensity: 0.2 });
    assert!(mse(&img, &out) > 0.5, "noise should change pixels");
}

#[test]
fn sharpness_keeps_dims() {
    let img = gradient(64, 64);
    let out = factory().apply(img, &FilterKind::Sharpness { amount: 1.0 });
    assert_eq!(out.dimensions(), (64, 64));
}

#[test]
fn sharpness_zero_is_identity() {
    let img = gradient(32, 32);
    let out = factory().apply(img.clone(), &FilterKind::Sharpness { amount: 0.0 });
    assert_eq!(
        mse(&img, &out),
        0.0,
        "zero sharpness should leave image unchanged"
    );
}

#[test]
fn sharpness_positive_changes_pixels() {
    let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(64, 64, |x, y| {
        let v = if (x + y) % 2 == 0 { 50u8 } else { 200u8 };
        image::Rgba([v, v, v, 255])
    }));
    let out = factory().apply(img.clone(), &FilterKind::Sharpness { amount: 2.0 });
    assert!(mse(&img, &out) > 0.5, "sharpness should change pixels");
}

#[test]
fn apply_pipeline_empty_is_identity() {
    let img = gradient(32, 32);
    let out = factory().apply_pipeline(img.clone(), &[]);
    assert_eq!(mse(&img, &out), 0.0);
}

#[test]
fn apply_pipeline_multiple_steps() {
    let img = gradient(64, 64);
    let filters = vec![
        FilterKind::GaussianBlur { sigma: 1.0 },
        FilterKind::Brighten { value: 20 },
        FilterKind::Resize { w: 32, h: 32 },
    ];
    let out = factory().apply_pipeline(img, &filters);
    assert_eq!(out.dimensions(), (32, 32));
}

#[test]
fn apply_pipeline_all_new_filters() {
    let img = gradient(64, 64);
    let filters = vec![
        FilterKind::Contrast { value: 1.2 },
        FilterKind::Saturation { value: 1.5 },
        FilterKind::Grayscale,
        FilterKind::Noise { intensity: 0.05 },
        FilterKind::Sharpness { amount: 0.5 },
    ];
    let out = factory().apply_pipeline(img, &filters);
    assert_eq!(out.dimensions(), (64, 64));
}

#[test]
fn apply_pipeline_crop_then_resize() {
    let img = gradient(128, 128);
    let filters = vec![
        FilterKind::Crop {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        },
        FilterKind::Resize { w: 224, h: 224 },
    ];
    let out = factory().apply_pipeline(img, &filters);
    assert_eq!(out.dimensions(), (224, 224));
}
