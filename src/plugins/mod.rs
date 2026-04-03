pub mod ipc_daemon;
pub mod manifest;
pub mod shared_lib;

pub use ipc_daemon::IpcStatus;
pub use manifest::{BackendKind, PluginCapability, PluginManifest, load_manifest};

use ipc_daemon::DaemonBackend;
use log::{debug, error, info};
use shared_lib::SharedLibBackend;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait Backend: Send + Sync {
    fn start(&self) {}
    fn stop(&self) {}
    fn decode(&self, _path: &Path) -> Option<image::DynamicImage> {
        None
    }
    fn encode(&self, _path: &Path, _buf: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        false
    }
    fn set_image(&self, _buf: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        false
    }
    fn click(&self, _x: u32, _y: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        None
    }
    fn rect_select(
        &self,
        _x1: u32,
        _y1: u32,
        _x2: u32,
        _y2: u32,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        None
    }
    fn semantic_image_search(&self, _paths: &Vec<PathBuf>, _query: &str) -> Option<Vec<PathBuf>> {
        None
    }
    /// Callback invoked whenever the backend status changes
    fn on_status_change(&self, _cb: Box<dyn Fn(IpcStatus) + Send + Sync>) {}
}

pub struct Plugin {
    pub id: String,
    pub manifest: PluginManifest,
    backend: Box<dyn Backend>,
}

impl Plugin {
    pub fn new(id: String, manifest: PluginManifest, dir: PathBuf) -> Option<Self> {
        let backend: Box<dyn Backend> = match manifest.backend {
            BackendKind::Daemon => Box::new(DaemonBackend::new(&manifest, &dir) as Arc<_>),
            BackendKind::SharedLib => Box::new(SharedLibBackend::new(&manifest, &dir)?),
        };
        Some(Self {
            id,
            manifest,
            backend,
        })
    }

    pub fn start(&self) {
        self.backend.start();
    }

    pub fn stop(&self) {
        self.backend.stop();
    }

    pub fn version_compatible(&self) -> bool {
        self.manifest.version == env!("CARGO_PKG_VERSION")
    }

    // -- decoder/encoder (shared lib) --
    pub fn decode_dynamic(&self, path: &Path) -> Option<image::DynamicImage> {
        if !self.manifest.has_capability(&PluginCapability::Decoder) {
            error!("Plugin '{}' does not support decoding", self.manifest.name);
            return None;
        }
        self.backend.decode(path)
    }

    pub fn decode(&self, path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let rgba = self.decode_dynamic(path)?.to_rgba8();
        Some(SharedPixelBuffer::clone_from_slice(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
        ))
    }

    pub fn encode(&self, path: &Path, buf: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        if !self.manifest.has_capability(&PluginCapability::Encoder) {
            error!("Plugin '{}' does not support encoding", self.manifest.name);
            return false;
        }
        self.backend.encode(path, buf)
    }

    // -- interactive (daemon) --
    pub fn set_interactive_image(&self, buf: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        self.backend.set_image(buf)
    }

    pub fn interactive_click(&self, x: u32, y: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        self.backend.click(x, y)
    }

    pub fn interactive_rect_select(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        self.backend.rect_select(x1, y1, x2, y2)
    }

    pub fn semantic_image_search(&self, paths: &Vec<PathBuf>, query: &str) -> Option<Vec<PathBuf>> {
        self.backend.semantic_image_search(paths, query)
    }

    pub fn on_status_change<F>(&self, cb: F)
    where
        F: Fn(IpcStatus) + Send + Sync + 'static,
    {
        self.backend.on_status_change(Box::new(cb));
    }
}

impl Backend for Arc<DaemonBackend> {
    fn set_image(&self, buf: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        DaemonBackend::set_image(self, buf)
    }
    fn click(&self, x: u32, y: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        DaemonBackend::click(self, x, y)
    }
    fn rect_select(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        DaemonBackend::rect_select(self, x1, y1, x2, y2)
    }
    fn semantic_image_search(&self, paths: &Vec<PathBuf>, query: &str) -> Option<Vec<PathBuf>> {
        DaemonBackend::semantic_image_search(self, paths, query)
    }
    fn on_status_change(&self, cb: Box<dyn Fn(IpcStatus) + Send + Sync>) {
        DaemonBackend::on_status_change(self, move |s| cb(s));
    }
}

