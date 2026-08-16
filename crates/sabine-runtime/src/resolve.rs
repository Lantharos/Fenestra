use crate::detect::detect_runtime;
use crate::error::RuntimeError;
use crate::types::{RuntimeConfig, RuntimeInfo, RuntimeLocation, RuntimeMode};
use crate::version::{version_satisfies, version_sort_key};

pub fn resolve_runtime(config: &RuntimeConfig) -> Result<RuntimeInfo, RuntimeError> {
    let runtimes = detect_runtime(config);
    select_runtime(config, runtimes).ok_or_else(|| {
        RuntimeError::NotFound(format!(
            "no compatible CEF runtime found for mode {:?}",
            config.mode
        ))
    })
}

pub fn ensure_runtime(config: &RuntimeConfig) -> Result<RuntimeInfo, RuntimeError> {
    match resolve_runtime(config) {
        Ok(runtime) => Ok(runtime),
        Err(_) if config.allow_user_install && should_install_user_runtime(config) => {
            crate::install::install_user_runtime(config)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn select_runtime(
    config: &RuntimeConfig,
    runtimes: Vec<RuntimeInfo>,
) -> Option<RuntimeInfo> {
    let mut compatible = runtimes
        .into_iter()
        .filter(|runtime| runtime.verified)
        .filter(|runtime| version_satisfies(&runtime.version, crate::MIN_CEF_MAJOR))
        .filter(|runtime| location_allowed(config.mode, &runtime.location))
        .collect::<Vec<_>>();

    compatible.sort_by_key(|runtime| {
        (
            runtime_priority(config.mode, &runtime.location),
            std::cmp::Reverse(version_sort_key(&runtime.version)),
        )
    });
    compatible.into_iter().next()
}

fn location_allowed(mode: RuntimeMode, location: &RuntimeLocation) -> bool {
    match mode {
        RuntimeMode::SystemRequired => matches!(location, RuntimeLocation::System(_)),
        RuntimeMode::Bundled => matches!(location, RuntimeLocation::Bundled(_)),
        RuntimeMode::SystemPreferred | RuntimeMode::SharedPreferred => true,
    }
}

fn runtime_priority(mode: RuntimeMode, location: &RuntimeLocation) -> u8 {
    match mode {
        RuntimeMode::SystemRequired => match location {
            RuntimeLocation::System(_) => 0,
            _ => 9,
        },
        RuntimeMode::SystemPreferred => match location {
            RuntimeLocation::System(_) => 0,
            RuntimeLocation::UserLocal(_) => 1,
            RuntimeLocation::Bundled(_) => 2,
        },
        RuntimeMode::SharedPreferred => match location {
            RuntimeLocation::UserLocal(_) => 0,
            RuntimeLocation::System(_) => 1,
            RuntimeLocation::Bundled(_) => 2,
        },
        RuntimeMode::Bundled => match location {
            RuntimeLocation::Bundled(_) => 0,
            _ => 9,
        },
    }
}

pub(crate) fn should_install_user_runtime(config: &RuntimeConfig) -> bool {
    matches!(
        config.mode,
        RuntimeMode::SystemPreferred | RuntimeMode::SharedPreferred
    )
}
