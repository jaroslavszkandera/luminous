use clap::Parser;
use directories::ProjectDirs;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub path: String,
    pub log_level: String,
    pub threads: usize,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Luminous - Image viewer and editor built with Rust and Slint.", long_about = None)]
struct Cli {
    path: Option<String>,
    /// Logging level (error, warn, info, debug, trace)
    #[arg(short, long)]
    log_level: Option<String>,
    #[arg(short, long)]
    threads: Option<usize>,
    #[arg(long)]
    config_file: Option<PathBuf>,
}

#[derive(Deserialize, Default)]
struct TomlConfig {
    path: Option<String>,
    log_level: Option<String>,
    threads: Option<usize>,
}

impl Config {
    pub fn load() -> Self {
        let cli = Cli::parse();

        let config_path = if let Some(p) = cli.config_file {
            Some(p)
        } else if let Some(proj_dirs) = ProjectDirs::from("", "", "luminous") {
            let config_dir = proj_dirs.config_dir();
            Some(config_dir.join("luminous.toml"))
        } else {
            None
        };

        let toml_config: TomlConfig = if let Some(path) = &config_path {
            if path.exists() {
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
            } else {
                TomlConfig::default()
            }
        } else {
            TomlConfig::default()
        };

        // CLI overrides TOML, TOML overrides Defaults
        let path = cli
            .path
            .or(toml_config.path)
            .unwrap_or_else(|| ".".to_string());

        let log_level = cli
            .log_level
            .or(toml_config.log_level)
            .unwrap_or_else(|| "info".to_string());

        let threads = cli
            .threads
            .or(toml_config.threads)
            .unwrap_or_else(num_cpus::get);

        Config {
            path,
            log_level,
            threads,
        }
    }
}
