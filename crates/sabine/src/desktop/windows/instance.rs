use std::{
    env,
    io::{self, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use sabine_platform::{PlatformEvent, SingleInstanceActivation, SingleInstancePolicy};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE},
        System::{
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegDeleteTreeW, RegGetValueW},
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{CreateMutexW, GetCurrentProcessId},
        },
    },
    core::PCWSTR,
};

use super::{
    EventQueue,
    helpers::{sanitize_id, set_registry_string, wide_null},
};

const INSTANCE_IO_TIMEOUT: Duration = Duration::from_secs(2);
const INSTANCE_CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
const INSTANCE_STARTUP_TIMEOUT: Duration = Duration::from_millis(750);
const INSTANCE_RETRY_INTERVAL: Duration = Duration::from_millis(25);
const MAX_ACTIVATION_BYTES: u64 = 1024 * 1024;
const MAX_ENDPOINT_BYTES: u32 = 4096;
const INSTANCE_WORKERS: usize = 4;
const MAX_PENDING_CONNECTIONS: usize = 32;

pub(super) struct SingleInstanceGuard {
    mutex: HANDLE,
    endpoint_key: String,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    wake_port: u16,
}

impl SingleInstanceGuard {
    pub(super) fn acquire(
        instance_id: Option<&str>,
        policy: SingleInstancePolicy,
        events: EventQueue,
    ) -> Result<Self, String> {
        let id = instance_key(instance_id)?;
        let session_id = current_session_id()?;
        let mutex_name = format!("Local\\sabine-{id}");
        let endpoint_key = format!("Software\\Sabine\\Instances\\{id}-{session_id}");
        let wide_mutex_name = wide_null(&mutex_name);
        let mutex =
            unsafe { CreateMutexW(None, false, PCWSTR::from_raw(wide_mutex_name.as_ptr())) }
                .map_err(|error| error.to_string())?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(mutex);
            }
            notify_existing_instance(&endpoint_key)?;
            return Err(crate::desktop::INSTANCE_ALREADY_RUNNING.to_string());
        }

        let setup = (|| {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let port = listener.local_addr()?.port();
            let token = authentication_token()?;
            Ok::<_, std::io::Error>((listener, port, token))
        })();
        let (listener, port, token) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                unsafe {
                    let _ = CloseHandle(mutex);
                }
                return Err(error.to_string());
            }
        };
        let endpoint = serde_json::json!({
            "port": port,
            "token": token.clone(),
        })
        .to_string();
        if let Err(error) =
            set_registry_string(HKEY_CURRENT_USER, &endpoint_key, "Endpoint", &endpoint)
        {
            unsafe {
                let _ = CloseHandle(mutex);
            }
            return Err(error);
        }

        let (running, thread) = spawn_instance_listener(listener, policy, events, token);
        Ok(Self {
            mutex,
            endpoint_key,
            running,
            thread: Some(thread),
            wake_port: port,
        })
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.wake_port)),
            INSTANCE_CONNECT_TIMEOUT,
        );
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        delete_endpoint(&self.endpoint_key);
        unsafe {
            let _ = CloseHandle(self.mutex);
        }
    }
}

