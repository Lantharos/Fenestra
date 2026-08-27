use std::{env, path::Path};

#[cfg(unix)]
use std::fs;

pub fn command_exists(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| {
            let candidate = directory.join(name);
            if is_executable(&candidate) {
                return true;
            }
            #[cfg(target_os = "windows")]
            if Path::new(name).extension().is_none() {
                return windows_extensions()
                    .any(|extension| is_executable(&directory.join(format!("{name}{extension}"))));
            }
            false
        })
    })
}

#[cfg(target_os = "windows")]
fn windows_extensions() -> impl Iterator<Item = String> {
    env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|extension| !extension.is_empty())
        .map(|extension| extension.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .into_iter()
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
