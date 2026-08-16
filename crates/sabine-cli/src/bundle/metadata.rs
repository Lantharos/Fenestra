use super::config::BundleApp;
use sabine_service::{AppArtifactKind, AppInstallMode, AppUpdateSource};

pub(super) fn runtime_manifest(
    app: &BundleApp,
    web_directory: &str,
    install_mode: AppInstallMode,
    package_kind: Option<AppArtifactKind>,
) -> String {
    let mut manifest = format!(
        "[app]\nid = \"{}\"\nname = \"{}\"\nversion = \"{}\"\n",
        quote(&app.id),
        quote(&app.name),
        quote(&app.version)
    );
    if let Some(web) = &app.web {
        manifest.push_str("\n[web]\n");
        if web.has_local_assets {
            let source = if web.dist.exists() {
                web.dist.as_path()
            } else {
                web.entry.parent().unwrap_or(&web.root)
            };
            let relative_entry = web
                .entry
                .strip_prefix(source)
                .ok()
                .filter(|entry| !entry.as_os_str().is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| web.entry.file_name().map(Into::into))
                .unwrap_or_else(|| "index.html".into());
            let entry = std::path::Path::new(web_directory).join(relative_entry);
            manifest.push_str(&format!(
                "entry = \"{}\"\n",
                quote(&entry.display().to_string())
            ));
        } else if let Some(url) = &web.url {
            manifest.push_str(&format!("url = \"{}\"\n", quote(url)));
        }
        if !web.allowed_origins.is_empty() {
            let origins = web
                .allowed_origins
                .iter()
                .map(|origin| format!("\"{}\"", quote(origin)))
                .collect::<Vec<_>>()
                .join(", ");
            manifest.push_str(&format!("allowed_origins = [{origins}]\n"));
        }
    }
    if let Some(updates) = &app.updates {
        manifest.push_str("\n[updates]\n");
        match &updates.source {
            AppUpdateSource::Github { repository } => manifest.push_str(&format!(
                "provider = \"github\"\nrepository = \"{}\"\n",
                quote(repository)
            )),
            AppUpdateSource::Http { url } => {
                manifest.push_str(&format!("provider = \"http\"\nurl = \"{}\"\n", quote(url)))
            }
        }
        manifest.push_str(&format!(
            "channel = \"{}\"\npolicy = \"{}\"\ninstall_mode = \"{}\"\npublic_key = \"{}\"\n",
            quote(&updates.channel),
            match updates.policy {
                sabine_service::UpdatePolicy::Disabled => "disabled",
                sabine_service::UpdatePolicy::Notify => "notify",
                sabine_service::UpdatePolicy::Automatic => "automatic",
            },
            match install_mode {
                AppInstallMode::Managed => "managed",
                AppInstallMode::Package => "package",
                AppInstallMode::Store => "store",
            },
            quote(&updates.public_key)
        ));
        if let Some(kind) = package_kind {
            manifest.push_str(&format!("package_kind = \"{}\"\n", kind.as_str()));
        }
    }
    manifest
}

pub(super) fn desktop_entry(app: &BundleApp, executable: &str, icon: Option<&str>) -> String {
    let icon = icon
        .map(|icon| format!("Icon={}\n", desktop_value(icon)))
        .unwrap_or_default();
    let mime_types = mime_type_line(&app.mime_types);
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={}\n{}{}Terminal=false\nCategories=Utility;Development;\nStartupNotify=true\n",
        desktop_value(&app.name),
        desktop_value(executable),
        icon,
        mime_types
    )
}

pub(super) fn app_run(executable: &str) -> String {
    format!(
        "#!/bin/sh\nHERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexec \"$HERE/usr/bin/{executable}\" \"$@\"\n"
    )
}

pub(super) fn info_plist(app: &BundleApp, executable: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{}</string>
<key>CFBundleName</key><string>{}</string>
<key>CFBundleDisplayName</key><string>{}</string>
<key>CFBundleExecutable</key><string>{}</string>
<key>CFBundleVersion</key><string>{}</string>
<key>CFBundleShortVersionString</key><string>{}</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
</dict></plist>
"#,
        xml(&app.id),
        xml(&app.name),
        xml(&app.name),
        xml(executable),
        xml(&app.version),
        xml(&app.version)
    )
}

pub(super) fn windows_manifest(app: &BundleApp) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="{}.0" processorArchitecture="*" name="{}" type="win32"/>
  <description>{}</description>
  <dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
</assembly>
"#,
        xml(&app.version),
        xml(&app.id),
        xml(&app.name)
    )
}