pub struct PluginManager {
    /// extension (lowercase) -> plugin, TODO: make more agnostic
    shlib_plugins: HashMap<String, Arc<Plugin>>,
    daemon_plugins: Vec<Arc<Plugin>>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            shlib_plugins: HashMap::new(),
            daemon_plugins: Vec::new(),
        }
    }

    /// Scan a directory for plugin subdirectories containing a `plugin.json`.
    pub fn discover(&mut self, plugins_dir: &Path) {
        info!("Discovering plugins in: {:?}", plugins_dir);
        if !plugins_dir.exists() {
            return;
        }

        let entries = match std::fs::read_dir(plugins_dir) {
            Ok(e) => e,
            Err(e) => {
                error!("Failed to read plugins dir: {}", e);
                return;
            }
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let id = match path.file_name().and_then(|n| n.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let manifest_path = path.join("plugin.json");
            if !manifest_path.exists() {
                error!("Plugin manifest missing: {:?}", manifest_path);
                continue;
            }
            if let Some(manifest) = load_manifest(&manifest_path) {
                self.register(id, path, manifest);
            }
        }
    }

    pub fn get_all_plugins(&self) -> Vec<Arc<Plugin>> {
        self.daemon_plugins.iter().cloned().collect()
    }

    pub fn get_interactive_plugins(&self) -> impl Iterator<Item = &Arc<Plugin>> {
        self.daemon_plugins.iter().filter(|p| {
            p.manifest
                .capabilities
                .contains(&PluginCapability::Interactive)
        })
    }

    pub fn get_search_plugins(&self) -> impl Iterator<Item = &Arc<Plugin>> {
        self.daemon_plugins
            .iter()
            .filter(|p| p.manifest.capabilities.contains(&PluginCapability::Search))
    }

    // WARN: tmp, returns the first plugin
    // TODO: return by some kind of UUID?
    pub fn get_interactive_plugin(&self) -> Option<Arc<Plugin>> {
        self.get_interactive_plugins().next().cloned()
    }

    pub fn get_search_plugin(&self) -> Option<Arc<Plugin>> {
        self.get_search_plugins().next().cloned()
    }

    pub fn get_supported_extensions(&self) -> Vec<&str> {
        self.shlib_plugins.keys().map(String::as_str).collect()
    }

    pub fn get_plugins_manifests(&self) -> Vec<PluginManifest> {
        self.daemon_plugins
            .iter()
            .map(|p| p.manifest.clone())
            .collect()
    }

    pub fn has_plugin_for(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| self.shlib_plugins.contains_key(&e.to_lowercase()))
            .unwrap_or(false)
    }

    pub fn decode(&self, path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let rgba = self.decode_dynamic(path)?.to_rgba8();
        Some(SharedPixelBuffer::clone_from_slice(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
        ))
    }

    pub fn decode_dynamic(&self, path: &Path) -> Option<image::DynamicImage> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        let plugin = self.shlib_plugins.get(&ext)?;
        debug!("Using plugin '{}' for {:?}", plugin.manifest.name, path);
        plugin.decode_dynamic(path)
    }

    fn register(&mut self, id: String, dir: PathBuf, manifest: PluginManifest) {
        let plugin = match Plugin::new(id, manifest.clone(), dir) {
            Some(p) => Arc::new(p),
            None => {
                error!("Failed to construct plugin '{}'", manifest.name);
                return;
            }
        };

        if !plugin.version_compatible() {
            error!(
                "Skipping plugin '{}': version mismatch (plugin={}, host={})",
                manifest.name,
                manifest.version,
                env!("CARGO_PKG_VERSION")
            );
            return;
        }

        for cap in &manifest.capabilities {
            match cap {
                PluginCapability::Decoder => {
                    debug!("Decoder support for {:?}", manifest.extensions);
                }
                PluginCapability::Encoder => {
                    debug!("Encoder support for {:?}", manifest.extensions);
                }
                PluginCapability::Interactive => {
                    self.daemon_plugins.push(plugin.clone());
                    debug!("Interactive plugin '{}'", manifest.name);
                }
                PluginCapability::Search => {
                    self.daemon_plugins.push(plugin.clone());
                    debug!("Search plugin '{}'", manifest.name);
                }
                PluginCapability::Unknown => {
                    // FIX: don't register plugins with Unknown capabilities
                    error!("Unknown capability in plugin '{}'", manifest.name);
                }
            }
        }

        for ext in &manifest.extensions {
            self.shlib_plugins
                .insert(ext.to_lowercase(), plugin.clone());
        }
    }
}
