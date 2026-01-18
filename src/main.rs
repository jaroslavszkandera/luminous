use log::{error, info};
use std::process;

use luminous::config::Config;
use luminous::fs_scan;

fn main() {
    let config = Config::load();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&config.log_level))
        .init();

    let scan_result = fs_scan::scan(&config.path);

    if scan_result.paths.is_empty() {
        error!("No supported images found in {}", config.path);
        process::exit(1);
    }

    info!("Starting with {} worker threads", config.threads);

    if let Err(e) = luminous::run(&scan_result, config.threads) {
        error!("Application error: {e}");
        process::exit(1);
    };
}
