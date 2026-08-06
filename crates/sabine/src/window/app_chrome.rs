use sabine_platform::{WindowRegion, WindowRegionRect};

use super::SabineWindow;
use super::config::SabineWindowControlAction;

/// App-drawn chrome layout: titlebar drag strip, optional sidebar glass regions,
/// and standard window control hit targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppChrome {
    pub titlebar: i32,
    pub sidebar: i32,
    pub radius: i32,
    pub control_width: i32,
}

impl Default for AppChrome {
    fn default() -> Self {
        Self {
            titlebar: 38,
            sidebar: 260,
            radius: 14,
            control_width: 46,
        }
    }
}

impl AppChrome {
    pub fn new(titlebar: i32, sidebar: i32) -> Self {
        Self {
            titlebar,
            sidebar,
            ..Self::default()
        }
    }

    pub fn titlebar_only(titlebar: i32) -> Self {
        Self {
            titlebar,
            sidebar: 0,
            radius: 0,
            ..Self::default()
        }
    }
}

impl SabineWindow {
    /// Sets drag/control regions and optional blur/opaque/input regions for an
    /// app-drawn titlebar (and optional sidebar glass layout).
    pub fn app_chrome(self, chrome: AppChrome) -> Self {
        let titlebar = chrome.titlebar.max(0);
        let sidebar = chrome.sidebar.max(0);
        let radius = chrome.radius.max(0);
        let control = chrome.control_width.max(1);
        let mut window = self.titlebar_drag_region(titlebar).control_region(
            SabineWindowControlAction::Minimize,
            WindowRegionRect::new(-(control * 3), 0, control, titlebar),
        );
        window = window.control_region(
            SabineWindowControlAction::Maximize,
            WindowRegionRect::new(-(control * 2), 0, control, titlebar),
        );
        window = window.control_region(
            SabineWindowControlAction::Close,
            WindowRegionRect::new(-control, 0, control, titlebar),
        );
        if sidebar > 0 {
            window = window
                .blur_region(WindowRegion::adaptive_titlebar_sidebar(
                    sidebar, titlebar, radius,
                ))
                .opaque_region(WindowRegion::adaptive_content_after_sidebar(
                    sidebar, titlebar,
                ));
            if radius > 0 {
                window = window.input_region(WindowRegion::adaptive_rounded_rect(radius));
            }
        }
        window
    }
}
