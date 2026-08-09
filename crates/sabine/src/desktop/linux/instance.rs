use std::{
    env, fs, io,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use sabine_platform::{PlatformEvent, SingleInstanceActivation, SingleInstancePolicy};

use super::EventQueue;
use super::util::sanitize_desktop_id;

pub(super) struct SingleInstanceGuard {
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
        let socket_path =
            single_instance_socket_path(instance_id).map_err(|error| error.to_string())?;
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        match UnixListener::bind(&socket_path) {
            Ok(listener) => Ok(spawn_single_instance_listener(
                socket_path,
                listener,
                policy,
                events,
            )),
            Err(_) if send_single_instance_activation(&socket_path, policy).is_ok() => {
                Err(crate::desktop::INSTANCE_ALREADY_RUNNING.to_string())
            }
            Err(_) => {
                let _ = fs::remove_file(&socket_path);
                let listener =
                    UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
                Ok(spawn_single_instance_listener(
                    socket_path,
                    listener,
                    policy,
                    events,
                ))
            }
        }
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

pub(super) fn spawn_single_instance_listener(
    socket_path: PathBuf,
    listener: UnixListener,
    policy: SingleInstancePolicy,
    events: EventQueue,
) -> SingleInstanceGuard {
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);
    let thread = thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if !thread_running.load(Ordering::Relaxed) {
                break;
            }
            if let Some(activation) = read_single_instance_activation(policy, stream) {
                let _ = events.send(PlatformEvent::SingleInstance(activation));
            }
        }
    });
    SingleInstanceGuard {
        socket_path,
        running,
        thread: Some(thread),
    }
}

pub(super) fn read_single_instance_activation(
    policy: SingleInstancePolicy,
    mut stream: UnixStream,
) -> Option<SingleInstanceActivation> {
    let mut body = String::new();
    stream.read_to_string(&mut body).ok()?;
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    let arguments = value
        .get("arguments")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
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

pub(super) fn send_single_instance_activation(
    socket_path: &PathBuf,
    policy: SingleInstancePolicy,
) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket_path)?;
    let cwd = env::current_dir()
        .ok()
        .map(|path| path.display().to_string());
    let body = serde_json::json!({
        "policy": format!("{policy:?}"),
        "arguments": env::args().collect::<Vec<_>>(),
        "cwd": cwd,
        "activationToken": startup_activation_token(),
    });
    stream.write_all(body.to_string().as_bytes())
}

pub(super) fn startup_activation_token() -> Option<String> {
    env::var("XDG_ACTIVATION_TOKEN")
        .or_else(|_| env::var("DESKTOP_STARTUP_ID"))
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

pub(super) fn single_instance_socket_path(instance_id: Option<&str>) -> io::Result<PathBuf> {
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let id = instance_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(sanitize_desktop_id)
        .map(Ok)
        .unwrap_or_else(|| {
            env::current_exe().map(|exe| {
                exe.file_stem()
                    .and_then(|name| name.to_str())
                    .map(sanitize_desktop_id)
                    .unwrap_or_else(|| "app".to_string())
            })
        })?;
    Ok(runtime.join("sabine").join(format!("{id}.sock")))
}
