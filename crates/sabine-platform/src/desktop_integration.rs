use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon {
    pub id: String,
    pub title: String,
    pub icon_path: Option<PathBuf>,
    pub tooltip: Option<String>,
    pub menu: Vec<TrayMenuItem>,
}

impl TrayIcon {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon_path: None,
            tooltip: None,
            menu: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayMenuItem {
    pub id: String,
    pub label: String,
    pub action: Option<String>,
    pub enabled: bool,
    pub separator: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutostartEntry {
    pub id: String,
    pub name: String,
    pub command: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShortcutModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: ShortcutModifiers,
    pub key: String,
}

impl Shortcut {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            modifiers: ShortcutModifiers::default(),
            key: key.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalShortcutRegistration {
    pub id: String,
    pub shortcut: Shortcut,
    pub action: String,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub description: Option<String>,
    pub desktop_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepLinkRegistration {
    pub id: String,
    pub schemes: Vec<String>,
}

impl DeepLinkRegistration {
    pub fn new(
        id: impl Into<String>,
        schemes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            schemes: schemes.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeMessagingHost {
    pub id: String,
    pub name: String,
    pub executable: PathBuf,
    pub allowed_origins: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SingleInstancePolicy {
    #[default]
    AllowMultiple,
    ReuseExisting,
    FocusExisting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayActivation {
    pub tray_id: String,
    pub item_id: Option<String>,
    pub action: Option<String>,
}

impl TrayActivation {
    pub fn new(tray_id: impl Into<String>) -> Self {
        Self {
            tray_id: tray_id.into(),
            item_id: None,
            action: None,
        }
    }

    pub fn item(
        tray_id: impl Into<String>,
        item_id: impl Into<String>,
        action: Option<String>,
    ) -> Self {
        Self {
            tray_id: tray_id.into(),
            item_id: Some(item_id.into()),
            action,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalShortcutActivation {
    pub id: String,
    pub action: String,
    pub activation_token: Option<String>,
}

impl GlobalShortcutActivation {
    pub fn new(id: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            action: action.into(),
            activation_token: None,
        }
    }

    pub fn activation_token(mut self, token: impl Into<String>) -> Self {
        self.activation_token = Some(token.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingleInstanceActivation {
    pub policy: SingleInstancePolicy,
    pub arguments: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub activation_token: Option<String>,
}

impl SingleInstanceActivation {
    pub fn new(policy: SingleInstancePolicy, arguments: Vec<String>) -> Self {
        Self {
            policy,
            arguments,
            working_directory: None,
            activation_token: None,
        }
    }

    pub fn working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    pub fn activation_token(mut self, token: impl Into<String>) -> Self {
        self.activation_token = Some(token.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformEvent {
    Tray(TrayActivation),
    GlobalShortcut(GlobalShortcutActivation),
    SingleInstance(SingleInstanceActivation),
}
