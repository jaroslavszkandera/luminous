use core::option::Option;
use dlopen2::wrapper::{Container, WrapperApi};
use log::{debug, error, info, warn};
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
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, RwLock};

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

#[derive(Serialize, Debug)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum IpcCmd {
    SetImage {
        shm_name: String,
        width: u32,
        height: u32,
    },
    Click {
        shm_name: String,
        x: u32,
        y: u32,
    },
    RectSelect {
        shm_name: String,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum IpcResponse {
    Ok,
    Busy,
    Error { message: String },
}

#[derive(Clone, Debug)]
pub enum IpcStatus {
    Init,
    Busy,
    Ready,
    Error,
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

struct PendingImage {
    buffer: SharedPixelBuffer<Rgba8Pixel>,
    token: u32,
}

enum IpcRequest {
    Click {
        x: u32,
        y: u32,
        result_tx: mpsc::SyncSender<Option<SharedPixelBuffer<Rgba8Pixel>>>,
    },
    RectSelect {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        result_tx: mpsc::SyncSender<Option<SharedPixelBuffer<Rgba8Pixel>>>,
    },
    ImagePending,
}

pub struct InteractiveDaemon {
    pub manifest_name: String,
    process: Mutex<Option<Child>>,
    tx: SyncSender<IpcRequest>,
    pending_image: Arc<Mutex<Option<PendingImage>>>,
    image_token: Arc<std::sync::atomic::AtomicU32>,
    status: Arc<RwLock<IpcStatus>>,
    on_status_change: Arc<Mutex<Option<Box<dyn Fn(IpcStatus) + Send + Sync>>>>,
}

impl InteractiveDaemon {
    pub fn new(manifest: &PluginManifest, dir_path: &Path) -> Arc<Self> {
        let port = manifest.daemon_port.unwrap_or(50051);

        let (tx, rx) = mpsc::sync_channel::<IpcRequest>(1);

        let status = Arc::new(RwLock::new(IpcStatus::Init));
        let on_status_change: Arc<Mutex<Option<Box<dyn Fn(IpcStatus) + Send + Sync>>>> =
            Arc::new(Mutex::new(None));
        let pending_image: Arc<Mutex<Option<PendingImage>>> = Arc::new(Mutex::new(None));
        let image_token = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let mut process = None;
        if let Some(interpreter) = &manifest.interpreter {
            let parts: Vec<&str> = interpreter.split_whitespace().collect();
            if let Some((&exe, args)) = parts.split_first() {
                let script = "main.py";
                info!("Starting daemon: {} {:?} {}", exe, args, script);
                process = Command::new(exe)
                    .args(args)
                    .arg(script)
                    .current_dir(dir_path)
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .ok();
            }
        }

        let daemon = Arc::new(Self {
            manifest_name: manifest.name.clone(),
            process: Mutex::new(process),
            tx,
            pending_image: pending_image.clone(),
            image_token: image_token.clone(),
            status: status.clone(),
            on_status_change: on_status_change.clone(),
        });

        let status_thread = status.clone();
        let on_status_thread = on_status_change.clone();
        std::thread::spawn(move || {
            let set_status = |s: IpcStatus| {
                *status_thread.write().unwrap() = s.clone();
                if let Some(cb) = on_status_thread.lock().unwrap().as_ref() {
                    cb(s);
                }
            };

            let mut stream = None;
            for _ in 0..20 {
                match TcpStream::connect(("127.0.0.1", port)) {
                    Ok(s) => {
                        info!("Connected to daemon on port {port}");
                        stream = Some(s);
                        break;
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(500)),
                }
            }
            let mut stream = match stream {
                Some(s) => s,
                None => {
                    error!("Failed to connect to daemon after 10s");
                    set_status(IpcStatus::Error);
                    return;
                }
            };

            debug!("Setting initial image");
            if let Some(pending) = pending_image.lock().unwrap().take() {
                set_status(IpcStatus::Busy);
                let current_token = image_token.load(std::sync::atomic::Ordering::Acquire);
                if pending.token == current_token {
                    match Self::ipc_send_image(&mut stream, &pending.buffer) {
                        Ok(_shm) => {
                            debug!("Initial image set, ready!");
                            set_status(IpcStatus::Ready)
                        }
                        Err(e) => {
                            error!("Initial send_image failed: {e}");
                            set_status(IpcStatus::Error);
                        }
                    }
                } else {
                    debug!(
                        "Initial pending image superseded (token {} vs {}), skipping",
                        pending.token, current_token
                    );
                    set_status(IpcStatus::Ready);
                }
            } else {
                error!("Initial image is not present, not setting");
            }

            let mut active_shm: Option<ActiveShmem> = None;

            while let Ok(req) = rx.recv() {
                match req {
                    IpcRequest::ImagePending => {
                        let Some(pending) = pending_image.lock().unwrap().take() else {
                            continue;
                        };
                        let current_token = image_token.load(std::sync::atomic::Ordering::Acquire);
                        if pending.token < current_token {
                            debug!(
                                "Skipping stale embedding (token {} < {})",
                                pending.token, current_token
                            );
                            continue;
                        }
                        set_status(IpcStatus::Busy);
                        match Self::ipc_send_image(&mut stream, &pending.buffer) {
                            Ok(shm) => {
                                if pending.token
                                    == image_token.load(std::sync::atomic::Ordering::Acquire)
                                {
                                    active_shm = Some(shm);
                                    set_status(IpcStatus::Ready);
                                } else {
                                    debug!("Embedding finished but image changed, discarding SHM");
                                    set_status(IpcStatus::Busy);
                                }
                            }
                            Err(e) => {
                                error!("send_image failed: {e}");
                                set_status(IpcStatus::Error);
                            }
                        }
                    }

                    IpcRequest::Click { x, y, result_tx } => {
                        let result = active_shm.as_ref().and_then(|shm| {
                            Self::ipc_click(&mut stream, shm, x, y)
                                .map_err(|e| error!("click failed: {e}"))
                                .ok()
                                .flatten()
                        });
                        let _ = result_tx.send(result);
                    }

                    IpcRequest::RectSelect {
                        x1,
                        y1,
                        x2,
                        y2,
                        result_tx,
                    } => {
                        let result = active_shm.as_ref().and_then(|shm| {
                            Self::ipc_rect_select(&mut stream, shm, x1, y1, x2, y2)
                                .map_err(|e| error!("rect_select failed: {e}"))
                                .ok()
                                .flatten()
                        });
                        let _ = result_tx.send(result);
                    }
                }
            }
        });

        daemon
    }

    pub fn on_status_change<F>(&self, callback: F)
    where
        F: Fn(IpcStatus) + Send + Sync + 'static,
    {
        *self.on_status_change.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn status(&self) -> IpcStatus {
        self.status.read().unwrap().clone()
    }

    pub fn set_interactive_image(&self, buffer: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        let token = self
            .image_token
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        *self.pending_image.lock().unwrap() = Some(PendingImage {
            buffer: buffer.clone(),
            token,
        });

        match self.tx.try_send(IpcRequest::ImagePending) {
            Ok(_) | Err(mpsc::TrySendError::Full(_)) => true,
            Err(mpsc::TrySendError::Disconnected(_)) => {
                error!("IPC thread disconnected");
                false
            }
        }
    }

    pub fn interactive_click(&self, x: u32, y: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        self.tx
            .try_send(IpcRequest::Click { x, y, result_tx })
            .map_err(|e| warn!("click enqueue failed: {e}"))
            .ok()?;
        result_rx.recv().ok().flatten()
    }

    pub fn interactive_rect_select(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        self.tx
            .try_send(IpcRequest::RectSelect {
                x1,
                y1,
                x2,
                y2,
                result_tx,
            })
            .map_err(|e| warn!("rect_select enqueue failed: {e}"))
            .ok()?;
        result_rx.recv().ok().flatten()
    }

    fn send_msg(stream: &mut TcpStream, cmd: &IpcCmd) -> Result<(), Box<dyn std::error::Error>> {
        debug!("IPC send: {:?}", cmd);
        let payload = serde_json::to_vec(cmd)?;
        stream.write_all(&(payload.len() as u32).to_be_bytes())?;
        stream.write_all(&payload)?;
        Ok(())
    }

    fn recv_msg(stream: &mut TcpStream) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf)?;
        let mut payload = vec![0u8; u32::from_be_bytes(len_buf) as usize];
        stream.read_exact(&mut payload)?;
        Ok(payload)
    }

    fn ipc_send_image(
        stream: &mut TcpStream,
        buffer: &SharedPixelBuffer<Rgba8Pixel>,
    ) -> Result<ActiveShmem, Box<dyn std::error::Error>> {
        let (w, h) = (buffer.width(), buffer.height());
        let img_mem = ShmemConf::new().size((w * h * 4) as usize).create()?;
        let mask_mem = ShmemConf::new().size((w * h) as usize).create()?;

        unsafe {
            std::ptr::copy_nonoverlapping(
                buffer.as_slice().as_ptr() as *const u8,
                img_mem.as_ptr(),
                (w * h * 4) as usize,
            );
        }

        Self::send_msg(
            stream,
            &IpcCmd::SetImage {
                shm_name: img_mem.get_os_id().into(),
                width: w,
                height: h,
            },
        )?;

        match serde_json::from_slice::<IpcResponse>(&Self::recv_msg(stream)?)? {
            IpcResponse::Ok => Ok(ActiveShmem {
                img: ShmemWrapper(img_mem),
                mask: ShmemWrapper(mask_mem),
                width: w,
                height: h,
            }),
            IpcResponse::Busy => Err("daemon busy".into()),
            IpcResponse::Error { message } => Err(message.into()),
        }
    }

    fn ipc_click(
        stream: &mut TcpStream,
        shm: &ActiveShmem,
        x: u32,
        y: u32,
    ) -> Result<Option<SharedPixelBuffer<Rgba8Pixel>>, Box<dyn std::error::Error>> {
        Self::send_msg(
            stream,
            &IpcCmd::Click {
                shm_name: shm.mask.0.get_os_id().into(),
                x,
                y,
            },
        )?;
        Self::read_mask_response(stream, shm)
    }

    fn ipc_rect_select(
        stream: &mut TcpStream,
        shm: &ActiveShmem,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    ) -> Result<Option<SharedPixelBuffer<Rgba8Pixel>>, Box<dyn std::error::Error>> {
        Self::send_msg(
            stream,
            &IpcCmd::RectSelect {
                shm_name: shm.mask.0.get_os_id().into(),
                x1,
                y1,
                x2,
                y2,
            },
        )?;
        Self::read_mask_response(stream, shm)
    }

    fn read_mask_response(
        stream: &mut TcpStream,
        shm: &ActiveShmem,
    ) -> Result<Option<SharedPixelBuffer<Rgba8Pixel>>, Box<dyn std::error::Error>> {
        match serde_json::from_slice::<IpcResponse>(&Self::recv_msg(stream)?)? {
            IpcResponse::Ok => {
                let (w, h) = (shm.width, shm.height);
                let mask_data =
                    unsafe { std::slice::from_raw_parts(shm.mask.0.as_ptr(), (w * h) as usize) };
                let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                for &v in mask_data {
                    if v > 0 {
                        rgba.extend_from_slice(&[255u8, 0, 0, 128]);
                    } else {
                        rgba.extend_from_slice(&[0u8, 0, 0, 0]);
                    }
                }
                Ok(Some(SharedPixelBuffer::clone_from_slice(&rgba, w, h)))
            }
            IpcResponse::Busy => {
                warn!("Daemon busy during prediction");
                Ok(None)
            }
            IpcResponse::Error { message } => Err(message.into()),
        }
    }
}

impl Drop for InteractiveDaemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.process.lock().unwrap().take() {
            info!("Killing daemon '{}'", self.manifest_name);
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
    pub fn decode_dynamic(&self, path: &Path) -> Option<image::DynamicImage> {
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
                error!("Received null FFI buffer");
                return None;
            }

            let pixel_slice =
                unsafe { std::slice::from_raw_parts(ffi_buffer.data, ffi_buffer.len) };

            let (l, w, h, c) = (
                ffi_buffer.len,
                ffi_buffer.width,
                ffi_buffer.height,
                ffi_buffer.channels,
            );
            debug!("pixel_slice loaded: len={l}, {w}x{h}x{c}");
            let dim_len = (w * h * c) as usize;
            if l != dim_len {
                error!("Image size and dimensions mismatch ({l} != {dim_len})");
                return None;
            }
            let img = match ffi_buffer.channels {
                1 => image::ImageBuffer::<image::Luma<u8>, _>::from_raw(w, h, pixel_slice.to_vec())
                    .map(image::DynamicImage::ImageLuma8),
                3 => image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(w, h, pixel_slice.to_vec())
                    .map(image::DynamicImage::ImageRgb8),
                4 => image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(w, h, pixel_slice.to_vec())
                    .map(image::DynamicImage::ImageRgba8),
                _ => None,
            };

            unsafe { container.free_image(ffi_buffer) };
            return img;
        } else {
            error!("PluginBackend is not of type SharedLibrary");
            None
        }
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
    /// Non-blocking; returns false if the daemon is already processing an image.
    pub fn set_interactive_image(&self, buffer: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        match &self.backend {
            PluginBackend::Daemon(d) => d.set_interactive_image(buffer),
            _ => false,
        }
    }

    pub fn interactive_click(&self, x: u32, y: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        match &self.backend {
            PluginBackend::Daemon(d) => d.interactive_click(x, y),
            _ => None,
        }
    }

    pub fn interactive_rect_select(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        match &self.backend {
            PluginBackend::Daemon(d) => d.interactive_rect_select(x1, y1, x2, y2),
            _ => None,
        }
    }

    pub fn on_status_change<F>(&self, callback: F)
    where
        F: Fn(IpcStatus) + Send + Sync + 'static,
    {
        if let PluginBackend::Daemon(d) = &self.backend {
            d.on_status_change(callback);
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
        let rgba = self.decode_dynamic(path)?.to_rgba8();
        Some(SharedPixelBuffer::clone_from_slice(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
        ))
    }

    pub fn decode_dynamic(&self, path: &Path) -> Option<image::DynamicImage> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        if let Some(plugin) = self.plugins.get(&ext) {
            debug!("Using plugin '{}' for {:?}", plugin.manifest.name, path);
            plugin.decode_dynamic(path)
        } else {
            // NOTE: Too much log...
            // error!("No plugins that have support for file extension {:?}", path);
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
