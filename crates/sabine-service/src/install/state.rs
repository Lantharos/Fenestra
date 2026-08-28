use crate::{
    SabineVersion, ServiceResult, SystemCompatibility, UPDATE_SOAK, registry::replace_file,
    service_data_dir,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

use super::complete_managed_system_at;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct SystemInstallationState {
    pub(super) schema: u32,
    pub(super) active: String,
    pub(super) previous: Option<String>,
    #[serde(default)]
    pub(super) compatibility: SystemCompatibility,
    #[serde(default)]
    pub(super) previous_compatibility: Option<SystemCompatibility>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SystemUpdateFailures {
    #[serde(default)]
    releases: BTreeMap<String, SystemUpdateFailure>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SystemUpdateFailure {
    attempts: u32,
    failed_at: u64,
}

pub(super) fn versions_dir() -> PathBuf {
    service_data_dir().join("bin/versions")
}

fn installation_state_path() -> PathBuf {
    service_data_dir().join("bin/current.json")
}

pub(super) fn read_installation_state() -> Option<SystemInstallationState> {
    let state = serde_json::from_slice::<SystemInstallationState>(
        &fs::read(installation_state_path()).ok()?,
    )
    .ok()?;
    (state.schema == 1).then_some(state)
}

pub(super) fn current_installation() -> Option<(String, PathBuf)> {
    let state = read_installation_state()?;
    let path = versions_dir().join(&state.active);
    complete_managed_system_at(&path)?;
    Some((state.active, path))
}

pub(super) fn write_installation_state(state: &SystemInstallationState) -> ServiceResult<()> {
    let path = installation_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("new");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(state).expect("system state is serializable"))?;
    file.sync_all()?;
    replace_file(&temporary, &path)?;
    Ok(())
}

pub(super) fn prune_system_versions(active: &str, previous: Option<&str>) -> ServiceResult<()> {
    let base = versions_dir();
    if !base.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(base)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if path.is_dir()
            && name != active
            && Some(name) != previous
            && !name.ends_with(".installing")
        {
            let _ = fs::remove_dir_all(path);
        }
    }
    Ok(())
}

pub(super) fn finalize_system_update(version: &str) -> ServiceResult<()> {
    let Some(mut state) = read_installation_state() else {
        return Ok(());
    };
    if state.active != version {
        return Ok(());
    }
    let Some(previous) = state.previous.take() else {
        return Ok(());
    };
    state.previous_compatibility = None;
    write_installation_state(&state)?;
    if previous != version {
        let directory = versions_dir().join(previous);
        if directory.is_dir() {
            let _ = fs::remove_dir_all(directory);
        }
    }
    Ok(())
}

pub(super) fn normalized_state_compatibility(
    state: &SystemInstallationState,
) -> SystemCompatibility {
    if state.compatibility.build == 0 {
        compatibility_for_version(&state.active)
    } else {
        state.compatibility
    }
}

pub(super) fn compatibility_for_version(version: &str) -> SystemCompatibility {
    SabineVersion::parse(version)
        .map(|version| SystemCompatibility {
            major: version.major,
            build: version.build,
            minimum_app_build: 1,
        })
        .unwrap_or_default()
}

fn update_failures_path() -> PathBuf {
    service_data_dir().join("bin/update-failures.json")
}

fn load_update_failures() -> SystemUpdateFailures {
    fs::read(update_failures_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_update_failures(failures: &SystemUpdateFailures) -> ServiceResult<()> {
    let path = update_failures_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("new");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(failures).expect("failure state is serializable"))?;
    file.sync_all()?;
    replace_file(&temporary, &path)?;
    Ok(())
}

pub(super) fn record_system_failure(version: &str) -> ServiceResult<()> {
    let mut failures = load_update_failures();
    let failure = failures
        .releases
        .entry(version.to_string())
        .or_insert(SystemUpdateFailure {
            attempts: 0,
            failed_at: 0,
        });
    failure.attempts = failure.attempts.saturating_add(1);
    failure.failed_at = crate::types::unix_timestamp();
    save_update_failures(&failures)
}

pub(super) fn clear_system_failure(version: &str) -> ServiceResult<()> {
    let mut failures = load_update_failures();
    if failures.releases.remove(version).is_some() {
        save_update_failures(&failures)?;
    }
    Ok(())
}

pub(super) fn system_update_is_backed_off(version: &str) -> bool {
    let failures = load_update_failures();
    let Some(failure) = failures.releases.get(version) else {
        return false;
    };
    let multiplier = 1_u64 << failure.attempts.saturating_sub(1).min(3);
    crate::types::unix_timestamp().saturating_sub(failure.failed_at)
        < UPDATE_SOAK.as_secs().saturating_mul(multiplier)
}
