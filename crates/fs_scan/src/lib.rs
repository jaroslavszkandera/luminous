use log::{debug, error, info};
use luminous_plugins::ImageFormat;
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

    let main_path = Path::new(&path_str);
    let metadata = match fs::metadata(main_path) {
        Ok(m) => m,
        Err(e) => {
            error!("Failed to get metadata for {}: {}", main_path.display(), e);
            return ScanResult {
                paths: vec![],
                start_index: 0,
                is_dir: false,
                image_formats,
            };
        }
    };

    let mut is_dir = false;

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut start_index: usize = 0;
    let mut start_img_path: Option<PathBuf> = None;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    fn make_files(dir: &TempDir, names: &[&str]) {
        for name in names {
            File::create(dir.path().join(name)).unwrap();
        }
    }

    fn no_extra() -> Vec<ImageFormat> {
        vec![]
    }

    #[test]
    fn default_decoding_exts_include_builtin_formats() {
        let fmts = ImageFormats::new();
        let exts = fmts.get_all_decoding_exts();
        for ext in &[
            "gif", "jpeg", "jpg", "png", "tiff", "tif", "webp", "avif", "pnm",
        ] {
            assert!(exts.contains(*ext), "missing decoding ext: {ext}");
        }
    }

    #[test]
    fn default_encoding_exts_include_builtin_formats() {
        let fmts = ImageFormats::new();
        let exts = fmts.get_all_encoding_exts();
        for ext in &["jpeg", "png", "tiff", "webp"] {
            assert!(exts.contains(*ext), "missing encoding ext: {ext}");
        }
    }

    #[test]
    fn encoding_exts_only_first_ext_of_each_format() {
        let fmts = ImageFormats::new();
        let exts = fmts.get_all_encoding_exts();
        assert!(exts.contains("tiff"));
        assert!(
            !exts.contains("tif"),
            "second ext 'tif' should not be in encoding exts"
        );
    }

    #[test]
    fn add_format_extends_decoding_and_encoding_exts() {
        let mut fmts = ImageFormats::new();
        fmts.add_format(ImageFormat {
            exts: vec!["xyz".to_string(), "xyzx".to_string()],
            decoding_support: true,
            encoding_support: true,
        });
        let dec = fmts.get_all_decoding_exts();
        let enc = fmts.get_all_encoding_exts();
        assert!(dec.contains("xyz"));
        assert!(dec.contains("xyzx"));
        assert!(enc.contains("xyz"));
        assert!(
            !enc.contains("xyzx"),
            "only first encoding ext should appear"
        );
    }

    #[test]
    fn add_format_decode_only_not_in_encoding_exts() {
        let mut fmts = ImageFormats::new();
        fmts.add_format(ImageFormat {
            exts: vec!["abc".to_string()],
            decoding_support: true,
            encoding_support: false,
        });
        assert!(fmts.get_all_decoding_exts().contains("abc"));
        assert!(!fmts.get_all_encoding_exts().contains("abc"));
    }

    #[test]
    fn scan_nonexistent_path_returns_empty() {
        let result = scan("/tmp/__luminous_nonexistent_path_xyz__", &no_extra());
        assert!(result.paths.is_empty());
        assert_eq!(result.start_index, 0);
        assert!(!result.is_dir);
    }

    #[test]
    fn scan_non_image_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["document.txt"]);
        let result = scan(
            dir.path().join("document.txt").to_str().unwrap(),
            &no_extra(),
        );
        assert!(result.paths.is_empty());
    }

    #[test]
    fn scan_directory_sets_is_dir_and_zero_start_index() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["a.jpg", "b.png"]);
        let result = scan(dir.path().to_str().unwrap(), &no_extra());
        assert!(result.is_dir);
        assert_eq!(result.start_index, 0);
        assert_eq!(result.paths.len(), 2);
    }

    #[test]
    fn scan_directory_ignores_non_images() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["a.jpg", "readme.txt", "b.png", "data.bin"]);
        let result = scan(dir.path().to_str().unwrap(), &no_extra());
        assert_eq!(result.paths.len(), 2);
    }

    #[test]
    fn scan_directory_returns_paths_in_sorted_order() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["c.jpg", "a.jpg", "b.png"]);
        let result = scan(dir.path().to_str().unwrap(), &no_extra());
        let names: Vec<_> = result
            .paths
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["a.jpg", "b.png", "c.jpg"]);
    }

    #[test]
    fn scan_file_sets_correct_start_index() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["a.jpg", "b.jpg", "c.jpg"]);
        let target = dir.path().join("b.jpg");
        let result = scan(target.to_str().unwrap(), &no_extra());
        assert!(!result.is_dir);
        assert_eq!(result.start_index, 1);
        assert_eq!(result.paths.len(), 3);
    }

    #[test]
    fn scan_file_start_index_first_file() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["a.jpg", "b.jpg"]);
        let target = dir.path().join("a.jpg");
        let result = scan(target.to_str().unwrap(), &no_extra());
        assert_eq!(result.start_index, 0);
    }

    #[test]
    fn scan_file_start_index_last_file() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["a.jpg", "b.jpg", "c.jpg"]);
        let target = dir.path().join("c.jpg");
        let result = scan(target.to_str().unwrap(), &no_extra());
        assert_eq!(result.start_index, 2);
    }

    #[test]
    fn scan_case_insensitive_extension() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["photo.JPG", "image.PNG"]);
        let result = scan(dir.path().to_str().unwrap(), &no_extra());
        assert_eq!(result.paths.len(), 2);
    }

    #[test]
    fn scan_with_extra_format_recognizes_custom_extension() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["data.myimg", "other.jpg"]);
        let extra = vec![ImageFormat {
            exts: vec!["myimg".to_string()],
            decoding_support: true,
            encoding_support: false,
        }];
        let result = scan(dir.path().to_str().unwrap(), &extra);
        assert_eq!(result.paths.len(), 2);
    }

    #[test]
    fn scan_does_not_recurse_into_subdirectories() {
        let dir = TempDir::new().unwrap();
        make_files(&dir, &["top.jpg"]);
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        File::create(sub.join("nested.jpg")).unwrap();
        let result = scan(dir.path().to_str().unwrap(), &no_extra());
        assert_eq!(result.paths.len(), 1);
    }
}
