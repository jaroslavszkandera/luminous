use std::ffi::CStr;
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

    let data = match std::fs::read(path_str) {
        Ok(d) => d,
        Err(_) => return ImageBuffer::null(),
    };

    let output = match heic::DecoderConfig::new().decode(&data, heic::PixelLayout::Rgba8) {
        Ok(out) => out,
        Err(_) => return ImageBuffer::null(),
    };

    let mut rgba_data = output.data;
    rgba_data.shrink_to_fit();

    let len = rgba_data.len();
    let ptr = rgba_data.as_mut_ptr();
    std::mem::forget(rgba_data);

    ImageBuffer {
        data: ptr,
        len,
        width: output.width as u32,
        height: output.height as u32,
        channels: 4,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn save_image(_path: *const i8, _img: ImageBuffer) -> bool {
    eprintln!("Error: HEIC encoding is not supported by the heic decoder crate.");
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
    let n = "HEIC Image Plugin\0";
    let e = "heic,heif\0";
    unsafe {
        std::ptr::copy_nonoverlapping(n.as_ptr() as *const i8, name, n.len().min(n_max as usize));
        std::ptr::copy_nonoverlapping(e.as_ptr() as *const i8, exts, e.len().min(e_max as usize));
    }
}
