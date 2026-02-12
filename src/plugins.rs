use log::{debug, error, info};
use serde::Deserialize;
use shared_memory::ShmemConf;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

#[derive(Deserialize, Debug, Clone)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub executable: String,
    pub extensions: Vec<String>,
    pub plugin_type: String,
}

#[derive(Deserialize, Debug)]
struct HandshakeRequest {
    status: String,
    width: u32,
    height: u32,
    required_bytes: usize,
    error: Option<String>,
}

pub struct PluginManager {
    registry: HashMap<String, (PathBuf, String)>, // (Path, Type)
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    pub fn discover(&mut self, plugins_dir: &Path) {
        info!("Discovering plugins in: {:?}", plugins_dir);
        if !plugins_dir.exists() {
            return;
        }

        let entries = match fs::read_dir(plugins_dir) {
            Ok(e) => e,
            Err(e) => {
                error!("Failed to read plugins dir: {}", e);
                return;
            }
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("plugin.json");
                if manifest_path.exists() {
                    self.load_manifest(&path, &manifest_path);
                }
            }
        }
    }

    fn load_manifest(&mut self, plugin_dir: &Path, manifest_path: &Path) {
        let content = match fs::read_to_string(manifest_path) {
            Ok(c) => c,
            Err(e) => {
                error!("Error reading manifest {:?}: {}", manifest_path, e);
                return;
            }
        };

        match serde_json::from_str::<PluginManifest>(&content) {
            Ok(manifest) => {
                let exec_path = plugin_dir.join(&manifest.executable);

                if !exec_path.exists() {
                    error!("Plugin executable not found: {:?}", exec_path);
                    return;
                }

                info!(
                    "Loaded Plugin: {} v{} ({})",
                    manifest.name, manifest.version, manifest.plugin_type
                );

                for ext in &manifest.extensions {
                    self.registry.insert(
                        ext.to_string(),
                        (exec_path.clone(), manifest.plugin_type.clone()),
                    );
                    debug!("Registered extension .{} to {:?}", ext, exec_path);
                }
            }
            Err(e) => error!("Failed to parse JSON {:?}: {}", manifest_path, e),
        }
    }

    pub fn get_supported_extensions(&self) -> Vec<String> {
        self.registry.keys().cloned().collect()
    }

    pub fn has_plugin(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            return self.registry.contains_key(&ext.to_lowercase());
        }
        false
    }

    pub fn load_via_plugin(&self, path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        let (exec_path, plugin_type) = self.registry.get(&ext)?;

        let start_time = Instant::now();

        let mut cmd = if plugin_type == "python" {
            let mut c = Command::new("python3");
            c.arg(exec_path);
            c
        } else {
            Command::new(exec_path)
        };

        let mut child = cmd
            .arg(path.to_str()?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .ok()?;

        let mut reader = BufReader::new(child.stdout.as_mut()?);
        let mut json_line = String::new();
        reader.read_line(&mut json_line).ok()?;

        let meta: HandshakeRequest = match serde_json::from_str(&json_line) {
            Ok(m) => m,
            Err(e) => {
                error!("Handshake failed. Invalid JSON: {}", e);
                return None;
            }
        };

        if meta.status != "ready" {
            error!("Plugin error: {:?}", meta.error);
            return None;
        }

        debug!(
            "Plugin requested {} bytes for {}x{}",
            meta.required_bytes, meta.width, meta.height
        );

        let shmem = match ShmemConf::new().size(meta.required_bytes).create() {
            Ok(m) => m,
            Err(e) => {
                error!("Host failed to allocate shmem: {}", e);
                return None;
            }
        };
        let os_id = shmem.get_os_id();

        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{}", os_id).ok()?;
        }

        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            error!("Plugin crashed during rendering");
            return None;
        }

        let raw_slice = unsafe {
            std::slice::from_raw_parts(shmem.as_ptr(), (meta.width * meta.height * 4) as usize)
        };

        let buffer =
            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(raw_slice, meta.width, meta.height);

        let duration = start_time.elapsed();
        info!("Plugin execution time: {:.2?}", duration);

        Some(buffer)
    }
}