pub(super) fn flatpak_manifest(app: &BundleApp, executable: &str) -> String {
    format!(
        "{{\"app-id\":\"{}\",\"runtime\":\"org.freedesktop.Platform\",\"runtime-version\":\"24.08\",\"sdk\":\"org.freedesktop.Sdk\",\"command\":\"{}\",\"modules\":[]}}\n",
        json(&app.id),
        json(executable)
    )
}

pub(super) fn deb_control(app: &BundleApp, installed_size_kb: u64) -> String {
    format!(
        "Package: {}\nVersion: {}\nSection: utils\nPriority: optional\nArchitecture: amd64\nMaintainer: Sabine <noreply@example.invalid>\nInstalled-Size: {}\nDepends: libc6\nDescription: {}\n",
        debian_name(&app.id),
        app.version,
        installed_size_kb.max(1),
        app.name
    )
}

pub(super) fn rpm_spec(app: &BundleApp, executable: &str) -> String {
    format!(
        r#"Name: {name}
Version: {version}
Release: 1%{{?dist}}
Summary: {summary}
License: unknown
BuildArch: x86_64

%description
{summary}

%prep

%build

%install
mkdir -p "%{{buildroot}}"
cp -a "%{{sabine_source}}/." "%{{buildroot}}/"

%files
/usr/bin/{executable}
/usr/share/applications/{id}.desktop
/usr/share/sabine/{id}
/usr/share/sabine/manifests/{executable}.toml
"#,
        name = rpm_name(&app.id),
        version = app.version,
        summary = app.name,
        executable = executable,
        id = app.id
    )
}

pub(super) fn wix_source(app: &BundleApp, staged_app_dir: &str, executable: &str) -> String {
    let upgrade_code = deterministic_guid(&app.id);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
    <Package Name="{}" Manufacturer="Lantharos" Version="{}" UpgradeCode="{}">
      <MediaTemplate EmbedCab="yes"/>
      <MajorUpgrade DowngradeErrorMessage="A newer version of this application is already installed."/>
    <StandardDirectory Id="ProgramFilesFolder">
      <Directory Id="INSTALLFOLDER" Name="{}">
      </Directory>
    </StandardDirectory>
    <StandardDirectory Id="ProgramMenuFolder"/>
    <Files Include="{}\**" Directory="INSTALLFOLDER">
      <Exclude Files="{}\{}"/>
    </Files>
    <Component Id="MainExecutable" Directory="INSTALLFOLDER" Guid="*">
      <File Id="MainExecutableFile" Source="{}\{}" KeyPath="yes">
        <Shortcut Id="StartMenuShortcut" Directory="ProgramMenuFolder" Name="{}" Description="{}" Advertise="yes" WorkingDirectory="INSTALLFOLDER"/>
      </File>
    </Component>
  </Package>
</Wix>
"#,
        xml(&app.name),
        xml(&app.version),
        upgrade_code,
        xml(&app.name),
        xml(staged_app_dir),
        xml(staged_app_dir),
        xml(executable),
        xml(staged_app_dir),
        xml(executable),
        xml(&app.name),
        xml(&app.name)
    )
}

fn deterministic_guid(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut bytes: [u8; 16] = Sha256::digest(value.as_bytes())[..16]
        .try_into()
        .expect("SHA-256 prefix has a fixed length");
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

pub(super) fn nsis_script(
    app: &BundleApp,
    staged_app_dir: &str,
    executable: &str,
    output: &str,
) -> String {
    format!(
        "Name \"{}\"\nOutFile \"{}\"\nInstallDir \"$PROGRAMFILES64\\{}\"\nSection\nSetOutPath \"$INSTDIR\"\nFile /r \"{}/*\"\nCreateShortcut \"$DESKTOP\\{}.lnk\" \"$INSTDIR\\{}\"\nSectionEnd\n",
        app.name, output, app.name, staged_app_dir, app.name, executable
    )
}

pub(super) fn shell_script(lines: &[&str]) -> String {
    format!("#!/bin/sh\nset -e\n{}\n", lines.join("\n"))
}

pub(super) fn sanitize_path(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn json(value: &str) -> String {
    quote(value)
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn desktop_value(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

fn mime_type_line(mime_types: &[String]) -> String {
    if mime_types.is_empty() {
        return String::new();
    }
    let values = mime_types
        .iter()
        .map(|mime_type| mime_type.trim())
        .filter(|mime_type| !mime_type.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        String::new()
    } else {
        format!("MimeType={};\n", values.join(";"))
    }
}

fn debian_name(value: &str) -> String {
    value.to_ascii_lowercase().replace('_', "-")
}

fn rpm_name(value: &str) -> String {
    value.replace(['.', '_'], "-")
}
