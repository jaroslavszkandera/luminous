use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use log::debug;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::path::PathBuf;

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
        std::thread::spawn(move || {
            if let Some(parent) = path.parent() {
                dialog = dialog.set_directory(parent);
            }
            if let Some(dst_file) = dialog.save_file() {
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
            }
        });
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
