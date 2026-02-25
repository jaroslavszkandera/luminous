use dlopen2::wrapper::{Container, WrapperApi};
use log::{debug, error, info};
use serde::Deserialize;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Expected ABI
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ImageBuffer {
    pub data: *mut u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub channels: u32,
}

// API from the shared library
#[derive(WrapperApi)]
struct ImagePluginApi {
    load_image: unsafe extern "C" fn(path: *const i8) -> ImageBuffer,
    save_image: unsafe extern "C" fn(path: *const i8, img: ImageBuffer) -> bool,
    free_image: unsafe extern "C" fn(img: ImageBuffer),
    get_plugin_info: unsafe extern "C" fn(name: *mut i8, n_max: i32, exts: *mut i8, e_max: i32),
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
    Encoder,
    Unknown,
}

pub struct Plugin {
    manifest: PluginManifest,
    container: Container<ImagePluginApi>,
}

impl Plugin {
    pub fn new(manifest: PluginManifest, dir_path: PathBuf) -> Option<Self> {
        let suffix = std::env::consts::DLL_SUFFIX;
        debug!(
            "Searching for library with suffix '{}' in {:?}",
            suffix, dir_path
        );

        let entries: Vec<_> = fs::read_dir(&dir_path)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();

        for p in &entries {
            debug!("Checking file: {:?}", p);
        }

        let lib_path = entries
            .into_iter()
            .find(|p| p.to_string_lossy().ends_with(suffix))?;

        info!("Found library: {:?}", lib_path);

        let abs_lib_path = std::fs::canonicalize(&lib_path).ok()?;
        info!("Attempting to load absolute path: {:?}", abs_lib_path);

        let container = unsafe {
            match Container::load(&abs_lib_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to load library {:?}: {}", abs_lib_path, e);
                    return None;
                }
            }
        };
        debug!("New plugin '{}' successfully registered", manifest.name);
        Some(Self {
            manifest,
            container,
        })
    }

    fn eval_version(&self) -> bool {
        self.manifest.version == env!("CARGO_PKG_VERSION")
    }

    pub fn decode(&self, path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        if !self
            .manifest
            .capabilities
            .contains(&PluginCapability::Decoder)
        {
            error!("Plugin '{}' does not support decoding", self.manifest.name);
            return None;
        }

        let c_path = CString::new(path.to_str()?).ok()?;
        let ffi_buffer = unsafe { self.container.load_image(c_path.as_ptr()) };

        if ffi_buffer.data.is_null() {
            return None;
        }

        let pixel_slice = unsafe { std::slice::from_raw_parts(ffi_buffer.data, ffi_buffer.len) };
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            pixel_slice,
            ffi_buffer.width,
            ffi_buffer.height,
        );

        unsafe { self.container.free_image(ffi_buffer) };
        Some(buffer)
    }

    pub fn encode(&self, path: &Path, buffer: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        if !self
            .manifest
            .capabilities
            .contains(&PluginCapability::Encoder)
        {
            error!("Plugin '{}' does not support encoding", self.manifest.name);
            return false;
        }

        let c_path = CString::new(path.to_str().unwrap_or_default())
            .ok()
            .unwrap();
        let ffi_buffer = ImageBuffer {
            data: buffer.as_slice().as_ptr() as *mut u8,
            len: buffer.as_slice().len() * 4,
            width: buffer.width(),
            height: buffer.height(),
            channels: 4,
        };
        unsafe { self.container.save_image(c_path.as_ptr(), ffi_buffer) }
    }

    pub fn get_info(&self) -> (String, String) {
        let mut name = vec![0u8; 256];
        let mut exts = vec![0u8; 256];
        unsafe {
            self.container.get_plugin_info(
                name.as_mut_ptr() as *mut i8,
                256,
                exts.as_mut_ptr() as *mut i8,
                256,
            );
        }
        (
            String::from_utf8_lossy(&name)
                .trim_matches(char::from(0))
                .to_string(),
            String::from_utf8_lossy(&exts)
                .trim_matches(char::from(0))
                .to_string(),
        )
    }
}

pub struct PluginManager {
    /// extension -> Plugin
    plugins: HashMap<String, Arc<Plugin>>,
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
            Some(p) => Arc::new(p),
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
                    debug!(
                        "Added decoding support for \"{:?}\" extension",
                        manifest.extensions
                    );
                }
                PluginCapability::Encoder => {
                    debug!(
                        "Added encoding support for \"{:?}\" extension",
                        manifest.extensions
                    );
                }
                PluginCapability::Unknown => {
                    error!("Unknown plugin capability in {}: {:?}", manifest.name, cap);
                }
            }
        }

        for ext in &manifest.extensions {
            self.plugins.insert(ext.to_lowercase(), plugin.clone());
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
