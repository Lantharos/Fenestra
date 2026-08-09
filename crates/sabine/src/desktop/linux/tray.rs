use ksni::blocking::TrayMethods;
use sabine_platform::{PlatformEvent, TrayActivation, TrayIcon, TrayMenuItem};

use super::EventQueue;
use super::util::sanitize_desktop_id;

pub(super) fn spawn_tray_icon(icon: &TrayIcon, events: EventQueue) -> Result<TrayRuntime, String> {
    let tray = LinuxTray {
        icon: icon.clone(),
        events,
    };
    tray.assume_sni_available(true)
        .spawn()
        .map(|handle| TrayRuntime { handle })
        .map_err(|error| error.to_string())
}
pub(super) struct TrayRuntime {
    handle: ksni::blocking::Handle<LinuxTray>,
}

impl Drop for TrayRuntime {
    fn drop(&mut self) {
        self.handle.shutdown().wait();
    }
}
#[derive(Clone)]
pub(super) struct LinuxTray {
    icon: TrayIcon,
    events: EventQueue,
}

impl LinuxTray {
    fn push(&self, event: PlatformEvent) {
        let _ = self.events.send(event);
    }
}

impl ksni::Tray for LinuxTray {
    fn id(&self) -> String {
        sanitize_desktop_id(&self.icon.id)
    }

    fn title(&self) -> String {
        self.icon.title.clone()
    }

    fn icon_name(&self) -> String {
        self.icon
            .icon_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "application-x-executable".to_string())
    }

    fn icon_theme_path(&self) -> String {
        self.icon
            .icon_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.icon.title.clone(),
            description: self.icon.tooltip.clone().unwrap_or_default(),
            ..ksni::ToolTip::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.push(PlatformEvent::Tray(TrayActivation::new(
            self.icon.id.clone(),
        )));
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.icon.menu.iter().map(tray_menu_item).collect()
    }
}

pub(super) fn tray_menu_item(item: &TrayMenuItem) -> ksni::MenuItem<LinuxTray> {
    if item.separator {
        return ksni::MenuItem::Separator;
    }
    let item_id = item.id.clone();
    let action = item.action.clone();
    let label = item.label.clone();
    let enabled = item.enabled;
    ksni::menu::StandardItem {
        label,
        enabled,
        activate: Box::new(move |tray: &mut LinuxTray| {
            tray.push(PlatformEvent::Tray(TrayActivation::item(
                tray.icon.id.clone(),
                item_id.clone(),
                action.clone(),
            )));
        }),
        ..ksni::menu::StandardItem::default()
    }
    .into()
}
