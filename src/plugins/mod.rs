pub mod ipc_daemon;
pub mod manifest;
pub mod shared_lib;

use image::DynamicImage;
pub use ipc_daemon::{IpcStatus, PluginControl};
pub use manifest::{BackendKind, PluginCapability, PluginManifest, load_manifest};

use crate::fs_scan::{ImageFormat, ImageFormats};
use ipc_daemon::DaemonBackend;
use log::{debug, error, info};
use shared_lib::SharedLibBackend;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub trait Backend: Send + Sync {
    fn start(&self) {}
    fn stop(&self, _timeout_ms: u64, _wait: bool) {}
    fn is_running(&self) -> bool {
        false
    }
    fn decode(&self, _path: &Path) -> Option<DynamicImage> {
        None
    }
    fn encode(&self, _path: &Path, _buf: &DynamicImage) -> bool {
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
    fn text_to_mask(&self, _text: String) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        None
    }
    fn semantic_image_search(&self, _paths: &Vec<PathBuf>, _query: &str) -> Option<Vec<PathBuf>> {
        None
    }
    /// Callback invoked whenever the backend status changes
    fn on_status_change(&self, _cb: Box<dyn Fn(IpcStatus) + Send + Sync>) {}
    fn get_state(&self) -> PluginControl {
        PluginControl::Enable
    }
    /// Callback invoked whenever the backend state changes (Enable, Starting, Disable, Stopping)
    fn on_state_change(&self, _cb: Box<dyn Fn(PluginControl) + Send + Sync>) {}
}

pub struct Plugin {
    pub id: String,
    pub manifest: PluginManifest,
    backend: Box<dyn Backend>,
    image_format_support: RwLock<ImageFormat>,
}

impl Plugin {
    pub fn new(
        id: String,
        manifest: PluginManifest,
        dir: PathBuf,
        auto_start: bool,
        image_format_support: ImageFormat,
    ) -> Option<Self> {
        let backend: Box<dyn Backend> = match manifest.backend {
            BackendKind::Daemon => {
                Box::new(DaemonBackend::new(id.clone(), &manifest, &dir) as Arc<_>)
            }
            BackendKind::SharedLib => Box::new(SharedLibBackend::new(&manifest, &dir)?),
        };
        let plugin = Self {
            id,
            manifest,
            backend,
            image_format_support: RwLock::new(image_format_support),
        };
        if auto_start {
            debug!("Auto-starting plugin: {}", plugin.id);
            plugin.start();
        }
        Some(plugin)
    }

    pub fn start(&self) {
        self.backend.start();
    }

    pub fn stop(&self, timeout_ms: u64, wait: bool) {
        self.backend.stop(timeout_ms, wait);
    }

    pub fn is_running(&self) -> bool {
        self.backend.is_running()
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

    pub fn encode(&self, path: &Path, buf: &DynamicImage) -> bool {
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

    pub fn text_to_mask(&self, text: String) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        self.backend.text_to_mask(text)
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

    pub fn get_state(&self) -> PluginControl {
        self.backend.get_state()
    }

    pub fn on_state_change<F>(&self, cb: F)
    where
        F: Fn(PluginControl) + Send + Sync + 'static,
    {
        self.backend.on_state_change(Box::new(cb));
    }
}

impl Backend for Arc<DaemonBackend> {
    fn start(&self) {
        DaemonBackend::start(self);
    }
    fn stop(&self, timeout_ms: u64, wait: bool) {
        DaemonBackend::stop(self, timeout_ms, wait);
    }
    fn is_running(&self) -> bool {
        DaemonBackend::is_running(self)
    }
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
    fn text_to_mask(&self, text: String) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        DaemonBackend::text_to_mask(self, text)
    }
    fn semantic_image_search(&self, paths: &Vec<PathBuf>, query: &str) -> Option<Vec<PathBuf>> {
        DaemonBackend::semantic_image_search(self, paths, query)
    }
    fn on_status_change(&self, cb: Box<dyn Fn(IpcStatus) + Send + Sync>) {
        DaemonBackend::on_status_change(self, move |s| cb(s));
    }
    fn get_state(&self) -> PluginControl {
        DaemonBackend::get_state(&self)
    }
    fn on_state_change(&self, cb: Box<dyn Fn(PluginControl) + Send + Sync>) {
        DaemonBackend::on_state_change(self, move |s| cb(s));
    }
}

pub struct PluginManager {
    // TODO: merge plugins backends
    shlib_plugins: Vec<Arc<Plugin>>,
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
            shlib_plugins: Vec::new(),
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

        let mut settings = crate::ui::settings_presenter::read_settings()
            .unwrap_or_else(|| crate::ui::settings_presenter::Settings { plugins: vec![] });

