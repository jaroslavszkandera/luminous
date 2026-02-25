use log::{debug, error, info};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// TMP: red should extension is for testing
// const SUPPORTED_EXTENSIONS: &[&str; 7] = &["jpg", "jpeg", "png", "bmp", "gif", "webp", "red"];

pub struct ScanResult {
    pub paths: Vec<PathBuf>,
    pub start_index: usize,
    pub is_dir: bool,
}

fn is_image(path: &Path, extensions: &HashSet<String>) -> bool {
    let ext_os = match path.extension() {
        Some(e) => e,
        None => return false,
    };
    let ext_str = match ext_os.to_str() {
        Some(s) => s,
        None => return false,
    };

    if extensions.contains(ext_str) {
        return true;
    }

    let lower = ext_str.to_lowercase();
    extensions.contains(&lower)
}

pub fn scan(path_str: &str, extra_exts: &[&str]) -> ScanResult {
    let main_path = Path::new(&path_str);
    let metadata = fs::metadata(main_path).unwrap();
    let mut is_dir = false;

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut starting_index: usize = 0;
    let mut start_img_path: Option<PathBuf> = None;

    let mut extensions: HashSet<String> = [
        "avif", "bmp", "dds", "exr", "ff", "gif", "hdr", "ico", "jpeg", "jpg", "png", "pnm", "qoi",
        "tga", "tif", "tiff", "webp",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    for ext in extra_exts {
        extensions.insert(ext.to_lowercase());
    }
    info!("Supported extensions: {:?}", extensions);

    let scan_dir = if metadata.is_file() {
        if !is_image(&main_path, &extensions) {
            error!(
                "File is not a supported image type: {}",
                main_path.display()
            );
            return ScanResult {
                paths: vec![],
                start_index: 0,
                is_dir: false,
            };
        }
        start_img_path = Some(main_path.to_path_buf());
        main_path.parent().unwrap_or(main_path)
    } else if metadata.is_dir() {
        main_path
    } else {
        error!(
            "Path is neither a file nor a directory: {}",
            main_path.display()
        );
        return ScanResult {
            paths: vec![],
            start_index: 0,
            is_dir: false,
        };
    };
    debug!("Scanning directory: {}", scan_dir.display());

    for entry in WalkDir::new(scan_dir)
        .max_depth(1)
        .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.into_path();
        if path.is_file() && is_image(&path, &extensions) {
            if let Some(ref curr) = start_img_path {
                if path == *curr {
                    starting_index = paths.len();
                    debug!("Starting image set to index: {}", starting_index);
                }
            }
            paths.push(path);
        }
    }
    if metadata.is_dir() {
        debug!("Path was a directory, starting index is 0.");
        starting_index = 0;
        is_dir = true;
    }

    info!(
        "Found {} images. Starting index: {}",
        paths.len(),
        starting_index
    );
    ScanResult {
        paths: paths,
        start_index: starting_index,
        is_dir: is_dir,
    }
}
