use layershellev::{WindowState, calloop::channel};

use crate::osr::host::OsrHostConfig;

mod alpha;
mod buffer;
mod events;
mod forward;
mod ime;
mod input;
mod lifecycle;
mod loading;
mod popup;
mod shell;
mod socket;
mod surface;
mod tooltip;
mod types;

use shell::{anchor_for_shell, keyboard_for_shell, layer_for_shell};
use types::OsrLayerHost;

pub(crate) fn run(mut config: OsrHostConfig) -> Result<(), String> {
    let shell_surface = config
        .shell_surface
        .clone()
        .ok_or_else(|| "missing Sabine shell surface options".to_string())?;
    let shell_surface = normalized_shell_surface(shell_surface, (config.width, config.height));
    config.shell_surface = Some(shell_surface.clone());
    let layer_size = shell_surface.size.expect("normalized shell surface size");
    let mut window_state = WindowState::new(&shell_surface.namespace)
        .with_size(layer_size)
        .with_layer(layer_for_shell(shell_surface.layer))
        .with_anchor(anchor_for_shell(shell_surface.anchor))
        .with_margin((
            shell_surface.margin.top,
            shell_surface.margin.right,
            shell_surface.margin.bottom,
            shell_surface.margin.left,
        ))
        .with_keyboard_interacivity(keyboard_for_shell(shell_surface.keyboard_interactivity))
        .with_events_transparent(shell_surface.events_transparent);
    if let Some(exclusive_zone) = shell_surface.exclusive_zone {
        window_state = window_state.with_exclusive_zone(exclusive_zone);
    }
    let window_state: WindowState<()> = window_state.build().map_err(|error| error.to_string())?;

    let (sender, receiver) = channel::channel();
    let mut host = OsrLayerHost::new(config, sender);
    window_state
        .running_with_proxy(receiver, move |event, state, id| {
            host.handle(event, state, id)
        })
        .map_err(|error| error.to_string())
}

fn normalized_shell_surface(
    mut shell_surface: sabine_platform::ShellSurfaceOptions,
    fallback_size: (u32, u32),
) -> sabine_platform::ShellSurfaceOptions {
    if shell_surface.size.is_none_or(|(width, height)| {
        height == 0 || (width == 0 && (!shell_surface.anchor.left || !shell_surface.anchor.right))
    }) {
        let (width, height) = shell_surface.size.unwrap_or((0, 0));
        shell_surface.size = Some((
            if width == 0 && shell_surface.anchor.left && shell_surface.anchor.right {
                0
            } else {
                fallback_size.0.max(1)
            },
            height.max(fallback_size.1.max(1)),
        ));
    }
    shell_surface
}

#[cfg(test)]
mod tests {
    use sabine_platform::{ShellSurfaceAnchor, ShellSurfaceOptions};

    use super::normalized_shell_surface;

    #[test]
    fn normalized_surface_keeps_stretch_anchors_and_commit_size() {
        let options = ShellSurfaceOptions::new("shell")
            .size(0, 0)
            .anchor(ShellSurfaceAnchor::ALL);
        let normalized = normalized_shell_surface(options, (1280, 720));

        assert_eq!(normalized.anchor, ShellSurfaceAnchor::ALL);
        assert_eq!(normalized.size, Some((0, 720)));
    }
}
