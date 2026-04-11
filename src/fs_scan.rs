use log::{debug, error, info};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct ScanResult {
    pub paths: Vec<PathBuf>,
    pub start_index: usize,
    pub is_dir: bool,
    pub image_formats: ImageFormats,
}

pub struct ImageFormats {
    pub image_formats: HashSet<ImageFormat>,
}

#[derive(PartialEq, Eq, Hash, Debug, Clone)]
pub struct ImageFormat {
    pub exts: Vec<String>,
    pub decoding_support: bool,
    pub encoding_support: bool,
}

impl ImageFormats {
    pub fn new() -> Self {
        let mut formats = HashSet::new();

        macro_rules! add_fmt {
            ($feature:literal, $exts:expr, $dec:expr, $enc:expr) => {
                if cfg!(feature = $feature) {
                    formats.insert(ImageFormat {
                        exts: $exts.iter().map(|&s| s.to_string()).collect(),
                        decoding_support: $dec,
                        encoding_support: $enc,
                    });
                }
            };
        }

        // TODO: test
        // add_fmt!("avif", ["avif"], false, true);
        add_fmt!("avif-native", ["avif"], true, true);
        add_fmt!("bmp", ["bmp"], true, true);
        add_fmt!("dds", ["dds"], true, false);
        add_fmt!("ff", ["ff"], true, false);
        add_fmt!("gif", ["gif"], true, false);
        add_fmt!("hdr", ["hdr"], true, false);
        add_fmt!("ico", ["ico"], true, true);
        add_fmt!("jpeg", ["jpeg", "jpg"], true, true);
        add_fmt!("exr", ["exr"], true, false);
        add_fmt!("png", ["png"], true, true);
        add_fmt!("pnm", ["pnm", "pbm", "pgm", "ppm", "pam"], true, false);
        add_fmt!("qoi", ["qoi"], true, true);
        add_fmt!("tga", ["tga"], true, false);
        add_fmt!("tiff", ["tiff", "tif"], true, true);
        add_fmt!("webp", ["webp"], true, true);

        ImageFormats {
            image_formats: formats,
        }
    }

    pub fn add_format(&mut self, image_format: ImageFormat) {
        self.image_formats.insert(image_format);
    }

    pub fn get_all_decoding_exts(&self) -> HashSet<String> {
        self.image_formats
            .iter()
            .filter(|f| f.decoding_support)
            .flat_map(|f| f.exts.iter().cloned())
            .collect()
    }

    pub fn get_all_encoding_exts(&self) -> HashSet<String> {
        self.image_formats
            .iter()
            .filter(|f| f.encoding_support)
            .flat_map(|f| f.exts.first().cloned())
            .collect()
    }
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

pub fn scan(path_str: &str, extra_image_formats: &Vec<ImageFormat>) -> ScanResult {
    let main_path = Path::new(&path_str);
    let metadata = fs::metadata(main_path).unwrap();
    let mut is_dir = false;

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut start_index: usize = 0;
    let mut start_img_path: Option<PathBuf> = None;

    let mut image_formats = ImageFormats::new();
    debug!(
        "Active decoding extensions: {:?}",
        image_formats.get_all_decoding_exts()
    );
    debug!(
        "Active encoding extensions: {:?}",
        image_formats.get_all_encoding_exts()
    );

    for image_format in extra_image_formats {
        image_formats.add_format(image_format.clone());
    }

    let decode_extensions = image_formats.get_all_decoding_exts();
    debug!(
        "Active decoding extensions (with plugins): {:?}",
        image_formats.get_all_decoding_exts()
    );
    debug!(
        "Active encoding extensions (with plugins): {:?}",
        image_formats.get_all_encoding_exts()
    );

    let scan_dir = if metadata.is_file() {
        if !is_image(&main_path, &decode_extensions) {
            error!(
                "File is not a supported image type: {}",
                main_path.display()
            );
            return ScanResult {
                paths: vec![],
                start_index: 0,
                is_dir: false,
                image_formats,
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
            image_formats,
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
        if path.is_file() && is_image(&path, &decode_extensions) {
            if let Some(ref curr) = start_img_path {
                if path == *curr {
                    start_index = paths.len();
                    debug!("Starting image set to index: {}", start_index);
                }
            }
            paths.push(path);
        }
    }
    if metadata.is_dir() {
        debug!("Path was a directory, starting index is 0.");
        start_index = 0;
        is_dir = true;
    }

    info!(
        "Found {} images. Starting index: {}",
        paths.len(),
        start_index
    );
    ScanResult {
        paths,
        start_index,
        is_dir,
        image_formats,
    }
}