fn spawn_instance_listener(
    listener: TcpListener,
    policy: SingleInstancePolicy,
    events: EventQueue,
    token: String,
) -> (Arc<AtomicBool>, JoinHandle<()>) {
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);
    let thread = thread::spawn(move || {
        let (connections, pending) =
            crossbeam_channel::bounded::<TcpStream>(MAX_PENDING_CONNECTIONS);
        let workers = (0..INSTANCE_WORKERS)
            .map(|_| {
                let pending = pending.clone();
                let events = events.clone();
                let token = token.clone();
                thread::spawn(move || {
                    while let Ok(stream) = pending.recv() {
                        if let Some(activation) =
                            read_single_instance_activation(policy, stream, &token)
                        {
                            let _ = events.send(PlatformEvent::SingleInstance(activation));
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        while let Ok((stream, peer)) = listener.accept() {
            if !thread_running.load(Ordering::Relaxed) {
                break;
            }
            if !peer.ip().is_loopback() {
                continue;
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
    (running, thread)
}

fn read_single_instance_activation(
    policy: SingleInstancePolicy,
    mut stream: TcpStream,
    expected_token: &str,
) -> Option<SingleInstanceActivation> {
    stream.set_read_timeout(Some(INSTANCE_IO_TIMEOUT)).ok()?;
    let mut body = Vec::new();
    Read::by_ref(&mut stream)
        .take(MAX_ACTIVATION_BYTES + 1)
        .read_to_end(&mut body)
        .ok()?;
    if body.len() as u64 > MAX_ACTIVATION_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
    let token = value.get("token")?.as_str()?;
    if !tokens_match(token, expected_token) {
        return None;
    }
    let arguments = value
        .get("arguments")?
        .as_array()?
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

fn notify_existing_instance(endpoint_key: &str) -> Result<(), String> {
    let deadline = Instant::now() + INSTANCE_STARTUP_TIMEOUT;
    loop {
        match send_single_instance_activation(endpoint_key) {
            Ok(()) => return Ok(()),
            Err(_) if Instant::now() < deadline => {
                thread::sleep(INSTANCE_RETRY_INTERVAL);
            }
            Err(error) => return Err(error),
        }
    }
}

fn send_single_instance_activation(endpoint_key: &str) -> Result<(), String> {
    let (port, token) = read_endpoint(endpoint_key)?;
    let body = serde_json::to_vec(&serde_json::json!({
        "token": token,
        "arguments": env::args().collect::<Vec<_>>(),
        "cwd": env::current_dir().ok().map(|path| path.display().to_string()),
        "activationToken": serde_json::Value::Null,
    }))
    .map_err(|error| error.to_string())?;
    if body.len() as u64 > MAX_ACTIVATION_BYTES {
        return Err("single-instance activation is too large".to_string());
    }
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, INSTANCE_CONNECT_TIMEOUT)
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(INSTANCE_IO_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream.write_all(&body).map_err(|error| error.to_string())?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|error| error.to_string())
}

fn instance_key(instance_id: Option<&str>) -> Result<String, String> {
    if let Some(id) = instance_id.map(str::trim).filter(|id| !id.is_empty()) {
        return Ok(sanitize_id(id));
    }
    env::current_exe()
        .map_err(|error| error.to_string())
        .map(|path| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .map(sanitize_id)
                .unwrap_or_else(|| "app".to_string())
        })
}

fn read_endpoint(endpoint_key: &str) -> Result<(u16, String), String> {
    let body = read_registry_string(endpoint_key, "Endpoint")?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let port = value
        .get("port")
        .and_then(|value| value.as_u64())
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| "single-instance endpoint has an invalid port".to_string())?;
    let token = value
        .get("token")
        .and_then(|value| value.as_str())
        .filter(|token| token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_string)
        .ok_or_else(|| "single-instance endpoint has an invalid token".to_string())?;
    Ok((port, token))
}

fn read_registry_string(subkey: &str, value_name: &str) -> Result<String, String> {
    let subkey = wide_null(subkey);
    let value_name = wide_null(value_name);
    let mut byte_len = 0_u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(subkey.as_ptr()),
            PCWSTR::from_raw(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut byte_len),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(format!("RegGetValueW failed: {result:?}"));
    }
    if !(2..=MAX_ENDPOINT_BYTES).contains(&byte_len) || !byte_len.is_multiple_of(2) {
        return Err("single-instance endpoint metadata has an invalid size".to_string());
    }
    let mut buffer = vec![0_u16; byte_len as usize / 2];
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(subkey.as_ptr()),
            PCWSTR::from_raw(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut byte_len),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(format!("RegGetValueW failed: {result:?}"));
    }
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..end]).map_err(|error| error.to_string())
}

fn delete_endpoint(endpoint_key: &str) {
    let endpoint_key = wide_null(endpoint_key);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR::from_raw(endpoint_key.as_ptr()));
    }
}

fn current_session_id() -> Result<u32, String> {
    let mut session_id = 0_u32;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session_id) }
        .map_err(|error| error.to_string())?;
    Ok(session_id)
}

fn authentication_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn tokens_match(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
