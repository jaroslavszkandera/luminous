use hdf5_metno::File;
use ndarray::ArrayView4;
use ndarray::s;
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
pub extern "C" fn load_image(_path: *const c_char) -> ImageBuffer {
    eprintln!("HDF5 plugin does not have decoding functionality");
    return ImageBuffer::null();
}

/*
/// One image per dataset approach
#[unsafe(no_mangle)]
pub extern "C" fn save_image(path: *const i8, img: ImageBuffer) -> bool {
    if path.is_null() || img.data.is_null() {
        return false;
    }

    let c_str = unsafe { CStr::from_ptr(path) };
    let file_path = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };

    let data_slice = unsafe { std::slice::from_raw_parts(img.data, img.len) };

    match File::create(file_path) {
        Ok(file) => {
            let shape = [
                img.height as usize,
                img.width as usize,
                img.channels as usize,
            ];
            let res = file.new_dataset::<u8>().shape(&shape).create("image");

            match res {
                Ok(ds) => ds.write(data_slice).is_ok(),
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}
*/

/// Multiple images per dataset approach
#[unsafe(no_mangle)]
pub extern "C" fn save_image(path: *const i8, img: ImageBuffer) -> bool {
    if path.is_null() || img.data.is_null() {
        return false;
    }

    let file_path = unsafe { CStr::from_ptr(path).to_string_lossy() };
    let data_slice = unsafe { std::slice::from_raw_parts(img.data, img.len) };

    let h = img.height as usize;
    let w = img.width as usize;
    let c = img.channels as usize;

    if data_slice.len() != h * w * c {
        eprintln!(
            "HDF5 Error: Data length {} does not match dimensions {}x{}x{}",
            data_slice.len(),
            h,
            w,
            c
        );
        return false;
    }

    let file = match File::append(file_path.as_ref()) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("HDF5 Error: Failed to open file: {}", e);
            return false;
        }
    };

    let dataset = match file.dataset("images") {
        Ok(ds) => ds,
        Err(_) => {
            let res = file
                .new_dataset::<u8>()
                .chunk((1, h, w, c))
                .shape((0.., h, w, c))
                .create("images");

            match res {
                Ok(ds) => ds,
                Err(e) => {
                    eprintln!("HDF5 Error: Failed to create dataset: {}", e);
                    return false;
                }
            }
        }
    };

    let curr_shape = dataset.shape();
    if curr_shape.len() != 4 {
        eprintln!("HDF5 Error: Dataset exists but is not 4D: {:?}", curr_shape);
        return false;
    }

    let new_count = curr_shape[0] + 1;

    if let Err(e) = dataset.resize((new_count, h, w, c)) {
        eprintln!("HDF5 Error: Resize failed: {}.", e);
        return false;
    }

    let view = match ArrayView4::from_shape((1, h, w, c), data_slice) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("HDF5 Error: Reshape failed: {}", e);
            return false;
        }
    };

    if let Err(e) = dataset.write_slice(view, s![new_count - 1..new_count, .., .., ..]) {
        eprintln!("HDF5 Error: Write failed: {}", e);
        return false;
    }

    true
}

#[unsafe(no_mangle)]
pub extern "C" fn free_image(_img: ImageBuffer) {
    eprint!("HDF5 free image does nothing");
}

#[unsafe(no_mangle)]
pub extern "C" fn get_plugin_info(name: *mut i8, n_max: i32, exts: *mut i8, e_max: i32) {
    let n = "HDF5 Encoding Plugin\0";
    let e = "hdf5\0";
    unsafe {
        std::ptr::copy_nonoverlapping(n.as_ptr() as *const i8, name, n.len().min(n_max as usize));
        std::ptr::copy_nonoverlapping(e.as_ptr() as *const i8, exts, e.len().min(e_max as usize));
    }
}
