use color_print::cformat;
use std::io::Write;
use std::process;

use luminous::config::Config;

fn main() {
    let config = Config::load();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&config.log))
        .format(|buf, record| {
            let level = record.level();
            let level_char = match level {
                log::Level::Error => cformat!("<s><red>ERROR</>"),
                log::Level::Warn => cformat!("<yellow>WARN </>"),
                log::Level::Info => cformat!("<green>INFO </>"),
                log::Level::Debug => cformat!("<blue>DEBUG</>"),
                log::Level::Trace => cformat!("<cyan>TRACE</>"),
            };

            let curr_thread = std::thread::current();
            let thread_name = curr_thread.name().unwrap_or("----");
            let timestamp = chrono::Local::now().format("%T");

            writeln!(
                buf,
                "{}{} {} {} {}{} {}",
                cformat!("<bright-black>[</>"),
                timestamp,
                level_char,
                thread_name,
                record.module_path().unwrap_or(""),
                cformat!("<bright-black>]</>"),
                record.args()
            )
        })
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("sctk", log::LevelFilter::Warn)
        .filter_module("luminous::image_loader", log::LevelFilter::Info)
        .init();

    log::info!("Starting with {} worker threads", config.threads);

    if let Err(e) = luminous::run(config) {
        log::error!("Application error: {e}");
        process::exit(1);
    };
}
