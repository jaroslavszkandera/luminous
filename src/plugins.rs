use log::{debug, error};
use serde::Deserialize;
use shared_memory::ShmemConf;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use uuid::Uuid;

// #[derive(Clone)]
// pub struct PluginConfig {
//     pub command: String,
//     pub extensions: Vec<String>,
// }

#[derive(Deserialize, Debug)]
struct PluginResponse {
    status: String,
    width: u32,
    height: u32,
    error: Option<String>,
}

pub struct PluginManager {
    registry: HashMap<String, String>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    // TODO: load from a config file or just search in dir
    pub fn register(&mut self, ext: &str, cmd: &str) {
        self.registry.insert(ext.to_lowercase(), cmd.to_string());
    }

    // pub fn get_supported_extensions(&self) -> Vec<String> {
    //     self.registry.keys().cloned().collect()
    // }

    pub fn has_plugin(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            return self.registry.contains_key(&ext.to_lowercase());
        }
        false
    }

    pub fn load_via_plugin(&self, path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        let cmd_str = self.registry.get(&ext)?;

        // 10 MB for a start, negotiate with the plugin in the future
        let shmem_size = 10 * 1024 * 1024;
        let shmem_id = format!("luminous-{}", Uuid::new_v4());

        let shmem = match ShmemConf::new().size(shmem_size).os_id(&shmem_id).create() {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to create shared memory: {}", e);
                return None;
            }
        };

        // Arguments: <file_path> <shmem_id> <shmem_size>
        let args: Vec<&str> = cmd_str.split_whitespace().collect();
        let mut command = Command::new(args[0]);
        if args.len() > 1 {
            command.args(&args[1..]);
        }

        command
            .arg(path.to_str()?)
            .arg(&shmem_id)
            .arg(shmem_size.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        debug!("Invoking plugin: {:?}", command);

        let output = command.output().ok()?;

        if !output.status.success() {
            error!("Plugin exited with error");
            return None;
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let response: PluginResponse = match serde_json::from_str(&output_str) {
            Ok(r) => r,
            Err(e) => {
                error!("Invalid JSON from plugin: {}. Output: {}", e, output_str);
                return None;
            }
        };

        if response.status != "ok" {
            error!("Plugin reported error: {:?}", response.error);
            return None;
        }

        let raw_slice = unsafe {
            std::slice::from_raw_parts(
                shmem.as_ptr(),
                (response.width * response.height * 4) as usize,
            )
        };

        Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            raw_slice,
            response.width,
            response.height,
        ))
    }
}
