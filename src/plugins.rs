use dlopen2::wrapper::{Container, WrapperApi};
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use shared_memory::{Shmem, ShmemConf};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

// Expected ABI for shared libs
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
// NOTE: Could be split into decode/encode?
#[derive(WrapperApi)]
pub struct ImagePluginApi {
    // pub is to suppress warning for now
    load_image: unsafe extern "C" fn(path: *const i8) -> ImageBuffer,
    save_image: unsafe extern "C" fn(path: *const i8, img: ImageBuffer) -> bool,
    free_image: unsafe extern "C" fn(img: ImageBuffer),
    get_plugin_info: unsafe extern "C" fn(name: *mut i8, n_max: i32, exts: *mut i8, e_max: i32),
}

#[derive(Deserialize, Debug, Clone)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default = "default_plugin_type")]
    pub backend: String,
    pub extensions: Vec<String>,
    pub capabilities: Vec<PluginCapability>,
    pub daemon_port: Option<u16>,
    pub interpreter: Option<String>,
}

fn default_plugin_type() -> String {
    "shared_lib".to_string()
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PluginCapability {
    Decoder,
    Encoder,
    Interactive,
    Unknown,
}

#[derive(Serialize)]
struct IpcCmd<'a> {
    action: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    shm_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    y: Option<u32>,
}

// Seems so sketchy, is there better way to do this?
pub struct ShmemWrapper(pub Shmem);
unsafe impl Send for ShmemWrapper {}
unsafe impl Sync for ShmemWrapper {}

pub struct ActiveShmem {
    pub img: ShmemWrapper,
    pub mask: ShmemWrapper,
    pub width: u32,
    pub height: u32,
}

pub struct InteractiveDaemon {
    pub manifest_name: String,
    stream: Arc<Mutex<Option<TcpStream>>>,
    process: Mutex<Option<Child>>,
    active_shm: Arc<Mutex<Option<ActiveShmem>>>,
    pending_image: Arc<Mutex<Option<SharedPixelBuffer<Rgba8Pixel>>>>,
}

impl InteractiveDaemon {
    pub fn new(manifest: &PluginManifest, dir_path: &Path) -> Arc<Self> {
        let port = manifest.daemon_port.unwrap_or(50051);
        let mut process = None;

        if let Some(interpreter) = &manifest.interpreter {
            let parts: Vec<&str> = interpreter.split_whitespace().collect();
            if let Some((&cmd_exe, cmd_args)) = parts.split_first() {
                let script_name = "main.py"; // tmp
                info!(
                    "Starting daemon process: {} {:?} {}",
                    cmd_exe, cmd_args, script_name
                );

                process = Command::new(cmd_exe)
                    .args(cmd_args)
                    .arg(script_name)
                    .current_dir(dir_path)
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .ok();
            }
        }

        let daemon = Arc::new(Self {
            manifest_name: manifest.name.clone(),
            stream: Arc::new(Mutex::new(None)),
            process: Mutex::new(process),
            active_shm: Arc::new(Mutex::new(None)),
            pending_image: Arc::new(Mutex::new(None)),
        });

        let stream_clone = daemon.stream.clone();
        let pending_clone = daemon.pending_image.clone();
        let active_shm_clone = daemon.active_shm.clone();

        std::thread::spawn(move || {
            for _ in 0..20 {
                if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
                    info!("Successfully connected to daemon on port {}", port);
                    if let Some(img) = pending_clone.lock().unwrap().take() {
                        let _ = Self::send_image(&mut s, &active_shm_clone, &img);
                    }
                    *stream_clone.lock().unwrap() = Some(s);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            error!("Failed to connect to daemon after 10 seconds.");
        });

        daemon
    }

    fn send_image(
        stream: &mut TcpStream,
        active_shm: &Mutex<Option<ActiveShmem>>,
        buffer: &SharedPixelBuffer<Rgba8Pixel>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let width = buffer.width();
        let height = buffer.height();
        let size = (width * height * 4) as usize;
        let mask_size = (width * height) as usize;

        let img_mem = ShmemConf::new().size(size).create()?;
        let mask_mem = ShmemConf::new().size(mask_size).create()?;

        unsafe {
            std::ptr::copy_nonoverlapping(
                buffer.as_slice().as_ptr() as *const u8,
                img_mem.as_ptr(),
                size,
            );
        }

        let cmd = IpcCmd {
            action: "set_image",
            shm_name: Some(img_mem.get_os_id()),
            width: Some(width),
            height: Some(height),
            x: None,
            y: None,
        };

        let mut payload = serde_json::to_string(&cmd)?;
        payload.push('\n');
        stream.write_all(payload.as_bytes())?;

        let mut ack = [0u8; 2];
        stream.read_exact(&mut ack)?;

        *active_shm.lock().unwrap() = Some(ActiveShmem {
            img: ShmemWrapper(img_mem),
            mask: ShmemWrapper(mask_mem),
            width,
            height,
        });

        Ok(())
    }

    pub fn set_interactive_image(&self, buffer: &SharedPixelBuffer<Rgba8Pixel>) {
        let mut stream_guard = self.stream.lock().unwrap();
        if let Some(s) = stream_guard.as_mut() {
            if let Err(e) = Self::send_image(s, &self.active_shm, buffer) {
                error!("Daemon image sync failed: {}", e);
            }
        } else {
            *self.pending_image.lock().unwrap() = Some(buffer.clone());
        }
    }

    // FIX: communication will stop working if click request is send when image is being sent
    pub fn interactive_click(&self, x: u32, y: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        debug!("Interative click requested: [{},{}]", x, y);
        let shm_guard = self.active_shm.lock().unwrap();
        let active = shm_guard.as_ref().or_else(|| {
            error!("Click received but no active image SHM found in daemon");
            None
        })?;

        let cmd = IpcCmd {
            action: "click",
            shm_name: Some(active.mask.0.get_os_id()),
            width: None,
            height: None,
            x: Some(x),
            y: Some(y),
        };

        let mut stream_guard = self.stream.lock().unwrap();
        let s = stream_guard.as_mut().or_else(|| {
            error!("Daemon stream not connected");
            None
        })?;

        let mut payload = serde_json::to_string(&cmd).ok()?;
        payload.push('\n');
        if let Err(e) = s.write_all(payload.as_bytes()) {
            error!("Failed to send click to daemon: {}", e);
            return None;
        }

        let mut ack = [0u8; 2];
        if let Err(e) = s.read_exact(&mut ack) {
            error!("Daemon failed to ACK click: {}", e);
            return None;
        }

        let w = active.width;
        let h = active.height;
        let size = (w * h) as usize;
        let mask_data = unsafe { std::slice::from_raw_parts(active.mask.0.as_ptr(), size) };

        let mut raw_bytes = Vec::with_capacity(size * 4);
        for &val in mask_data.iter() {
            if val > 0 {
                raw_bytes.extend_from_slice(&[255, 0, 0, 128]);
            } else {
                raw_bytes.extend_from_slice(&[0, 0, 0, 0]);
            }
        }

        Some(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            &raw_bytes, w, h,
        ))
    }
}

