use dlopen2::wrapper::{Container, WrapperApi};
use log::{debug, error, info};
use serde::Deserialize;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

// Expected ABI
#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiImage {
    pub data: *mut u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub channels: u8,
}

// API from the shared library
#[derive(WrapperApi)]
struct ImagePluginApi {
    load_image: unsafe extern "C" fn(path: *const i8) -> FfiImage,
    free_image: unsafe extern "C" fn(img: FfiImage),
}

#[derive(Deserialize, Debug, Clone)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub extensions: Vec<String>,
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PluginCapability {
    Decoder,
    Unknown,
}

#[derive(Clone)]
pub struct Plugin {
    manifest: PluginManifest,
    lib_path: PathBuf,
}

impl Plugin {
    pub fn new(manifest: PluginManifest, dir_path: PathBuf) -> Option<Self> {
        let suffix = std::env::consts::DLL_SUFFIX;
        let lib_path = fs::read_dir(&dir_path)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().ends_with(suffix))?;

        Some(Self { manifest, lib_path })
    }

    fn eval_version(&self) -> bool {
        self.manifest.version == env!("CARGO_PKG_VERSION")
    }

    pub fn decode(&self, image_path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let container: Container<ImagePluginApi> = unsafe {
            match Container::load(&self.lib_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to load library {:?}: {}", self.lib_path, e);
                    return None;
                }
            }
        };

        let c_path_str = image_path.to_str()?;
        let c_path = CString::new(c_path_str).ok()?;

        let ffi_img = unsafe { container.load_image(c_path.as_ptr()) };

        if ffi_img.data.is_null() {
            return None;
        }

        let pixel_slice = unsafe { std::slice::from_raw_parts(ffi_img.data, ffi_img.len) };
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            pixel_slice,
            ffi_img.width,
            ffi_img.height,
        );

        unsafe { container.free_image(ffi_img) };

        Some(buffer)
    }
}

pub struct PluginManager {
    /// extension -> Plugin
    plugins: HashMap<String, Plugin>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
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
            debug!("Plugin path: {:?}", path.to_str());
            if path.is_dir() {
                let manifest_path = path.join("plugin.json");
                if manifest_path.exists() {
                    if let Some(manifest) = self.load_manifest(&manifest_path) {
                        self.register(path, manifest);
                    }
                } else {
                    error!("Plugin manifest missing: {:?}", &manifest_path);
                }
            }
        }
    }

    fn register(&mut self, dir_path: PathBuf, manifest: PluginManifest) {
        let plugin = match Plugin::new(manifest.clone(), dir_path) {
            Some(p) => p,
            None => {
                error!("No library file found for plugin {}", manifest.name);
                return;
            }
        };
        if !plugin.eval_version() {
            error!(
                "Skipping plugin {}: Version mismatch (found {}, expected {})",
                manifest.name,
                manifest.version,
                env!("CARGO_PKG_VERSION")
            );
            return;
        }

        for cap in &manifest.capabilities {
            match cap {
                PluginCapability::Decoder => {
                    for ext in &manifest.extensions {
                        self.plugins.insert(ext.to_lowercase(), plugin.clone());
                        debug!("Added decoding support for \"{}\" extension", ext);
                    }
                }
                PluginCapability::Unknown => {
                    error!("Unknown plugin capability in {}: {:?}", manifest.name, cap);
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

        info!("Loaded plugin manifest {}: {:#?}", manifest.name, manifest);
        Some(manifest)
    }

    pub fn get_supported_extensions(&self) -> Vec<&str> {
        self.plugins.keys().map(|s| s.as_str()).collect()
    }

    pub fn decode(&self, path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        if let Some(plugin) = self.plugins.get(&ext) {
            debug!("Using plugin '{}' for {:?}", plugin.manifest.name, path);
            plugin.decode(path)
        } else {
            None
        }
    }

    pub fn has_plugin(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            return self.plugins.contains_key(&ext.to_lowercase());
        }
        false
    }
}
