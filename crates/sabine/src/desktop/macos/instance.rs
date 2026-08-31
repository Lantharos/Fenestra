use std::{
    env, fs,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    net::Shutdown,
    os::fd::AsRawFd,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use sabine_platform::{PlatformEvent, SingleInstanceActivation, SingleInstancePolicy};

use super::{EventQueue, helpers::sanitize_id};

const INSTANCE_IO_TIMEOUT: Duration = Duration::from_secs(2);
const INSTANCE_STARTUP_TIMEOUT: Duration = Duration::from_millis(750);
const INSTANCE_RETRY_INTERVAL: Duration = Duration::from_millis(25);
const MAX_ACTIVATION_BYTES: u64 = 1024 * 1024;
const INSTANCE_WORKERS: usize = 4;
const MAX_PENDING_CONNECTIONS: usize = 32;

pub(super) struct SingleInstanceGuard {
    _lock: File,
    socket_path: PathBuf,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SingleInstanceGuard {
    pub(super) fn acquire(
        instance_id: Option<&str>,
        policy: SingleInstancePolicy,
        events: EventQueue,
    ) -> Result<Self, String> {
        let socket_path = single_instance_socket_path(instance_id)?;
        if let Some(parent) = socket_path.parent() {
            prepare_socket_directory(parent).map_err(|error| error.to_string())?;
        }
        let lock = acquire_instance_lock(&socket_path.with_extension("lock"))
            .map_err(|error| error.to_string())?;
        if !try_lock_instance(&lock).map_err(|error| error.to_string())? {
            notify_existing_instance(&socket_path).map_err(|error| error.to_string())?;
            return Err(crate::desktop::INSTANCE_ALREADY_RUNNING.to_string());
        }
        remove_stale_socket(&socket_path).map_err(|error| error.to_string())?;
        let listener = bind_listener(&socket_path).map_err(|error| error.to_string())?;
        Ok(spawn_single_instance_listener(
            lock,
            socket_path,
            listener,
            policy,
            events,
        ))
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn spawn_single_instance_listener(
    lock: File,
    socket_path: PathBuf,
    listener: UnixListener,
    policy: SingleInstancePolicy,
    events: EventQueue,
) -> SingleInstanceGuard {
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);
    let thread = thread::spawn(move || {
        let (connections, pending) =
            crossbeam_channel::bounded::<UnixStream>(MAX_PENDING_CONNECTIONS);
        let workers = (0..INSTANCE_WORKERS)
            .map(|_| {
                let pending = pending.clone();
                let events = events.clone();
                thread::spawn(move || {
                    while let Ok(stream) = pending.recv() {
                        if let Some(activation) = read_single_instance_activation(policy, stream) {
                            let _ = events.send(PlatformEvent::SingleInstance(activation));
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        while let Ok((stream, _)) = listener.accept() {
            if !thread_running.load(Ordering::Relaxed) {
                break;
            }
            if connections.send(stream).is_err() {
                break;
            }
        }
        drop(connections);
        for worker in workers {
            let _ = worker.join();
        }
    });
    SingleInstanceGuard {
        _lock: lock,
        socket_path,
        running,
        thread: Some(thread),
    }
}

fn read_single_instance_activation(
    policy: SingleInstancePolicy,
    mut stream: UnixStream,
) -> Option<SingleInstanceActivation> {
    crate::osr::transport::authenticate_peer(&stream).ok()?;
    stream.set_read_timeout(Some(INSTANCE_IO_TIMEOUT)).ok()?;
    let mut body = Vec::new();
    Read::by_ref(&mut stream)
        .take(MAX_ACTIVATION_BYTES + 1)
        .read_to_end(&mut body)
        .ok()?;
    if body.len() as u64 > MAX_ACTIVATION_BYTES {
        return None;
    }
    activation_from_json(policy, &body)
}

fn activation_from_json(
    policy: SingleInstancePolicy,
    body: &[u8],
) -> Option<SingleInstanceActivation> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let arguments = value
        .get("arguments")
        .and_then(|value| value.as_array())?
        .iter()
        .map(|value| value.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    let mut activation = SingleInstanceActivation::new(policy, arguments);
    if let Some(cwd) = value.get("cwd").and_then(|value| value.as_str()) {
        activation = activation.working_directory(cwd);
    }
    if let Some(token) = value
        .get("activationToken")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        activation = activation.activation_token(token);
    }
    Some(activation)
}

fn send_single_instance_activation(socket_path: &Path) -> io::Result<()> {
    let body = serde_json::to_vec(&serde_json::json!({
        "arguments": env::args().collect::<Vec<_>>(),
        "cwd": env::current_dir().ok().map(|path| path.display().to_string()),
        "activationToken": startup_activation_token(),
    }))
    .map_err(io::Error::other)?;
    if body.len() as u64 > MAX_ACTIVATION_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "single-instance activation is too large",
        ));
    }
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_write_timeout(Some(INSTANCE_IO_TIMEOUT))?;
    stream.write_all(&body)?;
    stream.shutdown(Shutdown::Write)
}

fn notify_existing_instance(socket_path: &Path) -> io::Result<()> {
    let deadline = std::time::Instant::now() + INSTANCE_STARTUP_TIMEOUT;
    loop {
        match send_single_instance_activation(socket_path) {
            Ok(()) => return Ok(()),
            Err(_) if std::time::Instant::now() < deadline => {
                thread::sleep(INSTANCE_RETRY_INTERVAL);
            }
            Err(error) => return Err(error),
        }
    }
}

fn startup_activation_token() -> Option<String> {
    env::var("XDG_ACTIVATION_TOKEN")
        .or_else(|_| env::var("DESKTOP_STARTUP_ID"))
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn single_instance_socket_path(instance_id: Option<&str>) -> Result<PathBuf, String> {
    let runtime = match env::var_os("XDG_RUNTIME_DIR") {
        Some(path) => PathBuf::from(path).join("sabine"),
        None => super::helpers::home_dir()?
            .join("Library")
            .join("Caches")
            .join("sabine"),
    };
    let id = instance_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(sanitize_id)
        .map(Ok)
        .unwrap_or_else(|| {
            env::current_exe().map(|exe| {
                exe.file_stem()
                    .and_then(|name| name.to_str())
                    .map(sanitize_id)
                    .unwrap_or_else(|| "app".to_string())
            })
        })
        .map_err(|error| error.to_string())?;
    Ok(runtime.join(format!("{id}.sock")))
}

fn prepare_socket_directory(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "single-instance directory must not be a symlink",
        ));
    }
    fs::create_dir_all(path)?;
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "single-instance directory is not owned by this user",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn bind_listener(path: &Path) -> io::Result<UnixListener> {
    let listener = UnixListener::bind(path)?;
    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        drop(listener);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(listener)
}

fn remove_stale_socket(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing to replace an untrusted single-instance endpoint",
        ));
    }
    fs::remove_file(path)
}

fn acquire_instance_lock(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "single-instance lock is not an owned regular file",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

fn try_lock_instance(file: &File) -> io::Result<bool> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(error)
    }
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}
