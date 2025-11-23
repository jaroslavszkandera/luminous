use log::error;
use std::env;
use std::process;

use luminous::Config;

fn main() {
    let config = Config::build(env::args()).unwrap_or_else(|err| {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("error")).init();
        error!("Problem parsing arguments: {}", err);
        process::exit(1);
    });
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&config.log_level))
        .init();

    if let Err(e) = luminous::run(&config) {
        error!("Application error: {e}");
        process::exit(1);
    };
}
