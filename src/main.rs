use std::process;

use luminous::config::Config;

fn main() {
    let config = Config::load();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&config.log))
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("sctk", log::LevelFilter::Warn)
        .init();

    log::info!("Starting with {} worker threads", config.threads);

    if let Err(e) = luminous::run(config) {
        log::error!("Application error: {e}");
        process::exit(1);
    };
}
