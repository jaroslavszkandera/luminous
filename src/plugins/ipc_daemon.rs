use crate::plugins::Backend;
use crate::plugins::manifest::PluginManifest;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use shared_memory::{Shmem, ShmemConf};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Serialize, Debug)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum IpcCmd {
    // Ping,
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
    Search {
        paths: Vec<PathBuf>,
        query: String,
    },
    Shutdown,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "status", rename_all = "lowercase")]
pub(crate) enum IpcResponse {
    Ok,
    Busy,
    // SearchResult { paths: Vec<PathBuf> },
    Error { message: String },
}

#[derive(Deserialize)]
pub(crate) enum IpcSearchResponse {
    SearchResult { paths: Vec<PathBuf> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum IpcStatus {
    NotRunning,
    Init,
    Busy,
    Ready,
    Error,
}

pub(crate) struct ShmemWrapper(pub Shmem);

unsafe impl Send for ShmemWrapper {}
unsafe impl Sync for ShmemWrapper {}

pub(crate) struct ActiveShmem {
    #[allow(dead_code)]
    pub img: ShmemWrapper,
    pub mask: ShmemWrapper,
    pub width: u32,
    pub height: u32,
}

struct PendingImage {
    buffer: SharedPixelBuffer<Rgba8Pixel>,
    token: u32,
}

#[derive(Debug)]
enum WorkerRequest {
    ImagePending,
    Click {
        x: u32,
        y: u32,
        tx: mpsc::SyncSender<Option<SharedPixelBuffer<Rgba8Pixel>>>,
    },
    RectSelect {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        tx: mpsc::SyncSender<Option<SharedPixelBuffer<Rgba8Pixel>>>,
    },
    Search {
        paths: Vec<PathBuf>,
        query: String,
        tx: mpsc::SyncSender<Option<Vec<PathBuf>>>,
    },
    Shutdown,
}

pub struct DaemonBackend {
    id: String,
    manifest: PluginManifest,
    dir: PathBuf,
    process: Mutex<Option<Child>>,
    tx: SyncSender<WorkerRequest>,
    rx: Arc<Mutex<Option<Receiver<WorkerRequest>>>>,
    pending_image: Arc<Mutex<Option<PendingImage>>>,
    image_token: Arc<std::sync::atomic::AtomicU32>,
    status: Arc<RwLock<IpcStatus>>,
    on_status_change: Arc<Mutex<Option<Box<dyn Fn(IpcStatus) + Send + Sync>>>>,
    running: AtomicBool,
}

impl DaemonBackend {
    pub fn new(id: String, manifest: &PluginManifest, dir: &Path) -> Arc<Self> {
        let (tx, rx) = mpsc::sync_channel::<WorkerRequest>(1);

        let status = Arc::new(RwLock::new(IpcStatus::Init));
        let on_status_change: Arc<Mutex<Option<Box<dyn Fn(IpcStatus) + Send + Sync>>>> =
            Arc::new(Mutex::new(None));
        let pending_image: Arc<Mutex<Option<PendingImage>>> = Arc::new(Mutex::new(None));
        let image_token = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let daemon = Arc::new(Self {
            id: id,
            manifest: manifest.clone(),
            dir: dir.to_path_buf(),
            process: Mutex::new(None),
            tx: tx,
            rx: Arc::new(Mutex::new(Some(rx))),
            pending_image: pending_image.clone(),
            image_token: image_token.clone(),
            status: status.clone(),
            on_status_change: on_status_change.clone(),
            running: AtomicBool::new(false),
        });
        daemon
    }

    pub fn status(&self) -> IpcStatus {
        self.status.read().unwrap().clone()
    }

    pub fn on_status_change<F>(&self, cb: F)
    where
        F: Fn(IpcStatus) + Send + Sync + 'static,
    {
        *self.on_status_change.lock().unwrap() = Some(Box::new(cb));
    }
}

impl Backend for DaemonBackend {
    fn start(&self) {
        info!("Starting {} plugin...", self.id);
        if self.process.lock().unwrap().is_some() {
            error!(
                "Plugin already running, not starting {}",
                self.manifest.name
            );
            return;
        }
        let rx_mutex = self.rx.clone();
        let rx = {
            let Ok(mut rx_guard) = rx_mutex.lock() else {
                error!("Plugin channel mutex is already held");
                return;
            };
            let Some(rx) = rx_guard.take() else {
                debug!("Worker thread already running or rx already taken");
                return;
            };
            rx
        };

        let process = self.manifest.interpreter.as_ref().and_then(|interp| {
            let parts: Vec<&str> = interp.split_whitespace().collect();
            let (&exe, args) = parts.split_first()?;
            info!(
                "Starting daemon: {} {:?} {:?}",
                exe, args, self.manifest.entry
            );
            Command::new(exe)
                .args(args)
                .arg(
                    self.manifest
                        .entry
                        .as_ref()
                        .expect("Missing daemon entry should be handled by manifest parsing."),
                )
                .current_dir(self.dir.clone())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .ok()
        });
        if process.is_none() {
            error!("Failed to start daemon: {}", self.manifest.name)
        }
        *self.process.lock().unwrap() = process;
        self.running.store(true, Ordering::SeqCst);

        let port = self
            .manifest
            .daemon_port
            .expect("Missing daemon port should be handled by manifest parsing.");

        let status_w = self.status.clone();
        let on_status_w = self.on_status_change.clone();
        let pending_image = self.pending_image.clone();
        let image_token = self.image_token.clone();

        let thread_name = self.id.clone();
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let set_status = |s: IpcStatus| {
                    debug!("Status changing to: {:?}", s);
                    *status_w.write().unwrap() = s.clone();
                    if let Some(cb) = on_status_w.lock().unwrap().as_ref() {
                        cb(s);
                    }
                };

                let mut stream = match connect_with_retry(port, 30, 500) {
                    Some(s) => s,
                    None => {
                        error!("Failed to connect to daemon on port {port} after retries");
                        set_status(IpcStatus::Error);
                        *rx_mutex.lock().unwrap() = Some(rx);
                        return;
                    }
                };

                let mut active_shm: Option<ActiveShmem> = None;
                set_status(IpcStatus::Ready);

                while let Ok(req) = rx.recv() {
                    log::trace!("request received: {:#?}", req);
                    match req {
                        WorkerRequest::ImagePending => {
                            let Some(pending) = pending_image.lock().unwrap().take() else {
                                debug!("image lock taken");
                                continue;
                            };
                            let current = image_token.load(std::sync::atomic::Ordering::Acquire);
                            if pending.token < current {
                                debug!(
                                    "Skipping stale embedding (token {} < {})",
                                    pending.token, current
                                );
                                continue;
                            }
                            set_status(IpcStatus::Busy);
                            match ipc_send_image(&mut stream, &pending.buffer) {
                                Ok(shm) => {
                                    if pending.token
                                        == image_token.load(std::sync::atomic::Ordering::Acquire)
                                    {
                                        active_shm = Some(shm);
                                        set_status(IpcStatus::Ready);
                                    } else {
                                        debug!("Embedding done but image changed, discarding");
                                        set_status(IpcStatus::Busy);
                                    }
                                }
                                Err(e) => {
                                    error!("send_image failed: {e}");
                                    set_status(IpcStatus::Error);
                                    break;
                                }
                            }
                        }

                        WorkerRequest::Click { x, y, tx } => {
                            debug!("click ({x},{y})");
                            match &active_shm {
                                None => {
                                    warn!("Click ignored: no active embedding (image not set yet)");
                                    let _ = tx.send(None);
                                }
                                Some(shm) => match ipc_click(&mut stream, shm, x, y) {
                                    Ok(result) => {
                                        let _ = tx.send(result);
                                    }
                                    Err(e) => {
                                        error!("click failed: {e}");
                                        let _ = tx.send(None);
                                    }
                                },
                            }
                        }

                        WorkerRequest::RectSelect { x1, y1, x2, y2, tx } => {
                            debug!("rect_select ({x1},{y1})-({x2},{y2})");
                            match &active_shm {
                                None => {
                                    warn!("RectSelect ignored: no active embedding");
                                    let _ = tx.send(None);
                                }
                                Some(shm) => {
                                    match ipc_rect_select(&mut stream, shm, x1, y1, x2, y2) {
                                        Ok(result) => {
                                            let _ = tx.send(result);
                                        }
                                        Err(e) => {
                                            error!("rect_select failed: {e}");
                                            let _ = tx.send(None);
                                        }
                                    }
                                }
                            }
                        }
                        WorkerRequest::Search { paths, query, tx } => {
                            debug!("search ({query})"); // paths...
                            match ipc_search(&mut stream, paths, query) {
                                Ok(result) => {
                                    let _ = tx.send(result);
                                }
                                Err(e) => {
                                    error!("semantic image search failed: {e}");
                                    let _ = tx.send(None);
                                }
                            }
                        }
                        WorkerRequest::Shutdown => break,
                    }
                }

                let _ = send_msg(&mut stream, &IpcCmd::Shutdown);
                *rx_mutex.lock().unwrap() = Some(rx);
            })
            .expect("Failed to spawn worker thread");
    }

