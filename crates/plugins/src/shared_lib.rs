use crate::Backend;
use crate::manifest::{PluginCapability, PluginManifest};
use dlopen2::wrapper::{Container, WrapperApi};
use image::DynamicImage;
use log::{debug, error, info};
use std::ffi::CString;
use std::path::Path;

/// FFI image buffer shared with the plugin ABI.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ImageBuffer {
    pub data: *mut u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub channels: u32,
}

#[derive(WrapperApi)]
pub struct ImagePluginApi {
    load_image: unsafe extern "C" fn(path: *const i8) -> ImageBuffer,
    save_image: unsafe extern "C" fn(path: *const i8, img: ImageBuffer) -> bool,
    free_image: unsafe extern "C" fn(img: ImageBuffer),
    get_plugin_info: unsafe extern "C" fn(name: *mut i8, n_max: i32, exts: *mut i8, e_max: i32),
}

pub struct SharedLibBackend {
    container: Container<ImagePluginApi>,
    manifest: PluginManifest,
}

impl SharedLibBackend {
    pub fn new(manifest: &PluginManifest, dir: &Path) -> Option<Self> {
        let suffix = std::env::consts::DLL_SUFFIX;
        debug!(
            "Searching for library with suffix '{}' in {:?}",
            suffix, dir
        );

        let lib_path = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().ends_with(suffix))?;

        info!("Found library: {:?}", lib_path);
        let abs_path = std::fs::canonicalize(&lib_path).ok()?;

        let container = unsafe {
            match Container::load(&abs_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to load {:?}: {}", abs_path, e);
                    return None;
                }
            }
        };

        debug!("Plugin '{}' loaded from {:?}", manifest.name, abs_path);
        Some(Self {
            container,
            manifest: manifest.clone(),
        })
    }

    pub fn get_info(&self) -> (String, String) {
        const BUF: usize = 256;
        let mut name = vec![0u8; BUF];
        let mut exts = vec![0u8; BUF];
        unsafe {
            self.container.get_plugin_info(
                name.as_mut_ptr() as *mut i8,
                BUF as i32,
                exts.as_mut_ptr() as *mut i8,
                BUF as i32,
            );
        }
        let trim = |v: Vec<u8>| String::from_utf8_lossy(&v).trim_matches('\0').to_string();
        (trim(name), trim(exts))
    }
}

impl Backend for SharedLibBackend {
    fn decode(&self, path: &Path) -> Option<image::DynamicImage> {
        if !self.manifest.has_capability(&PluginCapability::Decoder) {
            error!("Plugin '{}' does not support decoding", self.manifest.name);
            return None;
        }

        let c_path = CString::new(path.to_str()?).ok()?;
        let ffi_buf = unsafe { self.container.load_image(c_path.as_ptr()) };

        if ffi_buf.data.is_null() {
            error!("Plugin '{}' returned null buffer", self.manifest.name);
            return None;
        }

        let result = decode_ffi_buffer(&ffi_buf);
        unsafe { self.container.free_image(ffi_buf) };
        result
    }

    fn encode(&self, path: &Path, buf: &DynamicImage) -> bool {
        if !self.manifest.has_capability(&PluginCapability::Encoder) {
            error!("Plugin '{}' does not support encoding", self.manifest.name);
            return false;
        }

        let c_path = match CString::new(path.to_str().unwrap_or_default()) {
            Ok(p) => p,
            Err(_) => return false,
        };

        let rgba_buf = buf.to_rgba8();
        let ffi_buf = ImageBuffer {
            data: rgba_buf.as_ptr() as *mut u8,
            len: rgba_buf.len(),
            width: rgba_buf.width(),
            height: rgba_buf.height(),
            channels: 4,
        };

        let res = unsafe { self.container.save_image(c_path.as_ptr(), ffi_buf) };
        debug!("Plugin FFI res={res}");
        res
    }
}

fn decode_ffi_buffer(ffi_buf: &ImageBuffer) -> Option<image::DynamicImage> {
    let (len, w, h, c) = (ffi_buf.len, ffi_buf.width, ffi_buf.height, ffi_buf.channels);

    let expected = (w * h * c) as usize;
    if len != expected {
        error!("Buffer size mismatch: got {len}, expected {expected}");
        return None;
    }

    let pixels = unsafe { std::slice::from_raw_parts(ffi_buf.data, len) }.to_vec();

    debug!("Decoded FFI buffer: {w}x{h}x{c}");

    match c {
        1 => image::ImageBuffer::<image::Luma<u8>, _>::from_raw(w, h, pixels)
            .map(image::DynamicImage::ImageLuma8),
        3 => image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(w, h, pixels)
            .map(image::DynamicImage::ImageRgb8),
        4 => image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(w, h, pixels)
            .map(image::DynamicImage::ImageRgba8),
        _ => {
            error!("Unsupported channel count: {c}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_with(data: &mut [u8], w: u32, h: u32, c: u32, len: usize) -> ImageBuffer {
        ImageBuffer {
            data: data.as_mut_ptr(),
            len,
            width: w,
            height: h,
            channels: c,
        }
    }

    #[test]
    fn decode_ffi_buffer_size_mismatch() {
        let mut data = vec![0u8; 10];
        let buf = buffer_with(&mut data, 4, 4, 4, 10);
        assert!(decode_ffi_buffer(&buf).is_none());
    }

    #[test]
    fn decode_ffi_buffer_rgba() {
        let mut data = vec![1u8; 4 * 4 * 4];
        let len = data.len();
        let buf = buffer_with(&mut data, 4, 4, 4, len);
        let img = decode_ffi_buffer(&buf).unwrap();
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 4);
        assert!(matches!(img, DynamicImage::ImageRgba8(_)));
    }

    #[test]
    fn decode_ffi_buffer_rgb() {
        let mut data = vec![2u8; 3 * 3 * 3];
        let len = data.len();
        let buf = buffer_with(&mut data, 3, 3, 3, len);
        let img = decode_ffi_buffer(&buf).unwrap();
        assert!(matches!(img, DynamicImage::ImageRgb8(_)));
    }

    #[test]
    fn decode_ffi_buffer_luma() {
        let mut data = vec![3u8; 5 * 5];
        let len = data.len();
        let buf = buffer_with(&mut data, 5, 5, 1, len);
        let img = decode_ffi_buffer(&buf).unwrap();
        assert!(matches!(img, DynamicImage::ImageLuma8(_)));
    }

    #[test]
    fn decode_ffi_buffer_unsupported_channels() {
        let mut data = vec![0u8; 2 * 2 * 2];
        let len = data.len();
        let buf = buffer_with(&mut data, 2, 2, 2, len);
        assert!(decode_ffi_buffer(&buf).is_none());
    }
}
