slint::include_modules!();

mod image_loader;
use image_loader::ImageLoader;

use log::{debug, error, info};
use slint::{Image, Model, VecModel};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
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

    let mut paths: Vec<PathBuf> = Vec::new();
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
                    starting_index = paths.len();
                    info!("Starting image set to index: {}", starting_index);
                }
            }
            paths.push(path);
        }
    }
    if metadata.is_dir() {
        info!("Path was a directory, starting index is 0.");
        starting_index = 0;
    }

    info!(
        "Found {} images. Starting index: {}",
        paths.len(),
        starting_index
    );
    (paths, starting_index)
}

pub fn run(config: &Config) -> Result<(), Box<dyn Error>> {
    info!("Running with path: {}", &config.path);
    let (paths, start_idx) = load_img_paths(&config.path);

    if paths.is_empty() {
        error!("No images found at path: {}", &config.path);
        return Err("No images found".into());
    }

    let main_window = MainWindow::new().unwrap();
    let loader = Rc::new(ImageLoader::new(paths.clone(), 8));

    let mut grid_data = Vec::new();
    for (i, _) in paths.iter().enumerate() {
        grid_data.push(GridItem {
            image: slint::Image::default(),
            index: i as i32,
        });
    }
    let grid_model = Rc::new(VecModel::from(grid_data));
    main_window.set_grid_model(grid_model.clone().into());

    main_window.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });

    // Grid View
    let loader_grid = loader.clone();
    let window_weak = main_window.as_weak();

    // FIX: Grid data loading init blocks Full loading
    main_window.on_request_grid_data(move |index| {
        let index = index as usize;
        if let Some(loader) = loader_grid.clone().into() {
            let on_loaded = move |ui: MainWindow, idx: usize, img: slint::Image| {
                let model = ui.get_grid_model();
                if let Some(mut item) = model.row_data(idx) {
                    item.image = img;
                    model.set_row_data(idx, item);
                }
            };

            let cached = loader.load_grid_thumb(index, window_weak.clone(), on_loaded);

            if let Some(buffer) = cached {
                let window_weak_defer = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = window_weak_defer.upgrade() {
                        let model = ui.get_grid_model();
                        if let Some(mut item) = model.row_data(index) {
                            item.image = Image::from_rgba8(buffer);
                            model.set_row_data(index, item);
                        }
                    }
                });
            }
        }
    });

    // Full View
    let loader_full = loader.clone();
    let paths_len = paths.len();

    let update_full_view = move |ui: MainWindow, index: usize| {
        let window_weak_cb = ui.as_weak();

        let display_img =
            loader_full.load_full_progressive(index, window_weak_cb, move |ui, final_img| {
                ui.set_full_view_image(final_img);
            });

        ui.set_full_view_image(display_img);
        ui.set_curr_image_index(index as i32);

        loader_full.update_sliding_window(index);
    };

    // Callback: Selection from Grid
    let update_fn = update_full_view.clone();
    let window_weak_select = main_window.as_weak();
    main_window.on_image_selected(move |index| {
        if let Some(ui) = window_weak_select.upgrade() {
            update_fn(ui, index as usize);
        }
    });

    // Callback: Next
    let update_fn = update_full_view.clone();
    let window_weak_next = main_window.as_weak();
    main_window.on_request_next_image(move || {
        if let Some(ui) = window_weak_next.upgrade() {
            let mut idx = ui.get_curr_image_index() as usize;
            idx += 1;
            if idx >= paths_len {
                idx = 0;
            }
            update_fn(ui, idx);
        }
    });

    // Callback: Prev
    let update_fn = update_full_view.clone();
    let window_weak_prev = main_window.as_weak();
    main_window.on_request_prev_image(move || {
        if let Some(ui) = window_weak_prev.upgrade() {
            let mut idx = ui.get_curr_image_index() as isize;
            idx -= 1;
            if idx < 0 {
                idx = (paths_len - 1) as isize;
            }
            update_fn(ui, idx as usize);
        }
    });

    // Init
    if !paths.is_empty() {
        debug!("Initializing Full View at index {}", start_idx);
        let handle = main_window.as_weak().upgrade().unwrap();
        update_full_view(handle, start_idx);
    }

    main_window.run()?;
    Ok(())
}
