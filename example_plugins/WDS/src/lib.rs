use image::{DynamicImage, ImageFormat};
use std::ffi::CStr;
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::os::raw::c_char;
use tar::{Builder, Header};

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

fn prepare_file_for_append(file: &mut File) -> std::io::Result<()> {
    let metadata = file.metadata()?;
    let file_len = metadata.len();
    if file_len == 0 {
        return Ok(());
    }

    let scan_size = file_len.min(2048) as usize;
    let mut buffer = vec![0u8; scan_size];
    file.seek(SeekFrom::End(-(scan_size as i64)))?;
    file.read_exact(&mut buffer)?;

    let mut last_data_byte_index = None;
    for (i, &byte) in buffer.iter().enumerate().rev() {
        if byte != 0 {
            last_data_byte_index = Some(i);
            break;
        }
    }

    if let Some(idx) = last_data_byte_index {
        let block_end_relative_to_buffer_start = ((idx / 512) + 1) * 512;
        let new_len = (file_len - scan_size as u64) + block_end_relative_to_buffer_start as u64;
        file.set_len(new_len.min(file_len))?;
        file.seek(SeekFrom::Start(new_len.min(file_len)))?;
    } else {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn load_image(_path: *const c_char) -> ImageBuffer {
    eprintln!("WDS plugin does not have decoding functionality");
    ImageBuffer::null()
}

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
            "WDS Error: Data length {} does not match dimensions {}x{}x{}",
            data_slice.len(),
            h,
            w,
            c
        );
        return false;
    }

    let dynamic_img = match image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(
        img.width,
        img.height,
        data_slice.to_owned(),
    ) {
        Some(buf) => DynamicImage::ImageRgba8(buf),
        None => return false,
    };

    let mut png_bytes = Cursor::new(Vec::new());
    if dynamic_img
        .write_to(&mut png_bytes, ImageFormat::Png)
        .is_err()
    {
        eprintln!("WDS Error: Failed to convert to PNG");
        return false;
    }
    let png_data = png_bytes.into_inner();

    let basename = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros()
    );

    let json_data = format!(
        r#"{{"__key__": "{}", "width": {}, "height": {}, "cls": 0}}"#,
        basename, img.width, img.height
    );
    let json_bytes = json_data.as_bytes();

    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(file_path.as_ref())
    {
        Ok(f) => f,
        Err(_) => {
            eprintln!("WDS Error: Failed to open file");
            return false;
        }
    };

    if let Err(e) = prepare_file_for_append(&mut file) {
        eprintln!("WDS Error: Failed to prepare tar for append: {}", e);
        return false;
    }

    let mut builder = Builder::new(file);

    let mut img_header = Header::new_gnu();
    img_header.set_size(png_data.len() as u64);
    img_header.set_path(format!("{}.png", basename)).unwrap();
    img_header.set_cksum();
    if builder.append(&img_header, &png_data[..]).is_err() {
        eprintln!("WDS Error: Failed to append image");
        return false;
    }

    let mut json_header = Header::new_gnu();
    json_header.set_size(json_bytes.len() as u64);
    json_header.set_path(format!("{}.json", basename)).unwrap();
    json_header.set_cksum();
    if builder.append(&json_header, json_bytes).is_err() {
        eprintln!("WDS Error: Failed to append json");
        return false;
    }

    builder.finish().is_ok()
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
    let n = "WebDataset Plugin\0";
    let e = "tar,wds\0";
    unsafe {
        std::ptr::copy_nonoverlapping(n.as_ptr() as *const i8, name, n.len().min(n_max as usize));
        std::ptr::copy_nonoverlapping(e.as_ptr() as *const i8, exts, e.len().min(e_max as usize));
    }
}
