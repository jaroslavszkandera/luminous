use log::{debug, error, info};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "bmp", "gif", "webp"];

pub struct ScanResult {
    pub paths: Vec<PathBuf>,
    pub start_index: usize,
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext_str| SUPPORTED_EXTENSIONS.contains(&ext_str.to_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn scan(path_str: &str) -> ScanResult {
    let main_path = Path::new(&path_str);
    let metadata = fs::metadata(main_path).unwrap();

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut starting_index: usize = 0;
    let mut start_img_path: Option<PathBuf> = None;

    let scan_dir = if metadata.is_file() {
        if !is_image(main_path) {
            error!(
                "File is not a supported image type: {}",
                main_path.display()
            );
            return ScanResult {
                paths: vec![],
                start_index: 0,
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
        };
    };
    debug!("Scanning directory: {}", scan_dir.display());

    for entry in WalkDir::new(scan_dir)
        .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.into_path();
        if path.is_file() && is_image(&path) {
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
    }

    info!(
        "Found {} images. Starting index: {}",
        paths.len(),
        starting_index
    );
    ScanResult {
        paths: paths,
        start_index: starting_index,
    }
}
