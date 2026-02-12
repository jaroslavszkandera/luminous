use log::{debug, error, info};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct ScanResult {
    pub paths: Vec<PathBuf>,
    pub start_index: usize,
}

fn is_image(path: &Path, extensions: &HashMap<String, Option<String>>) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext_str| {
            let lower = ext_str.to_lowercase();
            extensions.contains_key(&lower)
        })
        .unwrap_or(false)
}

pub fn scan(path_str: &str, extra_exts: &[String]) -> ScanResult {
    let main_path = Path::new(&path_str);
    let metadata = fs::metadata(main_path).unwrap();

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut starting_index: usize = 0;
    let mut start_img_path: Option<PathBuf> = None;

    // All supported formats from the image crate
    let mut extensions = HashMap::from([
        ("avif".into(), None),
        ("bmp".into(), None),
        ("dds".into(), None),
        ("exr".into(), None),
        ("ff".into(), None),
        ("gif".into(), None),
        ("hdr".into(), None),
        ("ico".into(), None),
        ("jpeg".into(), None),
        ("jpg".into(), None),
        ("png".into(), None),
        ("pnm".into(), None),
        ("qoi".into(), None),
        ("tga".into(), None),
        ("tif".into(), None),
        ("tiff".into(), None),
        ("webp".into(), None),
    ]);
    for ext in extra_exts {
        extensions.insert(ext.to_lowercase(), None); // Option to plugin cmd
    }
    info!("Supported extensions: {:?}", extensions);

    let scan_dir = if metadata.is_file() {
        if !is_image(main_path, &extensions) {
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
