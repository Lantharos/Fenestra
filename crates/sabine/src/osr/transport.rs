use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
pub(crate) type IpcListener = std::os::unix::net::UnixListener;
#[cfg(unix)]
pub(crate) type IpcStream = std::os::unix::net::UnixStream;
#[cfg(not(unix))]
pub(crate) type IpcListener = std::net::TcpListener;
#[cfg(not(unix))]
pub(crate) type IpcStream = std::net::TcpStream;

pub(crate) const OSR_TOKEN_ENV: &str = "SABINE_OSR_TOKEN";

#[derive(Clone, Debug)]
pub(crate) enum IpcEndpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    #[cfg(not(unix))]
    Tcp(std::net::SocketAddr),
}

pub(crate) fn authentication_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Reject peers that are not the same OS user as this process.
#[cfg(unix)]
pub(crate) fn authenticate_peer(stream: &IpcStream) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let peer_uid = peer_uid(stream.as_raw_fd())?;
    let our_uid = unsafe { libc::getuid() };
    if peer_uid != our_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "OSR peer uid mismatch",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn peer_uid(fd: std::os::fd::RawFd) -> io::Result<u32> {
    use std::mem::MaybeUninit;

    let mut cred = MaybeUninit::<libc::ucred>::uninit();
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            cred.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { cred.assume_init() }.uid)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn peer_uid(fd: std::os::fd::RawFd) -> io::Result<u32> {
    let mut uid = 0_u32;
    let mut gid = 0_u32;
    let result = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "macos", target_os = "ios"))
))]
fn peer_uid(_fd: std::os::fd::RawFd) -> io::Result<u32> {
    Ok(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
pub(crate) fn authenticate_peer(_stream: &IpcStream) -> io::Result<()> {
    Ok(())
}

pub(crate) fn authenticate(stream: &mut IpcStream, expected: &str) -> io::Result<()> {
    authenticate_peer(stream)?;
    use std::io::Read;

    let mut received = Vec::with_capacity(expected.len());
    loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        if received.len() >= 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "OSR authentication token is too long",
            ));
        }
        received.push(byte[0]);
    }
    if received == expected.as_bytes() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "OSR authentication failed",
        ))
    }
}

impl IpcEndpoint {
    pub(crate) fn bind(app_id: &str) -> io::Result<(Self, IpcListener)> {
        #[cfg(unix)]
        {
            let dir = runtime_ipc_dir(app_id)?;
            ensure_ipc_dir(&dir)?;
            sweep_stale_sockets(&dir);
            let path = socket_path_in(&dir)?;
            let _ = std::fs::remove_file(&path);
            let listener = IpcListener::bind(&path)?;
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
            Ok((Self::Unix(path), listener))
        }
        #[cfg(not(unix))]
        {
            let _ = app_id;
            let listener = IpcListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
            Ok((Self::Tcp(listener.local_addr()?), listener))
        }
    }

    pub(crate) fn argument(&self) -> String {
        match self {
            #[cfg(unix)]
            Self::Unix(path) => path.display().to_string(),
            #[cfg(not(unix))]
            Self::Tcp(address) => address.to_string(),
        }
    }

    pub(crate) fn unlink(&self) {
        #[cfg(unix)]
        {
            let Self::Unix(path) = self;
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(token_file_for(path));
        }
    }
}

/// Write the OSR auth token next to the endpoint so CEF process-singleton
/// handoff can pass the path on argv without putting the secret in cmdline.
pub(crate) fn write_token_file(endpoint: &IpcEndpoint, token: &str) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        let IpcEndpoint::Unix(socket) = endpoint;
        let path = token_file_for(socket);
        write_token_bytes(&path, token)?;
        Ok(path)
    }
    #[cfg(not(unix))]
    {
        let _ = endpoint;
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "sabine-osr-token-{}-{nanos}.token",
            std::process::id()
        ));
        write_token_bytes(&path, token)?;
        Ok(path)
    }
}

fn write_token_bytes(path: &Path, token: &str) -> io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

#[cfg(unix)]
fn token_file_for(socket: &Path) -> PathBuf {
    socket.with_extension("token")
}

#[cfg(unix)]
fn ensure_ipc_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    Ok(())
}

#[cfg(unix)]
fn socket_path_in(dir: &Path) -> io::Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Ok(dir.join(format!("osr-{}-{nanos}.sock", std::process::id())))
}

/// Remove socket files that are no longer accepting connections.
#[cfg(unix)]
pub(crate) fn sweep_stale_sockets(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("sock") {
            continue;
        }
        if socket_is_live(&path) {
            continue;
        }
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(unix)]
fn socket_is_live(path: &Path) -> bool {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let Ok(stream) = UnixStream::connect(path) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    true
}

#[cfg(unix)]
fn runtime_ipc_dir(app_id: &str) -> io::Result<PathBuf> {
    let sanitized = sanitize_app_id(app_id);
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime_dir);
        if path.is_absolute() {
            return Ok(path.join("sabine").join(sanitized));
        }
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME is required when XDG_RUNTIME_DIR is unset",
        )
    })?;
    Ok(Path::new(&home)
        .join(".cache")
        .join("sabine")
        .join("run")
        .join(sanitized))
}

#[cfg(any(unix, test))]
pub(crate) fn sanitize_app_id(app_id: &str) -> String {
    let sanitized = app_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "app".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_app_id_keeps_safe_chars() {
        assert_eq!(sanitize_app_id("com.sabine.notes"), "com.sabine.notes");
        assert_eq!(sanitize_app_id("a/b c"), "a_b_c");
        assert_eq!(sanitize_app_id("@@@"), "___");
    }

    #[cfg(unix)]
    #[test]
    fn authenticate_rejects_wrong_token() {
        let (mut client, mut server) = std::os::unix::net::UnixStream::pair().expect("pair");
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = client.write_all(b"wrong-token\n");
        });
        let error = authenticate(&mut server, "expected-token").expect_err("token");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(unix)]
    #[test]
    fn authenticate_accepts_matching_token_from_same_uid() {
        let (mut client, mut server) = std::os::unix::net::UnixStream::pair().expect("pair");
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = client.write_all(b"expected-token\n");
        });
        authenticate(&mut server, "expected-token").expect("auth");
    }

    #[cfg(unix)]
    #[test]
    fn sweep_removes_dead_socket_files() {
        let dir = std::env::temp_dir().join(format!(
            "sabine-sweep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let dead = dir.join("osr-dead.sock");
        std::fs::write(&dead, []).expect("dead socket placeholder");
        let live = dir.join("osr-live.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&live).expect("bind");
        assert!(dead.exists());
        sweep_stale_sockets(&dir);
        assert!(!dead.exists());
        assert!(live.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
