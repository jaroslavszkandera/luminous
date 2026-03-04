use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use log::debug;
use rayon::prelude::*;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::{path::PathBuf, time::Instant};

use crate::ImgFmt; // TODO: Consider rename

pub fn save_image(
    image_buffer: Option<SharedPixelBuffer<Rgba8Pixel>>,
    image_path: Option<PathBuf>,
    format: ImgFmt,
) {
    if let Some(path) = image_path {
        let new_name = path
            .with_extension(format_to_str(format))
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "image.png".into());

        let mut dialog = rfd::FileDialog::new().set_file_name(&new_name);
        if let Some(parent) = path.parent() {
            dialog = dialog.set_directory(parent);
        }
        if let Some(dst_file) = dialog.save_file() {
            std::thread::spawn(move || {
                debug!(
                    "Saving {:?} -> {:?} (picked format {:#?})",
                    new_name, dst_file, format
                );

                let img: DynamicImage = if let Some(buffer) = image_buffer {
                    let width = buffer.width();
                    let height = buffer.height();
                    let pixels = buffer.as_bytes().to_vec();

                    let img_buf = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, pixels)
                        .expect("Failed to create image buffer from Slint pixels");

                    DynamicImage::ImageRgba8(img_buf)
                } else {
                    image::open(path).map_err(|e| e.to_string()).unwrap()
                };

                let mut out = std::fs::File::create(&dst_file)
                    .map_err(|e| e.to_string())
                    .unwrap();

                match format {
                    ImgFmt::Jpeg => {
                        let quality = 90;
                        let encoder =
                            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
                        img.write_with_encoder(encoder)
                            .map_err(|e| e.to_string())
                            .unwrap();
                    }
                    _ => {
                        img.save_with_format(&dst_file, format_to_image_format(format))
                            .map_err(|e| e.to_string())
                            .unwrap();
                    }
                }
                debug!("Saved to: {:?}", dst_file);
            });
        }
    }
}

pub fn batch_save_images(paths: Vec<PathBuf>, format: ImgFmt) {
    if paths.is_empty() {
        debug!("Batch save received no image");
        return;
    }

    let mut dialog = rfd::FileDialog::new();
    if let Some(parent) = paths
        .get(0)
        .expect("At least one path should be present")
        .parent()
    {
        dialog = dialog.set_directory(parent);
        if let Some(dst_path) = dialog.pick_folder() {
            std::thread::spawn(move || {
                paths.par_iter().for_each(|path| {
                    let start = Instant::now();
                    let replace_ext = path.with_extension(format_to_str(format));
                    let new_image_name = replace_ext.file_name().unwrap();
                    let dst_file = dst_path.join(new_image_name);
                    debug!("Saving {:?} -> {:?}", path, dst_file);

                    let mut out = std::fs::File::create(&dst_file)
                        .map_err(|e| e.to_string())
                        .unwrap();
                    let img = image::open(path).map_err(|e| e.to_string()).unwrap();

                    match format {
                        ImgFmt::Jpeg => {
                            let quality = 90;
                            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                                &mut out, quality,
                            );
                            img.write_with_encoder(encoder)
                                .map_err(|e| e.to_string())
                                .unwrap();
                        }
                        _ => {
                            img.save_with_format(&dst_file, format_to_image_format(format))
                                .map_err(|e| e.to_string())
                                .unwrap();
                        }
                    }
                    debug!(
                        "Saved to: {:?} in {:.2}ms",
                        dst_file,
                        start.elapsed().as_secs_f64() * 1000.0
                    );
                });
            });
        }
    }
}

fn format_to_str(image_format: ImgFmt) -> String {
    let fmt_str = match image_format {
        ImgFmt::Png => "png",
        ImgFmt::Jpeg => "jpeg",
        ImgFmt::Webp => "webp",
    };
    fmt_str.into()
}

fn format_to_image_format(image_format: ImgFmt) -> ImageFormat {
    match image_format {
        ImgFmt::Png => ImageFormat::Png,
        ImgFmt::Jpeg => ImageFormat::Jpeg,
        ImgFmt::Webp => ImageFormat::WebP,
    }
}
