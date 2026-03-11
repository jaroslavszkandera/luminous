use bayer::{Demosaic, RasterDepth, RasterMut};
use std::ffi::CStr;
use std::io::Cursor;
use std::os::raw::c_char;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ImageBuffer {
    pub data: *mut u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub channels: u32,
}

impl ImageBuffer {
    fn null() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            width: 0,
            height: 0,
            channels: 0,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn load_image(path: *const c_char) -> ImageBuffer {
    let c_str = unsafe {
        if path.is_null() {
            return ImageBuffer::null();
        }
        CStr::from_ptr(path)
    };

    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return ImageBuffer::null(),
    };

    let raw_image = match rawloader::decode_file(path_str) {
        Ok(img) => img,
        Err(_) => return ImageBuffer::null(),
    };

    let (w, h) = (raw_image.width, raw_image.height);
    let max_wb = raw_image.wb_coeffs.iter().fold(0.0f32, |a, &b| a.max(b));
    let wb: Vec<f32> = raw_image.wb_coeffs.iter().map(|&c| c / max_wb).collect();

    let raw_bytes: Vec<u8> = match raw_image.data {
        rawloader::RawImageData::Integer(v) => {
            let black = raw_image.blacklevels[0] as f32;
            let white = raw_image.whitelevels[0] as f32;

            v.iter()
                .enumerate()
                .map(|(i, &x)| {
                    let color_idx = raw_image.cfa.color_at(i / w, i % w);
                    let wb_mult = wb[color_idx];

                    let linear = ((x as f32 - black) / (white - black)).clamp(0.0, 1.0);
                    let balanced = (linear * wb_mult).clamp(0.0, 1.0);
                    let gamma = balanced.powf(1.0 / 2.2);
                    (gamma * 255.0) as u8
                })
                .collect()
        }
        rawloader::RawImageData::Float(v) => v
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let color_idx = raw_image.cfa.color_at(i / w, i % w);
                let wb_mult = wb[color_idx];

                let balanced = (x * wb_mult).clamp(0.0, 1.0);
                let gamma = balanced.powf(1.0 / 2.2);
                (gamma * 255.0) as u8
            })
            .collect(),
    };

    let mut rgb_data = vec![0u8; w * h * 3];
    let mut dst = RasterMut::new(w, h, RasterDepth::Depth8, &mut rgb_data);

    let cfa = match raw_image.cfa.to_string().as_str() {
        "RGGB" => bayer::CFA::RGGB,
        "BGGR" => bayer::CFA::BGGR,
        "GRBG" => bayer::CFA::GRBG,
        "GBRG" => bayer::CFA::GBRG,
        _ => {
            eprintln!("Unsupported CFA pattern: {}", raw_image.cfa.to_string());
            return ImageBuffer::null();
        }
    };

    let mut reader = Cursor::new(raw_bytes);
    let result = bayer::run_demosaic(
        &mut reader,
        bayer::BayerDepth::Depth8,
        cfa,
        Demosaic::Linear,
        &mut dst,
    );

    if result.is_err() {
        eprintln!("Error demosaic the image");
        return ImageBuffer::null();
    }

    let mut rgba_data = Vec::with_capacity(w * h * 4);
    for chunk in rgb_data.chunks_exact(3) {
        rgba_data.extend_from_slice(chunk);
        rgba_data.push(255);
    }

    rgba_data.shrink_to_fit();
    let len = rgba_data.len();
    let ptr = rgba_data.as_mut_ptr();
    std::mem::forget(rgba_data);

    ImageBuffer {
        data: ptr,
        len,
        width: w as u32,
        height: h as u32,
        channels: 4,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn save_image(_path: *const i8, _img: ImageBuffer) -> bool {
    eprintln!("Error: image encoding is not supported");
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn free_image(img: ImageBuffer) {
    if !img.data.is_null() {
        unsafe {
            let _ = Vec::from_raw_parts(img.data, img.len, img.len);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn get_plugin_info(name: *mut i8, n_max: i32, exts: *mut i8, e_max: i32) {
    let n = "Raw Image Plugin\0";
    let e = "arw\0";
    unsafe {
        std::ptr::copy_nonoverlapping(n.as_ptr() as *const i8, name, n.len().min(n_max as usize));
        std::ptr::copy_nonoverlapping(e.as_ptr() as *const i8, exts, e.len().min(e_max as usize));
    }
}
