use std::path::{Path, PathBuf};

mod process;
mod process_tree;

pub use mullion_host::{ensure_host, host_release_binary};
pub use process::{MullionProcess, WindowId};
pub(crate) use process_tree::{
    ManagedChild, prepare_child_command, prepare_detachable_child_command,
};

pub fn browser_profile_dir(profile_key: &str) -> PathBuf {
    user_cache_home()
        .join("mullion")
        .join("profiles")
        .join(format!("{:016x}", stable_hash(&[profile_key])))
        .join("profile")
}

pub fn ld_library_path(release_dir: &Path) -> String {
    let existing = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    if existing.is_empty() {
        release_dir.display().to_string()
    } else {
        format!("{}:{existing}", release_dir.display())
    }
}

fn user_cache_home() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
}

fn stable_hash(parts: &[&str]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