    fn stop(&self, timeout_ms: u64, wait: bool) {
        debug!("Stopping plugin: {}", self.manifest.name);

        let _ = self.tx.try_send(WorkerRequest::Shutdown);

        if let Some(mut child) = self.process.lock().unwrap().take() {
            let plugin_name = self.manifest.name.clone();

            let mut cleanup = move || {
                let timeout = std::time::Duration::from_millis(timeout_ms);
                let start = std::time::Instant::now();

                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            debug!("Plugin {} exited: {:?}", plugin_name, status);
                            break;
                        }
                        Ok(None) => {
                            if start.elapsed() >= timeout {
                                warn!("Timeout reached for {}. Force killing tree.", plugin_name);
                                kill_process_group(&child);
                                let _ = child.wait();
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(_) => {
                            kill_process_group(&child);
                            let _ = child.wait();
                            break;
                        }
                    }
                }
            };

            if wait {
                cleanup();
            } else {
                std::thread::spawn(cleanup);
            }
        }

        *self.status.write().unwrap() = IpcStatus::NotRunning;
        self.running.store(false, Ordering::SeqCst);
    }

    fn set_image(&self, buf: &SharedPixelBuffer<Rgba8Pixel>) -> bool {
        debug!("set_image inside ipc daemon");
        let token = self
            .image_token
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;

        *self.pending_image.lock().unwrap() = Some(PendingImage {
            buffer: buf.clone(),
            token,
        });

        match self.tx.try_send(WorkerRequest::ImagePending) {
            Ok(_) => {
                debug!("image pending request is successful");
                true
            }
            Err(mpsc::TrySendError::Full(_)) => {
                warn!("Worker queue full, image pending in mutex");
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                error!("IPC worker thread disconnected");
                false
            }
        }
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn click(&self, x: u32, y: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let status = self.status();
        if status == IpcStatus::Busy {
            warn!("Click ignored: daemon is busy");
            return None;
        }

        let (result_tx, result_rx) = mpsc::sync_channel(1);
        self.tx
            .try_send(WorkerRequest::Click {
                x,
                y,
                tx: result_tx,
            })
            .map_err(|e| warn!("click enqueue failed: {e}"))
            .ok()?;
        result_rx.recv().ok().flatten()
    }

    fn rect_select(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let status = self.status();
        if status == IpcStatus::Busy {
            warn!("Rectangle select ignored: daemon is busy");
            return None;
        }
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        self.tx
            .try_send(WorkerRequest::RectSelect {
                x1,
                y1,
                x2,
                y2,
                tx: result_tx,
            })
            .map_err(|e| warn!("rect_select enqueue failed: {e}"))
            .ok()?;
        result_rx.recv().ok().flatten()
    }

    fn semantic_image_search(
        &self,
        paths: &Vec<PathBuf>,
        query: &str,
    ) -> Option<Vec<std::path::PathBuf>> {
        if self.status() == IpcStatus::Busy {
            warn!("Semantic image search ignored: daemon is busy");
            return None;
        }
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        self.tx
            .try_send(WorkerRequest::Search {
                paths: paths.clone(),
                query: query.to_string(),
                tx: result_tx,
            })
            .map_err(|e| warn!("semantic_image_search enqueue failed: {e}"))
            .ok()?;
        result_rx.recv().ok().flatten()
    }

    fn on_status_change(&self, cb: Box<dyn Fn(IpcStatus) + Send + Sync>) {
        *self.on_status_change.lock().unwrap() = Some(cb);
    }
}