        let mut discovered_ids = Vec::new();

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let id = match path.file_name().and_then(|n| n.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };

            discovered_ids.push(id.clone());

            let manifest_path = path.join("plugin.json");
            if !manifest_path.exists() {
                error!("Plugin manifest missing: {:?}", manifest_path);
                continue;
            }
            let auto_start = settings
                .plugins
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.auto_start)
                .unwrap_or(false);
            if let Some(manifest) = load_manifest(&manifest_path) {
                self.register(id, path, manifest, auto_start);
            }
        }
        settings.sync_plugins(discovered_ids);
        if let Err(e) = crate::ui::settings_presenter::write_settings(&settings) {
            error!("Failed to save plugins settings: {}", e);
        }
    }

    pub fn get_all_plugins(&self) -> Vec<Arc<Plugin>> {
        self.daemon_plugins.iter().cloned().collect()
    }

    pub fn get_plugin_by_id(&self, id: &str) -> Option<Arc<Plugin>> {
        self.daemon_plugins.iter().find(|&p| p.id == id).cloned()
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

    pub fn get_supported_extensions(&self) -> Vec<ImageFormat> {
        self.shlib_plugins
            .iter()
            .filter_map(|p| match p.image_format_support.read() {
                Ok(support) => Some(support.clone()),
                Err(_) => None,
            })
            .collect()
    }

    pub fn get_supprted_image_formats(&self) -> Vec<ImageFormats> {
        unimplemented!();
    }

    pub fn get_plugins_manifests(&self) -> Vec<PluginManifest> {
        self.daemon_plugins
            .iter()
            .map(|p| p.manifest.clone())
            .collect()
    }

    // pub fn has_decoding(&self, path: &Path) -> bool {
    pub fn has_plugin_for(&self, path: &Path) -> bool {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => return false,
        };

        self.shlib_plugins.iter().any(|p| {
            if let Ok(support) = p.image_format_support.read() {
                support.decoding_support && support.exts.contains(&ext)
            } else {
                false
            }
        })
    }

    pub fn has_encoding(&self, path: &Path) -> bool {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => return false,
        };

        self.shlib_plugins.iter().any(|p| {
            if let Ok(support) = p.image_format_support.read() {
                support.encoding_support && support.exts.contains(&ext)
            } else {
                false
            }
        })
    }

    pub fn decode(&self, path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let rgba = self.decode_dynamic(path)?.to_rgba8();
        Some(SharedPixelBuffer::clone_from_slice(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
        ))
    }

    pub fn encode(&self, path: &Path, buf: &DynamicImage) -> bool {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => {
                error!("Cannot encode: Path has no extension {:?}", path);
                return false;
            }
        };

        let plugin = self.shlib_plugins.iter().find(|p| {
            if let Ok(support) = p.image_format_support.read() {
                support.encoding_support && support.exts.contains(&ext)
            } else {
                false
            }
        });

        if let Some(p) = plugin {
            debug!("Encoding with plugin '{}' to {:?}", p.manifest.name, path);

            p.encode(path, buf)
        } else {
            error!("No encoding plugin found for extension: {}", ext);
            false
        }
    }

    pub fn decode_dynamic(&self, path: &Path) -> Option<image::DynamicImage> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        let plugin = self.shlib_plugins.iter().find(|p| {
            if let Ok(support) = p.image_format_support.read() {
                support.decoding_support && support.exts.contains(&ext)
            } else {
                false
            }
        })?;
        debug!("Using plugin '{}' for {:?}", plugin.manifest.name, path);
        plugin.decode_dynamic(path)
    }

    fn register(&mut self, id: String, dir: PathBuf, manifest: PluginManifest, auto_start: bool) {
        let plugin = match Plugin::new(
            id,
            manifest.clone(),
            dir,
            auto_start,
            ImageFormat {
                exts: vec![],
                decoding_support: false,
                encoding_support: false,
            },
        ) {
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

        if manifest.has_capability(&PluginCapability::Decoder)
            || manifest.has_capability(&PluginCapability::Encoder)
        {
            let can_decode = manifest.has_capability(&PluginCapability::Decoder);
            let can_encode = manifest.has_capability(&PluginCapability::Encoder);

            match plugin.image_format_support.write() {
                Ok(mut support) => {
                    *support = ImageFormat {
                        exts: manifest.extensions,
                        decoding_support: can_decode,
                        encoding_support: can_encode,
                    };
                }
                Err(e) => error!("Failed to acquire write lock: {}", e),
            }
            self.shlib_plugins.push(plugin.clone());
        }
    }
}
