use std::{io, path::Path};

pub fn prepare_runtime_assets(runtime_dir: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let target = runtime_dir.join("Release/icudtl.dat");
        if !target.is_file() {
            let source = runtime_dir.join("Resources/icudtl.dat");
            match std::fs::hard_link(&source, &target) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && target.is_file() => {}
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!(
                            "could not prepare CEF ICU data at {}: {error}",
                            target.display()
                        ),
                    ));
                }
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = runtime_dir;
    Ok(())
}
