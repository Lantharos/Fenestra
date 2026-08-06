#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRegionRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl WindowRegionRect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width: width.max(0),
            height: height.max(0),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowRegion {
    pub rects: Vec<WindowRegionRect>,
    pub adaptive: Option<WindowRegionAdaptive>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowRegionAdaptive {
    Full,
    RoundedRect {
        radius: i32,
    },
    RoundedLeft {
        width: i32,
        radius: i32,
    },
    TitlebarAndSidebar {
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    },
    ContentAfterSidebar {
        sidebar_width: i32,
        titlebar_height: i32,
    },
    ContentAfterSidebarRoundedRight {
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    },
}

impl WindowRegion {
    pub fn empty() -> Self {
        Self {
            rects: Vec::new(),
            adaptive: None,
        }
    }

    pub fn rect(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self::empty().add_rect(x, y, width, height)
    }

    pub fn full(width: i32, height: i32) -> Self {
        Self::rect(0, 0, width, height)
    }

    pub fn adaptive_full() -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::Full),
        }
    }

    pub fn rounded_rect(width: i32, height: i32, radius: i32) -> Self {
        let mut region = Self::empty();
        let radius = radius.min(width / 2).min(height / 2).max(0);
        if radius == 0 {
            return Self::full(width, height);
        }
        for y in 0..height.max(0) {
            let inset = rounded_region_row_inset(y, height, radius);
            region = region.add_rect(inset, y, width - inset * 2, 1);
        }
        region
    }

    pub fn adaptive_rounded_rect(radius: i32) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::RoundedRect { radius }),
        }
    }

    pub fn rounded_left(width: i32, height: i32, radius: i32) -> Self {
        let mut region = Self::empty();
        let radius = radius.min(width).min(height / 2).max(0);
        if radius == 0 {
            return Self::full(width, height);
        }
        for y in 0..height.max(0) {
            let inset = rounded_region_row_inset(y, height, radius);
            region = region.add_rect(inset, y, width - inset, 1);
        }
        region
    }

    pub fn adaptive_rounded_left(width: i32, radius: i32) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::RoundedLeft { width, radius }),
        }
    }

    pub fn adaptive_titlebar_sidebar(
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    ) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::TitlebarAndSidebar {
                sidebar_width,
                titlebar_height,
                radius,
            }),
        }
    }

    pub fn adaptive_content_after_sidebar(sidebar_width: i32, titlebar_height: i32) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::ContentAfterSidebar {
                sidebar_width,
                titlebar_height,
            }),
        }
    }

    pub fn adaptive_content_after_sidebar_rounded_right(
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    ) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::ContentAfterSidebarRoundedRight {
                sidebar_width,
                titlebar_height,
                radius,
            }),
        }
    }

    pub fn add_rect(mut self, x: i32, y: i32, width: i32, height: i32) -> Self {
        let rect = WindowRegionRect::new(x, y, width, height);
        if !rect.is_empty() {
            self.rects.push(rect);
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty() && self.adaptive.is_none()
    }

    pub fn resolved_rects(&self, width: i32, height: i32) -> Vec<WindowRegionRect> {
        match self.adaptive {
            Some(WindowRegionAdaptive::Full) => WindowRegion::full(width, height).rects,
            Some(WindowRegionAdaptive::RoundedRect { radius }) => {
                WindowRegion::rounded_rect(width, height, radius).rects
            }
            Some(WindowRegionAdaptive::RoundedLeft {
                width: sidebar_width,
                radius,
            }) => WindowRegion::rounded_left(sidebar_width, height, radius).rects,
            Some(WindowRegionAdaptive::TitlebarAndSidebar {
                sidebar_width,
                titlebar_height,
                radius,
            }) => {
                WindowRegion::titlebar_sidebar(
                    width,
                    height,
                    sidebar_width,
                    titlebar_height,
                    radius,
                )
                .rects
            }
            Some(WindowRegionAdaptive::ContentAfterSidebar {
                sidebar_width,
                titlebar_height,
            }) => {
                WindowRegion::content_after_sidebar(width, height, sidebar_width, titlebar_height)
                    .rects
            }
            Some(WindowRegionAdaptive::ContentAfterSidebarRoundedRight {
                sidebar_width,
                titlebar_height,
                radius,
            }) => {
                WindowRegion::content_after_sidebar_rounded_right(
                    width,
                    height,
                    sidebar_width,
                    titlebar_height,
                    radius,
                )
                .rects
            }
            None => self.rects.clone(),
        }
    }

    fn titlebar_sidebar(
        width: i32,
        height: i32,
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    ) -> Self {
        let mut region = Self::empty();
        let width = width.max(0);
        let height = height.max(0);
        let sidebar_width = sidebar_width.clamp(0, width);
        let titlebar_height = titlebar_height.clamp(0, height);
        let radius = radius.min(width / 2).min(height / 2).max(0);

        for y in 0..titlebar_height {
            let inset = if radius > 0 && y < radius {
                rounded_region_row_inset(y, radius * 2, radius)
            } else {
                0
            };
            region = region.add_rect(inset, y, width - inset * 2, 1);
        }

        for y in titlebar_height..height {
            let inset = if radius > 0 && y >= height - radius {
                rounded_region_row_inset(y, height, radius)
            } else {
                0
            };
            region = region.add_rect(inset, y, sidebar_width - inset, 1);
        }

        region
    }

    fn content_after_sidebar(
        width: i32,
        height: i32,
        sidebar_width: i32,
        titlebar_height: i32,
    ) -> Self {
        let width = width.max(0);
        let height = height.max(0);
        let sidebar_width = sidebar_width.clamp(0, width);
        let titlebar_height = titlebar_height.clamp(0, height);
        Self::rect(
            sidebar_width,
            titlebar_height,
            width - sidebar_width,
            height - titlebar_height,
        )
    }

    fn content_after_sidebar_rounded_right(
        width: i32,
        height: i32,
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    ) -> Self {
        let mut region = Self::empty();
        let width = width.max(0);
        let height = height.max(0);
        let sidebar_width = sidebar_width.clamp(0, width);
        let titlebar_height = titlebar_height.clamp(0, height);
        let radius = radius
            .min((width - sidebar_width) / 2)
            .min(height / 2)
            .max(0);
        if radius == 0 {
            return Self::content_after_sidebar(width, height, sidebar_width, titlebar_height);
        }

        for y in titlebar_height..height {
            let inset = rounded_region_row_inset(y, height, radius);
            region = region.add_rect(sidebar_width, y, width - sidebar_width - inset, 1);
        }

        region
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowRegions {
    pub blur: Option<WindowRegion>,
    pub opaque: Option<WindowRegion>,
    pub input: Option<WindowRegion>,
}

impl WindowRegions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn blur(mut self, region: WindowRegion) -> Self {
        self.blur = Some(region);
        self
    }

    pub fn opaque(mut self, region: WindowRegion) -> Self {
        self.opaque = Some(region);
        self
    }

    pub fn input(mut self, region: WindowRegion) -> Self {
        self.input = Some(region);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.blur.as_ref().is_none_or(WindowRegion::is_empty)
            && self.opaque.as_ref().is_none_or(WindowRegion::is_empty)
            && self.input.as_ref().is_none_or(WindowRegion::is_empty)
    }
}

fn rounded_region_row_inset(y: i32, height: i32, radius: i32) -> i32 {
    let top = y < radius;
    let bottom = y >= height - radius;
    if !top && !bottom {
        return 0;
    }

    let center_y = if top { radius } else { height - radius - 1 };
    let dy = (y - center_y).abs() as f64;
    let radius = radius as f64;
    (radius - (radius * radius - dy * dy).max(0.0).sqrt()).ceil() as i32
}
