use log::{error, info};
use std::process;

use luminous::config::Config;

fn main() {
    let config = Config::load();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&config.log_level))
        .init();

    info!("Starting with {} worker threads", config.threads);

    if let Err(e) = luminous::run(&config) {
        error!("Application error: {e}");
        process::exit(1);
    };
}
