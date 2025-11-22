slint::include_modules!();

mod image_ring_cache;
use image_ring_cache::ImageRingCache;

use image;
use log::{debug, error, info};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;
use walkdir::WalkDir;

pub struct Config {
    pub path: String,
    pub log_level: String,
}

impl Config {
    pub fn build(mut args: impl Iterator<Item = String>) -> Result<Config, &'static str> {
        let app_name = args.next().unwrap();
        let mut path: Option<String> = None;
        let mut log_level: Option<String> = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-l" | "--log" => log_level = args.next(),
                _ if path.is_none() => path = Some(arg),
                _ => return Err("Invalid option or too many arguments"),
            }
        }

        let path = path.ok_or("Didn't get a path")?;
        let log_level = log_level.unwrap_or_else(|| "debug".to_string());
        info!("Starting {}", app_name);
        Ok(Config { path, log_level })
    }
}

fn is_img_path(path: &Path) -> bool {
    let supported_extensions = &["jpg", "jpeg", "png"];
    path.extension()
        .and_then(|ext| ext.to_str())
        .map_or(false, |ext_str| {
            supported_extensions.contains(&ext_str.to_lowercase().as_str())
        })
}

fn load_img_paths(path_str: &str) -> (Vec<PathBuf>, usize) {
    let main_path = Path::new(&path_str);
    let metadata = fs::metadata(main_path).unwrap();

    let mut img_paths: Vec<PathBuf> = Vec::new();
    let mut starting_index: usize = 0;
    let mut start_img_path: Option<PathBuf> = None;

    let scan_dir = if metadata.is_file() {
        if !is_img_path(main_path) {
            error!(
                "File is not a supported image type: {}",
                main_path.display()
            );
            return (Vec::new(), 0);
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
        return (Vec::new(), 0);
    };
    debug!("Scanning directory: {}", scan_dir.display());

    for entry in WalkDir::new(scan_dir)
        .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.into_path();
        if path.is_file() && is_img_path(&path) {
            if let Some(ref curr) = start_img_path {
                if path == *curr {
                    starting_index = img_paths.len();
                    info!("Starting image set to index: {}", starting_index);
                }
            }
            img_paths.push(path);
        }
    }
    if metadata.is_dir() {
        info!("Path was a directory, starting index is 0.");
        starting_index = 0;
    }

    info!(
        "Found {} images. Starting index: {}",
        img_paths.len(),
        starting_index
    );
    (img_paths, starting_index)
}

fn load_img(path: &Path) -> Result<Image, Box<dyn Error>> {
    let img_name = path.file_name().unwrap();
    debug!("Loading full image: {}", img_name.display());
    let load_start = Instant::now();
    let dyn_img = image::open(path)?;
    let rgba_img = dyn_img.to_rgba8();
    let (width, height) = rgba_img.dimensions();
    let buffer =
        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba_img.as_raw(), width, height);
    debug!(
        "Image loaded in {:.2} ms ({})",
        load_start.elapsed().as_millis(),
        img_name.display()
    );
    Ok(Image::from_rgba8(buffer))
}

fn create_placeholder_image() -> Image {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::new(200, 200);
    Image::from_rgba8(buffer)
}

fn setup_callbacks(main_window: &MainWindow, cache: Rc<RefCell<ImageRingCache>>) {
    let window_weak = main_window.as_weak();
    let cache_clone = cache.clone();

    main_window.on_request_next_image(move || {
        if let Some(main_window) = window_weak.upgrade() {
            match cache_clone.borrow_mut().get_next() {
                Ok(img) => {
                    main_window.set_curr_image(img);
                    // main_window.set_curr_image_index(cache_clone.borrow().paths_index as i32);
                }
                Err(e) => {
                    error!("Error moving to next image: {}", e);
                    main_window.set_curr_image(create_placeholder_image());
                }
            }
        }
    });

    let window_weak = main_window.as_weak();
    let cache_clone = cache.clone();

    main_window.on_request_prev_image(move || {
        if let Some(main_window) = window_weak.upgrade() {
            match cache_clone.borrow_mut().get_prev() {
                Ok(img) => {
                    main_window.set_curr_image(img);
                    // main_window.set_curr_image_index(cache_clone.borrow().paths_index as i32);
                }
                Err(e) => {
                    error!("Error moving to previous image: {}", e);
                    main_window.set_curr_image(create_placeholder_image());
                }
            }
        }
    });
}

pub fn run(config: &Config) -> Result<(), Box<dyn Error>> {
    info!("Running with path: {}", &config.path);
    let (img_paths, start_idx) = load_img_paths(&config.path);

    if img_paths.is_empty() {
        error!("No images found at path: {}", &config.path);
        return Err("No images found".into());
    }
    let main_window = MainWindow::new().unwrap();

    let initial_image = load_img(&img_paths[start_idx]).unwrap_or_else(|e| {
        error!(
            "Failed to load initial image {}: {}",
            img_paths[start_idx].display(),
            e
        );
        create_placeholder_image()
    });
    main_window.set_curr_image(initial_image);
    main_window.set_curr_image_index(start_idx as i32);

    let img_paths = Rc::new(img_paths);
    let image_ring_cache = Rc::new(std::cell::RefCell::new(ImageRingCache::new(
        img_paths.clone(),
        10, // half_cache_size
        start_idx,
    )));
    setup_callbacks(&main_window, image_ring_cache.clone());

    info!("Starting Slint event loop...");
    main_window.run().unwrap();
    Ok(())
}
