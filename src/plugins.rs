use log::{debug, error, info, warn};
use serde::Deserialize;
use shared_memory::ShmemConf;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

pub trait ImageDecoder: Send + Sync {
    fn decode(&self, image_path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>>;
}

// pub trait ImageEncoder: Send + Sync {
//     fn encoder(&self, image_path: &Path, buffer: &SharedPixelBuffer<Rgba8Pixel>) -> bool;
// }

// pub trait ImageModifier: Send + Sync {
//     fn modify(&self, buffer: SharedPixelBuffer<Rgba8Pixel>) -> Option<SharedPixelBuffer<Rgba8Pixel>>;
// }

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PluginCapability {
    Decoder,
    // Encoder,
    // Filter,
    // Metadata,
    // Export
    Unknown,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub executable: String,
    pub interpreter: Option<String>,
    pub extensions: Vec<String>,
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Deserialize, Debug)]
struct PluginHandshake {
    status: String,
    width: u32,
    height: u32,
    required_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub struct Plugin {
    manifest: PluginManifest,
    path: PathBuf,
}

impl Plugin {
    pub fn new(manifest: PluginManifest, path: PathBuf) -> Self {
        Self { manifest, path }
    }

    fn exec(&self, arg: &str) -> Command {
        let exec_path = &self.path.join(&self.manifest.executable);
        let mut cmd = if let Some(ref interp) = self.manifest.interpreter {
            let mut c = Command::new(interp);
            c.arg(exec_path);
            c
        } else {
            Command::new(exec_path)
        };
        cmd.arg(arg);
        cmd
    }
}

pub struct PluginManager {
    decoders: HashMap<String, Arc<dyn ImageDecoder>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            decoders: HashMap::new(),
        }
    }

    pub fn discover(&mut self, plugins_dir: &Path) {
        info!("Discovering plugins in: {:?}", plugins_dir);
        if !plugins_dir.exists() {
            return;
        }

        let plugin_entries = match fs::read_dir(plugins_dir) {
            Ok(e) => e,
            Err(e) => {
                error!("Failed to read plugins dir: {}", e);
                return;
            }
        };

        for entry in plugin_entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            debug!("path: {:?}", path.to_str());
            if path.is_dir() {
                let manifest_path = path.join("plugin.json");
                if manifest_path.exists() {
                    let m = self.load_manifest(&manifest_path);
                    self.register(m, path);
                }
            }
        }
    }

    fn register(&mut self, manifest: Option<PluginManifest>, path: PathBuf) {
        let manifest = if let Some(m) = manifest {
            info!("Manifest ok");
            m
        } else {
            error!("Manifest not ok");
            return;
        };
        let plugin = Arc::new(Plugin::new(manifest.clone(), path));
        for cap in &manifest.capabilities {
            match cap {
                PluginCapability::Decoder => {
                    for ext in &manifest.extensions {
                        self.decoders.insert(ext.to_lowercase(), plugin.clone());
                        debug!("Added decoding support for \"{}\" extension", ext);
                    }
                }
                PluginCapability::Unknown => {
                    error!("Unknown plugin capability: {:?}", cap);
                }
            }
        }
    }

    fn load_manifest(&mut self, path: &Path) -> Option<PluginManifest> {
        info!("Loading plugin manifest: {:?}", path.to_str());
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to read manifest {:?}: {}", path, e);
                return None;
            }
        };

        let manifest: PluginManifest = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                error!("Invalid manifest {:?}: {}", path, e);
                return None;
            }
        };

        let plugin_dir = path.parent().unwrap();
        let exec_path = plugin_dir.join(&manifest.executable);

        if !exec_path.exists() {
            error!(
                "Plugin executable not found: {:?} for manifest {}",
                exec_path, manifest.name
            );
            return None;
        }

        info!("Loaded plugin manifest {}: {:#?}", manifest.name, manifest);
        Some(manifest)
    }

    pub fn get_decoder(&self, path: &Path) -> Option<Arc<dyn ImageDecoder>> {
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            self.decoders.get(&ext.to_lowercase()).cloned()
        } else {
            None
        }
    }

    pub fn get_supported_extensions(&self) -> Vec<String> {
        self.decoders.keys().cloned().collect()
    }

    pub fn has_plugin(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            return self.decoders.contains_key(&ext.to_lowercase());
        }
        false
    }
}

impl ImageDecoder for Plugin {
    fn decode(&self, image_path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let mut child = self
            .exec("decode")
            .arg(image_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| error!("Spawn failed: {}", e))
            .ok()?;

        let mut reader = BufReader::new(child.stdout.take().expect("Failed to take stdout"));
        let mut json_line = String::new();

        if let Err(e) = reader.read_line(&mut json_line) {
            error!("Failed to read JSON line from plugin: {}", e);
            return None;
        }

        debug!("Raw JSON from plugin: {:?}", json_line);

        let meta: PluginHandshake = serde_json::from_str(&json_line).ok()?;
        debug!("meta: {:#?}", meta);
        let shmem = ShmemConf::new().size(meta.required_bytes).create().ok()?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{}", shmem.get_os_id());
        }

        child.wait().ok()?.success().then(|| {
            let raw = unsafe { std::slice::from_raw_parts(shmem.as_ptr(), meta.required_bytes) };
            SharedPixelBuffer::clone_from_slice(raw, meta.width, meta.height)
        })
    }
}
