use clap::Parser;
use directories::ProjectDirs;
use serde::Deserialize;
use slint::Color;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub path: String,
    pub log: String,
    pub threads: usize,
    pub window_size: usize,
    pub background: Color,
    pub bindings: HashMap<String, String>,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Luminous - Image viewer and editor.", long_about = None)]
struct Cli {
    /// The path to the image or directory to open
    path: Option<String>,
    /// Logging level (error, warn, info, debug, trace)
    /// Defaults to "warn"
    #[arg(short, long)]
    log: Option<String>,
    /// Number of worker threads
    /// Defaults to the number of CPUs available
    #[arg(short, long)]
    threads: Option<usize>,
    /// Custom path to a config file
    #[arg(long)]
    config_file: Option<PathBuf>,
    #[arg(long)]
    // Cache size in full view
    window_size: Option<usize>,
    #[arg(long)]
    // Background window color
    background: Option<String>,
}

#[derive(Deserialize, Default)]
struct TomlConfig {
    path: Option<String>,
    log: Option<String>,
    threads: Option<usize>,
    window_size: Option<usize>,
    background: Option<String>,
    bindings: Option<HashMap<String, String>>,
    #[serde(flatten)]
    unknown: HashMap<String, toml::Value>,
}

impl Config {
    pub fn load() -> Self {
        let cli = Cli::parse();

        let toml_config = Self::find_config_path(&cli.config_file)
            .map(|p| Self::load_toml(&p))
            .unwrap_or_default();

        if !toml_config.unknown.is_empty() {
            eprintln!("Unknown config keys: {:?}", toml_config.unknown.keys());
        }

        let path = Self::resolve(cli.path, toml_config.path, ".".to_string());
        let log = Self::resolve(cli.log, toml_config.log, "warn".to_string());
        let threads = cli
            .threads
            .or(toml_config.threads)
            .filter(|&t| t > 0)
            .unwrap_or_else(num_cpus::get);
        let window_size = Self::resolve(cli.window_size, toml_config.window_size, 3);
        let background_str = Self::resolve(
            cli.background,
            toml_config.background,
            "#000000".to_string(),
        );
        let background = Self::parse_color(&background_str);

        let mut bindings = Self::default_bindings();
        if let Some(user_bindings) = toml_config.bindings {
            bindings.extend(user_bindings);
        }

        Config {
            path,
            log,
            threads,
            window_size,
            background,
            bindings,
        }
    }

    fn resolve<T>(cli: Option<T>, toml: Option<T>, default: T) -> T {
        cli.or(toml).unwrap_or(default)
    }

    fn find_config_path(cli_path: &Option<PathBuf>) -> Option<PathBuf> {
        if let Some(p) = cli_path {
            return Some(p.clone());
        }
        if let Some(proj_dirs) = ProjectDirs::from("", "", "luminous") {
            let config_dir = proj_dirs.config_dir();
            let default_loc = config_dir.join("luminous.toml");
            if default_loc.exists() {
                return Some(default_loc);
            }
        }
        None
    }

    fn load_toml(path: &PathBuf) -> TomlConfig {
        if !path.exists() {
            return TomlConfig::default();
        }

        match fs::read_to_string(path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("Warning: Failed to parse config file {:?}: {}", path, e);
                    TomlConfig::default()
                }
            },
            Err(e) => {
                eprintln!("Warning: Failed to read config file {:?}: {}", path, e);
                TomlConfig::default()
            }
        }
    }

    fn parse_color(color_str: &str) -> slint::Color {
        csscolorparser::parse(color_str)
            .map(|c| {
                slint::Color::from_argb_u8(
                    (c.a * 255.0) as u8,
                    (c.r * 255.0) as u8,
                    (c.g * 255.0) as u8,
                    (c.b * 255.0) as u8,
                )
            })
            .unwrap_or_else(|_| {
                eprintln!(
                    "Warning: Invalid color '{}', defaulting to black",
                    color_str
                );
                slint::Color::from_rgb_u8(0, 0, 0)
            })
    }

    fn default_bindings() -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("quit".into(), "q".into());
        map.insert("toggle_fullscreen".into(), "f".into());
        map.insert("switch_view_mode".into(), "Escape".into());
        map.insert("switch_mouse_mode".into(), "m".into());
        map.insert("grid_page_down".into(), "PageDown".into());
        map.insert("grid_page_up".into(), "PageUp".into());
        map.insert("reset_zoom".into(), "z".into());
        map
    }

    pub fn get_slint_key_string(key_name: &str) -> slint::SharedString {
        use slint::platform::Key;
        match key_name {
            "Right" => Key::RightArrow.into(),
            "Left" => Key::LeftArrow.into(),
            "Up" => Key::UpArrow.into(),
            "Down" => Key::DownArrow.into(),
            "Escape" | "Esc" => Key::Escape.into(),
            "Return" | "Enter" => Key::Return.into(),
            "Tab" => Key::Tab.into(),
            "Backspace" => Key::Backspace.into(),
            "PageUp" => Key::PageUp.into(),
            "PageDown" => Key::PageDown.into(),
            "Home" => Key::Home.into(),
            "End" => Key::End.into(),
            "Delete" => Key::Delete.into(),
            // For single characters, return as is
            other => slint::SharedString::from(other),
        }
    }
}
