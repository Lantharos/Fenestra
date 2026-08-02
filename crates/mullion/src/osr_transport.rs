use std::io;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
pub(crate) type IpcListener = std::os::unix::net::UnixListener;
#[cfg(unix)]
pub(crate) type IpcStream = std::os::unix::net::UnixStream;
#[cfg(not(unix))]
pub(crate) type IpcListener = std::net::TcpListener;
#[cfg(not(unix))]
pub(crate) type IpcStream = std::net::TcpStream;

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

pub(crate) fn authenticate(stream: &mut IpcStream, expected: &str) -> io::Result<()> {
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
    pub(crate) fn bind() -> io::Result<(Self, IpcListener)> {
        #[cfg(unix)]
        {
            let path = socket_path();
            let _ = std::fs::remove_file(&path);
            Ok((Self::Unix(path.clone()), IpcListener::bind(path)?))
        }
        #[cfg(not(unix))]
        {
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
}

#[cfg(unix)]
fn socket_path() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("mullion-osr-{}-{nanos}.sock", std::process::id()))
}