impl Drop for InteractiveDaemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.process.lock().unwrap().take() {
            info!("Shutting down daemon process for '{}'", self.manifest_name);
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub enum PluginBackend {
    SharedLib(Container<ImagePluginApi>),
    Daemon(Arc<InteractiveDaemon>),
}

pub struct Plugin {
    manifest: PluginManifest,
    backend: PluginBackend,
}

impl Plugin {
    pub fn new(manifest: PluginManifest, dir_path: PathBuf) -> Option<Self> {
        if manifest.backend == "daemon" {
            // Plugin through IPC and shared memory
            Some(Self {
                manifest: manifest.clone(),
                backend: PluginBackend::Daemon(InteractiveDaemon::new(&manifest, &dir_path)),
            })
        } else {
            // Shared library plugin
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
                backend: PluginBackend::SharedLib(container),
            })
        }
    }

    fn eval_version(&self) -> bool {
        self.manifest.version == env!("CARGO_PKG_VERSION")
    }

    // --- Shared library methods
    pub fn decode(&self, path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        if !self
            .manifest
            .capabilities
            .contains(&PluginCapability::Decoder)
        {
            error!("Plugin '{}' does not support decoding", self.manifest.name);
            return None;
        }

        if let PluginBackend::SharedLib(container) = &self.backend {
            let c_path = CString::new(path.to_str()?).ok()?;
            let ffi_buffer = unsafe { container.load_image(c_path.as_ptr()) };

            if ffi_buffer.data.is_null() {
                return None;
            }

            let pixel_slice =
                unsafe { std::slice::from_raw_parts(ffi_buffer.data, ffi_buffer.len) };
            let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                pixel_slice,
                ffi_buffer.width,
                ffi_buffer.height,
            );

            unsafe { container.free_image(ffi_buffer) };
            return Some(buffer);
        }
        None
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

        if let PluginBackend::SharedLib(container) = &self.backend {
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
            unsafe { container.save_image(c_path.as_ptr(), ffi_buffer) };
            return true;
        }
        false
    }

    pub fn get_info(&self) -> (String, String) {
        if let PluginBackend::SharedLib(container) = &self.backend {
            let mut name = vec![0u8; 256];
            let mut exts = vec![0u8; 256];
            unsafe {
                container.get_plugin_info(
                    name.as_mut_ptr() as *mut i8,
                    256,
                    exts.as_mut_ptr() as *mut i8,
                    256,
                );
            }
            return (
                String::from_utf8_lossy(&name)
                    .trim_matches(char::from(0))
                    .to_string(),
                String::from_utf8_lossy(&exts)
                    .trim_matches(char::from(0))
                    .to_string(),
            );
        }
        (String::from(""), String::from(""))
    }
    // --- Shared library methods

    // --- IPC Methods ---
    pub fn set_interactive_image(&self, buffer: &SharedPixelBuffer<Rgba8Pixel>) {
        if let PluginBackend::Daemon(daemon) = &self.backend {
            daemon.set_interactive_image(buffer);
        }
    }

    pub fn interactive_click(&self, x: u32, y: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        if let PluginBackend::Daemon(daemon) = &self.backend {
            daemon.interactive_click(x, y)
        } else {
            None
        }
    }
    // --- IPC Methods ---
}

pub struct PluginManager {
    /// extension -> Plugin
    plugins: HashMap<String, Arc<Plugin>>,
    interactive_plugins: Vec<Arc<Plugin>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            interactive_plugins: Vec::new(),
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

    pub fn get_interactive_plugin(&self) -> Option<Arc<Plugin>> {
        self.interactive_plugins.first().cloned()
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
                PluginCapability::Interactive => {
                    self.interactive_plugins.push(plugin.clone());
                    debug!("Added interactive plugin \"{}\"", manifest.name);
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