impl Drop for DaemonBackend {
    fn drop(&mut self) {
        self.stop(200, true);
    }
}

fn connect_with_retry(port: u16, attempts: u32, delay_ms: u64) -> Option<TcpStream> {
    for attempt in 0..attempts {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => {
                info!("Connected to daemon on port {port} (attempt {attempt})");
                return Some(s);
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(delay_ms)),
        }
    }
    None
}

pub(crate) fn send_msg(
    stream: &mut TcpStream,
    cmd: &IpcCmd,
) -> Result<(), Box<dyn std::error::Error>> {
    let payload = serde_json::to_vec(cmd)?;
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(&payload)?;
    Ok(())
}

pub(crate) fn recv_msg(stream: &mut TcpStream) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let mut payload = vec![0u8; u32::from_be_bytes(len_buf) as usize];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

fn ipc_send_image(
    stream: &mut TcpStream,
    buf: &SharedPixelBuffer<Rgba8Pixel>,
) -> Result<ActiveShmem, Box<dyn std::error::Error>> {
    let (w, h) = (buf.width(), buf.height());
    let img_mem = ShmemConf::new().size((w * h * 4) as usize).create()?;
    let mask_mem = ShmemConf::new().size((w * h) as usize).create()?;

    unsafe {
        std::ptr::copy_nonoverlapping(
            buf.as_slice().as_ptr() as *const u8,
            img_mem.as_ptr(),
            (w * h * 4) as usize,
        );
    }

    send_msg(
        stream,
        &IpcCmd::SetImage {
            shm_name: img_mem.get_os_id().into(),
            width: w,
            height: h,
        },
    )?;

    match serde_json::from_slice::<IpcResponse>(&recv_msg(stream)?)? {
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
    send_msg(
        stream,
        &IpcCmd::Click {
            shm_name: shm.mask.0.get_os_id().into(),
            x,
            y,
        },
    )?;
    read_mask_response(stream, shm)
}

fn ipc_rect_select(
    stream: &mut TcpStream,
    shm: &ActiveShmem,
    x1: u32,
    y1: u32,
    x2: u32,
    y2: u32,
) -> Result<Option<SharedPixelBuffer<Rgba8Pixel>>, Box<dyn std::error::Error>> {
    send_msg(
        stream,
        &IpcCmd::RectSelect {
            shm_name: shm.mask.0.get_os_id().into(),
            x1,
            y1,
            x2,
            y2,
        },
    )?;
    read_mask_response(stream, shm)
}

fn ipc_search(
    stream: &mut TcpStream,
    paths: Vec<PathBuf>,
    query: String,
) -> Result<Option<Vec<PathBuf>>, Box<dyn std::error::Error>> {
    send_msg(stream, &IpcCmd::Search { paths, query })?;
    let response = serde_json::from_slice::<IpcSearchResponse>(&recv_msg(stream)?)?;
    match response {
        IpcSearchResponse::SearchResult { paths } => Ok(Some(paths)),
    }
}

fn read_mask_response(
    stream: &mut TcpStream,
    shm: &ActiveShmem,
) -> Result<Option<SharedPixelBuffer<Rgba8Pixel>>, Box<dyn std::error::Error>> {
    match serde_json::from_slice::<IpcResponse>(&recv_msg(stream)?)? {
        IpcResponse::Ok => {
            let (w, h) = (shm.width, shm.height);
            let mask = unsafe { std::slice::from_raw_parts(shm.mask.0.as_ptr(), (w * h) as usize) };
            let rgba = mask_to_rgba_overlay(mask);
            Ok(Some(SharedPixelBuffer::clone_from_slice(&rgba, w, h)))
        }
        IpcResponse::Busy => {
            warn!("Daemon busy during prediction");
            Ok(None)
        }
        IpcResponse::Error { message } => Err(message.into()),
    }
}

pub(crate) fn mask_to_rgba_overlay(mask: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(mask.len() * 4);
    for &v in mask {
        if v > 0 {
            rgba.extend_from_slice(&[255u8, 0, 0, 128]);
        } else {
            rgba.extend_from_slice(&[0u8, 0, 0, 0]);
        }
    }
    rgba
}

fn kill_process_group(child: &std::process::Child) {
    let pid = child.id();
    debug!("Attempting to kill process tree for PID: {}", pid);

    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut procs_to_kill = Vec::new();

    for (p, proc) in sys.processes() {
        if let Some(parent_pid) = proc.parent() {
            if parent_pid.as_u32() == pid {
                procs_to_kill.push(*p);
            }
        }
    }

    for p in procs_to_kill {
        if let Some(proc) = sys.process(p) {
            debug!(
                "Killing sub-process: {} ({})",
                proc.name().to_string_lossy(),
                p
            );
            proc.kill();
        }
    }

    if let Some(proc) = sys.process(sysinfo::Pid::from_u32(pid)) {
        proc.kill();
    }
}
