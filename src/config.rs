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
    pub log_level: String,
    pub threads: usize,
    pub window_size: usize,
    pub background: Color,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Luminous - Image viewer and editor.", long_about = None)]
struct Cli {
    /// The path to the image or directory to open
    path: Option<String>,
    /// Logging level (error, warn, info, debug, trace)
    /// Defaults to "warn"
    #[arg(short, long)]
    log_level: Option<String>,
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
    log_level: Option<String>,
    threads: Option<usize>,
    window_size: Option<usize>,
    background: Option<String>,
    #[serde(flatten)]
    unknown: HashMap<String, toml::Value>,
}

impl Config {
    pub fn load() -> Self {
        let cli = Cli::parse();

        let config_path = Self::find_config_path(&cli.config_file);
        let toml_config = if let Some(path) = &config_path {
            Self::load_toml(path)
        } else {
            TomlConfig::default()
        };

        if !toml_config.unknown.is_empty() {
            eprintln!(
                "Warning: Unknown keys found in config file: {:?}",
                toml_config.unknown.keys().collect::<Vec<_>>()
            );
        }

        // CLI overrides TOML, TOML overrides Defaults
        let path = cli
            .path
            .or(toml_config.path)
            .unwrap_or_else(|| ".".to_string());

        let log_level = cli
            .log_level
            .or(toml_config.log_level)
            .unwrap_or_else(|| "warn".to_string());

        let threads = cli
            .threads
            .or(toml_config.threads)
            .unwrap_or_else(num_cpus::get);

        let window_size = cli
            .window_size
            .or(toml_config.window_size)
            .unwrap_or_else(|| 3);

        let background_str = cli
            .background
            .or(toml_config.background)
            .unwrap_or_else(|| "#000000".to_string());

        let background = Self::parse_color(&background_str);

        Config {
            path,
            log_level,
            threads,
            window_size,
            background,
        }
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
}
